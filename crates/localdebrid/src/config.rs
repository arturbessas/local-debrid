use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 8765;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Address the HTTP(S) listener binds to. `0.0.0.0` makes it reachable from the LAN.
    pub listen_host: String,
    /// 0 picks a free port (useful for tests).
    pub port: u16,
    /// Host advertised in plain-HTTP stream URLs. Empty means "the local address the
    /// client connected to".
    pub public_host: String,
    /// When non-empty, every request must carry it as the first path segment, as a
    /// `Bearer` token or as `?token=`.
    pub access_token: String,
    /// Refuse requests coming from non-private (internet) addresses.
    pub lan_only: bool,
    pub addon_name: String,
    pub upstreams: Vec<UpstreamConfig>,
    pub max_streams: usize,
    /// Resolution groups listed, see [`RESOLUTION_GROUPS`].
    pub resolutions: Vec<String>,
    /// Streams listed per resolution group. 0 disables the limit.
    pub max_per_resolution: usize,
    pub hide_incompatible: bool,
    pub hide_low_quality: bool,
    /// Upper bound for a single video file, in GiB. 0 disables the limit.
    pub max_file_size_gb: f64,
    pub device: DeviceProfile,
    pub tls: TlsConfig,
    pub stream_url_scheme: StreamUrlScheme,
    pub cache: CacheConfig,
    pub engine: EngineConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_host: "0.0.0.0".into(),
            port: DEFAULT_PORT,
            public_host: String::new(),
            access_token: String::new(),
            lan_only: true,
            addon_name: "Local Debrid".into(),
            upstreams: vec![UpstreamConfig {
                name: "Torrentio".into(),
                url: "https://torrentio.strem.fun/manifest.json".into(),
                enabled: true,
            }],
            max_streams: 20,
            resolutions: RESOLUTION_GROUPS.iter().map(|s| s.to_string()).collect(),
            max_per_resolution: 0,
            hide_incompatible: true,
            hide_low_quality: true,
            max_file_size_gb: 0.0,
            device: DeviceProfile::default(),
            tls: TlsConfig::default(),
            stream_url_scheme: StreamUrlScheme::Http,
            cache: CacheConfig::default(),
            engine: EngineConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct UpstreamConfig {
    pub name: String,
    /// Manifest URL of any Stremio addon that returns `infoHash` streams.
    pub url: String,
    pub enabled: bool,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            enabled: true,
        }
    }
}

impl UpstreamConfig {
    /// Base URL that resource paths are appended to.
    pub fn base_url(&self) -> String {
        let url = self.url.trim().trim_end_matches('/');
        url.strip_suffix("/manifest.json").unwrap_or(url).to_string()
    }
}

/// What the playback device can decode. Defaults describe a 4K Roku on a TV without
/// Dolby Vision and without a DTS-capable receiver.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DeviceProfile {
    pub max_resolution: u32,
    pub hevc: bool,
    pub av1: bool,
    pub vp9: bool,
    pub hdr10: bool,
    pub dolby_vision: bool,
    pub dts: bool,
    pub truehd: bool,
}

