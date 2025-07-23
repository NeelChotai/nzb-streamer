// use anyhow::Result;
// use axum::{
//     body::Body,
//     extract::{Multipart, Path, Query, State},
//     http::{HeaderMap, StatusCode},
//     response::{Json, Response},
//     routing::{get, post},
//     Router,
// };
// use nzb_streamer::{
//     http::{range::RangeRequest, response::create_streaming_response},
//     nzb::NzbParser,
//     nntp::{NntpClient},
//     Result as NzbResult
// };
// use serde::Deserialize;
// use serde_json::{json, Value};
// use std::io::Cursor;
// use std::sync::Arc;
// use tokio::net::TcpListener;
// use tower::ServiceBuilder;
// use tower_http::{
//     cors::CorsLayer,
//     trace::TraceLayer,
// };
// use tracing::{info, warn};
// use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// #[derive(Clone)]
// struct AppState {
//     session_manager: Arc<SessionManager>,
//     nntp_config: Option<Arc<NntpConfig>>,
// }

// #[tokio::main]
// async fn main() -> Result<()> {
//     // Initialize tracing
//     tracing_subscriber::registry()
//         .with(
//             tracing_subscriber::EnvFilter::try_from_default_env()
//                 .unwrap_or_else(|_| "nzb_streamer=debug,tower_http=debug".into()),
//         )
//         .with(tracing_subscriber::fmt::layer())
//         .init();

//     info!("Starting NZB Streamer server...");

//     // Create shared state
//     let session_manager = Arc::new(SessionManager::new());
    
//     // Load NNTP configuration from environment
//     let nntp_config = match NntpConfig::from_env() {
//         Ok(config) => {
//             info!("NNTP configuration loaded: {}:{}", config.host, config.port);
//             Some(Arc::new(config))
//         }
//         Err(e) => {
//             warn!("NNTP configuration not available: {}. Running in mock mode only.", e);
//             None
//         }
//     };

//     let app_state = AppState {
//         session_manager,
//         nntp_config,
//     };

//     // Build our application with routes
//     let app = Router::new()
//         .route("/", get(root))
//         .route("/health", get(health_check))
//         .route("/upload", post(upload_nzb))
//         .route("/sessions", get(list_sessions))
//         .route("/sessions/:session_id", get(get_session_info))
//         .route("/sessions/:session_id/files", get(list_session_files))
//         .route("/stream/:session_id/*file_path", get(stream_file))
//         .with_state(app_state)
//         .layer(
//             ServiceBuilder::new()
//                 .layer(TraceLayer::new_for_http())
//                 .layer(CorsLayer::permissive()),
//         );

//     // Run the server
//     let listener = TcpListener::bind("127.0.0.1:3334").await?;
//     info!("Server listening on http://127.0.0.1:3334");
    
//     axum::Server::from_tcp(listener.into_std()?)?
//         .serve(app.into_make_service())
//         .await?;

//     Ok(())
// }

// async fn root() -> Json<Value> {
//     Json(json!({
//         "name": "NZB Streamer",
//         "version": "0.1.0",
//         "status": "running",
//         "endpoints": {
//             "health": "/health",
//             "upload": "/upload",
//             "sessions": "/sessions",
//             "stream": "/stream/:session_id/*file_path",
//             "test_video": "/test-video"
//         }
//     }))
// }

// async fn health_check() -> Json<Value> {
//     Json(json!({
//         "status": "healthy",
//         "timestamp": chrono::Utc::now().to_rfc3339()
//     }))
// }

// async fn upload_nzb(
//     State(app_state): State<AppState>,
//     mut multipart: Multipart,
// ) -> Result<Json<Value>, StatusCode> {
//     while let Some(field) = multipart.next_field().await.map_err(|_| StatusCode::BAD_REQUEST)? {
//         let name = field.name().unwrap_or("unknown");
        
//         if name == "nzb" {
//             let filename = field.file_name().map(|s| s.to_string());
//             let content_type = field.content_type().map(|s| s.to_string());
//             let data = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
            
//             info!("Received NZB upload: {:?} ({} bytes)", filename, data.len());
            
//             // Parse the NZB
//             match parse_nzb_and_create_session(&app_state.session_manager, &data).await {
//                 Ok((session, info)) => {
//                     return Ok(Json(json!({
//                         "success": true,
//                         "session_id": session.id,
//                         "filename": filename,
//                         "content_type": content_type,
//                         "size": data.len(),
//                         "files": info.file_count,
//                         "total_size": info.total_size,
//                         "video_files": info.video_files,
//                         "streaming_urls": create_streaming_urls(&session),
//                         "message": "NZB parsed successfully and session created"
//                     })));
//                 }
//                 Err(e) => {
//                     warn!("Failed to parse NZB: {}", e);
//                     return Ok(Json(json!({
//                         "success": false,
//                         "error": format!("Failed to parse NZB: {}", e)
//                     })));
//                 }
//             }
//         }
//     }
    
