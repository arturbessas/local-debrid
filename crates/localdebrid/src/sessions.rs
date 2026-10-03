//! A streaming session is one video file of one torrent being served over HTTP.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::{debug, info, warn};

use crate::engine::{Engine, EngineError, ManagedTorrentHandle, TorrentSnapshot};
use crate::infohash::InfoHash;
use crate::selection::{self, SelectionError, Selector};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Selection(#[from] SelectionError),
    #[error("{0}")]
    Metadata(#[from] EngineError),
    #[error("file is {size} bytes, above the configured limit of {limit} bytes")]
    TooLarge { size: u64, limit: u64 },
    #[error("not enough free space: need {needed} bytes, {available} available")]
    NoSpace { needed: u64, available: u64 },
    #[error("torrent engine error: {0}")]
    Engine(anyhow::Error),
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct StreamSession {
    pub id: String,
    pub info_hash: InfoHash,
    pub file_idx: usize,
    pub file_path: String,
    pub file_len: u64,
    pub torrent_name: String,
    pub title: String,
    pub created_at_ms: u64,
    last_activity_ms: AtomicU64,
    active_readers: AtomicUsize,
    bytes_served: AtomicU64,
    video_hash: Mutex<Option<String>>,
    handle: ManagedTorrentHandle,
}

impl StreamSession {
    pub fn file_name(&self) -> &str {
        self.file_path.rsplit('/').next().unwrap_or(&self.file_path)
    }

    pub fn handle(&self) -> &ManagedTorrentHandle {
        &self.handle
    }

    pub fn touch(&self) {
        self.last_activity_ms.store(now_ms(), Ordering::Relaxed);
    }

    pub fn last_activity_ms(&self) -> u64 {
        self.last_activity_ms.load(Ordering::Relaxed)
    }

    pub fn active_readers(&self) -> usize {
        self.active_readers.load(Ordering::Relaxed)
    }

    pub fn add_served(&self, bytes: u64) {
        self.bytes_served.fetch_add(bytes, Ordering::Relaxed);
        self.touch();
    }

    pub fn video_hash(&self) -> Option<String> {
        self.video_hash.lock().clone()
    }

    /// Counts an open HTTP response until the guard is dropped, which also happens when
    /// the client disconnects mid-body.
    pub fn reader_guard(self: &Arc<Self>) -> ReaderGuard {
        self.active_readers.fetch_add(1, Ordering::Relaxed);
        self.touch();
        ReaderGuard {
            session: self.clone(),
        }
    }

    pub fn is_idle(&self, idle: Duration, now_ms: u64) -> bool {
        self.active_readers() == 0
            && now_ms.saturating_sub(self.last_activity_ms()) >= idle.as_millis() as u64
    }

    pub fn dto(&self, engine: &Engine) -> SessionDto {
        let stats = engine.snapshot(&self.info_hash);
        let downloaded = stats
            .as_ref()
            .and_then(|s| s.file_progress.get(self.file_idx).copied())
            .unwrap_or(0);
        SessionDto {
            id: self.id.clone(),
            info_hash: self.info_hash.to_string(),
            file_idx: self.file_idx,
            file_name: self.file_name().to_string(),
            file_path: self.file_path.clone(),
            file_size: self.file_len,
            torrent_name: self.torrent_name.clone(),
            title: self.title.clone(),
            created_at_ms: self.created_at_ms,
            last_activity_ms: self.last_activity_ms(),
            active_readers: self.active_readers(),
            bytes_served: self.bytes_served.load(Ordering::Relaxed),
            downloaded_bytes: downloaded,
            progress: if self.file_len == 0 {
                1.0
            } else {
                downloaded as f64 / self.file_len as f64
            },
            video_hash: self.video_hash(),
            torrent: stats,
        }
    }
}

pub struct ReaderGuard {
    session: Arc<StreamSession>,
}

impl Drop for ReaderGuard {
    fn drop(&mut self) {
        self.session.active_readers.fetch_sub(1, Ordering::Relaxed);
        self.session.touch();
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub id: String,
    pub info_hash: String,
    pub file_idx: usize,
    pub file_name: String,
    pub file_path: String,
    pub file_size: u64,
    pub torrent_name: String,
    pub title: String,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub active_readers: usize,
    pub bytes_served: u64,
    pub downloaded_bytes: u64,
    pub progress: f64,
    pub video_hash: Option<String>,
    pub torrent: Option<TorrentSnapshot>,
}

#[derive(Clone, Debug)]
pub struct OpenRequest {
    pub info_hash: InfoHash,
    pub selector: Selector,
    pub trackers: Vec<String>,
    pub title: String,
}

/// Limits checked before a download is started.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenLimits {
    pub max_file_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

/// File facts that outlive a session (used to enrich later stream listings).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownFile {
    pub file_idx: usize,
    pub path: String,
    pub size: u64,
    pub video_hash: Option<String>,
}

#[derive(Default)]
struct Inner {
    by_id: HashMap<String, Arc<StreamSession>>,
    by_file: HashMap<(InfoHash, usize), String>,
}

pub struct SessionManager {
    engine: Arc<Engine>,
    inner: Mutex<Inner>,
    open_locks: Mutex<HashMap<InfoHash, Arc<tokio::sync::Mutex<()>>>>,
    known_dir: PathBuf,
}

impl SessionManager {
    pub fn new(engine: Arc<Engine>, state_dir: PathBuf) -> Self {
        let known_dir = state_dir.join("files");
        let _ = std::fs::create_dir_all(&known_dir);
        Self {
            engine,
            inner: Mutex::new(Inner::default()),
            open_locks: Mutex::new(HashMap::new()),
            known_dir,
        }
    }

    fn lock_for(&self, info_hash: InfoHash) -> Arc<tokio::sync::Mutex<()>> {
        self.open_locks
            .lock()
            .entry(info_hash)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    fn existing(&self, info_hash: InfoHash, file_idx: usize) -> Option<Arc<StreamSession>> {
        let inner = self.inner.lock();
        inner
            .by_file
            .get(&(info_hash, file_idx))
            .and_then(|id| inner.by_id.get(id))
            .cloned()
    }

    /// Returns the session for the requested file, creating it (and starting the
    /// download) if needed. Concurrent calls for the same torrent are serialised so a
    /// player opening several connections at once creates a single session.
    pub async fn open(
        &self,
        request: OpenRequest,
        limits: OpenLimits,
    ) -> Result<Arc<StreamSession>, SessionError> {
        let lock = self.lock_for(request.info_hash);
        let _guard = lock.lock().await;

        if let Some(idx) = request.selector.file_idx
            && let Some(s) = self.existing(request.info_hash, idx)
        {
            s.touch();
            return Ok(s);
        }

        let meta = self
            .engine
            .resolve_metadata(request.info_hash, &request.trackers)
            .await?;
        let file_idx = selection::select(&meta.files, &request.selector)?;
        if let Some(s) = self.existing(request.info_hash, file_idx) {
            s.touch();
            return Ok(s);
        }
        let file = meta.files[file_idx].clone();
        if let Some(limit) = limits.max_file_bytes
            && file.len > limit
        {
            return Err(SessionError::TooLarge {
                size: file.len,
                limit,
            });
        }
        if let Some(available) = limits.free_bytes {
            let on_disk = crate::cache::allocated_bytes(&self.engine.torrent_dir(&request.info_hash));
            let needed = file.len.saturating_sub(on_disk);
            if needed > available {
                return Err(SessionError::NoSpace { needed, available });
            }
        }

        let handle = self
            .engine
            .open(&meta, &[file_idx], &request.trackers)
            .await
            .map_err(SessionError::Engine)?;

        let known = self.load_known(&request.info_hash);
        let video_hash = known
            .iter()
            .find(|k| k.file_idx == file_idx)
            .and_then(|k| k.video_hash.clone());

        let now = now_ms();
        let session = Arc::new(StreamSession {
            id: new_session_id(),
            info_hash: request.info_hash,
            file_idx,
            file_path: file.path.clone(),
            file_len: file.len,
            torrent_name: meta.name.clone(),
            title: request.title,
            created_at_ms: now,
            last_activity_ms: AtomicU64::new(now),
            active_readers: AtomicUsize::new(0),
            bytes_served: AtomicU64::new(0),
            video_hash: Mutex::new(video_hash.clone()),
            handle,
        });
        {
            let mut inner = self.inner.lock();
            inner.by_id.insert(session.id.clone(), session.clone());
            inner
                .by_file
                .insert((request.info_hash, file_idx), session.id.clone());
        }
        info!(
            session = %session.id,
            info_hash = %request.info_hash,
            file = %file.path,
            size = file.len,
            "session opened"
        );
        self.store_known(&request.info_hash, KnownFile {
            file_idx,
            path: file.path.clone(),
            size: file.len,
            video_hash: video_hash.clone(),
        });
        if video_hash.is_none() {
            self.spawn_video_hash(session.clone());
        }
        Ok(session)
    }

    fn spawn_video_hash(&self, session: Arc<StreamSession>) {
        let known_dir = self.known_dir.clone();
        tokio::spawn(async move {
            let result = tokio::time::timeout(
                Duration::from_secs(600),
                opensubtitles_hash(&session),
            )
            .await;
            match result {
                Ok(Ok(hash)) => {
                    debug!(session = %session.id, %hash, "computed OpenSubtitles hash");
                    *session.video_hash.lock() = Some(hash.clone());
                    let mut known = load_known(&known_dir, &session.info_hash);
                    if let Some(k) = known.iter_mut().find(|k| k.file_idx == session.file_idx) {
                        k.video_hash = Some(hash);
                    }
                    save_known(&known_dir, &session.info_hash, &known);
                }
                Ok(Err(e)) => debug!(session = %session.id, error = %e, "video hash not computed"),
                Err(_) => debug!(session = %session.id, "video hash timed out"),
            }
        });
    }

    pub fn load_known(&self, info_hash: &InfoHash) -> Vec<KnownFile> {
        load_known(&self.known_dir, info_hash)
    }

    fn store_known(&self, info_hash: &InfoHash, file: KnownFile) {
        let mut known = self.load_known(info_hash);
        match known.iter_mut().find(|k| k.file_idx == file.file_idx) {
            Some(k) => {
                let hash = k.video_hash.take().or(file.video_hash.clone());
                *k = KnownFile {
                    video_hash: hash,
                    ..file
                };
            }
            None => known.push(file),
        }
        save_known(&self.known_dir, info_hash, &known);
    }

    pub fn get(&self, id: &str) -> Option<Arc<StreamSession>> {
        self.inner.lock().by_id.get(id).cloned()
    }

    pub fn list(&self) -> Vec<Arc<StreamSession>> {
        let mut list: Vec<_> = self.inner.lock().by_id.values().cloned().collect();
        list.sort_by_key(|s| std::cmp::Reverse(s.created_at_ms));
        list
    }

    pub fn active_hashes(&self) -> HashSet<InfoHash> {
        self.inner.lock().by_id.values().map(|s| s.info_hash).collect()
    }

    /// Ends a session: its file is no longer downloaded, and the torrent is removed from
    /// the engine when no other session uses it.
    pub async fn close(&self, id: &str, delete_data: bool) -> bool {
        let Some(session) = self.inner.lock().by_id.remove(id) else {
            return false;
        };
        let lock = self.lock_for(session.info_hash);
        let _guard = lock.lock().await;
        let remaining: BTreeSet<usize> = {
            let mut inner = self.inner.lock();
            inner.by_file.remove(&(session.info_hash, session.file_idx));
            inner
                .by_id
                .values()
                .filter(|s| s.info_hash == session.info_hash)
                .map(|s| s.file_idx)
                .collect()
        };
        let delete = delete_data && remaining.is_empty();
        if let Err(e) = self
            .engine
            .retain_files(&session.info_hash, &remaining, delete)
            .await
        {
            warn!(session = %id, error = %e, "error while closing session");
        }
        if remaining.is_empty() {
            self.open_locks.lock().remove(&session.info_hash);
        }
        info!(session = %id, deleted = delete, "session closed");
        true
    }

    pub async fn close_idle(&self, idle: Duration, delete_data: bool) -> Vec<String> {
        let now = now_ms();
        let idle_ids: Vec<String> = self
            .inner
            .lock()
            .by_id
            .values()
            .filter(|s| s.is_idle(idle, now))
            .map(|s| s.id.clone())
            .collect();
        for id in &idle_ids {
            self.close(id, delete_data).await;
        }
        idle_ids
    }

    pub async fn close_all(&self, delete_data: bool) -> usize {
        let ids: Vec<String> = self.inner.lock().by_id.keys().cloned().collect();
        for id in &ids {
            self.close(id, delete_data).await;
        }
        ids.len()
    }
}

fn known_path(dir: &std::path::Path, info_hash: &InfoHash) -> PathBuf {
    dir.join(format!("{info_hash}.json"))
}

fn load_known(dir: &std::path::Path, info_hash: &InfoHash) -> Vec<KnownFile> {
    std::fs::read(known_path(dir, info_hash))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_known(dir: &std::path::Path, info_hash: &InfoHash, known: &[KnownFile]) {
    if let Ok(bytes) = serde_json::to_vec(known)
        && let Err(e) = std::fs::write(known_path(dir, info_hash), bytes)
    {
        warn!(error = %e, "could not persist file info");
    }
}

fn new_session_id() -> String {
    let bytes: [u8; 8] = rand::random();
    data_encoding::HEXLOWER.encode(&bytes)
}

const OSHASH_CHUNK: u64 = 64 * 1024;

/// OpenSubtitles hash: file size plus the 64-bit little-endian word sums of the first
/// and last 64 KiB. Waits until those pieces are downloaded.
async fn opensubtitles_hash(session: &StreamSession) -> anyhow::Result<String> {
    let mut stream = session.handle.clone().stream(session.file_idx).await?;
    let len = session.file_len;
    let chunk = OSHASH_CHUNK.min(len) as usize;
    let mut head = vec![0u8; chunk];
    stream.read_exact(&mut head).await?;
    stream.seek(SeekFrom::Start(len - chunk as u64)).await?;
    let mut tail = vec![0u8; chunk];
    stream.read_exact(&mut tail).await?;
    Ok(oshash_from_parts(len, &head, &tail))
}

pub fn oshash_from_parts(len: u64, head: &[u8], tail: &[u8]) -> String {
    let sum = |data: &[u8]| {
        data.chunks(8).fold(0u64, |acc, c| {
            let mut word = [0u8; 8];
            word[..c.len()].copy_from_slice(c);
            acc.wrapping_add(u64::from_le_bytes(word))
        })
    };
    let hash = len.wrapping_add(sum(head)).wrapping_add(sum(tail));
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oshash_matches_reference_algorithm() {
        let data: Vec<u8> = (0..131072u32).map(|i| (i % 251) as u8).collect();
        let len = data.len() as u64;
        let head = &data[..65536];
        let tail = &data[data.len() - 65536..];
        assert_eq!(oshash_from_parts(len, head, tail), reference(&data));
    }

    fn reference(data: &[u8]) -> String {
        let mut h = data.len() as u64;
        for part in [&data[..65536], &data[data.len() - 65536..]] {
            for word in part.chunks_exact(8) {
                h = h.wrapping_add(u64::from_le_bytes(word.try_into().unwrap()));
            }
        }
        format!("{h:016x}")
    }

    #[test]
    fn session_ids_are_unique_hex() {
        let a = new_session_id();
        let b = new_session_id();
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
    }
}
