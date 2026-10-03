//! Accept loop serving plain HTTP and HTTPS on the same port: the first byte of a TLS
//! connection is always 0x16 (handshake record), never a valid HTTP method character.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tower::{Layer, ServiceExt};
use tracing::{debug, warn};

use crate::app::AppState;
use crate::http::{self, ConnInfo};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn accept_loop(
    listener: TcpListener,
    state: Arc<AppState>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
) {
    let router = http::router(state.clone());
    let service = axum::middleware::from_fn_with_state(state.clone(), http::gate).layer(router);
    loop {
        let (stream, remote) = tokio::select! {
            _ = shutdown.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(e) => {
                    warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        let Ok(local) = stream.local_addr() else { continue };
        let service = service.clone();
        let state = state.clone();
        let shutdown = shutdown.clone();
        tasks.spawn(async move {
            if let Err(e) = handle_connection(stream, remote, local, service, state, shutdown).await {
                debug!(%remote, error = %e, "connection ended with error");
            }
        });
    }
}

async fn handle_connection<S>(
    stream: TcpStream,
    remote: std::net::SocketAddr,
    local: std::net::SocketAddr,
    service: S,
    state: Arc<AppState>,
    shutdown: CancellationToken,
) -> anyhow::Result<()>
where
    S: tower::Service<axum::extract::Request, Response = axum::response::Response, Error = std::convert::Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    let _ = stream.set_nodelay(true);
    let mut first = [0u8; 1];
    let peeked = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first)).await??;
    if peeked == 0 {
        return Ok(());
    }
    let tls = first[0] == 0x16;
    let info = ConnInfo { remote, local, tls };
    let service = service.map_request(move |mut request: hyper::Request<Incoming>| {
        request.extensions_mut().insert(info);
        request.map(Body::new)
    });
    let service = TowerToHyperService::new(service);
    let builder = Builder::new(TokioExecutor::new());

    if tls {
        let Some(acceptor) = state.tls.acceptor() else {
            return Ok(());
        };
        let tls_stream = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await??;
        let connection = builder.serve_connection_with_upgrades(TokioIo::new(tls_stream), service);
        drive(connection, shutdown).await
    } else {
        let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
        drive(connection, shutdown).await
    }
}

async fn drive<C>(connection: C, shutdown: CancellationToken) -> anyhow::Result<()>
where
    C: hyper_util::server::graceful::GracefulConnection,
    C::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    tokio::pin!(connection);
    tokio::select! {
        result = connection.as_mut() => result.map_err(|e| anyhow::anyhow!(e.into())),
        _ = shutdown.cancelled() => {
            connection.as_mut().graceful_shutdown();
            let _ = tokio::time::timeout(DRAIN_TIMEOUT, connection).await;
            Ok(())
        }
    }
}
