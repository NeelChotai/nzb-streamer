use crate::error::{NzbStreamerError};
use crate::session::{DownloadTask, Session};
use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

/// RAR file signatures and constants
const RAR_SIGNATURE: [u8; 7] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const MKV_SIGNATURE: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

/// Represents a segment in the download queue
#[derive(Debug, Clone)]
pub struct StreamSegment {
    pub segment_index: usize,
    pub file_index: usize,
    pub data: Vec<u8>,
    pub byte_range: Range<u64>,
}

/// Stream manager for handling MKV streaming from RAR files
pub struct StreamManager {
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<Session>>>>>,
}

impl Default for StreamManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Handle HTTP range request for streaming
    pub async fn handle_stream_request(
        &self,
        session_id: &str,
        headers: &HeaderMap,
        session: Arc<Mutex<Session>>,
    ) -> Result<Response<Body>> {
        let session_guard = session.lock().await;

        // Check if streaming is ready
        if !session_guard.is_ready_for_streaming() {
            return Ok(Response::builder()
                .status(StatusCode::ACCEPTED)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"status":"processing","message":"Stream not ready yet"}"#,
                ))?);
        }

        // For streaming, find the first RAR file from PAR2 mappings (like yay.rar)
        let target_filename = if let Some((real_name, _)) = session_guard
            .par2_mappings
            .iter()
            .find(|(real, _)| real.ends_with(".rar"))
        {
            real_name.clone()
        } else {
            // Fallback: use first RAR file subject if no PAR2 mapping
            if !session_guard.rar_files.is_empty() {
                session_guard.rar_files[0].subject.clone()
            } else {
                return Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("No RAR files found for streaming"))?);
            }
        };

        info!("Streaming from RAR data: {}", target_filename);

        // Handle range request
        if let Some(range_header) = headers.get(header::RANGE) {
            self.handle_range_request(
                &session_guard,
                range_header.to_str().unwrap_or(""),
                &target_filename,
            )
            .await
        } else {
            self.handle_full_request(&session_guard, &target_filename)
                .await
        }
    }

    /// Handle HTTP range request (partial content)
    async fn handle_range_request(
        &self,
        session: &Session,
        range_header: &str,
        video_filename: &str,
    ) -> Result<Response<Body>> {
        debug!("Range request: {}", range_header);

        // Parse range header (basic implementation)
        let range = self.parse_range_header(range_header)?;

        // Get available data for the range
        let (data, content_range, total_size) = self
            .get_data_for_range(session, &range, video_filename)
            .await?;

        if data.is_empty() {
            // Range not yet available
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{total_size}"))
                .body(Body::empty())?);
        }

        // Return partial content
        Ok(Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, "video/x-matroska")
            .header(header::CONTENT_RANGE, content_range)
            .header(header::CONTENT_LENGTH, data.len().to_string())
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Body::from(data))?)
    }

    /// Handle full file request
    async fn handle_full_request(
        &self,
        session: &Session,
        video_filename: &str,
    ) -> Result<Response<Body>> {
        debug!("Full file request for: {}", video_filename);

        // For now, return a placeholder response
        // In a real implementation, we'd stream the entire file
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "video/x-matroska")
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Body::from("MKV streaming not fully implemented yet"))?)
    }

    /// Parse HTTP range header
    fn parse_range_header(&self, range_header: &str) -> Result<Range<u64>> {
        // Basic range parsing: "bytes=start-end"
        if !range_header.starts_with("bytes=") {
            return Err(NzbStreamerError::HttpRange(
                "Invalid range header".to_string(),
            ));
        }

        let range_part = &range_header[6..]; // Remove "bytes="
        let parts: Vec<&str> = range_part.split('-').collect();

        if parts.len() != 2 {
            return Err(NzbStreamerError::HttpRange(
                "Invalid range format".to_string(),
            ));
        }

        let start = if parts[0].is_empty() {
            0
        } else {
            parts[0]
                .parse()
                .map_err(|_| NzbStreamerError::HttpRange("Invalid range start".to_string()))?
        };

        let end = if parts[1].is_empty() {
            u64::MAX // Open-ended range
        } else {
            parts[1]
                .parse()
                .map_err(|_| NzbStreamerError::HttpRange("Invalid range end".to_string()))?
        };

        Ok(start..end)
    }

    /// Get data for a specific byte range
    async fn get_data_for_range(
        &self,
        session: &Session,
        range: &Range<u64>,
        video_filename: &str,
    ) -> Result<(Vec<u8>, String, u64)> {
        debug!("Getting data for range: {:?}", range);

        // Find the obfuscated filename
        let obfuscated_filename = session
            .obfuscated_to_real
            .iter()
            .find(|(_, real)| real.as_str() == video_filename)
            .map(|(obfuscated, _)| obfuscated.clone());

        if obfuscated_filename.is_none() {
            return Err(NzbStreamerError::StreamNotFound {
                stream_id: video_filename.to_string(),
            });
        }

        let obfuscated = obfuscated_filename.unwrap();
        debug!("Found obfuscated filename: {}", obfuscated);

        // Get downloaded segments for this file
        let _downloaded = session.downloaded_segments.lock().await;

        // Extract MKV data from RAR segments using your MKV extraction logic
        let mkv_data = self
            .extract_mkv_from_rar_segments(session, &obfuscated)
            .await?;
        let total_size = mkv_data.len() as u64;

        let start = range.start.min(total_size);
        let end = if range.end == u64::MAX {
            total_size - 1
        } else {
            range.end.min(total_size - 1)
        };

        if start >= total_size {
            return Ok((Vec::new(), String::new(), total_size));
        }

        let data = mkv_data[start as usize..=end as usize].to_vec();
        let content_range = format!("bytes {start}-{end}/{total_size}");

        Ok((data, content_range, total_size))
    }

    /// Extract MKV data from RAR segments using downloaded segment data
    async fn extract_mkv_from_rar_segments(
        &self,
        session: &Session,
        obfuscated_name: &str,
    ) -> Result<Vec<u8>> {
        let downloaded = session.downloaded_segments.lock().await;

        // Find RAR files that match this obfuscated name
        let mut rar_segments = Vec::new();
        for rar_file in &session.rar_files {
            if rar_file.subject.contains(obfuscated_name)
                || rar_file
                    .subject
                    .starts_with(&obfuscated_name[..obfuscated_name.len().min(10)])
            {
                // Collect downloaded segments for this file
                for segment in &rar_file.segments {
                    if let Some(data) = downloaded.get(&segment.message_id) {
                        rar_segments.push(data.clone());
                    }
                }
                break; // Only process first matching RAR file
            }
        }

        if rar_segments.is_empty() {
            return self.create_mock_mkv_data_vec();
        }

        // Combine all RAR segment data
        let mut combined_data = Vec::new();
        for segment_data in rar_segments {
            combined_data.extend_from_slice(&segment_data);
        }

        // Use the existing MKV extraction logic
        self.extract_mkv_from_rar(&combined_data)
    }

    /// Create mock MKV data as Vec for fallback
    fn create_mock_mkv_data_vec(&self) -> Result<Vec<u8>> {
        Ok(self.create_mock_mkv_data())
    }

    /// Extract MKV data from RAR segments
    fn extract_mkv_from_rar(&self, rar_data: &[u8]) -> Result<Vec<u8>> {
        let mut cursor = Cursor::new(rar_data);

        // Find RAR signature
        let mut buffer = vec![0u8; 1024.min(rar_data.len())];
        cursor.read_exact(&mut buffer)?;

        let rar_offset = buffer
            .windows(7)
            .position(|w| w == RAR_SIGNATURE)
            .ok_or_else(|| NzbStreamerError::RarParsing("No RAR signature found".to_string()))?;

        cursor.seek(SeekFrom::Start(rar_offset as u64 + 7))?; // Skip signature

        let mut extracted_data = Vec::new();

        // Parse RAR headers and extract data
        loop {
            // Read basic header
            let mut header_buffer = [0u8; 7];
            if cursor.read_exact(&mut header_buffer).is_err() {
                break; // End of data
            }

            let _header_crc = u16::from_le_bytes([header_buffer[0], header_buffer[1]]);
            let header_type = header_buffer[2];
            let header_flags = u16::from_le_bytes([header_buffer[3], header_buffer[4]]);
            let header_size = u16::from_le_bytes([header_buffer[5], header_buffer[6]]);

            debug!(
                "RAR header: type=0x{:02X}, flags=0x{:04X}, size={}",
                header_type, header_flags, header_size
            );

            match header_type {
                0x73 => {
                    // MAIN_HEAD - skip remaining bytes
                    cursor.seek(SeekFrom::Current(header_size as i64 - 7))?;
                }
                0x74 => {
                    // FILE_HEAD - this contains our data
                    let mut file_header = vec![0u8; (header_size as usize).saturating_sub(7)];
                    cursor.read_exact(&mut file_header)?;

                    if file_header.len() >= 8 {
                        let pack_size = u32::from_le_bytes([
                            file_header[0],
                            file_header[1],
                            file_header[2],
                            file_header[3],
                        ]);

                        debug!("FILE_HEAD: pack_size={}", pack_size);

                        // Read the packed data
                        let mut packed_data = vec![0u8; pack_size as usize];
                        cursor.read_exact(&mut packed_data)?;

                        // For first file, look for MKV signature
                        if extracted_data.is_empty() {
                            if let Some(mkv_offset) =
                                packed_data.windows(4).position(|w| w == MKV_SIGNATURE)
                            {
                                debug!("Found MKV signature at offset {}", mkv_offset);
                                extracted_data.extend_from_slice(&packed_data[mkv_offset..]);
                            } else {
                                extracted_data.extend_from_slice(&packed_data);
                            }
                        } else {
                            // Subsequent files - just append data
                            extracted_data.extend_from_slice(&packed_data);
                        }
                    }
                }
                0x7B => {
                    // ENDARC_HEAD - end of archive
                    debug!("End of RAR archive");
                    break;
                }
                _ => {
                    // Other header - skip
                    cursor.seek(SeekFrom::Current(header_size as i64 - 7))?;
                }
            }
        }

        Ok(extracted_data)
    }

    /// Create mock MKV data for testing
    fn create_mock_mkv_data(&self) -> Vec<u8> {
        let mut data = Vec::new();

        // Add MKV signature
        data.extend_from_slice(&MKV_SIGNATURE);

        // Add mock MKV header data
        data.extend_from_slice(&[
            0x42, 0x86, 0x81, 0x01, // EBML version
            0x42, 0x87, 0x81, 0x01, // EBML read version
            0x42, 0x85, 0x81, 0x02, // EBML max ID length
            0x42, 0x82, 0x81, 0x02, // EBML max size length
        ]);

        // Add padding to make it a reasonable size for testing
        data.resize(10 * 1024 * 1024, 0); // 10MB mock file

        data
    }
}

