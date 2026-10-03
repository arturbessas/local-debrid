//! HTTPS for clients that refuse plain-HTTP addon URLs (Stroku Native validates the
//! manifest URL starts with `https://` and checks the certificate against Roku's CA
//! bundle, so a self-signed certificate cannot work).
//!
//! `local-ip` mode relies on a wildcard DNS service: `192-168-1-25.local-ip.sh`
//! resolves to `192.168.1.25`, and the service publishes a publicly trusted
//! certificate for `*.local-ip.sh` intended for exactly this LAN use.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use arc_swap::ArcSwapOption;
use parking_lot::Mutex;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use serde::Serialize;
use tokio_rustls::TlsAcceptor;
use tracing::{info, warn};

use crate::config::{TlsConfig, TlsMode};

const RENEW_BEFORE: Duration = Duration::from_secs(21 * 24 * 3600);

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsStatus {
    pub mode: String,
    pub ready: bool,
    pub expires_at_ms: Option<u64>,
    pub error: Option<String>,
}

struct Loaded {
    key: Arc<CertifiedKey>,
    not_after: SystemTime,
}

#[derive(Debug)]
struct Resolver {
    current: Arc<ArcSwapOption<CertifiedKey>>,
}

impl ResolvesServerCert for Resolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.current.load_full()
    }
}

pub struct TlsManager {
    config: TlsConfig,
    dir: PathBuf,
    http: reqwest::Client,
    current: Arc<ArcSwapOption<CertifiedKey>>,
    not_after: Mutex<Option<SystemTime>>,
    error: Mutex<Option<String>>,
    acceptor: Option<TlsAcceptor>,
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// Parses a PEM certificate chain and key, checking that they belong together and
/// that the leaf covers `expected_name` (a wildcard such as `*.local-ip.sh` is fine).
fn load_pem(cert_pem: &[u8], key_pem: &[u8], expected_name: &str) -> anyhow::Result<Loaded> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<_, _>>()
        .context("invalid certificate PEM")?;
    let leaf = chain.first().context("certificate PEM is empty")?;
    let key = PrivateKeyDer::from_pem_slice(key_pem).context("invalid private key PEM")?;

    let (_, parsed) = x509_parser::parse_x509_certificate(leaf).context("invalid certificate")?;
    let not_after = UNIX_EPOCH + Duration::from_secs(parsed.validity().not_after.timestamp().max(0) as u64);
    if not_after <= SystemTime::now() {
        bail!("certificate expired");
    }
    let names: Vec<String> = parsed
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    x509_parser::extensions::GeneralName::DNSName(d) => Some(d.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if !expected_name.is_empty() && !names.iter().any(|n| name_matches(n, expected_name)) {
        bail!("certificate does not cover {expected_name} (has {names:?})");
    }

    let signing = provider()
        .key_provider
        .load_private_key(key)
        .context("unsupported private key")?;
    let certified = CertifiedKey::new(chain, signing);
    certified
        .keys_match()
        .context("private key does not match certificate")?;
    Ok(Loaded {
        key: Arc::new(certified),
        not_after,
    })
}

fn name_matches(pattern: &str, name: &str) -> bool {
    let (pattern, name) = (pattern.to_ascii_lowercase(), name.to_ascii_lowercase());
    match pattern.strip_prefix("*.") {
        Some(suffix) => name
            .split_once('.')
            .is_some_and(|(label, rest)| !label.is_empty() && rest == suffix),
        None => pattern == name,
    }
}

impl TlsManager {
    pub fn new(config: TlsConfig, dir: PathBuf, http: reqwest::Client) -> Self {
        let current = Arc::new(ArcSwapOption::empty());
        let acceptor = (config.mode != TlsMode::Off).then(|| {
            let mut server = rustls::ServerConfig::builder_with_provider(provider())
                .with_safe_default_protocol_versions()
                .expect("default TLS versions are supported")
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(Resolver {
                    current: current.clone(),
                }));
            server.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            TlsAcceptor::from(Arc::new(server))
        });
        Self {
            config,
            dir,
            http,
            current,
            not_after: Mutex::new(None),
            error: Mutex::new(None),
            acceptor,
        }
    }

    pub fn mode(&self) -> TlsMode {
        self.config.mode
    }

    /// `None` until a certificate is loaded.
    pub fn acceptor(&self) -> Option<TlsAcceptor> {
        self.current.load().as_ref()?;
        self.acceptor.clone()
    }

    /// Hostname clients must use for HTTPS when they reach the phone at `ip`.
    pub fn https_host(&self, ip: Ipv4Addr) -> Option<String> {
        match self.config.mode {
            TlsMode::Off => None,
            TlsMode::LocalIp => Some(format!(
                "{}.{}",
                ip.to_string().replace('.', "-"),
                self.config.local_ip_domain
            )),
            TlsMode::Custom => Some(self.config.custom_hostname.clone()),
        }
    }

