use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use arc_swap::ArcSwap;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{info, warn};

use crate::cache::{self, Cache, Policy};
use crate::config::{Config, ConfigStore};
use crate::engine::{Engine, TorrentSnapshot};
use crate::sessions::SessionManager;
use crate::tls::{TlsManager, TlsStatus};
use crate::upstream::UpstreamClient;

#[derive(Clone, Debug)]
pub struct StartOptions {
    /// Configuration, TLS material, torrent metadata, DHT state.
    pub data_dir: PathBuf,
    /// Downloaded torrent data.
    pub cache_dir: PathBuf,
    /// Used instead of `<data_dir>/config.json` when set (tests, CLI overrides).
    pub config_override: Option<Config>,
}

pub struct AppState {
    config: ArcSwap<Config>,
    /// Configuration the running listener and engine were built from.
    boot_config: Config,
    config_store: ConfigStore,
    pub engine: Arc<Engine>,
    pub sessions: SessionManager,
    pub upstream: UpstreamClient,
    pub cache: Cache,
    pub tls: TlsManager,
    /// Bounds open HTTP streams; see [`crate::engine::MAX_CONCURRENT_STREAMS`].
    pub stream_slots: Arc<tokio::sync::Semaphore>,
    pub port: u16,
    pub started: Instant,
    restart: Notify,
    persist_config: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Urls {
    pub ip: String,
    pub http_manifest: String,
    pub https_manifest: Option<String>,
    pub dashboard: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub version: &'static str,
    pub uptime_secs: u64,
    pub addon_name: String,
    pub port: u16,
    pub token_enabled: bool,
    pub urls: Vec<Urls>,
    pub tls: TlsStatus,
    pub bt_listen_port: Option<u16>,
    pub sessions: usize,
    pub active_streams: usize,
    pub engine: TorrentSnapshot,
    pub cache_used_bytes: u64,
    pub cache_limit_bytes: Option<u64>,
    pub free_disk_bytes: Option<u64>,
    pub restart_pending: bool,
}

impl AppState {
    pub fn config(&self) -> Arc<Config> {
        self.config.load_full()
    }

    /// Stores a new configuration. Returns whether a restart is needed for all of it to
    /// take effect (listener, TLS and engine settings are read only at start).
    pub fn apply_config(&self, config: Config) -> anyhow::Result<bool> {
        if self.persist_config {
            self.config_store.save(&config)?;
        }
        self.config.store(Arc::new(config));
        Ok(self.restart_pending())
    }

    fn restart_pending(&self) -> bool {
        let current = self.config();
        current.port != self.boot_config.port
            || current.listen_host != self.boot_config.listen_host
            || current.tls != self.boot_config.tls
            || current.engine != self.boot_config.engine
    }

    pub fn request_restart(&self) {
        self.restart.notify_one();
    }

    pub async fn restart_requested(&self) {
        self.restart.notified().await
    }

    pub fn urls(&self) -> Vec<Urls> {
        let config = self.config();
        let token_path = if config.access_token.is_empty() {
            String::new()
        } else {
            format!("/{}", config.access_token)
        };
        let mut ips: Vec<String> = crate::net::lan_ipv4s().iter().map(|i| i.to_string()).collect();
        if !config.public_host.is_empty() && !ips.contains(&config.public_host) {
            ips.insert(0, config.public_host.clone());
        }
        let tls_ready = self.tls.status().ready;
        ips.into_iter()
            .map(|ip| {
                let https_manifest = ip
                    .parse()
                    .ok()
                    .and_then(|v4| self.tls.https_host(v4))
                    .filter(|_| tls_ready)
                    .map(|host| format!("https://{host}:{}{token_path}/manifest.json", self.port));
                Urls {
                    http_manifest: format!("http://{ip}:{}{token_path}/manifest.json", self.port),
                    dashboard: format!("http://{ip}:{}{token_path}/", self.port),
                    https_manifest,
                    ip,
                }
            })
            .collect()
    }

    pub fn status(&self) -> Status {
        let config = self.config();
        let sessions = self.sessions.list();
        Status {
            version: env!("CARGO_PKG_VERSION"),
            uptime_secs: self.started.elapsed().as_secs(),
            addon_name: config.addon_name.clone(),
            port: self.port,
            token_enabled: !config.access_token.is_empty(),
            urls: self.urls(),
            tls: self.tls.status(),
            bt_listen_port: self.engine.bt_listen_addr().map(|a| a.port()),
            sessions: sessions.len(),
            active_streams: sessions.iter().map(|s| s.active_readers()).sum(),
            engine: self.engine.totals(),
            cache_used_bytes: self.cache.used_bytes(),
            cache_limit_bytes: config.max_cache_bytes(),
            free_disk_bytes: cache::free_disk_bytes(self.cache.dir()),
            restart_pending: self.restart_pending(),
        }
    }

    pub fn cache_policy(&self) -> Policy {
        let config = self.config();
        Policy {
            ttl: (config.cache.ttl_hours > 0.0)
                .then(|| Duration::from_secs_f64(config.cache.ttl_hours * 3600.0)),
            max_bytes: config.max_cache_bytes(),
        }
    }

