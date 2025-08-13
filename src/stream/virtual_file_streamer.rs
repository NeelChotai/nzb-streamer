use bytes::{Bytes, BytesMut};
use futures::Stream;
use std::{io::SeekFrom, path::PathBuf};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
};

use crate::stream::error::StreamError;

const VIDEO_CHUNK_SIZE: usize = 8 * 1024 * 1024; // 8MB chunks

#[derive(Debug, Clone)]
pub struct FileLayout {
    pub path: PathBuf,
    pub length: u64,
    pub virtual_start: u64, // Position in overall virtual stream
}

pub fn read_range(
    start: u64,
    length: u64,
    layout: Vec<FileLayout>,
) -> impl Stream<Item = Result<Bytes, StreamError>> {
    read_range_with_chunk_size(start, length, VIDEO_CHUNK_SIZE, layout)
}

pub fn read_range_with_chunk_size(
    start: u64,
    length: u64,
    chunk_size: usize,
    layout: Vec<FileLayout>,
) -> impl Stream<Item = Result<Bytes, StreamError>> + 'static {
    let end = start + length;

    async_stream::try_stream! {
        if layout.is_empty() {
            return;
        }

        let mut current_pos = start;
        let mut buffer = BytesMut::with_capacity(chunk_size);
        let mut current_file: Option<(PathBuf, File)> = None;

        for file_layout in layout.iter() {
            if current_pos >= end {
                break;
            }

            let file_end = file_layout.virtual_start + file_layout.length;

            // Skip files before our start position
            if file_end <= current_pos {
                continue;
            }

            // Stop if we hit a gap (file not ready yet)
            if file_layout.virtual_start > current_pos {
                break;
            }

            // Calculate read parameters
            let offset_in_file = current_pos.saturating_sub(file_layout.virtual_start);
            let physical_offset = offset_in_file;
            let bytes_to_read = (end.min(file_end) - current_pos) as usize;

            // Open or reuse file handle
            let file = match &mut current_file {
                Some((path, file)) if path == &file_layout.path => file,
                _ => {
                    let new_file = File::open(&file_layout.path).await?;
                    current_file = Some((file_layout.path.clone(), new_file));
                    &mut current_file.as_mut().unwrap().1
                }
            };

            // Seek to position
            file.seek(SeekFrom::Start(physical_offset)).await?;

            // Read in chunks
            let mut remaining = bytes_to_read;
            while remaining > 0 {
                let to_read = remaining.min(chunk_size);
                buffer.clear();
                buffer.resize(to_read, 0);

                let bytes_read = file.read(&mut buffer).await?;
                if bytes_read == 0 {
                    break;
                }

                buffer.truncate(bytes_read);
                yield buffer.clone().freeze();

                current_pos += bytes_read as u64;
                remaining -= bytes_read;
            }
        }
    }
}
