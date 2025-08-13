// TODO: needs to be stubbed out for the actual orchastrator class

use arc_swap::ArcSwap;
use bytes::Bytes;
use derive_more::Constructor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info};

use crate::stream::error::StreamError;
use crate::stream::virtual_file_streamer::{FileLayout, read_range_with_chunk_size};

#[derive(Debug, Clone, Constructor)]
pub struct OrchestratorEvent {
    path: PathBuf,
    total_size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BufferHealth {
    Critical,
    Poor,
    Good,
    Excellent,
}

#[derive(Debug)]
pub struct StreamOrchestrator {
    files: Arc<Vec<(PathBuf, parking_lot::RwLock<u64>)>>,
    layout: Arc<ArcSwap<Vec<FileLayout>>>,
    playback_position: Arc<AtomicU64>,
    health_tx: watch::Sender<BufferHealth>,
}

impl StreamOrchestrator {
    pub fn new(
        sorted_paths: &[PathBuf],
        mut event_rx: mpsc::Receiver<OrchestratorEvent>,
        health_tx: watch::Sender<BufferHealth>,
    ) -> Arc<Self> {
        let files: Vec<_> = sorted_paths
            .iter()
            .map(|path| (path.clone(), parking_lot::RwLock::new(0)))
            .collect();

        let orchestrator = Arc::new(Self {
            files: Arc::new(files),
            layout: Arc::new(ArcSwap::new(Arc::new(Vec::new()))),
            playback_position: Arc::new(AtomicU64::new(0)),
            health_tx,
        });

        let background = Arc::clone(&orchestrator);
        tokio::spawn(async move {
            info!("Orchestrator metadata listener started");
            while let Some(event) = event_rx.recv().await {
                background.handle_worker_event(event).await;
                background.update_layout();
            }
            info!("Orchestrator metadata listener stopped.");
        });

        orchestrator
    }

    async fn handle_worker_event(&self, event: OrchestratorEvent) {
        if let Some((_, size_lock)) = self.files.iter().find(|(p, _)| p == &event.path) {
            info!(
                "Metadata received for {}: total size = {}",
                event.path.display(),
                event.total_size
            );
            let mut size = size_lock.write();
            if *size == 0 {
                *size = event.total_size;

                self.update_layout();
            }
        } else {
            error!(
                "Received metadata for unknown file: {}",
                event.path.display()
            );
        }
    }

    fn update_layout(&self) {
        let mut layouts = Vec::new();
        let mut virtual_offset = 0u64;
        for (path, size_lock) in self.files.iter() {
            let total_size = *size_lock.read();
            if total_size > 0 {
                layouts.push(FileLayout {
                    path: path.clone(),
                    length: total_size,
                    virtual_start: virtual_offset,
                });
                virtual_offset += total_size;
            }
        }

        if !layouts.is_empty() {
            debug!(
                "Layout updated: {} bytes across {} files",
                virtual_offset,
                layouts.len()
            );
            self.layout.store(Arc::new(layouts));
        }
    }

    fn health_check(&self) {
        let playback_pos = self.playback_position.load(Ordering::SeqCst);

        let buffer_bytes = self
            .files
            .iter()
            .try_fold(0u64, |acc, (path, size_lock)| {
                let total_size = *size_lock.read();

                match total_size {
                    0 => Err(acc), // unknown file size -> stop
                    _ => match std::fs::metadata(path) {
                        Err(_) => Err(acc), // file missing -> stop
                        Ok(metadata) => {
                            let disk_size = metadata.len();
                            if disk_size >= total_size {
                                Ok(acc + total_size) // fully downloaded -> continue
                            } else {
                                Err(acc + disk_size) // partial -> stop
                            }
                        }
                    },
                }
            })
            .unwrap_or_else(|acc| acc);

        if buffer_bytes == 0 {
            self.health_tx.send(BufferHealth::Critical).unwrap();
            return;
        }

        let buffer_ahead_bytes = buffer_bytes.saturating_sub(playback_pos);
        let buffer_mb = buffer_ahead_bytes / (1024 * 1024);

        let health = match buffer_mb {
            0..=100 => BufferHealth::Critical,
            101..=500 => BufferHealth::Poor,
            501..=1024 => BufferHealth::Good,
            _ => BufferHealth::Excellent,
        };

        debug!(
            "Health check: {}MB buffer ahead. Status: {:?}",
            buffer_mb, health
        );
        self.health_tx.send(health).unwrap();
    }

    pub fn update_playback_position(&self, byte_position: u64) {
        self.playback_position
            .store(byte_position, Ordering::SeqCst);
    }

    pub fn get_available_bytes(&self) -> u64 {
        self.layout
            .load()
            .last()
            .map(|f| f.virtual_start + f.length)
            .unwrap_or(0)
    }

    pub async fn get_stream(
        self: &Arc<Self>,
        start: u64,
        length: u64,
        chunk_size: usize,
    ) -> impl futures::Stream<Item = Result<Bytes, StreamError>> + 'static {
        self.update_playback_position(start);
        self.health_check();
        self.update_layout();

        let layout = self.layout.load_full();
        read_range_with_chunk_size(start, length, chunk_size, layout.to_vec())
    }
}
