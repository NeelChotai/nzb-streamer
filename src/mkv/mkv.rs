use std::{
    io::{self, SeekFrom}, path::{Path, PathBuf}, pin::Pin, sync::Arc
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, ReadBuf},
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio_util::io::ReaderStream;
use crate::par2::matcher::FileMatch;

const RAR_SIGNATURE: [u8; 7] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const MKV_SIGNATURE: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

#[derive(Debug, Clone)]
struct VolumeSegment {
    path: PathBuf,
    start: u64,
    length: u64,
}

pub struct MkvStreamer {
    segments: Vec<VolumeSegment>,
    total_size: u64,
}

impl MkvStreamer {
    pub async fn new(rar_directory: &Path, file_matches: &[FileMatch]) -> io::Result<Self> {
        let mut segments = Vec::new();
        let mut total_size = 0;
        
        // Create a custom sorter for RAR volumes
        let mut sorted_files = file_matches.to_vec();
        sorted_files.sort_by(|a, b| {
            // Extract base names for comparison
            let a_name = a.expected_name.as_deref().unwrap_or("");
            let b_name = b.expected_name.as_deref().unwrap_or("");
            
            // Handle .rar as first volume
            if a_name.ends_with(".rar") && !b_name.ends_with(".rar") {
                return std::cmp::Ordering::Less;
            }
            if !a_name.ends_with(".rar") && b_name.ends_with(".rar") {
                return std::cmp::Ordering::Greater;
            }
            
            // Handle .rXX volumes
            let a_ext = a_name.rsplit('.').next().unwrap_or("");
            let b_ext = b_name.rsplit('.').next().unwrap_or("");
            
            // Special case: .r00 comes after .rar but before .r01
            if a_ext.starts_with('r') && b_ext.starts_with('r') {
                let a_num = a_ext[1..].parse::<u32>().unwrap_or(u32::MAX);
                let b_num = b_ext[1..].parse::<u32>().unwrap_or(u32::MAX);
                return a_num.cmp(&b_num);
            }
            
            // Fallback: alphabetical order
            a_name.cmp(b_name)
        });
        
        // Debug print sorted order
        println!("Sorted RAR volumes:");
        for file in &sorted_files {
            println!("- {}", file.expected_name.as_deref().unwrap_or("unknown"));
        }
        
        for (idx, file_match) in sorted_files.iter().enumerate() {
            let is_first = idx == 0;
            let (start, length) = analyze_rar_volume(
                Path::new(&file_match.path),
                is_first
            ).await?;
            
            segments.push(VolumeSegment {
                path: PathBuf::from(&file_match.path),
                start,
                length,
            });
            
            total_size += length;
        }
        
        Ok(Self { segments, total_size })
    }
    
    pub fn total_size(&self) -> u64 {
        self.total_size
    }
    
    pub fn stream(self) -> impl Stream<Item = io::Result<Bytes>> + 'static {
        async_stream::try_stream! {
            for segment in self.segments {
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
}

/// Async version of your extraction logic
async fn analyze_rar_volume(path: &Path, is_first: bool) -> io::Result<(u64, u64)> {
    let mut file = File::open(path).await?;
    let file_size = file.metadata().await?.len();

    // Find RAR signature
    let mut sig_buf = vec![0u8; 1024.min(file_size as usize)];
    file.read_exact(&mut sig_buf).await?;

    let rar_offset = sig_buf
        .windows(7)
        .position(|w| w == RAR_SIGNATURE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "No RAR signature"))? as u64;

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
            0x73 => { // MAIN_HEAD
                file.seek(SeekFrom::Current(header_size as i64 - 7)).await?;
            }
            0x74 => { // FILE_HEAD
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

                    if let Some(mkv_offset) = search_buf.windows(4).position(|w| w == MKV_SIGNATURE) {
                        data_offset = current_pos + mkv_offset as u64;
                        data_length = pack_size - mkv_offset as u64;
                    }
                }
                break;
            }
            0x7B => { // ENDARC_HEAD
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