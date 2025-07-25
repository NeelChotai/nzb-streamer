use anyhow::{Context, Result};
use axum::{
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use nzb_streamer::{nntp::config::NntpConfig, session::SessionManager, streaming::StreamManager};

#[derive(Parser)]
#[command(name = "nzb-streamer")]
#[command(about = "NZB-to-MKV streaming service")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Args {
    /// Server port
    #[arg(short, long, default_value = "8080")]
    port: u16,

    /// Host to bind to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Cache directory for downloads
    #[arg(long, default_value = "./cache")]
    cache_dir: PathBuf,

    /// Use live NNTP downloads (requires .env config)
    #[arg(long)]
    live_download: bool,

    /// Enable debug logging
    #[arg(long)]
    debug: bool,

    /// Directory containing pre-downloaded segments (mock mode)
    #[arg(long, default_value = "/tmp/downloaded")]
    mock_data: Option<PathBuf>,
}

/// Main application state
#[derive(Clone)]
struct AppState {
    session_manager: Arc<SessionManager>,
    stream_manager: Arc<StreamManager>,
    mock_mode: bool,
    mock_data_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    setup_logging(args.debug)?;

    // Print banner
    print_banner(&args);

    // Load environment variables
    dotenvy::dotenv().ok();

    // Create session manager
    let mut session_manager = SessionManager::new(args.cache_dir.clone());

    // Configure NNTP if in live mode
    if args.live_download {
        info!("🌐 Configuring LIVE NNTP downloads");
        warn!("⚠️  This will consume your NNTP quota!");

        match load_nntp_config() {
            Ok(nntp_config) => {
                session_manager.set_nntp_config(nntp_config);
                info!("✅ NNTP configuration loaded");
            }
            Err(e) => {
                warn!("Failed to load NNTP config: {}", e);
                info!("Falling back to mock mode");
            }
        }
    } else {
        info!("📁 Using MOCK mode - no live downloads");
        if let Some(ref mock_dir) = args.mock_data {
            if !mock_dir.exists() {
                warn!("Mock data directory doesn't exist: {}", mock_dir.display());
            }
        }
    }

    let mock_data_dir = args.mock_data.clone();
    let app_state = AppState {
        session_manager: Arc::new(session_manager),
        stream_manager: Arc::new(StreamManager::new()),
        mock_mode: !args.live_download,
        mock_data_dir: args.mock_data,
    };

    // Create router
    let app = create_router(app_state).await?;

    // Start server
    let bind_addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Failed to bind to {bind_addr}"))?;

    info!("🚀 NZB Streaming Server started!");
    info!("");
    info!("📡 Server: http://{}", bind_addr);
    info!("📤 Upload: POST http://{}/upload", bind_addr);
    info!("📺 Stream: GET http://{}/stream/{{session_id}}", bind_addr);
    info!("🔍 Health: GET http://{}/health", bind_addr);
    info!("");

    if !args.live_download {
        info!("📁 Mock Mode: Using pre-downloaded test data");
        info!("   Data dir: {:?}", mock_data_dir);
    } else {
        info!("🌐 Live Mode: Will download from NNTP servers");
    }

    info!("");
    info!("💡 Usage:");
    info!(
        "   curl -X POST -F 'nzb=@/tmp/unencrypted.nzb' http://{}/upload",
        bind_addr
    );
    info!("   mpv http://{}/stream/{{session_id}}", bind_addr);
    info!("");
    info!("✋ Press Ctrl+C to stop");
    info!("");

    // Start server
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Server error")?;

    info!("👋 Server stopped gracefully");
    Ok(())
}

fn setup_logging(debug: bool) -> Result<()> {
    let filter_level = if debug { "debug" } else { "info" };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("nzb_streamer={filter_level},tower_http=info").into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    Ok(())
}

fn print_banner(args: &Args) {
    println!("╭─────────────────────────────────────────────────╮");
    println!("│          🎬 NZB-to-MKV Streaming Service         │");
    println!("│                                                 │");
    println!("│  🦀 Rust + Axum  •  ⚡ Async/Await             │");
    println!("│  📡 NNTP Client   •  🎥 MKV Streaming           │");
    println!("╰─────────────────────────────────────────────────╯");
    println!();

    if args.live_download {
        println!("⚠️  WARNING: Live NNTP mode enabled!");
        println!("   This will download from actual NNTP servers");
        println!("   and consume your quota. Use mock mode for");
        println!("   development by removing --live-download flag");
        println!();
    }
}

