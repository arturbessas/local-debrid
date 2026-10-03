mod addon;
mod admin;
mod play;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde_json::json;
use tower_http::cors::CorsLayer;

use crate::app::AppState;
use crate::config::StreamUrlScheme;
use crate::net::is_local_network;

pub use play::mime_for;

/// Per-connection facts injected by the listener.
#[derive(Clone, Copy, Debug)]
pub struct ConnInfo {
    pub remote: SocketAddr,
    pub local: SocketAddr,
    pub tls: bool,
}

/// Path prefix (the access token) that generated URLs must carry.
#[derive(Clone, Debug, Default)]
pub struct BasePath(pub String);

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, axum::Json(json!({ "error": self.message }))).into_response()
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let addon = Router::new()
        .route("/manifest.json", get(addon::manifest))
        .route("/stream/{kind}/{id}", get(addon::streams))
        .route("/stream/{kind}/{id}/{extra}", get(addon::streams_with_extra))
        .route("/play/{hash}/{selector}/{filename}", get(play::play).head(play::play))
        .route("/video/{session}/{filename}", get(play::video).head(play::video))
        .layer(CorsLayer::permissive());

    let api = Router::new()
        .route("/status", get(admin::status))
        .route("/sessions", get(admin::list_sessions).post(admin::create_session))
        .route("/sessions/{id}", get(admin::get_session).delete(admin::delete_session))
        .route("/cache", get(admin::cache))
        .route("/cache/clear", post(admin::clear_cache))
        .route("/config", get(admin::get_config).put(admin::put_config))
        .route("/logs", get(admin::logs))
        .route("/restart", post(admin::restart));

    Router::new()
        .merge(addon)
        .nest("/api", api)
        .route("/", get(admin::dashboard))
        .route("/configure", get(|| async { Redirect::to("./") }))
        .fallback(not_found)
        .with_state(state)
}

async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not found")
}

fn token_from_headers(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    if let Some(value) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok())
        && let Some(token) = value.strip_prefix("Bearer ")
    {
        return Some(token.trim().to_string());
    }
    uri.query()?.split('&').find_map(|pair| {
        pair.strip_prefix("token=")
            .map(|t| percent_encoding::percent_decode_str(t).decode_utf8_lossy().into_owned())
    })
}

/// Runs before routing: enforces LAN-only access and the access token, and strips the
/// token path segment so routes do not need to know about it.
pub async fn gate(State(state): State<Arc<AppState>>, mut request: Request, next: Next) -> Response {
    let config = state.config();
    let conn = request.extensions().get::<ConnInfo>().copied();
    if config.lan_only
        && let Some(conn) = conn
        && !is_local_network(conn.remote.ip())
    {
        return ApiError::new(StatusCode::FORBIDDEN, "only reachable from the local network").into_response();
    }

    let token = config.access_token.as_str();
    if token.is_empty() {
        request.extensions_mut().insert(BasePath::default());
        return next.run(request).await;
    }

    let path = request.uri().path().to_string();
    let prefix = format!("/{token}");
    if path == prefix {
        return Redirect::permanent(&format!("{prefix}/")).into_response();
    }
    if let Some(rest) = path.strip_prefix(&prefix)
        && rest.starts_with('/')
    {
        let new_path_and_query = match request.uri().query() {
            Some(q) => format!("{rest}?{q}"),
            None => rest.to_string(),
        };
        let mut parts = request.uri().clone().into_parts();
        parts.path_and_query = new_path_and_query.parse().ok();
        if let Ok(uri) = Uri::from_parts(parts) {
            *request.uri_mut() = uri;
        }
        request.extensions_mut().insert(BasePath(prefix));
        return next.run(request).await;
    }
    if token_from_headers(request.headers(), request.uri()).as_deref() == Some(token) {
        request.extensions_mut().insert(BasePath(prefix));
        return next.run(request).await;
    }
    ApiError::new(StatusCode::UNAUTHORIZED, "missing or wrong access token").into_response()
}

fn as_ipv4(ip: IpAddr) -> Option<Ipv4Addr> {
    match ip {
        IpAddr::V4(v4) if !v4.is_unspecified() => Some(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped(),
        _ => None,
    }
}

/// The phone's IPv4 address as seen by this client.
pub fn local_ipv4(conn: Option<ConnInfo>) -> Option<Ipv4Addr> {
    conn.and_then(|c| as_ipv4(c.local.ip()))
        .or_else(|| crate::net::lan_ipv4s().into_iter().next())
}

/// Base URL (scheme, host, port, token) for video URLs handed to this client.
pub fn stream_base_url(
    state: &AppState,
    conn: Option<ConnInfo>,
    headers: &HeaderMap,
    base: &BasePath,
) -> String {
    let config = state.config();
    if config.stream_url_scheme == StreamUrlScheme::SameAsRequest
        && let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok())
    {
        let scheme = if conn.is_some_and(|c| c.tls) { "https" } else { "http" };
        return format!("{scheme}://{host}{}", base.0);
    }
    let host = if !config.public_host.is_empty() {
        config.public_host.clone()
    } else {
        local_ipv4(conn)
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".into())
    };
    format!("http://{host}:{}{}", state.port, base.0)
}
