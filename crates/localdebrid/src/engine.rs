//! Thin layer over librqbit: metadata resolution, file-restricted downloads, streams.

use std::collections::{BTreeSet, HashSet};
use std::net::{Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use bytes::Bytes;
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::DhtPersistenceConfig;
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ListenerMode, ListenerOptions,
    ManagedTorrent, Session, SessionOptions, TorrentStats,
};
use serde::Serialize;
use tracing::{info, warn};

use crate::config::EngineConfig;
use crate::infohash::InfoHash;
use crate::selection::TorrentFile;

pub type ManagedTorrentHandle = Arc<ManagedTorrent>;

pub const MAX_CONCURRENT_STREAMS: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("timed out after {0}s waiting for torrent metadata (no reachable peers?)")]
    MetadataTimeout(u64),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Clone, Debug)]
pub struct TorrentMeta {
    pub info_hash: InfoHash,
    pub name: String,
    pub files: Vec<TorrentFile>,
    pub torrent_bytes: Bytes,
    pub seen_peers: Vec<SocketAddr>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentSnapshot {
    pub state: String,
    pub error: Option<String>,
    pub file_progress: Vec<u64>,
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub download_bps: u64,
    pub upload_bps: u64,
    pub peers_live: u32,
    pub peers_connecting: u32,
    pub peers_seen: u32,
}

impl From<TorrentStats> for TorrentSnapshot {
    fn from(s: TorrentStats) -> Self {
        let (download_bps, upload_bps, live, connecting, seen) = match &s.live {
            Some(l) => (
                l.download_speed.as_bytes(),
                l.upload_speed.as_bytes(),
                l.snapshot.peer_stats.live,
                l.snapshot.peer_stats.connecting,
                l.snapshot.peer_stats.seen,
            ),
            None => (0, 0, 0, 0, 0),
        };
        Self {
            state: s.state.to_string(),
            error: s.error,
            file_progress: s.file_progress,
            progress_bytes: s.progress_bytes,
            total_bytes: s.total_bytes,
            download_bps,
            upload_bps,
            peers_live: live,
            peers_connecting: connecting,
            peers_seen: seen,
        }
    }
}

pub struct Engine {
    session: Arc<Session>,
    cache_dir: PathBuf,
    meta_dir: PathBuf,
    config: EngineConfig,
}

impl Engine {
    pub async fn start(
        cache_dir: PathBuf,
        state_dir: PathBuf,
        config: EngineConfig,
    ) -> anyhow::Result<Self> {
        let meta_dir = state_dir.join("torrents");
        std::fs::create_dir_all(&cache_dir)?;
        std::fs::create_dir_all(&meta_dir)?;

        let dht = config.enable_dht.then(|| librqbit::DhtSessionConfig {
            persistence: Some(DhtPersistenceConfig {
                dump_interval: None,
                config_filename: Some(state_dir.join("dht.json")),
            }),
            ..Default::default()
        });
        let to_bps = |kbps: u32| NonZeroU32::new(kbps.saturating_mul(1024));
        let options = SessionOptions {
            dht,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpOnly,
                listen_addr: (Ipv6Addr::UNSPECIFIED, config.bt_port).into(),
                enable_upnp_port_forwarding: false,
                ..Default::default()
            }),
            fastresume: false,
            persistence: None,
            ratelimits: LimitsConfig {
                upload_bps: to_bps(config.upload_limit_kbps),
                download_bps: to_bps(config.download_limit_kbps),
            },
            trackers: config
                .extra_trackers
                .iter()
                .filter_map(|t| url::Url::parse(t).ok())
                .collect(),
            peer_limit: Some(config.peer_limit),
            disable_upload: !config.enable_upload,
            // Sizes librqbit's blocking-I/O semaphore. Every open FileStream holds one
            // permit for its lifetime and disk writes need one too, so this must stay
            // well above MAX_CONCURRENT_STREAMS or downloads deadlock.
            runtime_worker_threads: Some(MAX_CONCURRENT_STREAMS * 2),
            ..Default::default()
        };
        let session = Session::new_with_opts(cache_dir.clone(), options)
            .await
            .context("starting the BitTorrent session")?;
        info!(listen = ?session.listen_addr(), dht = config.enable_dht, "torrent engine started");
        Ok(Self {
            session,
            cache_dir,
            meta_dir,
            config,
        })
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn torrent_dir(&self, info_hash: &InfoHash) -> PathBuf {
        self.cache_dir.join(info_hash.to_string())
    }

