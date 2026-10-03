use std::path::PathBuf;

use clap::Parser;
use localdebrid::config::{ConfigStore, TlsMode};
use localdebrid::{StartOptions, run_supervised};
use tokio_util::sync::CancellationToken;

/// Runs the Local Debrid server (desktop, Termux or any Linux host).
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Directory for configuration and state.
    #[arg(long, env = "LOCALDEBRID_DATA_DIR", default_value = "./localdebrid-data")]
    data_dir: PathBuf,
    /// Directory for downloaded torrent data. Defaults to <data-dir>/cache.
    #[arg(long, env = "LOCALDEBRID_CACHE_DIR")]
    cache_dir: Option<PathBuf>,
    /// Overrides the configured HTTP port (persisted).
    #[arg(long, env = "LOCALDEBRID_PORT")]
    port: Option<u16>,
    /// Overrides the access token (persisted). Use "" to disable.
    #[arg(long, env = "LOCALDEBRID_TOKEN")]
    token: Option<String>,
    /// Disables HTTPS (persisted).
    #[arg(long)]
    no_tls: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    localdebrid::logbuf::init_logging("info,librqbit=warn,librqbit_dht=warn,librqbit_tracker_comms=warn");
    let args = Args::parse();
    std::fs::create_dir_all(&args.data_dir)?;
    let store = ConfigStore::new(&args.data_dir);
    let mut config = store.load()?;
    if let Some(port) = args.port {
        config.port = port;
    }
    if let Some(token) = args.token {
        config.access_token = token;
    }
    if args.no_tls {
        config.tls.mode = TlsMode::Off;
    }
    store.save(&config)?;

    let cache_dir = args.cache_dir.unwrap_or_else(|| args.data_dir.join("cache"));
    let stop = CancellationToken::new();
    tokio::spawn({
        let stop = stop.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            stop.cancel();
        }
    });
    run_supervised(
        StartOptions {
            data_dir: args.data_dir,
            cache_dir,
            config_override: None,
        },
        stop,
        |_| {},
    )
    .await
}
