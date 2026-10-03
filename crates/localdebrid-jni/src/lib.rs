//! Native side of `com.localdebrid.app.NativeServer`. The server runs inside the app
//! process (not as a child process, which Android 12+ may kill as a "phantom process").

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use jni::EnvUnowned;
use jni::objects::{JClass, JObject, JString};
use localdebrid::{AppState, StartOptions};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

struct Running {
    runtime: tokio::runtime::Runtime,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    state: Arc<ArcSwapOption<AppState>>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static STARTING: AtomicBool = AtomicBool::new(false);
static LOGGING: Once = Once::new();

const FILTER: &str = "info,librqbit=warn,librqbit_dht=warn,librqbit_tracker_comms=warn,librqbit_core=warn";

fn init_logging() {
    LOGGING.call_once(|| {
        use tracing_subscriber::prelude::*;
        let registry = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new(FILTER))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_target(false)
                    .with_writer(localdebrid::logbuf::LogBuffer::global().clone()),
            );
        #[cfg(target_os = "android")]
        let registry = registry.with(paranoid_android::layer("LocalDebrid"));
        let _ = registry.try_init();
        std::panic::set_hook(Box::new(|info| {
            tracing::error!("panic: {info}");
        }));
    });
}

fn start(data_dir: String, cache_dir: String) -> Result<(), String> {
    if RUNNING.lock().is_some() {
        return Ok(());
    }
    if STARTING.swap(true, Ordering::SeqCst) {
        return Err("the server is already starting".into());
    }
    let result = start_inner(data_dir, cache_dir);
    STARTING.store(false, Ordering::SeqCst);
    result
}

/// Runs without holding `RUNNING`, so status polls stay responsive during a slow start.
fn start_inner(data_dir: String, cache_dir: String) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("localdebrid")
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let stop = CancellationToken::new();
    let state: Arc<ArcSwapOption<AppState>> = Arc::new(ArcSwapOption::empty());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let options = StartOptions {
        data_dir: PathBuf::from(data_dir),
        cache_dir: PathBuf::from(cache_dir),
        config_override: None,
    };
    let task = runtime.spawn({
        let stop = stop.clone();
        let state = state.clone();
        async move {
            localdebrid::run_supervised(options, stop, move |current| {
                if current.is_some() {
                    let _ = ready_tx.send(());
                }
                state.store(current);
            })
            .await
        }
    });

    match ready_rx.recv_timeout(Duration::from_secs(90)) {
        Ok(()) => {
            *RUNNING.lock() = Some(Running {
                runtime,
                stop,
                task,
                state,
            });
            Ok(())
        }
        Err(_) => {
            stop.cancel();
            let message = if task.is_finished() {
                match runtime.block_on(task) {
                    Ok(Err(e)) => format!("{e:#}"),
                    Ok(Ok(())) => "server exited during start".into(),
                    Err(e) => e.to_string(),
                }
            } else {
                "server did not start within 90 seconds".into()
            };
            runtime.shutdown_timeout(Duration::from_secs(5));
            tracing::error!(error = %message, "start failed");
            Err(message)
        }
    }
}

fn stop() {
    let Some(running) = RUNNING.lock().take() else {
        return;
    };
    running.stop.cancel();
    let _ = running
        .runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(20), running.task).await });
    running.runtime.shutdown_timeout(Duration::from_secs(5));
}

fn status() -> String {
    let guard = RUNNING.lock();
    let Some(running) = guard.as_ref() else {
        if STARTING.load(Ordering::SeqCst) {
            return r#"{"running":false,"starting":true}"#.into();
        }
        return r#"{"running":false}"#.into();
    };
    if running.task.is_finished() {
        return r#"{"running":false,"error":"server stopped unexpectedly"}"#.into();
    }
    match running.state.load_full() {
        Some(state) => {
            let mut value = serde_json::to_value(state.status()).unwrap_or_default();
            value["running"] = true.into();
            let token = &state.config().access_token;
            let prefix = if token.is_empty() { String::new() } else { format!("/{token}") };
            value["localDashboard"] = format!("http://127.0.0.1:{}{prefix}/", state.port).into();
            value.to_string()
        }
        None => r#"{"running":true,"restarting":true}"#.into(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_localdebrid_app_NativeServer_nativeInit<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    context: JObject<'caller>,
) {
    env.with_env(|_env| -> Result<(), jni::errors::Error> {
        init_logging();
        #[cfg(target_os = "android")]
        rustls_platform_verifier::android::init_with_env(_env, context)?;
        #[cfg(not(target_os = "android"))]
        let _ = context;
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// Returns an empty string on success, or the error message.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_localdebrid_app_NativeServer_nativeStart<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    data_dir: JString<'caller>,
    cache_dir: JString<'caller>,
) -> JString<'caller> {
    env.with_env(|env| -> Result<_, jni::errors::Error> {
        let data_dir = data_dir.to_string();
        let cache_dir = cache_dir.to_string();
        let message = match start(data_dir, cache_dir) {
            Ok(()) => String::new(),
            Err(e) => e,
        };
        JString::from_str(env, message)
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_localdebrid_app_NativeServer_nativeStop<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
) {
    env.with_env(|_env| -> Result<(), jni::errors::Error> {
        stop();
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_localdebrid_app_NativeServer_nativeStatus<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
) -> JString<'caller> {
    env.with_env(|env| -> Result<_, jni::errors::Error> { JString::from_str(env, status()) })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}
