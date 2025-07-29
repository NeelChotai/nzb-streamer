use axum::{
    body::Body, extract::{Multipart, State}, http::StatusCode, response::{IntoResponse, Json}, routing::{get, post}, Router
};
use clap::Parser;
use http::HeaderMap;
use serde_json::json;
use std::{collections::HashMap, path::{Path as FilePath, PathBuf}, sync::Arc};
use tokio::{net::TcpListener, sync::Mutex};
use tower_http::cors::CorsLayer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;
use axum::extract::Path;

use nzb_streamer::{error::RestError, mkv::mkv::MkvStreamer, nntp::{client::nntp_client, NntpClient}, nzb, par2::{matcher::match_files_to_par2, parser::parse_par2_file}};

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

    #[arg(long, default_value = "false")]
    live_download: bool,

    #[arg(long, default_value = "true")]
    debug: bool,

    /// Directory containing pre-downloaded segments (mock mode)
    #[arg(long, default_value = "/tmp/downloaded")]
    mock_data: Option<PathBuf>,
}

#[derive(Clone)]
pub struct AppState {
    // session_manager: Arc<SessionManager>,
    // stream_manager: Arc<StreamManager>,
    active_streams: Arc<Mutex<HashMap<Uuid, MkvStreamer>>>,
    nntp_client: Arc<dyn NntpClient + Send + Sync>,
    mock_mode: bool,
}

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

    let nntp_client = nntp_client(args.live_download, args.mock_data);
    let app_state = AppState {
        active_streams: Arc::new(Mutex::new(HashMap::new())),
        nntp_client: Arc::from(nntp_client),
        mock_mode: !args.live_download,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/upload", post(upload))
        .route("/stream/{session_id}", get(stream))
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
        .unwrap_or_else(|e|panic!("server error: {e}"));
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
) -> Result<impl IntoResponse, RestError> {
    let streamer = {
        let mut active_streams = state.active_streams.lock().await;
        active_streams.remove(&session_id).unwrap()
    };
    
    let total_size = streamer.total_size();
    
    // Create headers
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "video/x-matroska".parse().unwrap());
    headers.insert("Content-Length", total_size.to_string().parse().unwrap());
    
    // Create the response
    let stream = streamer.stream();
    
    Ok((headers, Body::from_stream(stream)))
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
    let _nzb = nzb::parse(&content)?;
    
    // TODO: hardcoded for now

    let dir = FilePath::new("/tmp/downloaded");
    let path = dir.join("db5839e271decea10e9e713aa0e20573.par2");
    let parsed = parse_par2_file(&path)?;
    let mapping = match_files_to_par2(dir, &parsed).unwrap();

    let session_id = Uuid::new_v4();
    let streamer = MkvStreamer::new(dir, &mapping.matched_files).await.unwrap();
    {
        let mut active_streams = state.active_streams.lock().await;
        active_streams.insert(session_id, streamer);
    }

    Ok((
        StatusCode::OK,
        Json(json!({
            "session_id": session_id,
            "message": "NZB uploaded successfully. Background processing initiated.",
            "mode": if state.mock_mode { "mock" } else { "live" }
        })),
    ))
}

// async fn session_status(
//     State(state): State<AppState>,
//     Path(session_id): Path<Uuid>,
// ) -> impl IntoResponse {
//     match state.session_manager.get_session(session_id).await {
//         Ok(session_arc) => {
//             let session = session_arc.lock().await;

//             (
//                 StatusCode::OK,
//                 Json(serde_json::json!({
//                     "session_id": session_id,
//                     "created_at": session.created_at.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
//                     "age_seconds": session.age_seconds(),
//                     "par2_files_count": session.par2_files.len(),
//                     "rar_files_count": session.rar_files.len(),
//                     "video_files_count": session.video_files.len(),
//                     "par2_complete": session.par2_complete.load(std::sync::atomic::Ordering::Relaxed),
//                     "deobfuscation_complete": session.deobfuscation_complete.load(std::sync::atomic::Ordering::Relaxed),
//                     "stream_ready": session.stream_ready.load(std::sync::atomic::Ordering::Relaxed),
//                     "video_files": session.video_files,
//                     "obfuscated_mappings_count": session.obfuscated_to_real.len(),
//                 })),
//             )
//         }
//         Err(e) => (
//             StatusCode::NOT_FOUND,
//             Json(serde_json::json!({
//                 "error": "Session not found",
//                 "message": e.to_string()
//             })),
//         ),
//     }
// }

// async fn stream(
//     State(state): State<AppState>,
//     Path(session_id): Path<Uuid>,
//     headers: HeaderMap,
// ) -> impl IntoResponse {
//     info!("📺 Stream request for session {}", session_id);

//     match state.session_manager.get_session(session_id).await {
//         Ok(session_arc) => {
//             // Use the streaming manager to handle the request
//             match state
//                 .stream_manager
//                 .handle_stream_request(&session_id.to_string(), &headers, session_arc)
//                 .await
//             {
//                 Ok(response) => response,
//                 Err(e) => {
//                     warn!("Streaming error for session {}: {}", session_id, e);
//                     axum::response::Response::builder()
//                         .status(StatusCode::INTERNAL_SERVER_ERROR)
//                         .header(axum::http::header::CONTENT_TYPE, "application/json")
//                         .body(axum::body::Body::from(format!(
//                             r#"{{"error": "Streaming error", "message": "{e}"}}"#
//                         )))
//                         .unwrap_or_else(|_| {
//                             axum::response::Response::builder()
//                                 .status(StatusCode::INTERNAL_SERVER_ERROR)
//                                 .body(axum::body::Body::from("Internal server error"))
//                                 .unwrap()
//                         })
//                 }
//             }
//         }
//         Err(e) => axum::response::Response::builder()
//             .status(StatusCode::NOT_FOUND)
//             .header(axum::http::header::CONTENT_TYPE, "application/json")
//             .body(axum::body::Body::from(format!(
//                 r#"{{"error": "Session not found", "message": "{e}"}}"#
//             )))
//             .unwrap_or_else(|_| {
//                 axum::response::Response::builder()
//                     .status(StatusCode::NOT_FOUND)
//                     .body(axum::body::Body::from("Session not found"))
//                     .unwrap()
//             }),
//     }
// }

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