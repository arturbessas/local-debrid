//! Torrent discovery through other Stremio addons that answer with `infoHash` streams.
//! Any such addon works (Torrentio, MediaFusion, Comet, a self-hosted scraper...),
//! because only the public addon protocol is used.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, warn};

use crate::config::UpstreamConfig;
use crate::infohash::InfoHash;

#[derive(Clone, Debug, PartialEq)]
pub struct TorrentCandidate {
    pub info_hash: InfoHash,
    pub file_idx: Option<usize>,
    /// First line of the upstream title, usually the torrent name.
    pub release_name: String,
    /// The whole upstream title/description, used for release-name parsing.
    pub raw_title: String,
    pub filename: Option<String>,
    pub size_bytes: Option<u64>,
    pub seeders: Option<u32>,
    pub provider: Option<String>,
    pub trackers: Vec<String>,
    pub upstream: String,
}

#[derive(Deserialize)]
struct StreamsResponse {
    #[serde(default)]
    streams: Vec<Value>,
}

static SIZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)💾\s*([\d.,]+)\s*([KMGT]i?B)").unwrap());
static SEEDERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"👤\s*(\d+)").unwrap());
static PROVIDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"⚙️?\s*([^\n👤💾🌐]+)").unwrap());

pub fn parse_size(number: &str, unit: &str) -> Option<u64> {
    let value: f64 = number.replace(',', ".").parse().ok()?;
    let multiplier = match unit.to_ascii_uppercase().trim_end_matches('B').trim_end_matches('I') {
        "K" => 1024f64,
        "M" => 1024f64.powi(2),
        "G" => 1024f64.powi(3),
        "T" => 1024f64.powi(4),
        _ => return None,
    };
    Some((value * multiplier) as u64)
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

/// Converts one upstream stream object. Returns `None` for streams that are not torrents.
pub fn parse_stream(stream: &Value, upstream: &str) -> Option<TorrentCandidate> {
    let info_hash = match str_field(stream, "infoHash") {
        Some(hash) => InfoHash::parse(hash).ok()?,
        None => {
            let url = str_field(stream, "url")?;
            if !url.starts_with("magnet:") {
                return None;
            }
            InfoHash::from_magnet(url).ok()?
        }
    };
    let file_idx = stream
        .get("fileIdx")
        .and_then(Value::as_u64)
        .map(|i| i as usize);

    let title = str_field(stream, "title")
        .or_else(|| str_field(stream, "description"))
        .unwrap_or_default()
        .replace('\r', "");
    let name = str_field(stream, "name").unwrap_or_default();
    let hints = stream.get("behaviorHints");
    let filename = hints
        .and_then(|h| str_field(h, "filename"))
        .map(str::to_string);
    let mut size_bytes = hints
        .and_then(|h| h.get("videoSize"))
        .and_then(Value::as_u64);
    if size_bytes.is_none() {
        size_bytes = SIZE
            .captures(&title)
            .and_then(|c| parse_size(&c[1], &c[2]));
    }
    let seeders = SEEDERS.captures(&title).and_then(|c| c[1].parse().ok());
    let provider = title
        .find('⚙')
        .and_then(|pos| PROVIDER.captures(&title[pos..]))
        .map(|c| c[1].trim().to_string())
        .filter(|s| !s.is_empty());
    let release_name = title
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(name)
        .to_string();

    let trackers = stream
        .get("sources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|s| s.strip_prefix("tracker:"))
        .map(str::to_string)
        .collect();

    // The quality line in `name` ("Torrentio\n4k DV") helps parsing when the title lacks it.
    let raw_title = if name.is_empty() {
        title.clone()
    } else {
        format!("{title}\n{name}")
    };

    Some(TorrentCandidate {
        info_hash,
        file_idx,
        release_name,
        raw_title,
        filename,
        size_bytes,
        seeders,
        provider,
        trackers,
        upstream: upstream.to_string(),
    })
}

pub fn parse_response(body: &[u8], upstream: &str) -> anyhow::Result<Vec<TorrentCandidate>> {
    let response: StreamsResponse = serde_json::from_slice(body)?;
    Ok(response
        .streams
        .iter()
        .filter_map(|s| parse_stream(s, upstream))
        .collect())
}

/// Validates the `type` and `id` path parameters before they are forwarded upstream.
pub fn is_valid_content_id(kind: &str, id: &str) -> bool {
    let kind_ok = !kind.is_empty() && kind.chars().all(|c| c.is_ascii_alphanumeric());
    let id_ok = !id.is_empty()
        && id.len() <= 128
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.'));
    kind_ok && id_ok
}

struct CacheEntry {
    at: Instant,
    candidates: Vec<TorrentCandidate>,
}

/// Remembers what upstreams said about a torrent so that the play request, which only
/// carries the info-hash, can reuse the trackers and names.
#[derive(Clone, Debug, Default)]
pub struct TorrentHint {
    pub trackers: Vec<String>,
    pub release_name: String,
    pub filename: Option<String>,
}

pub struct UpstreamClient {
    http: reqwest::Client,
    cache: Mutex<HashMap<String, CacheEntry>>,
    hints: Mutex<HashMap<InfoHash, (Instant, TorrentHint)>>,
    cache_ttl: Duration,
}

const HINT_TTL: Duration = Duration::from_secs(24 * 3600);

impl UpstreamClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            cache: Mutex::new(HashMap::new()),
            hints: Mutex::new(HashMap::new()),
            cache_ttl: Duration::from_secs(10 * 60),
        }
    }

    pub fn hint(&self, info_hash: &InfoHash) -> Option<TorrentHint> {
        self.hints.lock().get(info_hash).map(|(_, h)| h.clone())
    }

    fn remember(&self, candidates: &[TorrentCandidate]) {
        let mut hints = self.hints.lock();
        let now = Instant::now();
        hints.retain(|_, (at, _)| now.duration_since(*at) < HINT_TTL);
        for c in candidates {
            let entry = hints
                .entry(c.info_hash)
                .or_insert_with(|| (now, TorrentHint::default()));
            entry.0 = now;
            for t in &c.trackers {
                if !entry.1.trackers.contains(t) {
                    entry.1.trackers.push(t.clone());
                }
            }
            entry.1.release_name = c.release_name.clone();
            if c.filename.is_some() {
                entry.1.filename = c.filename.clone();
            }
        }
    }

    /// Queries every enabled upstream in parallel. A failing upstream is logged and
    /// skipped so one slow addon does not hide the others.
    pub async fn find(
        &self,
        upstreams: &[UpstreamConfig],
        kind: &str,
        id: &str,
    ) -> Vec<TorrentCandidate> {
        let key = format!("{kind}/{id}");
        if let Some(entry) = self.cache.lock().get(&key)
            && entry.at.elapsed() < self.cache_ttl
        {
            return entry.candidates.clone();
        }

        let requests = upstreams.iter().filter(|u| u.enabled).map(|u| async move {
            let url = format!("{}/stream/{kind}/{id}.json", u.base_url());
            let name = if u.name.is_empty() { u.base_url() } else { u.name.clone() };
            let result = async {
                let response = self
                    .http
                    .get(&url)
                    .timeout(Duration::from_secs(20))
                    .send()
                    .await?
                    .error_for_status()?;
                let body = response.bytes().await?;
                parse_response(&body, &name)
            }
            .await;
            match result {
                Ok(list) => {
                    debug!(upstream = %name, count = list.len(), "upstream streams");
                    list
                }
                Err(e) => {
                    warn!(upstream = %name, error = %e, "upstream request failed");
                    Vec::new()
                }
            }
        });
        let mut all = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for list in futures::future::join_all(requests).await {
            for c in list {
                if seen.insert((c.info_hash, c.file_idx)) {
                    all.push(c);
                }
            }
        }
        self.remember(&all);
        if !all.is_empty() {
            let mut cache = self.cache.lock();
            cache.retain(|_, e| e.at.elapsed() < self.cache_ttl);
            cache.insert(
                key,
                CacheEntry {
                    at: Instant::now(),
                    candidates: all.clone(),
                },
            );
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_torrentio_movie_stream() {
        let stream = json!({
            "name": "Torrentio\n1080p",
            "title": "Movie.2020.1080p.BluRay.x264-GRP\n👤 154 💾 2.1 GB ⚙️ ThePirateBay",
            "infoHash": "DD8255ECDC7CA55FB0BBF81323D87062DB1F6D1C",
            "fileIdx": 1,
            "behaviorHints": {"bingeGroup": "torrentio|1080p|BluRay|x264", "filename": "Movie.2020.1080p.BluRay.x264-GRP.mkv"}
        });
        let c = parse_stream(&stream, "Torrentio").unwrap();
        assert_eq!(c.info_hash.to_string(), "dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c");
        assert_eq!(c.file_idx, Some(1));
        assert_eq!(c.release_name, "Movie.2020.1080p.BluRay.x264-GRP");
        assert_eq!(c.seeders, Some(154));
        assert_eq!(c.size_bytes, Some((2.1 * 1024f64.powi(3)) as u64));
        assert_eq!(c.provider.as_deref(), Some("ThePirateBay"));
        assert_eq!(c.filename.as_deref(), Some("Movie.2020.1080p.BluRay.x264-GRP.mkv"));
        assert!(c.raw_title.contains("1080p"));
    }

    #[test]
    fn parses_trackers_from_sources() {
        let stream = json!({
            "title": "x",
            "infoHash": "dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c",
            "sources": ["tracker:udp://tracker.example:1337/announce", "dht:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c"]
        });
        let c = parse_stream(&stream, "u").unwrap();
        assert_eq!(c.trackers, vec!["udp://tracker.example:1337/announce"]);
        assert_eq!(c.file_idx, None);
    }

    #[test]
    fn accepts_magnet_urls_and_ignores_plain_urls() {
        let magnet = json!({"url": "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=x", "title": "x"});
        assert!(parse_stream(&magnet, "u").is_some());
        let http = json!({"url": "https://cdn.example/video.mp4", "title": "x"});
        assert!(parse_stream(&http, "u").is_none());
        let bad = json!({"infoHash": "not-a-hash"});
        assert!(parse_stream(&bad, "u").is_none());
    }

    #[test]
    fn parse_response_skips_non_torrents() {
        let body = br#"{"streams":[{"infoHash":"dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c","title":"a"},{"url":"http://x/y.mp4"}]}"#;
        assert_eq!(parse_response(body, "u").unwrap().len(), 1);
        assert!(parse_response(b"not json", "u").is_err());
    }

    #[test]
    fn size_units() {
        assert_eq!(parse_size("1", "KB"), Some(1024));
        assert_eq!(parse_size("1,5", "GB"), Some(1610612736));
        assert_eq!(parse_size("2", "MiB"), Some(2 * 1024 * 1024));
        assert_eq!(parse_size("2", "XB"), None);
    }

    #[test]
    fn content_id_validation() {
        assert!(is_valid_content_id("movie", "tt0111161"));
        assert!(is_valid_content_id("series", "tt0944947:1:2"));
        assert!(is_valid_content_id("anime", "kitsu:1:5"));
        assert!(!is_valid_content_id("movie", "../../etc"));
        assert!(!is_valid_content_id("mo/vie", "tt1"));
        assert!(!is_valid_content_id("movie", ""));
    }
}