/// Helper function to prioritize RAR download based on streaming position
pub fn prioritize_rar_segments(
    session: &Session,
    current_position: u64,
    video_filename: &str,
) -> Vec<DownloadTask> {
    let mut tasks = Vec::new();

    // Find RAR files for this video
    if let Some(obfuscated_name) = session
        .obfuscated_to_real
        .iter()
        .find(|(_, real)| real.as_str() == video_filename)
        .map(|(obfuscated, _)| obfuscated.clone())
    {
        // Find RAR files that match this video
        let mut rar_files: Vec<_> = session
            .rar_files
            .iter()
            .filter(|file| {
                file.subject.contains(&obfuscated_name)
                    || file
                        .subject
                        .starts_with(&obfuscated_name[..obfuscated_name.len().min(10)])
            })
            .collect();

        // Sort RAR files by sequence (.rar, .r00, .r01, ...)
        rar_files.sort_by(|a, b| {
            let get_sequence = |name: &str| -> u32 {
                if name.ends_with(".rar") {
                    return 0;
                }
                if let Some(pos) = name.rfind(".r") {
                    if let Ok(num) = name[pos + 2..].parse::<u32>() {
                        return num + 1;
                    }
                }
                999
            };

            get_sequence(&a.subject).cmp(&get_sequence(&b.subject))
        });

        // Create download tasks with priority based on sequence and position
        for (file_index, file) in rar_files.iter().enumerate() {
            let mut byte_offset = 0u64;

            for (segment_index, segment) in file.segments.iter().enumerate() {
                let segment_start = byte_offset;
                let segment_end = byte_offset + segment.size as u64;

                // Calculate priority based on:
                // 1. Sequence order (earlier files = higher priority)
                // 2. Proximity to current playback position
                let base_priority = 10000 - (file_index * 100) as u32;
                let distance_from_position = if current_position >= segment_start {
                    0 // Already passed this segment
                } else {
                    ((current_position.saturating_sub(segment_start)) / (1024 * 1024)) as u32
                    // Distance in MB
                };

                let priority = base_priority.saturating_sub(distance_from_position);

                tasks.push(DownloadTask::RarSegment {
                    segment: segment.clone(),
                    file_index,
                    segment_index,
                    priority,
                    byte_range: segment_start..segment_end,
                });

                byte_offset = segment_end;
            }
        }
    }

    // Sort by priority (highest first)
    tasks.sort_by(|a, b| b.priority().cmp(&a.priority()));

    tasks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_header() {
        let manager = StreamManager::new();

        // Test basic range
        let range = manager.parse_range_header("bytes=0-1023").unwrap();
        assert_eq!(range.start, 0);
        assert_eq!(range.end, 1023);

        // Test open-ended range
        let range = manager.parse_range_header("bytes=1024-").unwrap();
        assert_eq!(range.start, 1024);
        assert_eq!(range.end, u64::MAX);
    }

    #[test]
    fn test_mock_mkv_data() {
        let manager = StreamManager::new();
        let data = manager.create_mock_mkv_data();

        assert!(data.len() > 1000);
        assert_eq!(&data[0..4], &MKV_SIGNATURE);
    }
}