//     Err(StatusCode::BAD_REQUEST)
// }

// #[derive(Debug)]
// struct NzbInfo {
//     file_count: usize,
//     total_size: u64,
//     video_files: usize,
// }

// async fn parse_nzb_and_create_session(
//     session_manager: &SessionManager, 
//     data: &[u8]
// ) -> NzbResult<(nzb_streamer::vfs::StreamingSession, NzbInfo)> {
//     // First try parsing normally
//     let cursor = Cursor::new(data);
//     let mut nzb = NzbParser::parse(cursor)?;
    
//     // Check if we need deobfuscation (files with hash-like names)
//     let needs_deobfuscation = nzb.files.iter()
//         .filter(|f| !f.path.contains('.') || f.path.len() == 32 || f.path.len() == 64)
//         .count() > nzb.files.len() / 2;
    
//     if needs_deobfuscation {
//         info!("Detected obfuscated filenames, attempting PAR2 deobfuscation...");
        
//         // Create a mock NNTP client for header fetching only
//         // In a real implementation, this would use the actual NNTP config
//         match std::env::var("NNTP_HOST") {
//             Ok(host) => {
//                 let config = NntpConfig {
//                     host,
//                     port: std::env::var("NNTP_PORT")
//                         .unwrap_or_else(|_| "563".to_string())
//                         .parse()
//                         .unwrap_or(563),
//                     username: std::env::var("NNTP_USER").unwrap_or_default(),
//                     password: std::env::var("NNTP_PASS").unwrap_or_default(),
//                     use_tls: true,
//                     connection_timeout: 30,
//                     read_timeout: 60,
//                     max_connections: 5,
//                 };
                
//                 let client = NntpClient::new(config);
                
//                 match NzbParser::parse_with_deobfuscation(data, &client).await {
//                     Ok(deobfuscated_nzb) => {
//                         info!("Successfully deobfuscated {} files", deobfuscated_nzb.files.len());
//                         nzb = deobfuscated_nzb;
//                     }
//                     Err(e) => {
//                         warn!("Failed to deobfuscate NZB: {}. Using original filenames.", e);
//                     }
//                 }
//             }
//             Err(_) => {
//                 warn!("NNTP configuration not available for deobfuscation. Using original filenames.");
//             }
//         }
//     }
    
//     let video_files = nzb.find_video_files().len();
//     let info = NzbInfo {
//         file_count: nzb.file_count(),
//         total_size: nzb.total_size(),
//         video_files,
//     };
    
//     let session = session_manager.create_session(nzb).await;
    
//     Ok((session, info))
// }

// fn create_streaming_urls(session: &nzb_streamer::vfs::StreamingSession) -> Vec<Value> {
//     session.get_video_files()
//         .iter()
//         .map(|file| {
//             json!({
//                 "filename": file.path,
//                 "size": file.size,
//                 "url": format!("http://127.0.0.1:3334/stream/{}/{}", session.id, file.path)
//             })
//         })
//         .collect()
// }

// // New endpoint handlers

// async fn list_sessions(
//     State(app_state): State<AppState>,
// ) -> Json<Value> {
//     let session_ids = app_state.session_manager.list_sessions().await;
//     Json(json!({
//         "sessions": session_ids,
//         "count": session_ids.len()
//     }))
// }

// async fn get_session_info(
//     State(app_state): State<AppState>,
//     Path(session_id): Path<String>,
// ) -> Result<Json<Value>, StatusCode> {
//     let session = app_state.session_manager.get_session(&session_id).await
//         .ok_or(StatusCode::NOT_FOUND)?;
    
//     Ok(Json(json!({
//         "session_id": session.id,
//         "created_at": session.created_at.to_rfc3339(),
//         "files": session.nzb.file_count(),
//         "total_size": session.nzb.total_size(),
//         "video_files": session.get_video_files().len()
//     })))
// }

// async fn list_session_files(
//     State(app_state): State<AppState>,
//     Path(session_id): Path<String>,
// ) -> Result<Json<Value>, StatusCode> {
//     let session = app_state.session_manager.get_session(&session_id).await
//         .ok_or(StatusCode::NOT_FOUND)?;
    
