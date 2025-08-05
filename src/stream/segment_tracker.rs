use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BufferHealth {
    Critical,  // < 10 segments
    Poor,      // 10-50 segments
    Good,      // 50-200 segments
    Excellent, // > 200 segments
}

#[derive(Debug, Default)]
pub struct SegmentTracker {
    /// Map of global segment index to (file_path, segment_in_file)
    segments: Arc<DashMap<usize, (PathBuf, usize)>>,

    /// Current segment being played
    playback_segment: AtomicUsize,

    /// Highest complete segment available for streaming
    available_segment: AtomicUsize,

    /// Total segments registered
    total_segments: AtomicUsize,
}

impl SegmentTracker {
    pub fn new() -> Self {
        Self {
            segments: Arc::new(DashMap::new()),
            playback_segment: AtomicUsize::new(0),
            available_segment: AtomicUsize::new(0),
            total_segments: AtomicUsize::new(0),
        }
    }

    /// Register a segment in the global index
    pub fn register_segment(&self, global_index: usize, path: PathBuf, segment_in_file: usize) {
        self.segments.insert(global_index, (path, segment_in_file));
        self.total_segments
            .fetch_max(global_index + 1, Ordering::Relaxed);
    }

    /// Update playback position from Range header
    pub fn update_playback_position(&self, byte_position: u64, bytes_per_segment: u64) {
        // Simple conversion - could be made more sophisticated
        let segment_index = (byte_position / bytes_per_segment) as usize;
        self.playback_segment
            .store(segment_index, Ordering::Relaxed);
        debug!("Playback at segment {}", segment_index);
    }

    /// Mark a segment as complete and ready for streaming
    pub fn mark_segment_complete(&self, global_index: usize) {
        // Update available_segment if this extends the continuous range
        let current_available = self.available_segment.load(Ordering::Relaxed);
        if global_index == current_available {
            // This segment extends our available range
            // Find how far we can extend
            let mut new_available = global_index + 1;
            while self.segments.contains_key(&new_available) {
                new_available += 1;
            }
            self.available_segment
                .store(new_available, Ordering::Relaxed);
            debug!("Available segments extended to {}", new_available);
        }
    }

    /// Get current buffer health
    pub fn get_buffer_health(&self) -> BufferHealth {
        let playback = self.playback_segment.load(Ordering::Relaxed);
        let available = self.available_segment.load(Ordering::Relaxed);
        let buffer = available.saturating_sub(playback);

        match buffer {
            0..=10 => BufferHealth::Critical,
            11..=50 => BufferHealth::Poor,
            51..=200 => BufferHealth::Good,
            _ => BufferHealth::Excellent,
        }
    }

    /// Get segments that should be prioritized for download
    pub fn get_priority_segments(&self, count: usize) -> Vec<usize> {
        let available = self.available_segment.load(Ordering::Relaxed);
        (available..available + count).collect()
    }

    /// Get progress statistics
    pub fn get_stats(&self) -> SegmentStats {
        let playback = self.playback_segment.load(Ordering::Relaxed);
        let available = self.available_segment.load(Ordering::Relaxed);
        let total = self.total_segments.load(Ordering::Relaxed);

        SegmentStats {
            playback_segment: playback,
            available_segment: available,
            total_segments: total,
            buffer_segments: available.saturating_sub(playback),
            completion_percent: if total > 0 {
                (available as f32 / total as f32) * 100.0
            } else {
                0.0
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct SegmentStats {
    pub playback_segment: usize,
    pub available_segment: usize,
    pub total_segments: usize,
    pub buffer_segments: usize,
    pub completion_percent: f32,
}
