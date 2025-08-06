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
use nzb_streamer::par2;
use nzb_streamer::stream::segment_tracker::SegmentTracker;
use serde_json::json;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use nzb_streamer::{
    error::RestError,
    nntp::config::NntpConfig,
    nzb::{self},
    scheduler::adaptive::AdaptiveScheduler,
    stream::virtual_file_streamer::VirtualFileStreamer,
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
pub struct SessionData {
    pub streamer: Arc<VirtualFileStreamer>,
    pub tracker: Arc<SegmentTracker>,
}

#[derive(Clone)]
pub struct AppState {
    sessions: Arc<RwLock<HashMap<Uuid, SessionData>>>,
    scheduler: Arc<AdaptiveScheduler>,
    mock_mode: bool,
}

const MIN_CHUNK_SIZE: usize = 2 * 1024 * 1024; // 2MB minimum
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
    let session = state
        .sessions
        .read()
        .await
        .get(&session_id)
        .cloned()
        .ok_or(RestError::SessionNotFound)?;

    let available = session.streamer.get_available_bytes().await;
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

    let stream = session
        .streamer
        .read_range_with_chunk_size(start, length, IDEAL_CHUNK_SIZE); // TODO: ideal chunk size?
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
            .status(StatusCode::OK)
            .header(header::CACHE_CONTROL, "public, max-age=3600")
    } else {
        response
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CACHE_CONTROL, "no-cache")
    };

    Ok(response.body(body).unwrap())
}

pub async fn stream_chunked(
    Path(session_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, RestError> {
    let streamer = state
        .sessions
        .read()
        .await
        .get(&session_id)
        .cloned()
        .ok_or(RestError::SessionNotFound)?
        .streamer;

    let available = streamer.get_available_bytes().await;
    if available == 0 {
        return Ok(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Retry-After", "2")
            .body(Body::from("No data available yet"))
            .unwrap());
    }

    // For chunked encoding, just stream from the beginning
    // Don't specify Content-Length at all
    let stream = streamer.read_range_with_chunk_size(0, available, MAX_CHUNK_SIZE);
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
    let range_header = headers.get(header::RANGE)?;
    let range_str = range_header.to_str().ok()?;

    if let Some(range) = range_str.strip_prefix("bytes=") {
        let parts: Vec<&str> = range.split('-').collect();

        match parts.as_slice() {
            // Format: "bytes=start-"
            [start, ""] => {
                let start = start.parse::<u64>().ok()?;
                if start >= available_bytes {
                    return None; // Unsatisfiable range
                }
                Some((start, available_bytes - start))
            }
            // Format: "bytes=-suffix"
            ["", suffix] => {
                let suffix_len = suffix.parse::<u64>().ok()?;
                // Handle suffix length larger than available bytes
                let start = available_bytes.saturating_sub(suffix_len);
                let length = available_bytes - start;
                if length == 0 {
                    None // Zero-length range not allowed
                } else {
                    Some((start, length))
                }
            }
            // Format: "bytes=start-end"
            [start, end] => {
                let start = start.parse::<u64>().ok()?;
                let end = end.parse::<u64>().ok()?;
                if start > end || end >= available_bytes {
                    return None; // Invalid range
                }
                Some((start, end - start + 1))
            }
            _ => None,
        }
    } else {
        None
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

    info!("Downloading main PAR2 file");
    // TODO: this operates on the assumption that the par2 file is one segment
    let par2_target = nzb.par2.first().unwrap().clone();
    let main_par2 = state
        .scheduler
        .download_first_segment(par2_target, &session_dir)
        .await
        .unwrap();

    info!("Background downloading first RAR segments");
    let first_segments = tokio::spawn({
        let scheduler = Arc::clone(&state.scheduler);
        let dir = session_dir.clone();
        let rars = nzb.obfuscated.clone();
        async move {
            scheduler.download_first_segments(&rars, &dir).await // TODO: handle files that are actually rar
        }
    });

    let manifest = par2::parse_file(&main_par2.path)?; // TODO: wasteful to write in download_first_segment, then immediately read here

    info!("Waiting for first segment downloads to complete");
    let first_segments = first_segments.await??;
    info!("Downloaded {} first segments", first_segments.len());

    let tasks = manifest.create_download_tasks(&first_segments);
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

    let paths: Vec<_> = first_segments
        .iter()
        .map(|segment| segment.path.clone())
        .collect();

    let streamer = Arc::new(VirtualFileStreamer::new(&paths).await.unwrap());
    let tracker = Arc::new(SegmentTracker::new());
    state.sessions.write().await.insert(
        session_id,
        SessionData {
            streamer: streamer.clone(),
            tracker: tracker.clone(),
        },
    );

    tokio::spawn({
        let scheduler = Arc::clone(&state.scheduler);
        async move {
            info!(
                "Starting background download of remaining segments for {} RAR files",
                paths.len()
            );

            

            scheduler
                .schedule_downloads(tasks, &session_dir, session_id, streamer, tracker)
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

// async fn upload_local(
//     State(state): State<AppState>,
//     mut multipart: Multipart,
// ) -> Result<impl IntoResponse, RestError> {
//     info!("NZB request received");

//     let mut body = None;

//     while let Some(field) = multipart.next_field().await? {
//         if field.name() == Some("nzb") {
//             let bytes = field.bytes().await?;
//             body = Some(String::from_utf8(bytes.to_vec())?);
//             break;
//         }
//     }

//     let content = body.ok_or_else(|| RestError::MissingNzb)?;
//     let nzb = nzb::parse(&content)?;

//     let session_id = Uuid::new_v4();
//     let session_dir = std::path::Path::new("/tmp/binzb/c660b3b0-eb03-420b-a213-1f564dc56096");
//     let parsed_par2 = parse_par2_file(&session_dir.join("0ae5e6761c69051d401054734c174e91.par2"))?;
//     let match_result = match_files_to_par2(session_dir, &parsed_par2).unwrap();

//     let mut name_to_nzb: HashMap<String, File> = HashMap::new();
//     for file_match in &match_result.matched_files {
//         if let Some(real_name) = &file_match.real_name {
//             let obfuscated_name = file_match.path.file_name().unwrap().to_string_lossy();
//             debug!("trying match for {}", obfuscated_name);
//             if let Some(file) = nzb
//                 .obfuscated
//                 .iter()
//                 .find(|file| file.subject.contains(&obfuscated_name.to_string()))
//             {
//                 name_to_nzb.insert(real_name.clone(), file.clone());
//             }
//         }
//     }

//     let ordered_files = order_rar_files_for_download(&name_to_nzb, &match_result.matched_files);
//     let fs_order: Vec<_> = ordered_files.iter().map(|(f, _)| f.path.clone()).collect();

//     let streamer = Arc::new(VirtualFileStreamer::new(&fs_order).await.unwrap());
//     let background = Arc::clone(&streamer);
//     {
//         let mut active_streams = state.active_streams.lock().await;
//         active_streams.insert(session_id, streamer);
//     }

//     tokio::spawn(async move {
//         info!(
//             "Starting background download of remaining segments for {} RAR files",
//             ordered_files.len()
//         );

//         for (file_match, _) in ordered_files {
//             let _ = background.mark_segment_downloaded(&file_match.path).await;
//             time::sleep(Duration::from_secs(100)).await;
//         }

//         info!("Background download complete");
//     });

//     Ok((
//         StatusCode::OK,
//         Json(json!({
//             "session_id": session_id,
//             "message": "NZB uploaded successfully. Background processing initiated.",
//             "mode": if state.mock_mode { "mock" } else { "live" }
//         })),
//     ))
// }

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
