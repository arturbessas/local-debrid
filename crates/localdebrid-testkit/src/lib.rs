//! Fixtures that make a self-contained BitTorrent swarm on localhost: an HTTP tracker,
//! a seeder and a Stremio addon that advertises the seeded torrent. They let tests and
//! the e2e script exercise the real engine without touching the internet.

use std::collections::HashMap;
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use axum::Json;
use axum::extract::{Path as AxPath, RawQuery, State};
use axum::routing::get;
use librqbit::{CreateTorrentOptions, ListenerMode, ListenerOptions, Session, SessionOptions};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Deterministic pseudo-random bytes so tests can verify any range without keeping the
/// whole file in memory.
pub fn pattern_bytes(seed: u64, offset: u64, len: usize) -> Vec<u8> {
    (offset..offset + len as u64)
        .map(|i| {
            let mut x = i.wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            x ^= x >> 33;
            x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
            x ^= x >> 33;
            x as u8
        })
        .collect()
}

pub fn write_pattern_file(path: &Path, len: u64, seed: u64) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut offset = 0;
    while offset < len {
        let chunk = (len - offset).min(1 << 20) as usize;
        file.write_all(&pattern_bytes(seed, offset, chunk))?;
        offset += chunk as u64;
    }
    file.flush()?;
    Ok(())
}

fn bencode_peers(peers: &[SocketAddrV4]) -> Vec<u8> {
    let mut compact = Vec::with_capacity(peers.len() * 6);
    for p in peers {
        compact.extend_from_slice(&p.ip().octets());
        compact.extend_from_slice(&p.port().to_be_bytes());
    }
    let mut out = format!("d8:intervali5e12:min intervali5e5:peers{}:", compact.len()).into_bytes();
    out.extend_from_slice(&compact);
    out.push(b'e');
    out
}

#[derive(Clone)]
struct TrackerState {
    peers: Arc<Mutex<HashMap<String, Vec<SocketAddrV4>>>>,
    /// Peers announcing from this machine's loopback are handed out under this address,
    /// so a client elsewhere (e.g. an emulator reaching the host as 10.0.2.2) can dial them.
    advertise_host: Ipv4Addr,
}

/// Minimal BEP-3/BEP-23 HTTP tracker. Peers that announce are handed to everyone else
/// announcing the same info-hash.
pub struct Tracker {
    /// URL for clients, using the advertised host.
    pub announce_url: String,
    /// URL for processes on this machine (the seeder).
    pub local_announce_url: String,
    task: JoinHandle<()>,
}

impl Tracker {
    pub async fn start(advertise_host: Ipv4Addr) -> anyhow::Result<Tracker> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
        let port = listener.local_addr()?.port();
        let state = TrackerState {
            peers: Default::default(),
            advertise_host,
        };
        let app = axum::Router::new()
            .route("/announce", get(announce))
            .with_state(state);
        let task = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        Ok(Tracker {
            announce_url: format!("http://{advertise_host}:{port}/announce"),
            local_announce_url: format!("http://127.0.0.1:{port}/announce"),
            task,
        })
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn announce(
    State(state): State<TrackerState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    RawQuery(query): RawQuery,
) -> Vec<u8> {
    let query = query.unwrap_or_default();
    let mut info_hash = String::new();
    let mut port = 0u16;
    let mut stopped = false;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "info_hash" => {
                let raw: Vec<u8> = percent_encoding::percent_decode_str(v).collect();
                info_hash = data_encoding::HEXLOWER.encode(&raw);
            }
            "port" => port = v.parse().unwrap_or(0),
            "event" => stopped = v == "stopped",
            _ => {}
        }
    }
    let ip = match remote.ip() {
        std::net::IpAddr::V4(v4) => v4,
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().unwrap_or(Ipv4Addr::LOCALHOST),
    };
    let ip = if ip.is_loopback() { state.advertise_host } else { ip };
    let me = SocketAddrV4::new(ip, port);
    let mut peers = state.peers.lock();
    let list = peers.entry(info_hash).or_default();
    list.retain(|p| *p != me);
    let others = list.clone();
    if !stopped && port != 0 {
        list.push(me);
    }
    bencode_peers(&others)
}

/// A librqbit session seeding one file or directory.
pub struct Seeder {
    pub session: Arc<Session>,
    pub info_hash: String,
    pub torrent_bytes: Vec<u8>,
    pub listen_addr: Option<SocketAddr>,
}