    fn meta_file(&self, info_hash: &InfoHash) -> PathBuf {
        self.meta_dir.join(format!("{info_hash}.torrent"))
    }

    pub fn has_cached_metadata(&self, info_hash: &InfoHash) -> bool {
        self.meta_file(info_hash).exists()
    }

    pub fn bt_listen_addr(&self) -> Option<SocketAddr> {
        self.session.listen_addr()
    }

    fn trackers_for(&self, extra: &[String]) -> Vec<String> {
        let mut all: Vec<String> = extra.to_vec();
        for t in &self.config.extra_trackers {
            if !all.contains(t) {
                all.push(t.clone());
            }
        }
        all
    }

    pub fn handle(&self, info_hash: &InfoHash) -> Option<ManagedTorrentHandle> {
        self.session
            .get(TorrentIdOrHash::Hash(info_hash.to_id20()))
    }

    fn meta_from_handle(info_hash: InfoHash, handle: &ManagedTorrentHandle) -> Option<TorrentMeta> {
        handle
            .with_metadata(|m| TorrentMeta {
                info_hash,
                name: m.info.name().map(|n| n.to_string()).unwrap_or_default(),
                files: files_of(&m.info),
                torrent_bytes: m.torrent_bytes.clone(),
                seen_peers: Vec::new(),
            })
            .ok()
    }

    /// Metadata from the running torrent, the on-disk cache, or the swarm (in that order).
    pub async fn resolve_metadata(
        &self,
        info_hash: InfoHash,
        trackers: &[String],
    ) -> Result<TorrentMeta, EngineError> {
        if let Some(meta) = self
            .handle(&info_hash)
            .and_then(|h| Self::meta_from_handle(info_hash, &h))
        {
            return Ok(meta);
        }
        let trackers = self.trackers_for(trackers);
        let cached = std::fs::read(self.meta_file(&info_hash)).ok();
        let from_cache = cached.is_some();
        let add = match cached {
            Some(bytes) => AddTorrent::from_bytes(bytes),
            None => AddTorrent::from_url(info_hash.magnet(&trackers)),
        };
        let options = AddTorrentOptions {
            list_only: true,
            trackers: Some(trackers),
            ..Default::default()
        };
        let timeout = self.config.metadata_timeout_secs;
        let response = tokio::time::timeout(
            Duration::from_secs(timeout),
            self.session.add_torrent(add, Some(options)),
        )
        .await
        .map_err(|_| EngineError::MetadataTimeout(timeout))?
        .context("resolving torrent metadata")?;

        let meta = match response {
            AddTorrentResponse::ListOnly(r) => {
                let resolved = InfoHash(r.info_hash.0);
                if resolved != info_hash {
                    return Err(anyhow::anyhow!("metadata info-hash mismatch").into());
                }
                TorrentMeta {
                    info_hash,
                    name: r.info.name().map(|n| n.to_string()).unwrap_or_default(),
                    files: files_of(&r.info),
                    torrent_bytes: r.torrent_bytes,
                    seen_peers: r.seen_peers,
                }
            }
            AddTorrentResponse::AlreadyManaged(_, handle) | AddTorrentResponse::Added(_, handle) => {
                Self::meta_from_handle(info_hash, &handle)
                    .context("managed torrent has no metadata")?
            }
        };
        if !from_cache
            && let Err(e) = std::fs::write(self.meta_file(&info_hash), &meta.torrent_bytes)
        {
            warn!(%info_hash, error = %e, "could not cache torrent metadata");
        }
        Ok(meta)
    }