    async fn maintenance_tick(&self) {
        let config = self.config();
        self.sessions
            .close_idle(
                Duration::from_secs(config.cache.idle_session_minutes * 60),
                config.cache.delete_on_session_end,
            )
            .await;
        self.cache
            .sweep(&self.sessions.active_hashes(), self.cache_policy())
            .await;
    }
}

pub struct Server {
    state: Arc<AppState>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    local_addr: SocketAddr,
}

impl Server {
    pub async fn start(options: StartOptions) -> anyhow::Result<Server> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let state_dir = options.data_dir.join("state");
        std::fs::create_dir_all(&state_dir)
            .with_context(|| format!("creating {}", state_dir.display()))?;
        std::fs::create_dir_all(&options.cache_dir)
            .with_context(|| format!("creating {}", options.cache_dir.display()))?;

        let config_store = ConfigStore::new(&options.data_dir);
        let persist_config = options.config_override.is_none();
        let config = match options.config_override.clone() {
            Some(c) => c,
            None => config_store.load()?,
        };
        config.validate()?;

        let http = reqwest::Client::builder()
            .user_agent(concat!("LocalDebrid/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .build()?;

        let tls = TlsManager::new(config.tls.clone(), options.data_dir.join("tls"), http.clone());
        if tokio::time::timeout(Duration::from_secs(25), tls.refresh()).await.is_err() {
            warn!("TLS certificate setup timed out; HTTPS will be retried later");
        }

        let listen_ip: IpAddr = config.listen_host.parse()?;
        let listener = TcpListener::bind((listen_ip, config.port))
            .await
            .with_context(|| format!("binding {}:{}", config.listen_host, config.port))?;
        let local_addr = listener.local_addr()?;

        let engine = Arc::new(
            Engine::start(options.cache_dir.clone(), state_dir.clone(), config.engine.clone()).await?,
        );
        let state = Arc::new(AppState {
            config: ArcSwap::from_pointee(config.clone()),
            boot_config: config,
            config_store,
            sessions: SessionManager::new(engine.clone(), state_dir),
            engine,
            upstream: UpstreamClient::new(http),
            cache: Cache::new(options.cache_dir.clone()),
            tls,
            stream_slots: Arc::new(tokio::sync::Semaphore::new(crate::engine::MAX_CONCURRENT_STREAMS)),
            port: local_addr.port(),
            started: Instant::now(),
            restart: Notify::new(),
            persist_config,
        });

        let shutdown = CancellationToken::new();
        let tasks = TaskTracker::new();
        tasks.spawn(crate::server::accept_loop(
            listener,
            state.clone(),
            shutdown.clone(),
            tasks.clone(),
        ));
        tasks.spawn({
            let state = state.clone();
            let shutdown = shutdown.clone();
            async move {
                let mut tick = tokio::time::interval(Duration::from_secs(15));
                let mut last_tls = Instant::now();
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        _ = tick.tick() => {
                            state.maintenance_tick().await;
                            let retry_tls = !state.tls.status().ready || last_tls.elapsed() > Duration::from_secs(6 * 3600);
                            if retry_tls && last_tls.elapsed() > Duration::from_secs(60) {
                                last_tls = Instant::now();
                                state.tls.refresh().await;
                            }
                        }
                    }
                }
            }
        });
        info!(addr = %local_addr, "Local Debrid listening");
        for url in state.urls() {
            info!(manifest = %url.http_manifest, https = ?url.https_manifest, "addon URL");
        }
        Ok(Server {
            state,
            shutdown,
            tasks,
            local_addr,
        })
    }

    pub fn state(&self) -> &Arc<AppState> {
        &self.state
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stops accepting connections, closes sessions (applying the cache policy) and
    /// shuts the torrent engine down.
    pub async fn stop(self) {
        self.shutdown.cancel();
        self.tasks.close();
        if tokio::time::timeout(Duration::from_secs(5), self.tasks.wait())
            .await
            .is_err()
        {
            warn!("connections did not close in time");
        }
        let delete = self.state.config().cache.delete_on_session_end;
        self.state.sessions.close_all(delete).await;
        self.state.engine.stop().await;
        info!("Local Debrid stopped");
    }
}

/// Runs the server until `stop` is cancelled, restarting it when asked through the API.
/// `on_start` sees every (re)started instance.
pub async fn run_supervised(
    options: StartOptions,
    stop: CancellationToken,
    on_start: impl Fn(Option<Arc<AppState>>),
) -> anyhow::Result<()> {
    loop {
        let server = Server::start(options.clone()).await?;
        on_start(Some(server.state().clone()));
        let restart = tokio::select! {
            _ = stop.cancelled() => false,
            _ = server.state().restart_requested() => true,
        };
        on_start(None);
        server.stop().await;
        if !restart {
            return Ok(());
        }
        info!("restarting with the new configuration");
    }
}
