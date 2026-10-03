use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Extension;
use serde_json::{Value, json};

use super::{ApiError, BasePath, ConnInfo, stream_base_url};
use crate::app::AppState;
use crate::present::{self, episode_from_id};
use crate::selection::parse_episode;
use crate::sessions::KnownFile;
use crate::upstream::{TorrentCandidate, is_valid_content_id};

pub const ADDON_ID: &str = "com.localdebrid.addon";
const TYPES: &[&str] = &["movie", "series", "anime"];
const ID_PREFIXES: &[&str] = &["tt", "kitsu"];

pub fn manifest_json(name: &str) -> Value {
    json!({
        "id": ADDON_ID,
        "version": env!("CARGO_PKG_VERSION"),
        "name": name,
        "description": "Streams torrents through your own phone over the local network. No debrid service involved.",
        "resources": [{ "name": "stream", "types": TYPES, "idPrefixes": ID_PREFIXES }],
        "types": TYPES,
        "idPrefixes": ID_PREFIXES,
        "catalogs": [],
        "behaviorHints": { "configurable": true, "configurationRequired": false }
    })
}

pub async fn manifest(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(manifest_json(&state.config().addon_name))
}

pub async fn streams_with_extra(
    state: State<Arc<AppState>>,
    conn: Option<Extension<ConnInfo>>,
    base: Extension<BasePath>,
    headers: HeaderMap,
    Path((kind, id, _extra)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    streams(state, conn, base, headers, Path((kind, format!("{id}.json")))).await
}

pub async fn streams(
    State(state): State<Arc<AppState>>,
    conn: Option<Extension<ConnInfo>>,
    Extension(base): Extension<BasePath>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let id = id.strip_suffix(".json").unwrap_or(&id).to_string();
    if !is_valid_content_id(&kind, &id) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid type or id"));
    }
    let config = state.config();
    let candidates = state.upstream.find(&config.upstreams, &kind, &id).await;
    let episode = episode_from_id(&id);
    let base_url = stream_base_url(&state, conn.map(|c| c.0), &headers, &base);
    let streams = present::present(&candidates, &config, episode, &base_url, |c| {
        known_file(&state.sessions.load_known(&c.info_hash), c, episode)
    });
    Ok(Json(json!({ "streams": streams })))
}

fn known_file(
    known: &[KnownFile],
    candidate: &TorrentCandidate,
    episode: Option<crate::selection::Episode>,
) -> Option<KnownFile> {
    if let Some(idx) = candidate.file_idx {
        return known.iter().find(|k| k.file_idx == idx).cloned();
    }
    match episode {
        Some(ep) => known
            .iter()
            .find(|k| {
                parse_episode(&k.path).is_some_and(|(season, episodes)| {
                    season.unwrap_or(ep.season) == ep.season && episodes.contains(&ep.episode)
                })
            })
            .cloned(),
        None if known.len() == 1 => known.first().cloned(),
        None => None,
    }
}
