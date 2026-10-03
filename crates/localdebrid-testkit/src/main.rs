//! Stand-alone swarm for the end-to-end script: seeds a file or directory, runs a
//! tracker and a fixture addon, prints their URLs as JSON and waits for Ctrl-C.

use std::net::Ipv4Addr;
use std::path::PathBuf;

use clap::Parser;
use localdebrid_testkit::{FixtureAddon, Seeder, Tracker, torrent_stream};
use serde_json::json;

#[derive(Parser)]
struct Args {
    /// File or directory to seed.
    #[arg(long)]
    content: PathBuf,
    /// Stremio id the fixture addon answers for (e.g. tt0000001 or tt0000002:1:2).
    #[arg(long, default_value = "tt0000001")]
    id: String,
    #[arg(long, default_value = "movie")]
    kind: String,
    /// Address other machines (e.g. an Android emulator: 10.0.2.2) use to reach this host.
    #[arg(long, default_value = "127.0.0.1")]
    advertise_host: Ipv4Addr,
    #[arg(long)]
    file_idx: Option<usize>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let tracker = Tracker::start(args.advertise_host).await?;
    let seeder = Seeder::start(&args.content, &tracker.local_announce_url).await?;
    let addon = FixtureAddon::start(args.advertise_host).await?;
    let title = args
        .content
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    addon.set(
        &args.kind,
        &args.id,
        json!([torrent_stream(&seeder.info_hash, args.file_idx, &title, &tracker.announce_url)]),
    );
    println!(
        "{}",
        json!({
            "infoHash": seeder.info_hash,
            "tracker": tracker.announce_url,
            "upstream": addon.manifest_url,
            "seederListen": seeder.listen_addr.map(|a| a.to_string()),
        })
    );
    tokio::signal::ctrl_c().await?;
    seeder.stop().await;
    Ok(())
}
