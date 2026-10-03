//! Turns upstream torrent candidates into Stremio stream objects with playable URLs.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use crate::compat::{self, Level};
use crate::config::Config;
use crate::release::{self, ReleaseInfo, VIDEO_EXTENSIONS};
use crate::selection::Episode;
use crate::sessions::KnownFile;
use crate::upstream::TorrentCandidate;

pub const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// Episode encoded in a Stremio id: `tt123:1:2` or `kitsu:123:5`.
pub fn episode_from_id(id: &str) -> Option<Episode> {
    let parts: Vec<&str> = id.split(':').collect();
    match parts.as_slice() {
        [imdb, season, episode] if imdb.starts_with("tt") => Some(Episode {
            season: season.parse().ok()?,
            episode: episode.parse().ok()?,
        }),
        ["kitsu", _, episode] => Some(Episode {
            season: 1,
            episode: episode.parse().ok()?,
        }),
        _ => None,
    }
}

/// Selector segment of a play URL: a file index, `s<season>e<episode>` or `auto`.
pub fn selector_segment(file_idx: Option<usize>, episode: Option<Episode>) -> String {
    match (file_idx, episode) {
        (Some(i), _) => i.to_string(),
        (None, Some(e)) => format!("s{}e{}", e.season, e.episode),
        (None, None) => "auto".into(),
    }
}

/// File name for the URL. Players (Stroku) infer the container from the extension, so
/// when the real name is unknown the most likely extension is guessed.
pub fn url_file_name(candidate: &TorrentCandidate, known: Option<&KnownFile>, info: &ReleaseInfo) -> String {
    if let Some(k) = known {
        return k.path.rsplit('/').next().unwrap_or(&k.path).to_string();
    }
    if let Some(name) = &candidate.filename {
        return name.rsplit('/').next().unwrap_or(name).to_string();
    }
    let base: String = candidate
        .release_name
        .chars()
        .map(|c| if c.is_alphanumeric() || "-_.()[] ".contains(c) { c } else { '_' })
        .collect();
    let base = base.trim();
    if release::is_video_file(base) {
        return base.to_string();
    }
    let lower = candidate.raw_title.to_lowercase();
    let ext = match info.container.as_deref() {
        Some(ext) if VIDEO_EXTENSIONS.contains(&ext) => ext,
        _ if lower.contains("yts") || lower.contains("yify") => "mp4",
        _ => "mkv",
    };
    format!("{base}.{ext}")
}

