use std::io::SeekFrom;
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::{debug, warn};

use super::ApiError;
use crate::app::AppState;
use crate::cache::free_disk_bytes;
use crate::engine::EngineError;
use crate::infohash::InfoHash;
use crate::range::{self, RangeError};
use crate::selection::{Episode, SelectionError, Selector};
use crate::sessions::{OpenLimits, OpenRequest, SessionError, StreamSession};

const CHUNK: usize = 64 * 1024;
const STREAM_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

pub fn mime_for(name: &str) -> &'static str {
    match crate::release::extension(name).as_deref() {
        Some("mkv") => "video/x-matroska",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("ts" | "m2ts" | "mts") => "video/mp2t",
        Some("avi") => "video/x-msvideo",
        Some("mpg" | "mpeg") => "video/mpeg",
        Some("wmv") => "video/x-ms-wmv",
        _ => "application/octet-stream",
    }
}

/// Parses the selector path segment produced by [`crate::present::selector_segment`].
pub fn parse_selector(segment: &str, filename: &str) -> Option<Selector> {
    let filename_hint = Some(filename.to_string()).filter(|f| !f.is_empty());
    if segment == "auto" {
        return Some(Selector {
            filename_hint,
            ..Default::default()
        });
    }
    if let Ok(idx) = segment.parse::<usize>() {
        return Some(Selector {
            file_idx: Some(idx),
            ..Default::default()
        });
    }
    let rest = segment.strip_prefix('s')?;
    let (season, episode) = rest.split_once('e')?;
    Some(Selector {
        episode: Some(Episode {
            season: season.parse().ok()?,
            episode: episode.parse().ok()?,
        }),
        filename_hint,
        ..Default::default()
    })
}

impl From<SessionError> for ApiError {
    fn from(e: SessionError) -> Self {
        let status = match &e {
            SessionError::Selection(SelectionError::NoSuchFile(_) | SelectionError::EpisodeNotFound { .. }) => {
                StatusCode::NOT_FOUND
            }
            SessionError::Selection(_) => StatusCode::UNPROCESSABLE_ENTITY,
            SessionError::Metadata(EngineError::MetadataTimeout(_)) => StatusCode::GATEWAY_TIMEOUT,
            SessionError::Metadata(EngineError::Other(_)) => StatusCode::BAD_GATEWAY,
            SessionError::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            SessionError::NoSpace { .. } => StatusCode::INSUFFICIENT_STORAGE,
            SessionError::Engine(_) => StatusCode::BAD_GATEWAY,
        };
        ApiError::new(status, e.to_string())
    }
}

pub async fn open_session(
    state: &AppState,
    info_hash: InfoHash,
    selector: Selector,
    extra_trackers: Vec<String>,
    title: String,
) -> Result<Arc<StreamSession>, ApiError> {
    let config = state.config();
    let hint = state.upstream.hint(&info_hash).unwrap_or_default();
    let mut trackers = hint.trackers;
    for t in extra_trackers {
        if !trackers.contains(&t) {
            trackers.push(t);
        }
    }
    let title = if hint.release_name.is_empty() { title } else { hint.release_name };
    let limits = OpenLimits {
        max_file_bytes: config.max_file_bytes(),
        free_bytes: free_disk_bytes(state.cache.dir()),
    };
    Ok(state
        .sessions
        .open(
            OpenRequest {
                info_hash,
                selector,
                trackers,
                title,
            },
            limits,
        )
        .await?)
}

pub async fn play(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
    Path((hash, selector, filename)): Path<(String, String, String)>,
) -> Response {
    let Ok(info_hash) = InfoHash::parse(&hash) else {
        return ApiError::new(StatusCode::BAD_REQUEST, "invalid info-hash").into_response();
    };
    let Some(selector) = parse_selector(&selector, &filename) else {
        return ApiError::new(StatusCode::BAD_REQUEST, "invalid file selector").into_response();
    };
    match open_session(&state, info_hash, selector, Vec::new(), filename).await {
        Ok(session) => serve(&state, session, &method, &headers).await,
        Err(e) => e.into_response(),
    }
}