impl Seeder {
    pub async fn start(content: &Path, tracker_url: &str) -> anyhow::Result<Seeder> {
        let options = SessionOptions {
            dht: None,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpOnly,
                listen_addr: (Ipv4Addr::UNSPECIFIED, 0).into(),
                ..Default::default()
            }),
            disable_local_service_discovery: true,
            persistence: None,
            fastresume: false,
            ..Default::default()
        };
        let parent = content.parent().unwrap_or(Path::new(".")).to_path_buf();
        let session = Session::new_with_opts(parent, options).await?;
        let (torrent, handle) = session
            .create_and_serve_torrent(
                content,
                CreateTorrentOptions {
                    name: None,
                    trackers: vec![tracker_url.to_string()],
                    piece_length: Some(256 * 1024),
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        handle.wait_until_initialized().await?;
        Ok(Seeder {
            info_hash: torrent.info_hash().as_string(),
            torrent_bytes: torrent.as_bytes()?.to_vec(),
            listen_addr: session.listen_addr(),
            session,
        })
    }

    pub async fn stop(&self) {
        self.session.stop().await;
    }
}

/// A Stremio addon answering `/stream/{type}/{id}.json` from a fixed table, standing in
/// for Torrentio and friends.
pub struct FixtureAddon {
    pub manifest_url: String,
    streams: Arc<Mutex<HashMap<String, Value>>>,
    task: JoinHandle<()>,
}

impl FixtureAddon {
    pub async fn start(advertise_host: Ipv4Addr) -> anyhow::Result<FixtureAddon> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
        let port = listener.local_addr()?.port();
        let streams: Arc<Mutex<HashMap<String, Value>>> = Default::default();
        let app = axum::Router::new()
            .route(
                "/manifest.json",
                get(|| async {
                    Json(json!({
                        "id": "test.fixture", "version": "1.0.0", "name": "Fixture",
                        "description": "test", "resources": ["stream"], "types": ["movie", "series"],
                        "catalogs": []
                    }))
                }),
            )
            .route("/stream/{kind}/{id}", get(fixture_streams))
            .with_state(streams.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(FixtureAddon {
            manifest_url: format!("http://{advertise_host}:{port}/manifest.json"),
            streams,
            task,
        })
    }

    /// Sets the `streams` array returned for `<kind>/<id>`.
    pub fn set(&self, kind: &str, id: &str, streams: Value) {
        self.streams.lock().insert(format!("{kind}/{id}"), streams);
    }
}

impl Drop for FixtureAddon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fixture_streams(
    State(streams): State<Arc<Mutex<HashMap<String, Value>>>>,
    AxPath((kind, id)): AxPath<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let id = id.trim_end_matches(".json");
    if id == "fail" {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response();
    }
    let list = streams
        .lock()
        .get(&format!("{kind}/{id}"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    Json(json!({ "streams": list })).into_response()
}

/// Torrentio-shaped stream object for a seeded torrent.
pub fn torrent_stream(info_hash: &str, file_idx: Option<usize>, title: &str, tracker: &str) -> Value {
    let mut v = json!({
        "name": "Fixture\n1080p",
        "title": format!("{title}\n👤 1 💾 1 MB ⚙️ fixture"),
        "infoHash": info_hash,
        "sources": [format!("tracker:{tracker}")],
    });
    if let Some(i) = file_idx {
        v["fileIdx"] = json!(i);
    }
    v
}

/// Lays out a season pack: three episodes, a sample, subtitles and an NFO.
pub fn write_season_pack(root: &Path, episode_len: u64) -> anyhow::Result<Vec<PathBuf>> {
    let dir = root.join("Show.S01.1080p.WEB.x264");
    let mut episodes = Vec::new();
    for e in 1..=3u64 {
        let p = dir.join(format!("Show.S01E0{e}.1080p.WEB.x264.mkv"));
        write_pattern_file(&p, episode_len + e * 1000, 100 + e)?;
        episodes.push(p);
    }
    write_pattern_file(&dir.join("Sample/show.sample.mkv"), 300_000, 7)?;
    std::fs::write(dir.join("Show.S01E02.1080p.WEB.x264.srt"), "1\n00:00:01,000 --> 00:00:02,000\nhi\n")?;
    std::fs::write(dir.join("info.nfo"), "nfo").context("nfo")?;
    Ok(episodes)
}
