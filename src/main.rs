use axum::extract::Path;
use axum::{
    Router,
    body::Body,
    extract::{Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use clap::Parser;
use http::{HeaderMap, header};
use nzb_streamer::archive::par2::{DownloadTask, create_download_tasks};
use nzb_streamer::archive::{self, par2};
use nzb_streamer::nzb::Nzb;
use nzb_streamer::scheduler::adaptive::FirstSegment;
use nzb_streamer::scheduler::error::SchedulerError;
use nzb_streamer::stream::orchestrator::{BufferHealth, StreamOrchestrator};
use serde_json::json;
use std::path;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, watch};
use tokio::task;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use nzb_streamer::{
    error::RestError,
    nntp::config::NntpConfig,
    nzb::{self},
    scheduler::adaptive::AdaptiveScheduler,
};

#[derive(Parser)]
#[command(name = "nzb-streamer")]
#[command(about = "NZB-to-MKV streaming service")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Args {
    #[arg(short, long, default_value = "3005")]
    port: u16,

    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value = "/tmp/nzb-cache")]
    cache_dir: PathBuf,

    #[arg(long, default_value = "true")]
    live_download: bool,

    #[arg(long, default_value = "true")]
    debug: bool,

    /// Directory containing pre-downloaded segments (mock mode)
    #[arg(long, default_value = "/tmp/downloaded")]
    mock_data: Option<PathBuf>,
}

#[derive(Clone)]
pub struct AppState {
    sessions: Arc<RwLock<HashMap<Uuid, Arc<StreamOrchestrator>>>>,
    scheduler: Arc<AdaptiveScheduler>,
    mock_mode: bool,
}

const IDEAL_CHUNK_SIZE: usize = 8 * 1024 * 1024; // 8MB ideal
const MAX_CHUNK_SIZE: usize = 16 * 1024 * 1024; // 16MB maximum

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let filter_level = if args.debug { "debug" } else { "info" };
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("nzb_streamer={filter_level},tower_http=info").into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    dotenvy::dotenv().ok();

    let nntp_config = NntpConfig::from_env()
        .unwrap_or_else(|e| panic!("Failed to load NNTP configuration from environment: {e}"));

    let scheduler = AdaptiveScheduler::new(nntp_config)
        .unwrap_or_else(|e| panic!("Failed to initialise scheduler: {e}"));

    scheduler.warm_pool().await; // TODO: make this better?

    let app_state = AppState {
        sessions: Arc::new(RwLock::new(HashMap::new())),
        scheduler: Arc::new(scheduler),
        mock_mode: !args.live_download,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/upload", post(upload))
        //.route("/local/upload", post(upload_local))
        .route("/stream/{session_id}", get(stream))
        .route("/chunked/{session_id}", get(stream_chunked))
        .route("/local/stream/{session_id}", get(stream))
        .route("/local/chunked/{session_id}", get(stream_chunked))
        .layer(CorsLayer::permissive())
        .with_state(app_state);

    let bind_addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind to {bind_addr}: {e}"));

    info!("NZB streaming server started on {}", bind_addr);
    info!("");
    info!("Usage:");
    info!(
        "   curl -X POST -F 'nzb=@/tmp/unencrypted.nzb' http://{}/upload",
        bind_addr
    );
    info!("   mpv http://{}/stream/{{session_id}}", bind_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|e| panic!("server error: {e}"));
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl+C signal");
        },
        _ = terminate => {
            info!("Received terminate signal");
        },
    }
}

async fn health() -> impl IntoResponse {
    Json(json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub async fn stream(
    Path(session_id): Path<Uuid>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let sessions = state.sessions.read().await;
    let orchestrator = sessions
        .get(&session_id)
        .ok_or(RestError::SessionNotFound)?;

    let available = orchestrator.get_available_bytes();
    if available == 0 {
        return Ok(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Retry-After", "2")
            .body(Body::from("No data available yet"))
            .unwrap());
    }

    let range = parse_range_header(&headers, available);
    let (start, length) = range.unwrap_or((0, available));
    let end = start + length - 1;

    let stream = orchestrator
        .get_stream(start, length, IDEAL_CHUNK_SIZE)
        .await; // TODO: ideal chunk size?
    let body = Body::from_stream(stream);

    let response = Response::builder()
        .header(header::CONTENT_TYPE, "video/x-matroska")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{available}"),
        )
        .header(header::CONTENT_LENGTH, length);

    let response = if range.is_some() {
        response
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CACHE_CONTROL, "public, max-age=3600")
    } else {
        response
            .status(StatusCode::OK)
            .header(header::CACHE_CONTROL, "no-cache")
    };

    Ok(response.body(body).unwrap())
}

