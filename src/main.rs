use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use clap::Parser;
use nzb_streamer::{
    debug::{create_debug_router, metrics::MetricsCollector},
    nzb::NzbParser,
    nntp::config::NntpConfig,
    streaming::{StreamingServer, StreamingConfig},
    stremio::StremioAddon,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "test-server")]
#[command(about = "Local testing server for NZB streaming")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Args {
    /// Path to NZB file to load
    #[arg(long, default_value = "/tmp/unencrypted.nzb")]
    nzb: PathBuf,

    /// Server port
    #[arg(short, long, default_value = "8080")]
    port: u16,

    /// Directory containing pre-downloaded segments (mock mode)
    #[arg(long, default_value = "/tmp/downloaded")]
    mock_data: Option<PathBuf>,

    /// Use live NNTP downloads instead of mock data
    #[arg(long)]
    live_download: bool,

    /// Enable debug logging
    #[arg(long)]
    debug: bool,

    /// Host to bind to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Disable debug UI
    #[arg(long)]
    no_debug_ui: bool,
}

/// Download mode for the test server
#[derive(Debug, Clone)]
enum DownloadMode {
    /// Use pre-downloaded files from local directory
    Mock(PathBuf),
    /// Use live NNTP connections
    Live(NntpConfig),
    /// Check local files first, fallback to NNTP
    Hybrid(PathBuf, NntpConfig),
}

/// Main application state
#[derive(Debug)]
struct AppState {
    streaming_server: Arc<StreamingServer>,
    stremio_addon: Arc<StremioAddon>,
    metrics: Arc<MetricsCollector>,
    download_mode: DownloadMode,
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

    // Determine download mode
    let download_mode = if args.live_download {
        info!("🌐 Using LIVE NNTP downloads");
        warn!("⚠️  This will consume your NNTP quota!");
        
        let nntp_config = load_nntp_config()?;
        DownloadMode::Live(nntp_config)
    } else if let Some(mock_dir) = args.mock_data {
        info!("📁 Using MOCK data from: {}", mock_dir.display());
        
        if !mock_dir.exists() {
            warn!("Mock data directory doesn't exist: {}", mock_dir.display());
            info!("Creating directory and some test files...");
            create_mock_data(&mock_dir).await?;
        }
        
        DownloadMode::Mock(mock_dir)
    } else {
        return Err(anyhow::anyhow!("No download mode specified"));
    };

    // Load and parse NZB
    info!("📋 Loading NZB: {}", args.nzb.display());
    let nzb_data = tokio::fs::read(&args.nzb)
        .await
        .with_context(|| format!("Failed to read NZB file: {}", args.nzb.display()))?;

    let nzb_content = String::from_utf8(nzb_data)
        .with_context(|| "NZB file is not valid UTF-8")?;
    
    let nzb = NzbParser::parse(&nzb_content)
        .with_context(|| "Failed to parse NZB file")?;

    info!("✅ NZB loaded successfully:");
    info!("   - {} files", nzb.files.len());
    let total_size: u64 = nzb.files.iter().map(|f| f.segments.iter().map(|s| s.size as u64).sum::<u64>()).sum();
    info!("   - {:.2} GB total", total_size as f64 / (1024.0 * 1024.0 * 1024.0));
    
    let video_files: Vec<_> = nzb.files.iter()
        .filter(|f| f.subject.to_lowercase().contains(".mkv") || 
                   f.subject.to_lowercase().contains(".mp4") ||
                   f.subject.to_lowercase().contains(".avi"))
        .collect();
    info!("   - {} video files found", video_files.len());

    // Create application components
    let streaming_config = StreamingConfig {
        cache_dir: PathBuf::from("./tmp/cache"),
        max_cache_size: 10 * 1024 * 1024 * 1024, // 10GB
        cache_ttl_secs: 24 * 60 * 60, // 24 hours
        max_concurrent_streams: 10,
        buffer_size: 64 * 1024, // 64KB
        lookahead_bytes: 5 * 1024 * 1024, // 5MB
        download_timeout_secs: 60,
    };

    let streaming_server = Arc::new(
        StreamingServer::new(streaming_config).await
            .with_context(|| "Failed to create streaming server")?
    );

    let stremio_addon = Arc::new(StremioAddon::new(streaming_server.clone()));
    let metrics = Arc::new(MetricsCollector::new());

    let app_state = Arc::new(AppState {
        streaming_server,
        stremio_addon,
        metrics,
        download_mode,
    });

