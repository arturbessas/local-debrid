//! Local Debrid: a Stremio addon that streams torrents from the device it runs on.

pub mod app;
pub mod cache;
pub mod compat;
pub mod config;
pub mod engine;
pub mod http;
pub mod infohash;
pub mod logbuf;
pub mod net;
pub mod present;
pub mod range;
pub mod release;
pub mod selection;
pub mod server;
pub mod sessions;
pub mod tls;
pub mod upstream;

pub use app::{AppState, Server, StartOptions, Status, run_supervised};
pub use config::Config;
