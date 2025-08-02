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
use nzb_rs::File;
use nzb_streamer::par2::matcher::match_files_to_par2;
use serde_json::json;
use std::time::Duration;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};
use tokio::time;
use tokio::{net::TcpListener, sync::Mutex};
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use nzb_streamer::{
    error::RestError,
    nntp::{NntpClient, client::nntp_client, config::NntpConfig, simple::SimpleNntpClient},
    nzb::{self},
    par2::{matcher::FileMatch, parser::parse_par2_file},
    streamer::virtual_file_streamer::VirtualFileStreamer,
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
    active_streams: Arc<Mutex<HashMap<Uuid, Arc<VirtualFileStreamer>>>>,
    nntp_client: Arc<dyn NntpClient + Send + Sync>,
    simple: Arc<SimpleNntpClient>,
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
        .map_err(|e| panic!("Failed to load NNTP configuration from environment: {e}"))
        .unwrap();

    let nntp_client = nntp_client(args.live_download, args.mock_data);
    let app_state = AppState {
        active_streams: Arc::new(Mutex::new(HashMap::new())),
        nntp_client: Arc::from(nntp_client),
        mock_mode: !args.live_download,
        simple: Arc::new(SimpleNntpClient::new(nntp_config)),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/upload", post(upload))
        .route("/local/upload", post(upload_local))
        .route("/stream/{session_id}", get(stream))
        .route("/chunked/{session_id}", get(stream_chunked))
        .route("/local/stream/{session_id}", get(stream))
        .route("/local/chunked/{session_id}", get(stream_chunked))
        // .route("/session/{session_id}/status", get(session_status))
        .layer(CorsLayer::permissive())
        .with_state(app_state);

    let bind_addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind to {bind_addr}: {e}"));

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
    let streamer = {
        let active_streams = state.active_streams.lock().await;
        Arc::clone(
            active_streams
                .get(&session_id)
                .ok_or(RestError::SessionNotFound)?,
        )
    };

    let available_bytes = streamer.get_available_bytes().await;
    if available_bytes == 0 {
        return Ok(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Retry-After", "5")
            .body(Body::from("No data available yet"))
            .unwrap());
    }

    let range = parse_range_header(&headers, available_bytes);

    if let Some((start, end)) = range {
        let length = end - start + 1;

        let chunk_size = if length < MIN_CHUNK_SIZE as u64 {
            MIN_CHUNK_SIZE
        } else if length > MAX_CHUNK_SIZE as u64 {
            MAX_CHUNK_SIZE
        } else {
            IDEAL_CHUNK_SIZE
        };

        let stream = streamer.read_range_with_chunk_size(start, length, chunk_size);
        let body = Body::from_stream(stream);

        Ok(Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, "video/x-matroska")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{available_bytes}"),
            )
            .header(header::CONTENT_LENGTH, length)
            .header(header::CACHE_CONTROL, "no-cache")
            .body(body)
            .unwrap())
    } else {
        let stream = streamer.read_range_with_chunk_size(0, available_bytes, MAX_CHUNK_SIZE);
        let body = Body::from_stream(stream);

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "video/x-matroska")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, available_bytes)
            .header(header::CACHE_CONTROL, "public, max-age=3600")
            .body(body)
            .unwrap())
    }
}