pub async fn video(
    State(state): State<Arc<AppState>>,
    _base: Option<Extension<super::BasePath>>,
    method: Method,
    headers: HeaderMap,
    Path((id, _filename)): Path<(String, String)>,
) -> Response {
    match state.sessions.get(&id) {
        Some(session) => serve(&state, session, &method, &headers).await,
        None => ApiError::new(StatusCode::NOT_FOUND, "no such session").into_response(),
    }
}

fn content_disposition(name: &str) -> HeaderValue {
    let encoded = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
    HeaderValue::from_str(&format!("inline; filename*=UTF-8''{encoded}"))
        .unwrap_or_else(|_| HeaderValue::from_static("inline"))
}

pub async fn serve(
    state: &AppState,
    session: Arc<StreamSession>,
    method: &Method,
    headers: &HeaderMap,
) -> Response {
    session.touch();
    let len = session.file_len;
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let mut builder = Response::builder()
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, mime_for(session.file_name()))
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::ETAG, format!("\"{}-{}\"", session.info_hash, session.file_idx))
        .header(header::CONTENT_DISPOSITION, content_disposition(session.file_name()));

    let (start, count) = match range::parse(range_header, len) {
        Err(RangeError::Unsatisfiable) => {
            return builder
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())
                .unwrap();
        }
        Ok(None) => {
            builder = builder.status(StatusCode::OK);
            (0, len)
        }
        Ok(Some(r)) => {
            builder = builder
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {}-{}/{len}", r.start, r.end));
            (r.start, r.len())
        }
    };
    builder = builder.header(header::CONTENT_LENGTH, count);

    if *method == Method::HEAD || count == 0 {
        return builder.body(Body::empty()).unwrap();
    }

    let acquire = async {
        let slot = state.stream_slots.clone().acquire_owned().await?;
        let stream = session.handle().clone().stream(session.file_idx).await?;
        anyhow::Ok((slot, stream))
    };
    let (slot, stream) = match tokio::time::timeout(STREAM_ACQUIRE_TIMEOUT, acquire).await {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            return ApiError::new(StatusCode::BAD_GATEWAY, format!("cannot open stream: {e:#}")).into_response();
        }
        Err(_) => {
            return ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "too many concurrent streams").into_response();
        }
    };
    let mut stream = stream;
    if let Err(e) = stream.seek(SeekFrom::Start(start)).await {
        return ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("seek failed: {e}")).into_response();
    }
    debug!(session = %session.id, start, count, "serving range");

    let stall = Duration::from_secs(state.config().engine.read_stall_timeout_secs);
    let guard = (session.reader_guard(), slot);
    let body = futures::stream::unfold(
        (stream, count, session, guard),
        move |(mut reader, remaining, session, guard)| async move {
            if remaining == 0 {
                return None;
            }
            let mut buf = vec![0u8; (remaining as usize).min(CHUNK)];
            let item = match tokio::time::timeout(stall, reader.read(&mut buf)).await {
                Ok(Ok(0)) => Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "stream ended early")),
                Ok(Ok(n)) => {
                    buf.truncate(n);
                    session.add_served(n as u64);
                    Ok(Bytes::from(buf))
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "no data from peers within the stall timeout",
                )),
            };
            let remaining = match &item {
                Ok(b) => remaining - b.len() as u64,
                Err(e) => {
                    warn!(session = %session.id, error = %e, remaining, "stream aborted");
                    0
                }
            };
            Some((item, (reader, remaining, session, guard)))
        },
    );
    builder.body(Body::from_stream(body)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors() {
        assert_eq!(parse_selector("3", "x.mkv").unwrap().file_idx, Some(3));
        let s = parse_selector("s2e10", "Show.mkv").unwrap();
        assert_eq!(s.episode, Some(Episode { season: 2, episode: 10 }));
        assert_eq!(s.filename_hint.as_deref(), Some("Show.mkv"));
        assert!(parse_selector("auto", "").unwrap().file_idx.is_none());
        assert!(parse_selector("x", "").is_none());
        assert!(parse_selector("s1", "").is_none());
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_for("a.MKV"), "video/x-matroska");
        assert_eq!(mime_for("a.mp4"), "video/mp4");
        assert_eq!(mime_for("a"), "application/octet-stream");
    }
}
