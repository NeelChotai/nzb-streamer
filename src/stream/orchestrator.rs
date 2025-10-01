use bytes::Bytes;
use futures::Stream;
use memmap2::MmapMut;
use parking_lot::RwLock;
use rustix::fs::{SeekFrom, seek};
use std::fs::File;
use std::os::fd::AsFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::watch;

use crate::archive::par2::DownloadTask;
use crate::stream::error::StreamError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BufferHealth {
    Critical,
    Poor,
    Good,
    Excellent,
}

impl BufferHealth {
    /// how many parts to process concurrently
    pub fn concurrent_jobs(&self, total_jobs: usize) -> usize {
        match self {
            Self::Critical => 1,
            Self::Poor => 2,
            Self::Good => 4,
            Self::Excellent => total_jobs,
        }
    }

    pub fn segment_parallelism(&self, max_workers: usize, concurrent_parts: usize) -> usize {
        let workers_per_job = max_workers / concurrent_parts.max(1);

        match self {
            Self::Critical => max_workers,
            Self::Poor => workers_per_job,
            Self::Good => workers_per_job,
            Self::Excellent => workers_per_job,
        }
    }
}

#[derive(Debug)]
pub struct StreamOrchestrator {
    pub mmap: Arc<RwLock<MmapMut>>,
    file: Arc<File>,
    total_size: u64,
    playback_position: Arc<AtomicU64>,
    health_tx: watch::Sender<BufferHealth>,
}

impl StreamOrchestrator {
    pub fn new(
        tasks: Vec<DownloadTask>,
        session_dir: &Path,
        health_tx: watch::Sender<BufferHealth>,
    ) -> Arc<Self> {
        // let files: Vec<_> = tasks
        //     .iter()
        //     .map(|segment| (segment.path().clone(), parking_lot::RwLock::new(0)))
        //     .collect();
        let total_size: u64 = tasks.iter().map(|f| f.length()).sum();

        let path = session_dir.join("test.mkv");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        file.set_len(total_size).unwrap();
        let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };

        let mut offset = 0;
        for task in tasks {
            println!("Writing {} bytes at offset {}", task.bytes().len(), offset);

            let end = offset + task.bytes().len();

            // Write the task's data to the correct offset in the memory map
            mmap[offset..end].copy_from_slice(task.bytes());
            offset += *task.length() as usize;
        }

        mmap.flush().unwrap();

        Arc::new(Self {
            mmap: Arc::new(RwLock::new(mmap)),
            file: Arc::new(file),
            total_size,
            playback_position: AtomicU64::new(0).into(),
            health_tx,
        })
    }

    fn update_health(&self) {
        let playback_pos = self.playback_position.load(Ordering::SeqCst);
        let available = self.get_available_bytes();
        let buffer_ahead = available.saturating_sub(playback_pos);

        // Percentage of remaining video buffered
        let remaining = self.total_size.saturating_sub(playback_pos);
        let buffer_percentage = if remaining > 0 {
            (buffer_ahead * 100) / remaining
        } else {
            100
        };

        let health = match buffer_percentage {
            0..=5 => BufferHealth::Critical,
            6..=15 => BufferHealth::Poor,
            16..=35 => BufferHealth::Good,
            _ => BufferHealth::Excellent,
        };
    }

    pub fn update_playback_position(&self, position: u64) {
        self.playback_position.store(position, Ordering::SeqCst);
    }

    /// Find the number of continuous bytes available from position 0
    pub fn get_available_bytes(&self) -> u64 {
        let fd = self.file.as_fd();

        // Start from beginning and find first hole
        match seek(fd, SeekFrom::Data(0)) {
            Ok(data_start) => {
                if data_start > 0 {
                    return 0;
                }

                match seek(fd, SeekFrom::Hole(0)) {
                    Ok(hole_start) => hole_start,
                    Err(_) => self.total_size,
                }
            }
            Err(_) => 0,
        }
    }

    pub fn is_range_available(&self, start: u64, length: u64) -> bool {
        let fd = self.file.as_fd();
        let end = start + length;
        let pos = start;

        while pos < end {
            match seek(fd, SeekFrom::Data(pos)) {
                Ok(data_pos) => {
                    if data_pos > pos {
                        return false;
                    }
                    match seek(fd, SeekFrom::Hole(pos)) {
                        Ok(hole_pos) => {
                            if hole_pos < end {
                                return false;
                            }
                            return true;
                        }
                        Err(_) => return true,
                    }
                }
                Err(_) => return false,
            }
        }
        true
    }

    pub async fn get_stream(
        self: &Arc<Self>,
        start: u64,
        length: u64,
        chunk_size: usize,
    ) -> impl Stream<Item = Result<Bytes, StreamError>> + 'static {
        self.update_playback_position(start);
        let mmap = Arc::clone(&self.mmap);
        let end = start + length;

        async_stream::try_stream! {
            let mut pos = start;

            while pos < end {
                let chunk_end = (pos + chunk_size as u64).min(end);
                let chunk_len = (chunk_end - pos) as usize;

                // wait for data to be available
                // TODO: check if range is available and wait/retry if not

                let chunk = {
                    let start_idx = pos as usize;
                    let end_idx = start_idx + chunk_len;
                    Bytes::copy_from_slice(&mmap.read()[start_idx..end_idx])
                };

                yield chunk;
                pos += chunk_len as u64;
            }
        }
    }
}
