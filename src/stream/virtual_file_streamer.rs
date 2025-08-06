use bytes::{Bytes, BytesMut};
use futures::Stream;
use std::{
    io::{self, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, ReadBuf},
    sync::RwLock,
};
use tracing::info;

use crate::stream::{archive::analyse_rar_volume, error::StreamError};

const VIDEO_CHUNK_SIZE: usize = 8 * 1024 * 1024; // 8MB chunks
const PREFETCH_SIZE: usize = 16 * 1024 * 1024; // 16MB prefetch buffer

#[derive(Debug, Clone)]
pub struct VolumeSegment {
    pub path: PathBuf,
    pub start: u64, // RAR header offset
    pub length: u64, // Content length
    pub virtual_start: u64, // Position in overall virtual stream
    pub is_complete: bool,
}

#[derive(Debug, Default)]
pub struct VirtualFileStreamer {
    segments: Arc<RwLock<Vec<VolumeSegment>>>,
}

impl VirtualFileStreamer {
    pub async fn new(sorted_files: &[PathBuf]) -> Result<Self, StreamError> {
        let mut segments = Vec::new();

        for path in sorted_files.iter() {
            segments.push(VolumeSegment {
                path: path.to_path_buf(),
                start: 0,
                length: 0,
                virtual_start: 0,
                is_complete: false
            });
        }

        Ok(Self {
            segments: Arc::new(RwLock::new(segments)),
        })
    }

    pub async fn get_available_bytes(&self) -> u64 {
self.segments
.read()
.await
.iter()
.take_while(|seg| seg.is_complete)
.last()
.map(|seg| seg.virtual_start + seg.length)
.unwrap_or(0)
 }

     pub async fn complete_count(&self) -> usize {
        self.segments
            .read()
            .await
            .iter()
            .filter(|s| s.is_complete)
            .count()
    }

    pub async fn mark_file_complete(&self, path: &Path) -> Result<(), StreamError> {
        let mut segments = self.segments.write().await;

        let (idx, segment) = segments
            .iter_mut()
            .enumerate()
            .find(|(_, s)| s.path == path)
            .ok_or_else(|| StreamError::FileNotFound(path.to_path_buf()))?;
        
        // Analyse RAR volume
        let is_first = idx == 0; // TODO: this isn't correct
        let (start, length) = analyse_rar_volume(path, is_first).await?;
        
        // Update segment with real data
        segment.start = start;
        segment.length = length;
        segment.is_complete = true;
        
        // Recalculate virtual offsets for all complete segments
        let mut offset = 0u64;
        
        for segment in segments.iter_mut() {
            segment.virtual_start = offset;
            if segment.is_complete {
                offset += segment.length;
            }
        }
        
        info!("File {} complete: {} bytes at offset {}", 
              path.display(), length, segment.virtual_start);
        
        Ok(())
    }

    pub async fn len(&self) -> usize {
        self.segments.read().await.len()
    }

    /// Get the last accessed segment index (for prioritisation)
    pub async fn get_last_accessed_index(&self) -> Option<usize> {
        // This could be tracked during read_range operations
        // For now, return None
        None
    }

        fn recalculate_virtual_offsets(&self, segments: &mut [VolumeSegment]) {
        let mut offset = 0u64;
        
        for segment in segments.iter_mut() {
            segment.virtual_start = offset;
            if segment.is_complete {
                offset += segment.length;
            }
        }
    }

    pub fn read_range(
        &self,
        start: u64,
        length: u64,
    ) -> impl Stream<Item = Result<Bytes, StreamError>> {
        self.read_range_with_chunk_size(start, length, VIDEO_CHUNK_SIZE)
    }

    pub fn read_range_with_chunk_size(
        &self,
        start: u64,
        length: u64,
        chunk_size: usize,
    ) -> impl Stream<Item = Result<Bytes, StreamError>> + 'static {
        let segments = Arc::clone(&self.segments);
        let end = start + length;

        async_stream::try_stream! {
            let snapshot = segments.read().await.clone();

            let mut max_available = 0u64;
            for segment in &snapshot {
                if segment.is_complete {
                    max_available = segment.virtual_start + segment.length;
                } else {
                    break;
                }
            }

            let actual_end = end.min(max_available);

            let mut buffer = BytesMut::with_capacity(chunk_size);
            let mut current_pos = start;
            let mut pending_data = BytesMut::new();

            for segment in &snapshot {
                let segment_end = segment.virtual_start + segment.length;

                if segment_end <= current_pos {
                    continue;
                }

                if segment.virtual_start >= end {
                    break;
                }

                if !segment.is_complete {
                    break;
                }

                let mut file = File::open(&segment.path).await?;
                let seg_offset = current_pos.saturating_sub(segment.virtual_start);
                file.seek(SeekFrom::Start(segment.start + seg_offset)).await?;

                let segment_read_end = segment_end.min(actual_end);
                let mut segment_remaining = segment_read_end - current_pos;

                while segment_remaining > 0 && current_pos < actual_end {
                    // Read into temporary buffer
                    let to_read = chunk_size.min(segment_remaining as usize);
                    buffer.clear();
                    buffer.resize(to_read, 0);

                    let bytes_read = file.read(&mut buffer).await?;
                    if bytes_read == 0 {
                        break;
                    }

                    buffer.truncate(bytes_read);
                    pending_data.extend_from_slice(&buffer);

                    // Yield complete chunks
                    while pending_data.len() >= chunk_size {
                        let chunk = pending_data.split_to(chunk_size);
                        yield chunk.freeze();
                    }

                    current_pos += bytes_read as u64;
                    segment_remaining -= bytes_read as u64;
                }
            }

            // Yield any remaining data
            if !pending_data.is_empty() {
                yield pending_data.freeze();
            }
        }
    }

    pub async fn read_at(&self, offset: u64, length: u64) -> Result<Bytes, StreamError> {
        // Find the segment
        let segment = self.segments.read().await;

        let chunk = segment
            .iter()
            .find(|s| s.virtual_start <= offset && offset < s.virtual_start + s.length)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid offset"))?;

        let mut file = File::open(&chunk.path).await?;
        let seg_offset = offset - chunk.virtual_start;
        let physical_pos = chunk.start + seg_offset;

        file.seek(SeekFrom::Start(physical_pos)).await?;

        let read_length = length.min(chunk.virtual_start + chunk.length - offset);
        let mut buffer = vec![0u8; read_length as usize];
        file.read_exact(&mut buffer).await?;

        Ok(Bytes::from(buffer))
    }
}
