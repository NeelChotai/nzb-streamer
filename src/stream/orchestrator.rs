// TODO: needs to be stubbed out for the actual orchastrator class

use dashmap::DashMap;
use uuid::Uuid;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::{debug, info};

use crate::stream::error::StreamError;
use crate::stream::VirtualFileStreamer;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BufferHealth {
    Critical,
    Poor,
    Good,
    Excellent,
}

#[derive(Debug, Default)]
pub struct StreamOrchestrator {
    streamer: Arc<VirtualFileStreamer>,
    playback_position: AtomicUsize,
}

impl StreamOrchestrator {
    pub async fn new(sorted_files: &[PathBuf]) -> Result<Self, StreamError> {
        let streamer = Arc::new(VirtualFileStreamer::new(&sorted_files).await?);
        // TODO: maybe this should own VirtualFile
        // two methods we need to expose: stream, get_available_bytes
        // also expose a channel, allowing file writer events to be 
        // sent, which triggers analyse_rar_file (or it's been triggered
        // already and data has been sent)

        Ok(Self {
            streamer,
            playback_position: AtomicUsize::new(0),
        })
    }

    pub async fn mark_file_complete(&self, path: &PathBuf) -> Result<(), StreamError> {
        self.streamer.mark_file_complete(path).await?;

        let complete = self.streamer.complete_count().await;
        let total = self.streamer.len().await;

        info!("Progress: {}/{} files complete", complete, total);

        Ok(())
    }

    pub fn update_playback_position(&self, byte_position: u64) {
        // TODO: rough estimate: assume ~700MB per file, we should be smarter about this
        let estimated_file = (byte_position / (700 * 1024 * 1024)) as usize;
        self.playback_position.store(estimated_file, Ordering::Relaxed);
        debug!("Playback approximately at file {}", estimated_file);
    }


    pub async fn get_buffer_health(&self) -> BufferHealth {
        // TODO: just take segments.last?
        let playback = self.playback_position.load(Ordering::Relaxed);
        let complete = self.streamer.complete_count().await;
        let buffer = complete.saturating_sub(playback);

        match buffer {
            0..=2 => BufferHealth::Critical,
            3..=5 => BufferHealth::Poor,
            6..=10 => BufferHealth::Good,
            _ => BufferHealth::Excellent,
        }
    }

    pub fn get_priority_index(&self) -> usize {
        self.playback_position.load(Ordering::Relaxed) + 3
    }

    pub fn get_stream(
        &self,
        start: u64,
        length: u64,
        chunk_size: usize,
    ) -> impl futures::Stream<Item = Result<bytes::Bytes, StreamError>> {
        // Update playback position
        self.update_playback_position(start);
        
        // Delegate to streamer
        self.streamer.read_range_with_chunk_size(start, length, chunk_size)
    }

}