fn load_nntp_config() -> Result<NntpConfig> {
    let host = std::env::var("NNTP_HOST").context("NNTP_HOST environment variable not set")?;

    let port = std::env::var("NNTP_PORT")
        .unwrap_or_else(|_| "563".to_string())
        .parse()
        .context("Invalid NNTP_PORT")?;

    let username = std::env::var("NNTP_USER").context("NNTP_USER environment variable not set")?;

    let password = std::env::var("NNTP_PASS").context("NNTP_PASS environment variable not set")?;

    let use_tls = std::env::var("NNTP_USE_TLS")
        .unwrap_or_else(|_| "true".to_string())
        .parse()
        .unwrap_or(true);

    Ok(NntpConfig::new(host, username, password, port, use_tls))
}

async fn create_router(state: AppState) -> Result<Router> {
    let app = Router::new()
        .route("/", get(root_handler))
        .route("/health", get(health_handler))
        .route("/upload", post(upload_handler))
        .route("/stream/{session_id}", get(stream_handler))
        .route("/session/{session_id}/status", get(session_status_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    Ok(app)
}

// ============================================================================
// HTTP Handlers
// ============================================================================

async fn root_handler() -> impl IntoResponse {
    Html(
        r#"
<!DOCTYPE html>
<html>
<head>
    <title>NZB Streaming Service</title>
    <style>
        body { font-family: system-ui, sans-serif; max-width: 800px; margin: 50px auto; padding: 20px; line-height: 1.6; }
        .header { background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); color: white; padding: 30px; border-radius: 10px; text-align: center; margin-bottom: 30px; }
        .section { background: white; padding: 20px; border-radius: 8px; box-shadow: 0 2px 10px rgba(0,0,0,0.1); margin-bottom: 20px; border: 1px solid #e1e5e9; }
        .endpoint { background: #f8f9fa; padding: 15px; border-radius: 5px; margin: 10px 0; font-family: monospace; border-left: 4px solid #667eea; }
        .status { color: #28a745; font-weight: bold; }
        .warning { color: #dc3545; font-weight: bold; }
        a { color: #667eea; text-decoration: none; }
        a:hover { text-decoration: underline; }
        .upload-form { background: #e8f4f8; padding: 20px; border-radius: 8px; border: 2px dashed #667eea; }
        .upload-form input[type="file"] { margin: 10px 0; }
        .upload-form button { background: #667eea; color: white; padding: 10px 20px; border: none; border-radius: 5px; cursor: pointer; }
        .upload-form button:hover { background: #5a67d8; }
    </style>
</head>
<body>
    <div class="header">
        <h1>🎬 NZB-to-MKV Streaming Service</h1>
        <p>Status: <span class="status">Running</span></p>
        <p>Upload NZB files and stream MKV content while downloading</p>
    </div>
    
    <div class="section">
        <h2>📤 Upload NZB File</h2>
        <div class="upload-form">
            <form action="/upload" method="post" enctype="multipart/form-data">
                <div>
                    <label for="nzb">Select NZB file:</label><br>
                    <input type="file" id="nzb" name="nzb" accept=".nzb" required>
                </div>
                <div>
                    <button type="submit">🚀 Upload and Start Streaming</button>
                </div>
            </form>
        </div>
        <p><strong>Test file:</strong> Use <code>/tmp/unencrypted.nzb</code> for testing</p>
    </div>
    
    <div class="section">
        <h2>📡 API Endpoints</h2>
        <div class="endpoint">
            <strong>Upload NZB:</strong><br>
            POST /upload (multipart/form-data with 'nzb' field)
        </div>
        <div class="endpoint">
            <strong>Stream Video:</strong><br>
            GET /stream/{session_id}
        </div>
        <div class="endpoint">
            <strong>Session Status:</strong><br>
            GET /session/{session_id}/status
        </div>
        <div class="endpoint">
            <strong>Health Check:</strong><br>
            <a href="/health">GET /health</a>
        </div>
    </div>
    
    <div class="section">
        <h2>🎮 Testing with MPV</h2>
        <p>After uploading, test streaming with:</p>
        <div class="endpoint">
            mpv http://localhost:8080/stream/{session_id}
        </div>
        <p>MPV will handle range requests and show download progress</p>
    </div>
    
    <div class="section">
        <h2>🔄 Flow</h2>
        <ol>
            <li><strong>Upload NZB</strong> → Server creates session and returns session_id</li>
            <li><strong>Background Processing</strong> → Downloads PAR2 files, deobfuscates names</li>
            <li><strong>Priority Download</strong> → Starts downloading RAR files (.rar, .r00, .r01...)</li>
            <li><strong>Stream Ready</strong> → Start streaming after first few segments</li>
            <li><strong>Concurrent Streaming</strong> → Stream while download continues</li>
        </ol>
    </div>
</body>
</html>
    "#,
    )
}

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION"),
        "service": "nzb-streaming-server"
    }))
}

async fn upload_handler(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    info!("📤 NZB upload request received");

    // Extract NZB file from multipart form
    let mut nzb_data = None;

    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
        if let Some(name) = field.name() {
            if name == "nzb" {
                match field.bytes().await {
                    Ok(bytes) => {
                        nzb_data = Some(String::from_utf8_lossy(&bytes).to_string());
                        break;
                    }
                    Err(e) => {
                        warn!("Failed to read NZB file: {}", e);
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({
                                "error": "Failed to read NZB file",
                                "message": e.to_string()
                            })),
                        );
                    }
                }
            }
        }
    }

    let nzb_content = match nzb_data {
        Some(data) => data,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "No NZB file provided",
                    "message": "Please upload an NZB file using the 'nzb' form field"
                })),
            );
        }
    };

    // Create session
    match state.session_manager.create_session(&nzb_content).await {
        Ok(session_id) => {
            info!("✅ Created session {}", session_id);

            // Start background downloader if not in mock mode, or mock processor in mock mode
            if !state.mock_mode {
                if let Err(e) = state
                    .session_manager
                    .start_session_downloader(session_id)
                    .await
                {
                    warn!(
                        "Failed to start downloader for session {}: {}",
                        session_id, e
                    );
                }
            } else {
                info!("📁 Mock mode: Starting mock data processor");
                if let Some(ref mock_dir) = state.mock_data_dir {
                    if let Err(e) = state
                        .session_manager
                        .start_mock_processor(session_id, mock_dir.clone())
                        .await
                    {
                        warn!(
                            "Failed to start mock processor for session {}: {}",
                            session_id, e
                        );
                    }
                }
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "session_id": session_id,
                    "stream_url": format!("/stream/{}", session_id),
                    "status_url": format!("/session/{}/status", session_id),
                    "message": "NZB uploaded successfully. Processing PAR2 files...",
                    "mode": if state.mock_mode { "mock" } else { "live" }
                })),
            )
        }
        Err(e) => {
            warn!("Failed to create session: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to create session",
                    "message": e.to_string()
                })),
            )
        }
    }
}

