// TODO: needs to be stubbed out for the actual orchastrator class

use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::stream::archive::analyse_rar_volume;
use crate::stream::error::StreamError;
use crate::stream::virtual_file_streamer::{FileLayout, VirtualFileStreamer};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BufferHealth {
    Critical,
    Poor,
    Good,
    Excellent,
}

#[derive(Debug, Clone)]
struct FileState {
    index: usize,
    path: PathBuf,
    is_complete: bool,
    rar_start: u64,
    rar_length: u64,
}

#[derive(Debug, Default)]
pub struct StreamOrchestrator {
    files: Arc<DashMap<PathBuf, FileState>>,
    layout: Arc<RwLock<Vec<FileLayout>>>,
    streamer: Arc<VirtualFileStreamer>,
    total_files: usize,
    complete_files: AtomicUsize,
    available_bytes: AtomicU64,
    playback_position: AtomicUsize,
}

impl StreamOrchestrator {
    pub async fn new(sorted_paths: &[PathBuf]) -> Result<Self, StreamError> {
        let total_files = sorted_paths.len();

        // Initialize file states
        let files: DashMap<PathBuf, FileState> = sorted_paths
            .iter()
            .enumerate()
            .map(|(idx, path)| {
                (
                    path.clone(),
                    FileState {
                        index: idx,
                        path: path.clone(),
                        is_complete: false, // Nothing is complete initially
                        rar_start: 0,
                        rar_length: 0,
                    },
                )
            })
            .collect();

        let layout = vec![];
        let streamer = Arc::new(VirtualFileStreamer::new());

        Ok(Self {
            files: Arc::new(files),
            layout: Arc::new(RwLock::new(layout)),
            streamer,
            total_files,
            complete_files: AtomicUsize::new(0),
            available_bytes: AtomicU64::new(0),
            playback_position: AtomicUsize::new(0),
        })
    }

    pub async fn mark_file_complete(&self, path: &PathBuf) -> Result<(), StreamError> {
        let file_state = self
            .files
            .get(path)
            .ok_or_else(|| StreamError::FileNotFound(path.to_path_buf()))?;

        let index = file_state.index;
        let is_first = index == 0;

        // Analyse RAR volume
        let (rar_start, rar_length) = analyse_rar_volume(path, is_first).await?;

        // Update file state
        drop(file_state); // Release read lock
        self.files.alter(path, |_, mut state| {
            state.is_complete = true;
            state.rar_start = rar_start;
            state.rar_length = rar_length;
            state
        });

        self.complete_files.fetch_add(1, Ordering::Relaxed);

        // Rebuild layout if needed
        self.rebuild_layout_if_continuous().await;

        info!(
            "File {} complete: {} bytes (progress: {}/{})",
            path.display(),
            rar_length,
            self.complete_files.load(Ordering::Relaxed),
            self.total_files
        );

        Ok(())
    }

    async fn rebuild_layout_if_continuous(&self) {
        let mut states: Vec<FileState> = self
            .files
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        states.sort_by_key(|s| s.index);

        let continuous_files: Vec<FileState> =
            states.into_iter().take_while(|s| s.is_complete).collect();

        if continuous_files.is_empty() {
            return;
        }

        // Build new layout
        let mut virtual_offset = 0u64;
        let new_layout: Vec<FileLayout> = continuous_files
            .iter()
            .map(|state| {
                let layout = FileLayout {
                    path: state.path.clone(),
                    start: state.rar_start,
                    length: state.rar_length,
                    virtual_start: virtual_offset,
                };
                virtual_offset += state.rar_length;
                layout
            })
            .collect();

        // Update available bytes
        self.available_bytes
            .store(virtual_offset, Ordering::Relaxed);

        // Atomically update layout
        *self.layout.write().await = new_layout;

        debug!("Layout updated: {} bytes available", virtual_offset);
    }

    pub fn update_playback_position(&self, byte_position: u64) {
        // TODO: rough estimate: assume ~700MB per file, we should be smarter about this
        let estimated_file = (byte_position / (700 * 1024 * 1024)) as usize;
        self.playback_position
            .store(estimated_file, Ordering::Relaxed);
        debug!("Playback approximately at file {}", estimated_file);
    }

    pub fn get_buffer_health(&self) -> BufferHealth {
        // TODO: just take segments.last?
        let playback = self.playback_position.load(Ordering::Relaxed);
        let complete = self.complete_files.load(Ordering::Relaxed);
        let buffer = complete.saturating_sub(playback);

        match buffer {
            0..=2 => BufferHealth::Critical,
            3..=5 => BufferHealth::Poor,
            6..=10 => BufferHealth::Good,
            _ => BufferHealth::Excellent,
        }
    }

    pub fn get_priority_index(&self) -> usize {
        self.playback_position.load(Ordering::Relaxed) + 3 // TODO: hardcoded 3 file buffer for now
    }

    pub fn get_available_bytes(&self) -> u64 {
        self.available_bytes.load(Ordering::Relaxed)
    }

    pub fn complete_count(&self) -> usize {
        self.complete_files.load(Ordering::Relaxed)
    }

    pub async fn get_stream(
        &self,
        start: u64,
        length: u64,
        chunk_size: usize,
    ) -> impl futures::Stream<Item = Result<bytes::Bytes, StreamError>> + 'static {
        // Update playback position
        self.update_playback_position(start);
        let layout = self.layout.read().await.clone();

        // Delegate to streamer
        self.streamer
            .read_range_with_chunk_size(start, length, chunk_size, layout)
    }
}
