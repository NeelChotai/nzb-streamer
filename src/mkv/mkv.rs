use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt};
use std::{
    io::{self, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, ReadBuf},
    sync::RwLock,
};
use tracing::{debug, info};

const RAR_SIGNATURE: [u8; 7] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const MKV_SIGNATURE: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

const VIDEO_CHUNK_SIZE: usize = 8 * 1024 * 1024; // 8MB chunks
const PREFETCH_SIZE: usize = 16 * 1024 * 1024; // 16MB prefetch buffer

#[derive(Debug, Clone)]
pub struct VolumeSegment {
    pub path: PathBuf,
    pub start: u64,
    pub length: u64,
    pub virtual_start: u64,
    pub is_complete: bool,
}

pub struct MkvStreamer {
    segments: Arc<RwLock<Vec<VolumeSegment>>>,
}

impl MkvStreamer {
    pub async fn new(sorted_files: &[PathBuf]) -> io::Result<Self> {
        let mut segments = Vec::new();

        for path in sorted_files.iter() {
            segments.push(VolumeSegment {
                path: path.to_path_buf(),
                start: 0,
                length: 0,
                virtual_start: 0u64,
                is_complete: false,
            });
        }

        Ok(Self {
            segments: Arc::new(RwLock::new(segments)),
        })
    }

    pub async fn get_available_bytes(&self) -> u64 {
        let segments = self.segments.read().await;
        let mut available = 0u64;

        for segment in segments.iter() {
            if segment.is_complete {
                available = segment.virtual_start + segment.length;
            } else {
                // Stop at first incomplete segment
                break;
            }
        }

        available
    }

    pub async fn mark_segment_downloaded(&self, path: &Path) -> io::Result<()> {
        let mut segments = self.segments.write().await;
        let segment_idx = segments
            .iter()
            .position(|s| s.path == path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Segment not found"))?;
        let is_first = segment_idx == 0;

        let (start, length) = analyse_rar_volume(path, is_first).await?;

        let segment = &mut segments[segment_idx];
        segment.start = start;
        segment.length = length;
        segment.is_complete = true;

        let mut next_virtual_start = 0;
        let mut available_bytes = 0;
        let mut gap_encountered = false;

        for seg in segments.iter_mut() {
            seg.virtual_start = next_virtual_start;

            if !gap_encountered && seg.is_complete {
                next_virtual_start += seg.length;
                available_bytes = next_virtual_start; // Track contiguous bytes
            } else if !seg.is_complete {
                // Found first gap - subsequent segments aren't contiguous
                gap_encountered = true;
            }
        }

        info!(
            "Marked {} as complete. Available bytes: {}",
            path.display(),
            available_bytes
        );

        Ok(())
    }

    pub fn stream(self) -> impl Stream<Item = io::Result<Bytes>> + 'static {
        async_stream::try_stream! {
            let streamer = self.segments.read().await.clone();

            for segment in streamer {
                let mut file = File::open(&segment.path).await?;
                file.seek(std::io::SeekFrom::Start(segment.start)).await?;

                let mut remaining = segment.length;
                let mut buffer = vec![0; 64 * 1024]; // 64KB buffer

                while remaining > 0 {
                    let read_size = buffer.len().min(remaining as usize);
                    let buffer_slice = &mut buffer[..read_size];

                    let mut read_buf = ReadBuf::new(buffer_slice);
                    file.read_buf(&mut read_buf).await?;

                    let filled = read_buf.filled();
                    if filled.is_empty() {
                        break;
                    }

                    yield Bytes::copy_from_slice(filled);
                    remaining -= filled.len() as u64;
                }
            }
        }
    }

    pub fn read_range(
        &self,
        start: u64,
        length: u64,
    ) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
        let segments = Arc::clone(&self.segments);
        let end = start + length;