async fn session_status_handler(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> impl IntoResponse {
    match state.session_manager.get_session(session_id).await {
        Ok(session_arc) => {
            let session = session_arc.lock().await;

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "session_id": session_id,
                    "created_at": session.created_at.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
                    "age_seconds": session.age_seconds(),
                    "par2_files_count": session.par2_files.len(),
                    "rar_files_count": session.rar_files.len(),
                    "video_files_count": session.video_files.len(),
                    "par2_complete": session.par2_complete.load(std::sync::atomic::Ordering::Relaxed),
                    "deobfuscation_complete": session.deobfuscation_complete.load(std::sync::atomic::Ordering::Relaxed),
                    "stream_ready": session.stream_ready.load(std::sync::atomic::Ordering::Relaxed),
                    "video_files": session.video_files,
                    "obfuscated_mappings_count": session.obfuscated_to_real.len(),
                })),
            )
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Session not found",
                "message": e.to_string()
            })),
        ),
    }
}

async fn stream_handler(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
) -> impl IntoResponse {
    info!("📺 Stream request for session {}", session_id);

    match state.session_manager.get_session(session_id).await {
        Ok(session_arc) => {
            // Use the streaming manager to handle the request
            match state
                .stream_manager
                .handle_stream_request(&session_id.to_string(), &headers, session_arc)
                .await
            {
                Ok(response) => response,
                Err(e) => {
                    warn!("Streaming error for session {}: {}", session_id, e);
                    axum::response::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .header(axum::http::header::CONTENT_TYPE, "application/json")
                        .body(axum::body::Body::from(format!(
                            r#"{{"error": "Streaming error", "message": "{e}"}}"#
                        )))
                        .unwrap_or_else(|_| {
                            axum::response::Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .body(axum::body::Body::from("Internal server error"))
                                .unwrap()
                        })
                }
            }
        }
        Err(e) => axum::response::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(format!(
                r#"{{"error": "Session not found", "message": "{e}"}}"#
            )))
            .unwrap_or_else(|_| {
                axum::response::Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(axum::body::Body::from("Session not found"))
                    .unwrap()
            }),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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

    #[tokio::test]
    async fn test_app_state_creation() {
        let temp_dir = TempDir::new().unwrap();
        let session_manager = SessionManager::new(temp_dir.path().to_path_buf());

        let state = AppState {
            session_manager: Arc::new(session_manager),
            stream_manager: Arc::new(StreamManager::new()),
            mock_mode: true,
            mock_data_dir: Some(temp_dir.path().to_path_buf()),
        };

        assert!(state.mock_mode);
        assert_eq!(state.session_manager.session_count().await, 0);
    }
}