pub async fn stream_chunked(
    Path(session_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, RestError> {
    let sessions = state.sessions.read().await;
    let orchestrator = sessions
        .get(&session_id)
        .ok_or(RestError::SessionNotFound)?;

    let available = orchestrator.get_available_bytes();
    if available == 0 {
        return Ok(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Retry-After", "2")
            .body(Body::from("No data available yet"))
            .unwrap());
    }

    let stream = orchestrator.get_stream(0, available, MAX_CHUNK_SIZE).await;
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/x-matroska")
        .header(header::TRANSFER_ENCODING, "chunked")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .unwrap())
}

fn parse_range_header(headers: &HeaderMap, available_bytes: u64) -> Option<(u64, u64)> {
    let range_str = headers.get(header::RANGE)?.to_str().ok()?;
    let range = range_str
        .strip_prefix("bytes=")?
        .split('-')
        .collect::<Vec<_>>();
    match range.as_slice() {
        [start, ""] => {
            let start = start.parse().ok()?;
            Some((start, available_bytes - start))
        }
        [start, end] => {
            let start = start.parse().ok()?;
            let end = end.parse().ok()?;
            if start > end {
                return None;
            }
            Some((start, end - start + 1))
        }
        _ => None,
    }
}

// TODO: handle duplicates
async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, RestError> {
    info!("NZB request received");

    let content = {
        let mut body = None;

        while let Some(field) = multipart.next_field().await? {
            if field.name() == Some("nzb") {
                let bytes = field.bytes().await?;
                body = Some(String::from_utf8(bytes.to_vec())?);
                break;
            }
        }

        body.ok_or_else(|| RestError::MissingNzb)?
    };
    let nzb = nzb::parse(&content)?;

    let session_id = Uuid::new_v4();
    let session_dir = std::path::Path::new("/tmp/binzb").join(session_id.to_string());
    tokio::fs::create_dir_all(&session_dir).await.unwrap();

    let tasks = if !nzb.obfuscated.is_empty() {
        info!("NZB contains obfuscated files, decoding");
        obfuscated(nzb, &state.scheduler, &session_dir).await
    } else {
        info!("NZB contains plain RAR files, serving");
        plain(nzb, &state.scheduler, &session_dir).await
    }?;

    let (health_tx, health_rx) = watch::channel(BufferHealth::Critical);

    let orchestrator = StreamOrchestrator::new(tasks.clone(), &session_dir, health_tx);
    state
        .sessions
        .write()
        .await
        .insert(session_id, orchestrator.clone());

    tokio::spawn({
        let scheduler = Arc::clone(&state.scheduler);
        let mmap = Arc::clone(&orchestrator.mmap);
        async move {
            info!(
                "Starting background download of remaining segments for {} RAR files",
                tasks.len()
            );

            scheduler
                .schedule_downloads(tasks, mmap, health_rx)
                .await
                .unwrap();

            info!("Background download complete");
        }
    });

    Ok((
        StatusCode::OK,
        Json(json!({
            "session_id": session_id,
            "message": "NZB uploaded successfully. Background processing initiated.",
            "mode": if state.mock_mode { "mock" } else { "live" }
        })),
    ))
}

async fn obfuscated(
    nzb: Nzb,
    scheduler: &Arc<AdaptiveScheduler>,
    session_dir: &path::Path,
) -> Result<Vec<DownloadTask>, RestError> {
    let first_segments = download_first_segments(scheduler, nzb.obfuscated).await;

    info!("Downloading main PAR2 file");
    // TODO: this operates on the assumption that the par2 file is one segment
    let par2_target = nzb.par2.first().unwrap().clone();
    let main_par2 = scheduler.download_first_segment(par2_target).await.unwrap();

    let manifest = archive::parse_buffer(&main_par2.bytes)?;

    info!("Waiting for first segment downloads to complete");
    let first_segments = first_segments.await??;
    info!("Downloaded {} first segments", first_segments.len());

    let tasks =
        create_download_tasks(manifest.hash_to_filename(), &first_segments, session_dir).await?;
    info!("Created {} download tasks", tasks.len());

    let downloaded_hashes: Vec<_> = first_segments
        .iter()
        .map(|segment| segment.hash16k.clone())
        .collect();
    let missing = manifest.find_missing_files(&downloaded_hashes);
    if !missing.is_empty() {
        warn!("Missing files: {:?}", missing);
        // TODO: Attempt PAR2 recovery for missing files
    }

    Ok(tasks)
}

async fn plain(
    nzb: Nzb,
    scheduler: &Arc<AdaptiveScheduler>,
    session_dir: &path::Path,
) -> Result<Vec<DownloadTask>, RestError> {
    let first_segments = download_first_segments(scheduler, nzb.rar.clone()).await;
    let segments = first_segments.await??;

    let tasks = par2::create_download_tasks_plain(&segments, session_dir).await?;
    info!("Created {} download tasks", tasks.len());

    Ok(tasks)
}

async fn download_first_segments(
    scheduler: &Arc<AdaptiveScheduler>,
    rars: Vec<nzb_rs::File>,
) -> task::JoinHandle<Result<Vec<FirstSegment>, SchedulerError>> {
    info!("Background downloading first RAR segments");

    tokio::spawn({
        let scheduler = Arc::clone(scheduler);
        async move { scheduler.download_first_segments(&rars).await }
    })
}

// right now we match on first file
// main par2 file might not always be first! we should use this, even just as a fallback
fn find_main_par2(par2_files: &HashMap<String, PathBuf>) -> Result<PathBuf, RestError> {
    // Main PAR2 file doesn't have .volXX+XX in the name
    Ok(par2_files
        .iter()
        .find(|(name, _)| !name.contains(".vol") && name.ends_with(".par2"))
        .map(|(_, path)| path.clone())
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_parsing() {
        let args = Args::try_parse_from([
            "nzb-streamer",
            "--port",
            "9000",
            "--debug",
            "--live-download",
        ])
        .unwrap();

        assert_eq!(args.port, 9000);
        assert!(args.debug);
        assert!(args.live_download);
    }
}
