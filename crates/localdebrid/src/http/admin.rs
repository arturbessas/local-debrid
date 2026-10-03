use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::{Extension, Json};
use percent_encoding::utf8_percent_encode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ApiError, BasePath, ConnInfo, stream_base_url};
use crate::app::AppState;
use crate::config::Config;
use crate::infohash::InfoHash;
use crate::logbuf::LogBuffer;
use crate::selection::{Episode, Selector};

const DASHBOARD: &str = include_str!("dashboard.html");

pub async fn dashboard() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Html(DASHBOARD),
    )
}

pub async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(serde_json::to_value(state.status()).unwrap_or_default())
}

fn session_url(base_url: &str, id: &str, file_name: &str) -> String {
    format!(
        "{base_url}/video/{id}/{}",
        utf8_percent_encode(file_name, crate::present::SEGMENT)
    )
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    conn: Option<Extension<ConnInfo>>,
    Extension(base): Extension<BasePath>,
    headers: HeaderMap,
) -> Json<Value> {
    let base_url = stream_base_url(&state, conn.map(|c| c.0), &headers, &base);
    let sessions: Vec<Value> = state
        .sessions
        .list()
        .iter()
        .map(|s| {
            let mut v = serde_json::to_value(s.dto(&state.engine)).unwrap_or_default();
            v["url"] = json!(session_url(&base_url, &s.id, s.file_name()));
            v
        })
        .collect();
    Json(json!({ "sessions": sessions }))
}

pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such session"))?;
    Ok(Json(serde_json::to_value(session.dto(&state.engine)).unwrap_or_default()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSession {
    magnet: Option<String>,
    info_hash: Option<String>,
    file_idx: Option<usize>,
    season: Option<u32>,
    episode: Option<u32>,
    #[serde(default)]
    trackers: Vec<String>,
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    conn: Option<Extension<ConnInfo>>,
    Extension(base): Extension<BasePath>,
    headers: HeaderMap,
    Json(body): Json<CreateSession>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut trackers = body.trackers;
    let info_hash = match (&body.magnet, &body.info_hash) {
        (Some(magnet), _) => {
            if let Ok(url) = url::Url::parse(magnet) {
                trackers.extend(
                    url.query_pairs()
                        .filter(|(k, _)| k == "tr")
                        .map(|(_, v)| v.into_owned()),
                );
            }
            InfoHash::from_magnet(magnet)
        }
        (None, Some(hash)) => InfoHash::parse(hash),
        (None, None) => return Err(ApiError::new(StatusCode::BAD_REQUEST, "magnet or infoHash is required")),
    }
    .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    let episode = match (body.season, body.episode) {
        (Some(season), Some(episode)) => Some(Episode { season, episode }),
        (None, None) => None,
        _ => return Err(ApiError::new(StatusCode::BAD_REQUEST, "season and episode go together")),
    };
    let selector = Selector {
        file_idx: body.file_idx,
        episode,
        filename_hint: None,
    };
    let session = super::play::open_session(&state, info_hash, selector, trackers, String::new()).await?;
    let base_url = stream_base_url(&state, conn.map(|c| c.0), &headers, &base);
    let mut value = serde_json::to_value(session.dto(&state.engine)).unwrap_or_default();
    value["url"] = json!(session_url(&base_url, &session.id, session.file_name()));
    Ok((StatusCode::CREATED, Json(value)))
}

#[derive(Deserialize)]
pub struct DeleteParams {
    #[serde(rename = "deleteData")]
    delete_data: Option<bool>,
}

pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<DeleteParams>,
) -> Result<StatusCode, ApiError> {
    let delete = params
        .delete_data
        .unwrap_or(state.config().cache.delete_on_session_end);
    if state.sessions.close(&id, delete).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, "no such session"))
    }
}

pub async fn cache(State(state): State<Arc<AppState>>) -> Json<Value> {
    let active = state.sessions.active_hashes();
    Json(json!({
        "dir": state.cache.dir().display().to_string(),
        "usedBytes": state.cache.used_bytes(),
        "limitBytes": state.config().max_cache_bytes(),
        "freeDiskBytes": crate::cache::free_disk_bytes(state.cache.dir()),
        "entries": state.cache.entries(&active),
    }))
}

#[derive(Deserialize)]
pub struct ClearParams {
    /// Also end every open session first, so all data can be deleted.
    #[serde(default)]
    all: bool,
}

pub async fn clear_cache(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ClearParams>,
) -> Json<Value> {
    let closed = if params.all {
        state.sessions.close_all(true).await
    } else {
        0
    };
    let report = state.cache.clear(&state.sessions.active_hashes()).await;
    Json(json!({ "closedSessions": closed, "deleted": report.deleted, "freedBytes": report.freed_bytes }))
}

pub async fn get_config(State(state): State<Arc<AppState>>) -> Json<Config> {
    Json((*state.config()).clone())
}

pub async fn put_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    // Merge so clients can send only the fields they change.
    let mut merged = serde_json::to_value(&*state.config()).unwrap_or_default();
    merge_json(&mut merged, body);
    let config: Config = serde_json::from_value(merged)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    config
        .validate()
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    let restart_required = state
        .apply_config(config)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({ "restartRequired": restart_required, "config": *state.config() })))
}

fn merge_json(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                merge_json(t.entry(k).or_insert(Value::Null), v);
            }
        }
        (t, p) => *t = p,
    }
}

#[derive(Deserialize)]
pub struct LogParams {
    lines: Option<usize>,
}

pub async fn logs(Query(params): Query<LogParams>) -> Json<Value> {
    Json(json!({ "lines": LogBuffer::global().tail(params.lines.unwrap_or(200)) }))
}

pub async fn restart(State(state): State<Arc<AppState>>) -> StatusCode {
    state.request_restart();
    StatusCode::ACCEPTED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_merge_is_deep() {
        let mut target = json!({"a": 1, "cache": {"ttlHours": 12, "maxSizeGb": 10}});
        merge_json(&mut target, json!({"cache": {"ttlHours": 1}, "b": [1]}));
        assert_eq!(target, json!({"a": 1, "b": [1], "cache": {"ttlHours": 1, "maxSizeGb": 10}}));
    }
}
