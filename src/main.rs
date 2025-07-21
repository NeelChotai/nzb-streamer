use anyhow::Result;
use axum::{
    extract::Multipart,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use nzb_streamer::{nzb::NzbParser, Result as NzbResult};
use serde_json::{json, Value};
use std::io::Cursor;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::{
    cors::CorsLayer,
    trace::TraceLayer,
};
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nzb_streamer=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting NZB Streamer server...");

    // Build our application with routes
    let app = Router::new()
        .route("/", get(root))
        .route("/health", get(health_check))
        .route("/upload", post(upload_nzb))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        );

    // Run the server
    let listener = TcpListener::bind("127.0.0.1:3000").await?;
    info!("Server listening on http://127.0.0.1:3000");
    
    axum::Server::from_tcp(listener.into_std()?)?
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

async fn root() -> Json<Value> {
    Json(json!({
        "name": "NZB Streamer",
        "version": "0.1.0",
        "status": "running",
        "endpoints": {
            "health": "/health",
            "upload": "/upload"
        }
    }))
}

async fn health_check() -> Json<Value> {
    Json(json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

async fn upload_nzb(mut multipart: Multipart) -> Result<Json<Value>, StatusCode> {
    while let Some(field) = multipart.next_field().await.map_err(|_| StatusCode::BAD_REQUEST)? {
        let name = field.name().unwrap_or("unknown");
        
        if name == "nzb" {
            let filename = field.file_name().map(|s| s.to_string());
            let content_type = field.content_type().map(|s| s.to_string());
            let data = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
            
            info!("Received NZB upload: {:?} ({} bytes)", filename, data.len());
            
            // Parse the NZB
            match parse_nzb_data(&data).await {
                Ok(info) => {
                    return Ok(Json(json!({
                        "success": true,
                        "filename": filename,
                        "content_type": content_type,
                        "size": data.len(),
                        "files": info.file_count,
                        "total_size": info.total_size,
                        "video_files": info.video_files,
                        "message": "NZB parsed successfully"
                    })));
                }
                Err(e) => {
                    warn!("Failed to parse NZB: {}", e);
                    return Ok(Json(json!({
                        "success": false,
                        "error": format!("Failed to parse NZB: {}", e)
                    })));
                }
            }
        }
    }
    
    Err(StatusCode::BAD_REQUEST)
}

#[derive(Debug)]
struct NzbInfo {
    file_count: usize,
    total_size: u64,
    video_files: usize,
}

async fn parse_nzb_data(data: &[u8]) -> NzbResult<NzbInfo> {
    let cursor = Cursor::new(data);
    let nzb = NzbParser::parse(cursor)?;
    
    let video_files = nzb.find_video_files().len();
    
    Ok(NzbInfo {
        file_count: nzb.file_count(),
        total_size: nzb.total_size(),
        video_files,
    })
}