    /// Starts (or extends) the download of `file_ids` and waits until the torrent can
    /// serve streams.
    pub async fn open(
        &self,
        meta: &TorrentMeta,
        file_ids: &[usize],
        trackers: &[String],
    ) -> anyhow::Result<ManagedTorrentHandle> {
        if let Some(handle) = self.handle(&meta.info_hash) {
            let current: HashSet<usize> = handle.only_files().unwrap_or_default().into_iter().collect();
            let mut wanted = current.clone();
            wanted.extend(file_ids.iter().copied());
            if wanted != current {
                self.session.update_only_files(&handle, &wanted).await?;
            }
            if handle.is_paused() {
                self.session.unpause(&handle).await?;
            }
            handle.wait_until_initialized().await?;
            return Ok(handle);
        }

        let output = self.torrent_dir(&meta.info_hash);
        std::fs::create_dir_all(&output)?;
        let options = AddTorrentOptions {
            only_files: Some(file_ids.to_vec()),
            overwrite: true,
            output_folder: Some(output.to_string_lossy().into_owned()),
            initial_peers: (!meta.seen_peers.is_empty()).then(|| meta.seen_peers.clone()),
            trackers: Some(self.trackers_for(trackers)),
            ..Default::default()
        };
        let response = self
            .session
            .add_torrent(AddTorrent::from_bytes(meta.torrent_bytes.clone()), Some(options))
            .await
            .context("adding torrent")?;
        let handle = response
            .into_handle()
            .context("torrent was not added")?;
        handle.wait_until_initialized().await?;
        Ok(handle)
    }

    /// Restricts the download to `file_ids`; an empty set removes the torrent instead.
    pub async fn retain_files(
        &self,
        info_hash: &InfoHash,
        file_ids: &BTreeSet<usize>,
        delete_data: bool,
    ) -> anyhow::Result<()> {
        if file_ids.is_empty() {
            return self.remove(info_hash, delete_data).await;
        }
        if let Some(handle) = self.handle(info_hash) {
            let set: HashSet<usize> = file_ids.iter().copied().collect();
            self.session.update_only_files(&handle, &set).await?;
        }
        Ok(())
    }

    pub async fn remove(&self, info_hash: &InfoHash, delete_data: bool) -> anyhow::Result<()> {
        if self.handle(info_hash).is_some() {
            self.session
                .delete(TorrentIdOrHash::Hash(info_hash.to_id20()), false)
                .await?;
        }
        if delete_data {
            let dir = self.torrent_dir(info_hash);
            if dir.exists() {
                tokio::fs::remove_dir_all(&dir)
                    .await
                    .with_context(|| format!("deleting {}", dir.display()))?;
            }
        }
        Ok(())
    }

    pub fn snapshot(&self, info_hash: &InfoHash) -> Option<TorrentSnapshot> {
        self.handle(info_hash).map(|h| h.stats().into())
    }

    pub fn active_hashes(&self) -> Vec<InfoHash> {
        self.session.with_torrents(|it| {
            it.map(|(_, h)| InfoHash(h.info_hash().0)).collect()
        })
    }

    pub fn totals(&self) -> TorrentSnapshot {
        self.session.with_torrents(|it| {
            let mut total = TorrentSnapshot::default();
            for (_, h) in it {
                let s: TorrentSnapshot = h.stats().into();
                total.download_bps += s.download_bps;
                total.upload_bps += s.upload_bps;
                total.peers_live += s.peers_live;
                total.peers_connecting += s.peers_connecting;
                total.peers_seen += s.peers_seen;
                total.progress_bytes += s.progress_bytes;
                total.total_bytes += s.total_bytes;
            }
            total
        })
    }

    pub async fn stop(&self) {
        self.session.stop().await;
    }
}

fn files_of(
    info: &librqbit::ValidatedTorrentMetaV1Info<librqbit::ByteBufOwned>,
) -> Vec<TorrentFile> {
    info.iter_file_details()
        .enumerate()
        .map(|(index, d)| TorrentFile {
            index,
            path: d.filename.to_vec().join("/"),
            len: d.len,
            padding: d.attrs().padding,
        })
        .collect()
}