    /// Reverse of [`Self::https_host`] for local-ip names.
    pub fn ip_from_host(&self, host: &str) -> Option<Ipv4Addr> {
        let suffix = format!(".{}", self.config.local_ip_domain);
        let label = host.to_ascii_lowercase();
        let label = label.strip_suffix(&suffix)?;
        label.replace('-', ".").parse().ok()
    }

    pub fn status(&self) -> TlsStatus {
        TlsStatus {
            mode: format!("{:?}", self.config.mode),
            ready: self.current.load().is_some(),
            expires_at_ms: self
                .not_after
                .lock()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64),
            error: self.error.lock().clone(),
        }
    }

    fn install(&self, loaded: Loaded) {
        self.current.store(Some(loaded.key));
        *self.not_after.lock() = Some(loaded.not_after);
        *self.error.lock() = None;
    }

    fn needs_renewal(&self) -> bool {
        match *self.not_after.lock() {
            None => true,
            Some(t) => t
                .duration_since(SystemTime::now())
                .map(|left| left < RENEW_BEFORE)
                .unwrap_or(true),
        }
    }

    /// Loads or renews the certificate. Failures leave the previous certificate in
    /// place and are reported through [`Self::status`]; plain HTTP keeps working.
    pub async fn refresh(&self) {
        let result = match self.config.mode {
            TlsMode::Off => return,
            TlsMode::Custom => self.load_custom(),
            TlsMode::LocalIp => self.refresh_local_ip().await,
        };
        if let Err(e) = result {
            warn!(error = %format!("{e:#}"), "HTTPS certificate unavailable");
            *self.error.lock() = Some(format!("{e:#}"));
        }
    }

    fn load_custom(&self) -> anyhow::Result<()> {
        let cert = std::fs::read(&self.config.custom_cert_path)
            .with_context(|| format!("reading {}", self.config.custom_cert_path))?;
        let key = std::fs::read(&self.config.custom_key_path)
            .with_context(|| format!("reading {}", self.config.custom_key_path))?;
        let loaded = load_pem(&cert, &key, &self.config.custom_hostname)?;
        self.install(loaded);
        info!(host = %self.config.custom_hostname, "custom HTTPS certificate loaded");
        Ok(())
    }

    async fn refresh_local_ip(&self) -> anyhow::Result<()> {
        let probe_name = format!("192-168-0-1.{}", self.config.local_ip_domain);
        let cert_path = self.dir.join("local-ip.pem");
        let key_path = self.dir.join("local-ip.key");
        if self.current.load().is_none()
            && let (Ok(cert), Ok(key)) = (std::fs::read(&cert_path), std::fs::read(&key_path))
        {
            match load_pem(&cert, &key, &probe_name) {
                Ok(loaded) => self.install(loaded),
                Err(e) => warn!(error = %e, "cached certificate unusable"),
            }
        }
        if !self.needs_renewal() {
            return Ok(());
        }
        let fetch = |url: String| async move {
            let response = self
                .http
                .get(&url)
                .timeout(Duration::from_secs(30))
                .send()
                .await?
                .error_for_status()?;
            anyhow::Ok(response.bytes().await?)
        };
        let cert = fetch(self.config.local_ip_cert_url.clone())
            .await
            .context("downloading certificate")?;
        let key = fetch(self.config.local_ip_key_url.clone())
            .await
            .context("downloading key")?;
        let loaded = load_pem(&cert, &key, &probe_name)?;
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(&cert_path, &cert)?;
        std::fs::write(&key_path, &key)?;
        info!(expires = ?loaded.not_after, "local-ip certificate installed");
        self.install(loaded);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matching() {
        assert!(name_matches("*.local-ip.sh", "192-168-1-2.local-ip.sh"));
        assert!(!name_matches("*.local-ip.sh", "a.b.local-ip.sh"));
        assert!(!name_matches("*.local-ip.sh", "local-ip.sh"));
        assert!(name_matches("tv.example.org", "TV.example.org"));
    }

    #[test]
    fn local_ip_hostnames_round_trip() {
        let m = TlsManager::new(TlsConfig::default(), PathBuf::new(), reqwest::Client::new());
        let ip: Ipv4Addr = "192.168.1.25".parse().unwrap();
        let host = m.https_host(ip).unwrap();
        assert_eq!(host, "192-168-1-25.local-ip.sh");
        assert_eq!(m.ip_from_host(&host), Some(ip));
        assert_eq!(m.ip_from_host("example.com"), None);
    }

    #[test]
    fn off_mode_has_no_acceptor() {
        let config = TlsConfig {
            mode: TlsMode::Off,
            ..Default::default()
        };
        let m = TlsManager::new(config, PathBuf::new(), reqwest::Client::new());
        assert!(m.acceptor().is_none());
        assert!(m.https_host(Ipv4Addr::LOCALHOST).is_none());
    }

    #[test]
    fn rejects_garbage_pem() {
        assert!(load_pem(b"nope", b"nope", "").is_err());
    }
}