pub async fn stream_chunked(
    Path(session_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, RestError> {
    let streamer = {
        let active_streams = state.active_streams.lock().await;
        Arc::clone(
            active_streams
                .get(&session_id)
                .ok_or(RestError::SessionNotFound)?,
        )
    };

    let available_bytes = streamer.get_available_bytes().await;

    if available_bytes == 0 {
        return Ok(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Retry-After", "5")
            .body(Body::from("No data available yet"))
            .unwrap());
    }

    // For chunked encoding, just stream from the beginning
    // Don't specify Content-Length at all
    let stream = streamer.read_range_with_chunk_size(0, available_bytes, MAX_CHUNK_SIZE);
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

    // Handle different range formats
    if let Some(range) = range_str.strip_prefix("bytes=") {
        let parts: Vec<&str> = range.split('-').collect();

        match parts.as_slice() {
            [start, ""] => {
                // "bytes=1024-" means from 1024 to end
                let start = start.parse::<u64>().ok()?;
                Some((start, available_bytes - 1))
            }
            ["", end] => {
                // "bytes=-1024" means last 1024 bytes
                let end = end.parse::<u64>().ok()?;
                Some((available_bytes.saturating_sub(end), available_bytes - 1))
            }
            [start, end] => {
                // "bytes=1024-2048" means specific range
                let start = start.parse::<u64>().ok()?;
                let end = end.parse::<u64>().ok()?;
                if start <= end && end < available_bytes {
                    Some((start, end))
                } else {
                    None
                }
            }
            _ => None,
        }
    } else {
        None
    }
}

async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, RestError> {
    info!("NZB request received");

    let mut body = None;

    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("nzb") {
            let bytes = field.bytes().await?;
            body = Some(String::from_utf8(bytes.to_vec())?);
            break;
        }
    }

    let content = body.ok_or_else(|| RestError::MissingNzb)?;
    let nzb = nzb::parse(&content)?;

    let session_id = Uuid::new_v4();
    let session_dir = std::path::Path::new("/tmp/binzb").join(session_id.to_string());
    tokio::fs::create_dir_all(&session_dir).await.unwrap();

    info!("Downloading main PAR2 file");
    let par2_start = Instant::now();
    let main_par2 = state
        .simple
        .download_first_segment_to_file(&nzb.par2.first().unwrap().clone(), &session_dir)
        .await
        .unwrap();
    info!("Downloaded main PAR2 file in {:?}", par2_start.elapsed());

    info!("Background downloading first rar segments");
    let rars = nzb.obfuscated.clone();
    let client = Arc::clone(&state.simple);
    let dir = session_dir.clone();
    let first_segments_handle =
        tokio::spawn(async move { client.download_first_segments_to_files(&rars, &dir).await });

    let parsed_par2 = parse_par2_file(&main_par2)?;

    info!("Waiting for first segment downloads to complete");
    let first_segments = first_segments_handle.await.unwrap().unwrap();
    info!("Downloaded {} first segments", first_segments.len());

    let match_result = match_files_to_par2(&session_dir, &parsed_par2).unwrap();
    info!(
        "Matched {}/{} files",
        match_result.matched_files.len(),
        first_segments.len()
    );

    if match_result.matched_files.is_empty() {
        error!("No files matched! Check hash calculation");
    }

    let mut name_to_nzb: HashMap<String, File> = HashMap::new();
    for file_match in &match_result.matched_files {
        if let Some(real_name) = &file_match.real_name {
            let obfuscated_name = file_match.path.file_name().unwrap().to_string_lossy();
            if let Some((_, nzb_file)) = first_segments
                .iter()
                .find(|(path, _)| path.file_name().unwrap().to_string_lossy() == obfuscated_name)
            {
                name_to_nzb.insert(real_name.clone(), nzb_file.clone());
            }
        }
    }

    let ordered_files = order_rar_files_for_download(&name_to_nzb, &match_result.matched_files);
    let fs_order: Vec<_> = ordered_files.iter().map(|(f, _)| f.path.clone()).collect();

    let streamer = Arc::new(VirtualFileStreamer::new(&fs_order).await.unwrap());
    let background = Arc::clone(&streamer);
    {
        let mut active_streams = state.active_streams.lock().await;
        active_streams.insert(session_id, streamer);
    }

    tokio::spawn(async move {
        info!(
            "Starting background download of remaining segments for {} RAR files",
            ordered_files.len()
        );

        for (file_match, nzb_file) in ordered_files {
            // Download remaining segments
            state
                .simple
                .download_remaining_segments(&nzb_file, &file_match.path)
                .await
                .unwrap();

            let _ = background.mark_segment_downloaded(&file_match.path).await;
        }

        info!("Background download complete");
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

async fn upload_local(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, RestError> {
    info!("NZB request received");

    let mut body = None;

    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("nzb") {
            let bytes = field.bytes().await?;
            body = Some(String::from_utf8(bytes.to_vec())?);
            break;
        }
    }

    let content = body.ok_or_else(|| RestError::MissingNzb)?;
    let nzb = nzb::parse(&content)?;

    let session_id = Uuid::new_v4();
    let session_dir = std::path::Path::new("/tmp/binzb/c660b3b0-eb03-420b-a213-1f564dc56096");
    let parsed_par2 = parse_par2_file(&session_dir.join("0ae5e6761c69051d401054734c174e91.par2"))?;
    let match_result = match_files_to_par2(session_dir, &parsed_par2).unwrap();

    let mut name_to_nzb: HashMap<String, File> = HashMap::new();
    for file_match in &match_result.matched_files {
        if let Some(real_name) = &file_match.real_name {
            let obfuscated_name = file_match.path.file_name().unwrap().to_string_lossy();
            debug!("trying match for {}", obfuscated_name);
            if let Some(file) = nzb
                .obfuscated
                .iter()
                .find(|file| file.subject.contains(&obfuscated_name.to_string()))
            {
                name_to_nzb.insert(real_name.clone(), file.clone());
            }
        }
    }

    let ordered_files = order_rar_files_for_download(&name_to_nzb, &match_result.matched_files);
    let fs_order: Vec<_> = ordered_files.iter().map(|(f, _)| f.path.clone()).collect();

    let streamer = Arc::new(VirtualFileStreamer::new(&fs_order).await.unwrap());
    let background = Arc::clone(&streamer);
    {
        let mut active_streams = state.active_streams.lock().await;
        active_streams.insert(session_id, streamer);
    }

    tokio::spawn(async move {
        info!(
            "Starting background download of remaining segments for {} RAR files",
            ordered_files.len()
        );

        for (file_match, _) in ordered_files {
            let _ = background.mark_segment_downloaded(&file_match.path).await;
            time::sleep(Duration::from_secs(100)).await;
        }

        info!("Background download complete");
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

fn order_rar_files_for_download(
    name_to_nzb: &HashMap<String, File>,
    matched_files: &[FileMatch],
) -> Vec<(FileMatch, File)> {
    let mut files: Vec<(FileMatch, File)> = matched_files
        .iter()
        .filter_map(|file_match| {
            file_match
                .real_name
                .as_ref()
                .and_then(|name| name_to_nzb.get(name))
                .map(|nzb| (file_match.clone(), nzb.clone()))
        })
        .collect();

    files.sort_by(|a, b| {
        let a_name = a.0.real_name.as_ref().unwrap();
        let b_name = b.0.real_name.as_ref().unwrap();

        // .rar comes first
        if a_name.ends_with(".rar") && !b_name.ends_with(".rar") {
            return std::cmp::Ordering::Less;
        }
        if !a_name.ends_with(".rar") && b_name.ends_with(".rar") {
            return std::cmp::Ordering::Greater;
        }

        // Then .r00, .r01, etc
        if let (Some(a_num), Some(b_num)) = (extract_rar_number(a_name), extract_rar_number(b_name))
        {
            return a_num.cmp(&b_num);
        }

        a_name.cmp(b_name)
    });

    debug!("sorted files: {:#?}", files);
    files
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

fn extract_rar_number(filename: &str) -> Option<u32> {
    if let Some(ext) = filename.split('.').next_back() {
        if ext.starts_with('r') && ext.len() > 1 {
            return ext[1..].parse().ok();
        }
    }
    None
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