    // Create router
    let app = create_router(app_state.clone(), !args.no_debug_ui).await?;

    // Start server
    let bind_addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Failed to bind to {}", bind_addr))?;

    info!("🚀 Server started successfully!");
    info!("");
    info!("📡 Server listening on: http://{}", bind_addr);
    info!("🎬 Stremio addon: http://{}/manifest.json", bind_addr);
    
    if !args.no_debug_ui {
        info!("🔍 Debug UI: http://{}/debug", bind_addr);
    }
    
    info!("");
    info!("📺 Available streams:");
    for (i, file) in video_files.iter().enumerate() {
        info!("   http://{}/stream/test/{}", bind_addr, i);
        info!("     📁 {}", file.subject);
        let file_size: u64 = file.segments.iter().map(|s| s.size as u64).sum();
        info!("     📏 {:.2} MB", file_size as f64 / (1024.0 * 1024.0));
    }
    
    info!("");
    info!("🎮 Test with MPV:");
    if !video_files.is_empty() {
        info!("   mpv http://{}/stream/test/0", bind_addr);
    }
    info!("   ./scripts/test_with_mpv.sh http://{}/stream/test/0", bind_addr);
    
    info!("");
    info!("✋ Press Ctrl+C to stop the server");
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
                .unwrap_or_else(|_| format!("nzb_streamer={},tower_http=info", filter_level).into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    Ok(())
}

fn print_banner(args: &Args) {
    println!("╭─────────────────────────────────────────────────╮");
    println!("│            🎬 NZB Streaming Test Server          │");
    println!("│                                                 │");
    println!("│  🦀 Built with Rust  •  ⚡ Powered by Tokio     │");
    println!("╰─────────────────────────────────────────────────╯");
    println!();
    
    if args.live_download {
        println!("⚠️  WARNING: Live NNTP mode enabled!");
        println!("   This will download from actual NNTP servers");
        println!("   and consume your quota. Use mock mode for");
        println!("   development: remove --live-download flag");
        println!();
    }
}

fn load_nntp_config() -> Result<NntpConfig> {
    let host = std::env::var("NNTP_HOST")
        .context("NNTP_HOST environment variable not set")?;
    
    let port = std::env::var("NNTP_PORT")
        .unwrap_or_else(|_| "563".to_string())
        .parse()
        .context("Invalid NNTP_PORT")?;
    
    let username = std::env::var("NNTP_USER")
        .context("NNTP_USER environment variable not set")?;
    
    let password = std::env::var("NNTP_PASS")
        .context("NNTP_PASS environment variable not set")?;
    
    let use_tls = std::env::var("NNTP_USE_TLS")
        .unwrap_or_else(|_| "true".to_string())
        .parse()
        .unwrap_or(true);
    

    Ok(NntpConfig::new(host, username, password, port, use_tls))
}

async fn create_mock_data(mock_dir: &PathBuf) -> Result<()> {
    tokio::fs::create_dir_all(mock_dir).await?;
    
    // Create some mock segment files
    let mock_files = [
        "message-id-001@test.example.com",
        "message-id-002@test.example.com", 
        "message-id-003@test.example.com",
        "message-id-004@test.example.com",
        "message-id-005@test.example.com",
    ];
    
    for file in &mock_files {
        let file_path = mock_dir.join(file);
        if !file_path.exists() {
            // Create a 15MB file with test data
            let test_data = vec![0u8; 15 * 1024 * 1024];
            tokio::fs::write(&file_path, &test_data).await?;
            info!("Created mock file: {}", file_path.display());
        }
    }
    
    Ok(())
}

async fn create_router(
    state: Arc<AppState>,
    enable_debug: bool,
) -> Result<Router> {

    let mut app = Router::new()
        .route("/", get(root_handler))
        .route("/health", get(health_handler))
        .route("/manifest.json", get(manifest_handler))
        .route("/stream/{session_id}/{file_index}", get(stream_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    if enable_debug {
        println!("Debug UI would be enabled at /debug");
        // TODO: Fix debug router state type compatibility
        // app = app.merge(create_debug_router());
    }

    Ok(app)
}

async fn root_handler() -> impl IntoResponse {
    Html(r#"
<!DOCTYPE html>
<html>
<head>
    <title>NZB Streaming Test Server</title>
    <style>
        body { font-family: system-ui, sans-serif; max-width: 800px; margin: 50px auto; padding: 20px; }
        .header { background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); color: white; padding: 30px; border-radius: 10px; text-align: center; margin-bottom: 30px; }
        .section { background: white; padding: 20px; border-radius: 8px; box-shadow: 0 2px 10px rgba(0,0,0,0.1); margin-bottom: 20px; }
        .endpoint { background: #f8f9fa; padding: 15px; border-radius: 5px; margin: 10px 0; font-family: monospace; }
        .status { color: #28a745; font-weight: bold; }
        a { color: #667eea; text-decoration: none; }
        a:hover { text-decoration: underline; }
    </style>
</head>
<body>
    <div class="header">
        <h1>🎬 NZB Streaming Test Server</h1>
        <p>Status: <span class="status">Running</span></p>
    </div>
    
    <div class="section">
        <h2>📡 Available Endpoints</h2>
        <div class="endpoint">
            <strong>Stremio Addon:</strong><br>
            <a href="/manifest.json">/manifest.json</a>
        </div>
        <div class="endpoint">
            <strong>Stream Endpoint:</strong><br>
            /stream/{session_id}/{file_index}
        </div>
        <div class="endpoint">
            <strong>Debug UI:</strong><br>
            <a href="/debug">/debug</a>
        </div>
        <div class="endpoint">
            <strong>Health Check:</strong><br>
            <a href="/health">/health</a>
        </div>
    </div>
    
    <div class="section">
        <h2>🎮 Testing</h2>
        <p>Test streaming with MPV:</p>
        <div class="endpoint">
            mpv http://localhost:8080/stream/test/0
        </div>
        <p>Or use the test script:</p>
        <div class="endpoint">
            ./scripts/test_with_mpv.sh
        </div>
    </div>
    
    <div class="section">
        <h2>🔗 Add to Stremio</h2>
        <p>Add this URL as an addon in Stremio:</p>
        <div class="endpoint">
            <a href="/manifest.json">http://localhost:8080/manifest.json</a>
        </div>
    </div>
</body>
</html>
    "#)
}

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION"),
        "uptime": "TODO: implement uptime tracking"
    }))
}

async fn manifest_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    // For now, return a basic manifest
    Json(serde_json::json!({
        "id": "nzb-streamer-test",
        "name": "NZB Streamer Test",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Test server for NZB streaming",
        "logo": "https://via.placeholder.com/256x256/667eea/white?text=NZB",
        "types": ["movie", "series"],
        "resources": ["stream"],
        "idPrefixes": ["test:"],
        "catalogs": []
    }))
}

async fn stream_handler(
    State(state): State<Arc<AppState>>,
    Path((session_id, file_index)): Path<(String, String)>,
    _headers: HeaderMap,
) -> impl IntoResponse {
    info!("Stream request: session={}, file={}", session_id, file_index);
    
    // For now, return a simple response indicating the stream would be here
    match state.download_mode {
        DownloadMode::Mock(ref mock_dir) => {
            info!("Would serve from mock data in: {}", mock_dir.display());
        }
        DownloadMode::Live(ref _config) => {
            warn!("Would download from NNTP (not implemented yet)");
        }
        DownloadMode::Hybrid(ref mock_dir, ref _config) => {
            info!("Would check mock data first: {}", mock_dir.display());
        }
    }
    
    // Return a placeholder response for now
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "Streaming not yet implemented",
            "message": "This is a test server. Streaming functionality is being implemented.",
            "session_id": session_id,
            "file_index": file_index,
            "mode": match state.download_mode {
                DownloadMode::Mock(_) => "mock",
                DownloadMode::Live(_) => "live", 
                DownloadMode::Hybrid(_, _) => "hybrid"
            }
        }))
    )
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

    #[test]
    fn test_args_parsing() {
        let args = Args::try_parse_from(&[
            "test-server",
            "--nzb", "/tmp/test.nzb",
            "--port", "9000",
            "--debug"
        ]).unwrap();

        assert_eq!(args.nzb, PathBuf::from("/tmp/test.nzb"));
        assert_eq!(args.port, 9000);
        assert!(args.debug);
        assert!(!args.live_download);
    }

    #[tokio::test]
    async fn test_mock_data_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mock_dir = temp_dir.path().to_path_buf();
        
        create_mock_data(&mock_dir).await.unwrap();
        
        assert!(mock_dir.exists());
        assert!(mock_dir.join("message-id-001@test.example.com").exists());
    }
}