fn human_size(bytes: u64) -> String {
    let gib = bytes as f64 / 1024f64.powi(3);
    if gib >= 1.0 {
        format!("{gib:.2} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / 1024f64.powi(2))
    }
}

pub struct Presented {
    pub value: Value,
    level: Level,
    resolution: u32,
    group: &'static str,
}

pub fn present(
    candidates: &[TorrentCandidate],
    config: &Config,
    episode: Option<Episode>,
    base_url: &str,
    known: impl Fn(&TorrentCandidate) -> Option<KnownFile>,
) -> Vec<Value> {
    let max_file = config.max_file_bytes();
    let mut presented: Vec<Presented> = candidates
        .iter()
        .filter_map(|c| {
            let known = known(c);
            let mut info = release::parse(&c.raw_title);
            if let Some(name) = c.filename.as_deref().or(known.as_ref().map(|k| k.path.as_str())) {
                info = info.merge(release::parse(name));
            }
            if config.hide_low_quality && info.source.is_some_and(|s| s.is_low_quality()) {
                return None;
            }
            let group = info.resolution_group();
            if !config.resolutions.iter().any(|r| r == group) {
                return None;
            }
            let verdict = compat::assess(&info, &config.device);
            if config.hide_incompatible && verdict.level == Level::Incompatible {
                return None;
            }
            let size = known.as_ref().map(|k| k.size).or(c.size_bytes);
            if let (Some(limit), Some(size)) = (max_file, size)
                && size > limit
            {
                return None;
            }

            let file_idx = c.file_idx.or_else(|| known.as_ref().map(|k| k.file_idx));
            let file_name = url_file_name(c, known.as_ref(), &info);
            let url = format!(
                "{base_url}/play/{}/{}/{}",
                c.info_hash,
                selector_segment(file_idx, episode),
                utf8_percent_encode(&file_name, SEGMENT)
            );

            let mut meta_line = Vec::new();
            if let Some(s) = c.seeders {
                meta_line.push(format!("👤 {s}"));
            }
            if let Some(size) = size {
                meta_line.push(format!("💾 {}", human_size(size)));
            }
            if let Some(p) = &c.provider {
                meta_line.push(format!("⚙️ {p}"));
            }
            let mut tech = Vec::new();
            if let Some(v) = info.video_label() {
                tech.push(format!("🎞️ {v}"));
            }
            if let Some(a) = info.audio_label() {
                tech.push(format!("🔊 {a}"));
            }
            if let Some(ext) = release::extension(&file_name) {
                tech.push(ext.to_uppercase());
            }
            let mut lines = vec![c.release_name.clone()];
            if !meta_line.is_empty() {
                lines.push(meta_line.join(" "));
            }
            if !tech.is_empty() {
                lines.push(tech.join(" • "));
            }
            if verdict.level != Level::Ok {
                lines.push(format!("⚠️ {}", verdict.reasons.join(", ")));
            }
            let title = lines.join("\n");

            let mut hints = json!({
                "bingeGroup": format!(
                    "localdebrid|{}|{}",
                    info.resolution_label(),
                    info.video_label().unwrap_or_default()
                ),
                "filename": file_name,
                "notWebReady": release::extension(&file_name).as_deref() != Some("mp4"),
            });
            if let Some(size) = size {
                hints["videoSize"] = json!(size);
            }
            if let Some(hash) = known.as_ref().and_then(|k| k.video_hash.clone()) {
                hints["videoHash"] = json!(hash);
            }
            let value = json!({
                "name": format!("{}\n{}", config.addon_name, info.resolution_label()),
                "title": title,
                "description": title,
                "url": url,
                "behaviorHints": hints,
            });
            Some(Presented {
                value,
                level: verdict.level,
                resolution: info.resolution.unwrap_or(0),
                group,
            })
        })
        .collect();

    // Stable: keeps the upstream order (usually seeders) inside each group.
    presented.sort_by_key(|p| (p.level, std::cmp::Reverse(p.resolution)));
    let mut per_group: std::collections::HashMap<&str, usize> = Default::default();
    presented
        .into_iter()
        .filter(|p| {
            let count = per_group.entry(p.group).or_default();
            *count += 1;
            config.max_per_resolution == 0 || *count <= config.max_per_resolution
        })
        .take(config.max_streams)
        .map(|p| p.value)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infohash::InfoHash;

    fn candidate(title: &str, file_idx: Option<usize>, filename: Option<&str>) -> TorrentCandidate {
        TorrentCandidate {
            info_hash: InfoHash([7; 20]),
            file_idx,
            release_name: title.lines().next().unwrap().to_string(),
            raw_title: title.to_string(),
            filename: filename.map(str::to_string),
            size_bytes: Some(1_500_000_000),
            seeders: Some(42),
            provider: Some("YTS".into()),
            trackers: vec![],
            upstream: "test".into(),
        }
    }

    #[test]
    fn episode_ids() {
        assert_eq!(episode_from_id("tt0944947:2:5"), Some(Episode { season: 2, episode: 5 }));
        assert_eq!(episode_from_id("kitsu:42:7"), Some(Episode { season: 1, episode: 7 }));
        assert_eq!(episode_from_id("tt0111161"), None);
        assert_eq!(episode_from_id("tt1:x:2"), None);
    }

    #[test]
    fn stream_object_has_direct_url_and_hints() {
        let c = candidate("Movie.2020.1080p.WEB-DL.AAC.x264", Some(2), Some("Movie 2020 [1080p].mp4"));
        let streams = present(&[c], &Config::default(), None, "http://192.168.1.2:8765", |_| None);
        assert_eq!(streams.len(), 1);
        let s = &streams[0];
        assert_eq!(
            s["url"],
            "http://192.168.1.2:8765/play/0707070707070707070707070707070707070707/2/Movie%202020%20%5B1080p%5D.mp4"
        );
        assert!(s.get("infoHash").is_none());
        assert_eq!(s["name"], "Local Debrid\n1080p");
        assert_eq!(s["behaviorHints"]["filename"], "Movie 2020 [1080p].mp4");
        assert_eq!(s["behaviorHints"]["videoSize"], 1_500_000_000u64);
        assert_eq!(s["behaviorHints"]["notWebReady"], false);
        let title = s["title"].as_str().unwrap();
        assert!(title.contains("👤 42 💾 1.40 GB ⚙️ YTS"), "{title}");
        assert!(title.contains("H.264"));
    }

    #[test]
    fn url_ends_with_container_extension_even_when_filename_unknown() {
        let mkv = candidate("Show.S01E02.1080p.WEB.x265", None, None);
        let yts = candidate("Movie (2020) [1080p] [YTS.MX]", None, None);
        let streams = present(
            &[mkv, yts],
            &Config::default(),
            Some(Episode { season: 1, episode: 2 }),
            "http://h:1",
            |_| None,
        );
        assert!(streams[0]["url"].as_str().unwrap().ends_with(".mkv"));
        assert!(streams[0]["url"].as_str().unwrap().contains("/s1e2/"));
        assert!(streams[1]["url"].as_str().unwrap().ends_with(".mp4"));
    }

    #[test]
    fn incompatible_and_cam_releases_are_hidden_by_default() {
        let list = [
            candidate("Movie.2020.1080p.Hi10P.x264", None, None),
            candidate("Movie.2020.HDCAM.x264", None, None),
            candidate("Movie.2020.720p.WEB.x264", None, None),
        ];
        let streams = present(&list, &Config::default(), None, "http://h:1", |_| None);
        assert_eq!(streams.len(), 1);

        let lenient = Config {
            hide_incompatible: false,
            hide_low_quality: false,
            ..Default::default()
        };
        let streams = present(&list, &lenient, None, "http://h:1", |_| None);
        assert_eq!(streams.len(), 3);
        assert!(streams[2]["title"].as_str().unwrap().contains("⚠️ 10-bit H.264"));
    }

    #[test]
    fn sorted_by_compatibility_then_resolution_and_capped() {
        let list = [
            candidate("A.720p.WEB.x264", None, None),
            candidate("B.2160p.WEB.DV.HDR.x265", None, None),
            candidate("C.1080p.WEB.x264", None, None),
            candidate("D.2160p.WEB.x265", None, None),
        ];
        let config = Config {
            max_streams: 3,
            ..Default::default()
        };
        let streams = present(&list, &config, None, "http://h:1", |_| None);
        let names: Vec<_> = streams
            .iter()
            .map(|s| s["title"].as_str().unwrap().lines().next().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["D.2160p.WEB.x265", "C.1080p.WEB.x264", "A.720p.WEB.x264"]);
    }

    #[test]
    fn known_files_provide_exact_size_and_video_hash() {
        let c = candidate("Movie.2020.1080p.WEB.x264", None, None);
        let known = KnownFile {
            file_idx: 1,
            path: "Movie/movie.mkv".into(),
            size: 123,
            video_hash: Some("0011223344556677".into()),
        };
        let streams = present(&[c], &Config::default(), None, "http://h:1", |_| Some(known.clone()));
        let hints = &streams[0]["behaviorHints"];
        assert_eq!(hints["videoSize"], 123);
        assert_eq!(hints["videoHash"], "0011223344556677");
        assert_eq!(hints["filename"], "movie.mkv");
        assert!(streams[0]["url"].as_str().unwrap().contains("/1/movie.mkv"));
    }

    #[test]
    fn resolution_filter_and_per_resolution_cap() {
        let list = [
            candidate("A.2160p.WEB.x265", None, None),
            candidate("B.1080p.WEB.x264", None, None),
            candidate("C.1080p.BluRay.x264", None, None),
            candidate("D.720p.WEB.x264", None, None),
            candidate("E.Unknown.Quality", None, None),
        ];
        let config = Config {
            resolutions: vec!["1080p".into(), "720p".into()],
            max_per_resolution: 1,
            ..Default::default()
        };
        let streams = present(&list, &config, None, "http://h:1", |_| None);
        let names: Vec<_> = streams
            .iter()
            .map(|s| s["title"].as_str().unwrap().lines().next().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["B.1080p.WEB.x264", "D.720p.WEB.x264"]);
    }

    #[test]
    fn max_file_size_filter() {
        let c = candidate("Movie.2020.1080p.WEB.x264", None, None);
        let config = Config {
            max_file_size_gb: 1.0,
            ..Default::default()
        };
        assert!(present(&[c], &config, None, "http://h:1", |_| None).is_empty());
    }
}
