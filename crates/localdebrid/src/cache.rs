//! Temporary storage policy: every torrent lives in `<cache>/<infohash>/`; directories
//! not used by an open session are evicted by TTL and by total size (oldest first).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tracing::{info, warn};

use crate::infohash::InfoHash;

/// Bytes actually allocated on disk. Torrent files are sparse until downloaded, so the
/// apparent length would overstate usage.
pub fn allocated_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            return meta.blocks() * 512;
        }
        #[cfg(not(unix))]
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| allocated_bytes(&e.path()))
                .sum()
        })
        .unwrap_or(0)
}

fn last_used(path: &Path) -> SystemTime {
    fn newest(path: &Path) -> Option<SystemTime> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        let own = meta.modified().ok();
        if !meta.is_dir() {
            return own;
        }
        std::fs::read_dir(path)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|e| newest(&e.path()))
            .chain(own)
            .max()
    }
    newest(path).unwrap_or(SystemTime::UNIX_EPOCH)
}

pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        Some(stat.f_bavail as u64 * stat.f_frsize as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    pub info_hash: String,
    pub bytes: u64,
    pub last_used_ms: u64,
    pub active: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepReport {
    pub deleted: Vec<String>,
    pub freed_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub ttl: Option<Duration>,
    pub max_bytes: Option<u64>,
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn entries(&self, active: &HashSet<InfoHash>) -> Vec<CacheEntry> {
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut entries: Vec<CacheEntry> = read
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let hash = InfoHash::parse(&name).ok()?;
                let path = e.path();
                Some(CacheEntry {
                    info_hash: hash.to_string(),
                    bytes: allocated_bytes(&path),
                    last_used_ms: last_used(&path)
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                    active: active.contains(&hash),
                })
            })
            .collect();
        entries.sort_by_key(|e| e.last_used_ms);
        entries
    }

    pub fn used_bytes(&self) -> u64 {
        allocated_bytes(&self.dir)
    }

    async fn delete(&self, info_hash: &str) -> anyhow::Result<()> {
        let path = self.dir.join(info_hash);
        if path.exists() {
            tokio::fs::remove_dir_all(&path).await?;
        }
        Ok(())
    }

    /// Applies TTL then size limit to inactive entries, oldest first.
    pub async fn sweep(&self, active: &HashSet<InfoHash>, policy: Policy) -> SweepReport {
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let entries = self.entries(active);
        let mut total: u64 = entries.iter().map(|e| e.bytes).sum();
        let mut report = SweepReport::default();
        for entry in entries.iter().filter(|e| !e.active) {
            let expired = policy
                .ttl
                .is_some_and(|ttl| now_ms.saturating_sub(entry.last_used_ms) >= ttl.as_millis() as u64);
            let over_limit = policy.max_bytes.is_some_and(|max| total > max);
            if !(expired || over_limit) {
                continue;
            }
            match self.delete(&entry.info_hash).await {
                Ok(()) => {
                    total = total.saturating_sub(entry.bytes);
                    report.freed_bytes += entry.bytes;
                    report.deleted.push(entry.info_hash.clone());
                }
                Err(e) => warn!(info_hash = %entry.info_hash, error = %e, "cache eviction failed"),
            }
        }
        if let Some(max) = policy.max_bytes
            && total > max
        {
            warn!(total, max, "cache above limit, but everything left belongs to open sessions");
        }
        if !report.deleted.is_empty() {
            info!(count = report.deleted.len(), freed = report.freed_bytes, "cache swept");
        }
        report
    }

    /// Deletes every inactive entry.
    pub async fn clear(&self, active: &HashSet<InfoHash>) -> SweepReport {
        let mut report = SweepReport::default();
        for entry in self.entries(active).into_iter().filter(|e| !e.active) {
            if self.delete(&entry.info_hash).await.is_ok() {
                report.freed_bytes += entry.bytes;
                report.deleted.push(entry.info_hash);
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(n: u8) -> InfoHash {
        InfoHash([n; 20])
    }

    fn make_entry(dir: &Path, h: InfoHash, size: usize, age: Duration) {
        let d = dir.join(h.to_string());
        std::fs::create_dir_all(d.join("sub")).unwrap();
        let file = d.join("sub/video.mkv");
        std::fs::write(&file, vec![1u8; size]).unwrap();
        let when = SystemTime::now() - age;
        let f = std::fs::File::options().write(true).open(&file).unwrap();
        f.set_modified(when).unwrap();
        for p in [d.join("sub"), d.clone()] {
            std::fs::File::open(&p).unwrap().set_modified(when).unwrap();
        }
    }

    #[tokio::test]
    async fn ttl_deletes_only_old_inactive_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path().to_path_buf());
        make_entry(tmp.path(), hash(1), 10_000, Duration::from_secs(7200));
        make_entry(tmp.path(), hash(2), 10_000, Duration::from_secs(10));
        make_entry(tmp.path(), hash(3), 10_000, Duration::from_secs(7200));
        let active: HashSet<_> = [hash(3)].into();
        let report = cache
            .sweep(&active, Policy { ttl: Some(Duration::from_secs(3600)), max_bytes: None })
            .await;
        assert_eq!(report.deleted, vec![hash(1).to_string()]);
        assert!(tmp.path().join(hash(2).to_string()).exists());
        assert!(tmp.path().join(hash(3).to_string()).exists());
    }

    #[tokio::test]
    async fn size_limit_evicts_oldest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path().to_path_buf());
        make_entry(tmp.path(), hash(1), 100_000, Duration::from_secs(300));
        make_entry(tmp.path(), hash(2), 100_000, Duration::from_secs(200));
        make_entry(tmp.path(), hash(3), 100_000, Duration::from_secs(100));
        let one = cache.entries(&HashSet::new())[0].bytes;
        let report = cache
            .sweep(&HashSet::new(), Policy { ttl: None, max_bytes: Some(one * 2) })
            .await;
        assert_eq!(report.deleted, vec![hash(1).to_string()]);
        assert_eq!(cache.entries(&HashSet::new()).len(), 2);
    }

    #[tokio::test]
    async fn clear_keeps_active_and_ignores_foreign_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path().to_path_buf());
        make_entry(tmp.path(), hash(1), 1000, Duration::ZERO);
        make_entry(tmp.path(), hash(2), 1000, Duration::ZERO);
        std::fs::create_dir(tmp.path().join("not-a-torrent")).unwrap();
        let report = cache.clear(&[hash(2)].into()).await;
        assert_eq!(report.deleted, vec![hash(1).to_string()]);
        assert!(tmp.path().join("not-a-torrent").exists());
        assert!(tmp.path().join(hash(2).to_string()).exists());
    }

    #[test]
    fn sparse_files_count_allocated_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let f = std::fs::File::create(tmp.path().join("sparse")).unwrap();
        f.set_len(1 << 30).unwrap();
        assert!(allocated_bytes(tmp.path()) < 1 << 20);
    }

    #[test]
    fn free_space_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(free_disk_bytes(tmp.path()).unwrap() > 0);
    }
}