impl Default for DeviceProfile {
    fn default() -> Self {
        Self {
            max_resolution: 2160,
            hevc: true,
            av1: false,
            vp9: false,
            hdr10: true,
            dolby_vision: false,
            dts: false,
            truehd: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TlsMode {
    /// Plain HTTP only.
    Off,
    /// `<a-b-c-d>.local-ip.sh` wildcard DNS with the certificate that service publishes.
    LocalIp,
    /// Certificate and key files supplied by the user, for a hostname they control.
    Custom,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct TlsConfig {
    pub mode: TlsMode,
    pub local_ip_domain: String,
    pub local_ip_cert_url: String,
    pub local_ip_key_url: String,
    pub custom_hostname: String,
    pub custom_cert_path: String,
    pub custom_key_path: String,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            mode: TlsMode::LocalIp,
            local_ip_domain: "local-ip.sh".into(),
            local_ip_cert_url: "https://local-ip.sh/server.pem".into(),
            local_ip_key_url: "https://local-ip.sh/server.key".into(),
            custom_hostname: String::new(),
            custom_cert_path: String::new(),
            custom_key_path: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StreamUrlScheme {
    /// `http://<lan-ip>:<port>/...`, independent of DNS and cheaper for the phone.
    Http,
    /// Same scheme and host the addon request used.
    SameAsRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct CacheConfig {
    /// A session with no open HTTP connection for this long is closed.
    pub idle_session_minutes: u64,
    /// Delete the downloaded file as soon as its session closes.
    pub delete_on_session_end: bool,
    /// Downloaded data not used for this long is deleted. 0 disables the TTL.
    pub ttl_hours: f64,
    /// Upper bound for the cache directory, in GiB. 0 disables the limit.
    pub max_size_gb: f64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            idle_session_minutes: 20,
            delete_on_session_end: false,
            ttl_hours: 12.0,
            max_size_gb: 10.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct EngineConfig {
    /// BitTorrent listen port (TCP). 0 picks a random free port.
    pub bt_port: u16,
    pub enable_dht: bool,
    pub enable_upload: bool,
    pub download_limit_kbps: u32,
    pub upload_limit_kbps: u32,
    pub peer_limit: usize,
    pub metadata_timeout_secs: u64,
    /// An HTTP response that makes no progress for this long is aborted.
    pub read_stall_timeout_secs: u64,
    pub extra_trackers: Vec<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            bt_port: 0,
            enable_dht: true,
            enable_upload: true,
            download_limit_kbps: 0,
            upload_limit_kbps: 0,
            peer_limit: 100,
            metadata_timeout_secs: 60,
            read_stall_timeout_secs: 120,
            extra_trackers: DEFAULT_TRACKERS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

pub const RESOLUTION_GROUPS: &[&str] = &["4k", "1440p", "1080p", "720p", "sd", "unknown"];

pub const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://explodie.org:6969/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://tracker.moeking.me:6969/announce",
    "http://tracker.opentrackr.org:1337/announce",
];

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.listen_host
            .parse::<std::net::IpAddr>()
            .context("listenHost must be an IP address")?;
        if !self
            .access_token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!("accessToken may only contain letters, digits, '-' and '_'");
        }
        if matches!(self.access_token.as_str(), "api" | "play" | "stream" | "manifest.json") {
            bail!("accessToken collides with a route name");
        }
        for upstream in &self.upstreams {
            let url = url::Url::parse(upstream.url.trim())
                .with_context(|| format!("invalid upstream URL {:?}", upstream.url))?;
            if !matches!(url.scheme(), "http" | "https") {
                bail!("upstream URL must be http(s): {}", upstream.url);
            }
        }
        if self.max_streams == 0 {
            bail!("maxStreams must be at least 1");
        }
        if let Some(bad) = self.resolutions.iter().find(|r| !RESOLUTION_GROUPS.contains(&r.as_str())) {
            bail!("unknown resolution {bad:?}; expected one of {RESOLUTION_GROUPS:?}");
        }
        if self.tls.mode == TlsMode::Custom
            && (self.tls.custom_hostname.is_empty()
                || self.tls.custom_cert_path.is_empty()
                || self.tls.custom_key_path.is_empty())
        {
            bail!("custom TLS needs customHostname, customCertPath and customKeyPath");
        }
        if self.engine.metadata_timeout_secs == 0 || self.engine.read_stall_timeout_secs == 0 {
            bail!("engine timeouts must be positive");
        }
        if self.cache.idle_session_minutes == 0 {
            bail!("idleSessionMinutes must be positive");
        }
        Ok(())
    }

    pub fn max_cache_bytes(&self) -> Option<u64> {
        gib_to_bytes(self.cache.max_size_gb)
    }

    pub fn max_file_bytes(&self) -> Option<u64> {
        gib_to_bytes(self.max_file_size_gb)
    }
}

fn gib_to_bytes(gib: f64) -> Option<u64> {
    (gib > 0.0).then(|| (gib * 1024.0 * 1024.0 * 1024.0) as u64)
}

/// Persists the configuration as JSON next to the rest of the server state.
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("config.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> anyhow::Result<Config> {
        if !self.path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        let config: Config = serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", self.path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, config: &Config) -> anyhow::Result<()> {
        config.validate()?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(config)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn partial_json_is_completed_with_defaults() {
        let config: Config = serde_json::from_str(r#"{"port": 9000, "cache": {"ttlHours": 1}}"#).unwrap();
        assert_eq!(config.port, 9000);
        assert_eq!(config.cache.ttl_hours, 1.0);
        assert_eq!(config.cache.idle_session_minutes, 20);
        assert_eq!(config.addon_name, "Local Debrid");
    }

    #[test]
    fn token_with_path_characters_is_rejected() {
        let config = Config {
            access_token: "a/b".into(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn unknown_resolution_group_is_rejected() {
        let config = Config {
            resolutions: vec!["8k".into()],
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn upstream_base_url_strips_manifest() {
        let upstream = UpstreamConfig {
            url: "https://torrentio.strem.fun/providers=yts/manifest.json".into(),
            ..Default::default()
        };
        assert_eq!(upstream.base_url(), "https://torrentio.strem.fun/providers=yts");
    }

    #[test]
    fn store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path());
        assert_eq!(store.load().unwrap(), Config::default());
        let config = Config {
            port: 9999,
            ..Default::default()
        };
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap().port, 9999);
    }
}
