#![allow(dead_code)]

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use localdebrid::config::{Config, TlsMode, UpstreamConfig};
use localdebrid::{AppState, Server, StartOptions};
use localdebrid_testkit::{FixtureAddon, Seeder, Tracker};
use serde_json::Value;

pub const MOVIE_ID: &str = "tt0000001";

pub struct Harness {
    pub server: Option<Server>,
    pub base: String,
    pub client: reqwest::Client,
    pub tracker: Tracker,
    pub addon: FixtureAddon,
    pub tmp: tempfile::TempDir,
}

pub fn test_config(addon: &FixtureAddon) -> Config {
    let mut config = Config::default();
    config.listen_host = "127.0.0.1".into();
    config.port = 0;
    config.tls.mode = TlsMode::Off;
    config.upstreams = vec![UpstreamConfig {
        name: "fixture".into(),
        url: addon.manifest_url.clone(),
        enabled: true,
    }];
    config.engine.enable_dht = false;
    config.engine.extra_trackers = vec![];
    config.engine.metadata_timeout_secs = 20;
    config.engine.read_stall_timeout_secs = 10;
    config
}

impl Harness {
    pub async fn new(customize: impl FnOnce(&mut Config)) -> Harness {
        localdebrid::logbuf::init_logging("warn");
        let tmp = tempfile::tempdir().unwrap();
        let tracker = Tracker::start(Ipv4Addr::LOCALHOST).await.unwrap();
        let addon = FixtureAddon::start(Ipv4Addr::LOCALHOST).await.unwrap();
        let mut config = test_config(&addon);
        customize(&mut config);
        let server = Server::start(StartOptions {
            data_dir: tmp.path().join("data"),
            cache_dir: tmp.path().join("cache"),
            config_override: Some(config),
        })
        .await
        .unwrap();
        let base = format!("http://127.0.0.1:{}", server.local_addr().port());
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap();
        Harness {
            server: Some(server),
            base,
            client,
            tracker,
            addon,
            tmp,
        }
    }

    pub fn state(&self) -> &Arc<AppState> {
        self.server.as_ref().unwrap().state()
    }

    pub fn content_dir(&self) -> PathBuf {
        let dir = self.tmp.path().join("content");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.tmp.path().join("cache")
    }

    pub async fn seed(&self, content: &Path) -> Seeder {
        Seeder::start(content, &self.tracker.announce_url).await.unwrap()
    }

    pub async fn get_json(&self, path: &str) -> (u16, Value) {
        let response = self.client.get(format!("{}{path}", self.base)).send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    pub async fn stream_urls(&self, kind: &str, id: &str) -> Vec<String> {
        let (status, body) = self.get_json(&format!("/stream/{kind}/{id}.json")).await;
        assert_eq!(status, 200, "{body}");
        body["streams"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["url"].as_str().unwrap().to_string())
            .collect()
    }

    pub async fn get_range(&self, url: &str, range: Option<&str>) -> reqwest::Response {
        let mut request = self.client.get(url);
        if let Some(r) = range {
            request = request.header("Range", r);
        }
        request.send().await.unwrap()
    }

    pub async fn shutdown(mut self) {
        if let Some(server) = self.server.take() {
            server.stop().await;
        }
    }
}

/// Polls `condition` until true or the timeout expires.
pub async fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    condition()
}