//     let files: Vec<Value> = session.nzb.files.iter()
//         .map(|file| json!({
//             "path": file.path,
//             "size": file.size,
//             "segments": file.segments.len(),
//             "is_video": nzb_streamer::nzb::types::is_video_file(&file.path),
//             "streaming_url": format!("http://127.0.0.1:3334/stream/{}/{}", session_id, file.path)
//         }))
//         .collect();
    
//     Ok(Json(json!({
//         "session_id": session_id,
//         "files": files
//     })))
// }

// #[derive(Deserialize)]
// struct StreamQuery {
//     #[serde(default)]
//     mock: bool,  // For testing without NNTP
// }

// async fn stream_file(
//     State(app_state): State<AppState>,
//     Path((session_id, file_path)): Path<(String, String)>,
//     Query(query): Query<StreamQuery>,
//     headers: HeaderMap,
// ) -> Result<Response<Body>, StatusCode> {
//     let session = app_state.session_manager.get_session(&session_id).await
//         .ok_or(StatusCode::NOT_FOUND)?;
    
//     let file = session.find_file_by_path(&file_path)
//         .ok_or(StatusCode::NOT_FOUND)?;
    
//     info!("Streaming request for RAR file: {} (size: {} bytes)", file_path, file.size);
    
//     // Get the password from the NZB metadata
//     let rar_password = session.nzb.meta.attributes.get("password")
//         .map(|s| s.as_str());
    
//     if let Some(password) = &rar_password {
//         info!("RAR archive is password protected with: {}", password);
//     }
    
//     // Fetch and stream the video content
//     let video_data = if query.mock {
//         create_mock_video_data(file.size)
//     } else if let Some(nntp_config) = &app_state.nntp_config {
//         // Fetch actual data from NNTP and extract video from RAR
//         match fetch_and_extract_video(nntp_config, file, rar_password).await {
//             Ok(data) => data,
//             Err(e) => {
//                 warn!("Failed to fetch video from NNTP: {}", e);
//                 return Err(StatusCode::INTERNAL_SERVER_ERROR);
//             }
//         }
//     } else {
//         warn!("NNTP not configured and mock not requested");
//         return Err(StatusCode::SERVICE_UNAVAILABLE);
//     };
    
//     // Handle range requests
//     let actual_size = video_data.len() as u64;
//     match RangeRequest::parse_range_header(&headers, actual_size)? {
//         Some(range_request) => {
//             let range = range_request.to_range(actual_size);
//             let chunk = video_data.slice(range.start as usize..range.end as usize);
//             let content_length = chunk.len() as u64;
            
//             let range_headers = nzb_streamer::http::range::create_range_response_headers(
//                 &range_request,
//                 content_length,
//                 actual_size,
//                 &get_content_type(&file_path),
//             )?;
            
//             Ok(create_streaming_response(chunk, Some(range_headers), &file_path))
//         }
//         None => {
//             // Serve entire file
//             Ok(create_streaming_response(video_data, None, &file_path))
//         }
//     }
// }

// // Helper functions

// async fn fetch_and_extract_video(
//     nntp_config: &NntpConfig,
//     file: &nzb_streamer::nzb::types::NzbFile,
//     password: Option<&str>,
// ) -> Result<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>> {
//     info!("Fetching RAR file from NNTP: {} ({} segments)", file.path, file.segments.len());
    
//     // Create NNTP client
//     let client = NntpClient::new(nntp_config.clone());
    
//     // Collect all message IDs for this file
//     let message_ids: Vec<String> = file.segments
//         .iter()
//         .map(|segment| segment.message_id.clone())
//         .collect();
    
//     // Fetch all segments and combine them into the complete RAR file
//     let rar_data = client.fetch_segments(&message_ids).await?;
    
//     info!("Downloaded RAR file: {} bytes", rar_data.len());
    
//     // Try to extract video content from the RAR
//     match RarParser::extract_file_content(&rar_data, "video", password) {
//         Ok(video_data) => {
//             info!("Successfully extracted video from RAR: {} bytes", video_data.len());
//             Ok(bytes::Bytes::from(video_data))
//         }
//         Err(e) => {
//             warn!("Failed to extract video from RAR: {}. Serving RAR directly.", e);
//             // Fallback: serve the RAR file directly (some players can handle this)
//             Ok(bytes::Bytes::from(rar_data))
//         }
//     }
// }

// fn create_mock_video_data(size: u64) -> bytes::Bytes {
//     // Create test video data with a repeating pattern
//     let mut data = Vec::with_capacity(size as usize);
//     for i in 0..size {
//         data.push((i % 256) as u8);
//     }
//     bytes::Bytes::from(data)
// }

// fn get_content_type(filename: &str) -> String {
//     mime_guess::from_path(filename)
//         .first_or_octet_stream()
//         .to_string()
// }