        async_stream::try_stream! {
            let snapshot = segments.read().await.clone();

            let mut buffer = BytesMut::with_capacity(VIDEO_CHUNK_SIZE + PREFETCH_SIZE);
            let mut current_pos = start;
            let mut current_file: Option<(PathBuf, File)> = None;

             while current_pos < end {
                // Find the segment containing current_pos
                let segment = match snapshot
                    .iter()
                    .find(|s| s.virtual_start <= current_pos && current_pos < s.virtual_start + s.length)
                {
                    Some(s) => s,
                    None => break,
                };

                // Reuse file handle if it's the same segment
                let file = match &mut current_file {
                    Some((path, file)) if path == &segment.path => file,
                    _ => {
                        let new_file = File::open(&segment.path).await?;
                        current_file = Some((segment.path.clone(), new_file));
                        &mut current_file.as_mut().unwrap().1
                    }
                };

                // Calculate read position
                let seg_offset = current_pos - segment.virtual_start;
                let physical_pos = segment.start + seg_offset;
                file.seek(SeekFrom::Start(physical_pos)).await?;

                // Calculate how much to read
                let segment_end = segment.virtual_start + segment.length;
                let read_end = end.min(segment_end);
                let total_to_read = (read_end - current_pos) as usize;

                // Read a large chunk (up to VIDEO_CHUNK_SIZE)
                let chunk_size = total_to_read.min(VIDEO_CHUNK_SIZE);
                buffer.clear();
                buffer.resize(chunk_size, 0);

                let mut bytes_read = 0;
                while bytes_read < chunk_size {
                    match file.read(&mut buffer[bytes_read..]).await? {
                        0 => break,
                        n => bytes_read += n,
                    }
                }

                if bytes_read == 0 {
                    break;
                }

                // Yield the entire chunk at once
                buffer.truncate(bytes_read);
                yield buffer.clone().freeze();

                current_pos += bytes_read as u64;

                // Optional: Prefetch next chunk in background
                if current_pos < end && bytes_read == chunk_size {
                    // The next iteration will read the prefetched data
                    // This happens automatically due to OS file caching
                }
            }
        }
    }

    pub fn read_range_with_chunk_size(
        &self,
        start: u64,
        length: u64,
        chunk_size: usize,
    ) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
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

    // Optimized method for seeking within the video
    pub async fn read_at(&self, offset: u64, length: u64) -> io::Result<Bytes> {
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

async fn analyse_rar_volume(path: &Path, is_first: bool) -> io::Result<(u64, u64)> {
    let mut file = File::open(path).await?;
    let file_size = file.metadata().await?.len();

    // Find RAR signature
    let mut sig_buf = vec![0u8; 1024.min(file_size as usize)];
    file.read_exact(&mut sig_buf).await?;

    let rar_offset = sig_buf
        .windows(7)
        .position(|w| w == RAR_SIGNATURE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "No RAR signature"))?
        as u64;

    file.seek(SeekFrom::Start(rar_offset + 7)).await?;

    let mut data_offset = 0;
    let mut data_length = 0;

    // Parse volume headers
    loop {
        // Read header CRC
        let _ = match read_u16_le(&mut file).await {
            Ok(v) => v,
            Err(_) => break, // End of file
        };

        // Read header type
        let header_type = {
            let mut buf = [0u8; 1];
            file.read_exact(&mut buf).await?;
            buf[0]
        };

        let _ = read_u16_le(&mut file).await?; // flags
        let header_size = read_u16_le(&mut file).await?;

        match header_type {
            0x73 => {
                // MAIN_HEAD
                file.seek(SeekFrom::Current(header_size as i64 - 7)).await?;
            }
            0x74 => {
                // FILE_HEAD
                let pack_size = read_u32_le(&mut file).await? as u64;
                let _ = read_u32_le(&mut file).await?; // unpacked size

                // Skip remaining header
                let skip_size = header_size as i64 - 7 - 8;
                file.seek(SeekFrom::Current(skip_size)).await?;

                data_offset = file.stream_position().await?;
                data_length = pack_size;

                // For first volume, find MKV signature
                if is_first {
                    let mut search_buf = vec![0u8; 1024.min(pack_size as usize)];
                    let current_pos = data_offset;
                    file.read_exact(&mut search_buf).await?;

                    if let Some(mkv_offset) = search_buf.windows(4).position(|w| w == MKV_SIGNATURE)
                    {
                        data_offset = current_pos + mkv_offset as u64;
                        data_length = pack_size - mkv_offset as u64;
                    }
                }
                break;
            }
            0x7B => {
                // ENDARC_HEAD
                break;
            }
            _ => {
                file.seek(SeekFrom::Current(header_size as i64 - 7)).await?;
            }
        }
    }

    Ok((data_offset, data_length))
}

async fn read_u16_le(file: &mut File) -> io::Result<u16> {
    let mut buf = [0u8; 2];
    file.read_exact(&mut buf).await?;
    Ok(u16::from_le_bytes(buf))
}

async fn read_u32_le(file: &mut File) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).await?;
    Ok(u32::from_le_bytes(buf))
}
