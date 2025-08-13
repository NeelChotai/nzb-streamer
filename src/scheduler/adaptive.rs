use crate::archive::par2::DownloadTask;
use crate::archive::rar::{RarExt, analyse_rar_buffer};
use crate::nntp::client::NntpClient;
use crate::nntp::config::NntpConfig;
use crate::nntp::yenc::{compute_hash16k, extract_filename};
use crate::scheduler::batch::{self, BatchGenerator};
use crate::scheduler::{error::SchedulerError, queue::FileQueue, writer::FileWriterPool};
use crate::stream::orchestrator::{BufferHealth, OrchestratorEvent};
use bytes::Bytes;
use futures::stream::{self, StreamExt};
use std::path::PathBuf;
use std::{path::Path, sync::Arc};
use tokio::sync::{mpsc, watch};

use tracing::{debug, error, info};
use uuid::Uuid;

pub struct AdaptiveScheduler {
    client: Arc<NntpClient>,
    max_workers: usize,
}

#[derive(Debug)]
pub struct FirstSegment {
    pub path: PathBuf,
    pub nzb: nzb_rs::File,
    pub hash16k: Bytes,
    pub bytes: Bytes,
}

impl AdaptiveScheduler {
    pub fn new(config: NntpConfig) -> Result<Self, SchedulerError> {
        let client = NntpClient::new(config.clone())?; // TODO: don't clone here

        Ok(Self {
            client: Arc::new(client),
            max_workers: *config.max_connections,
        })
    }

    pub async fn warm_pool(&self) {
        self.client.warm_pool().await
    }

    pub async fn download_first_segments(
        &self,
        files: &[nzb_rs::File],
        output_dir: &Path,
    ) -> Result<Vec<FirstSegment>, SchedulerError> {
        info!("Downloading first segments of {} files", files.len());

        stream::iter(files.iter().cloned())
            .map(|file| self.download_first_segment(file, output_dir))
            .buffer_unordered(self.max_workers.min(20))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect()
    }

    pub async fn download_first_segment(
        &self,
        file: nzb_rs::File,
        output_dir: &Path,
    ) -> Result<FirstSegment, SchedulerError> {
        let first_segment = file
            .segments
            .first()
            .ok_or_else(|| SchedulerError::EmptyFile(file.subject.clone()))?;

        let data = self.client.download(first_segment).await?;
        let hash16k = compute_hash16k(&data);
        let filename = extract_filename(&file.subject);
        let path = output_dir.join(&filename);

        tokio::fs::write(&path, &data).await?;

        Ok(FirstSegment {
            path,
            nzb: file,
            hash16k: hash16k.into(),
            bytes: data,
        })
    }

    pub async fn schedule_downloads(
        &self,
        tasks: Vec<DownloadTask>,
        session_id: Uuid,
        event_tx: mpsc::Sender<OrchestratorEvent>,
        health_rx: watch::Receiver<BufferHealth>,
    ) -> Result<(), SchedulerError> {
        info!("Starting adaptive download for session {}", session_id);

        let writers = FileWriterPool::new();
        let mut file_queues = Vec::with_capacity(tasks.len());

        for task in &tasks {
            writers.add_file(task.path(), task.start_segment()).await?;
            file_queues.push(FileQueue::new(
                task.path().clone(),
                task.nzb().clone(),
                task.start_segment(),
            )?);
        }

        let (tx, rx) = async_channel::bounded(self.max_workers * 2);

        let workers = (0..self.max_workers)
            .map(|_| {
                let rx = rx.clone();
                let client = self.client.clone();
                let writers = writers.clone();
                let event_tx = event_tx.clone();

                tokio::spawn(async move { spawn_worker(rx, client, writers, event_tx).await })
            })
            .collect::<Vec<_>>();

        let generator = BatchGenerator::new(file_queues, self.max_workers, health_rx);

        for batch in generator {
            debug!(
                "Batch: {} jobs, priority {:?}",
                batch.jobs.len(),
                batch.priority
            );

            for job in batch.jobs {
                if tx.send(job).await.is_err() {
                    info!("Work channel closed, stopping generator.");
                    break;
                }
            }
        }

        drop(tx);
        futures::future::join_all(workers).await;
        writers.close_all().await;

        info!("Download complete for session {}", session_id);
        Ok(())
    }
}

async fn spawn_worker(
    rx: async_channel::Receiver<batch::Job>,
    client: Arc<NntpClient>,
    writers: FileWriterPool,
    event_tx: mpsc::Sender<OrchestratorEvent>,
) {
    while let Ok(job) = rx.recv().await {
        if let Err(e) = process_job(&job, &client, &writers, &event_tx).await {
            error!("Worker failed to process job: {}", e);
        }
    }
}

async fn process_job(
    job: &batch::Job,
    client: &NntpClient,
    writers: &FileWriterPool,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
) -> Result<(), SchedulerError> {
    let data = client.download(job.segment()).await?;

    if job.index() == 0 {
        let is_first = RarExt::from_filename(job.path()).is_some_and(|ext| ext == RarExt::Main);
        let (data_offset, total_size) = analyse_rar_buffer(&data, is_first).await?;

        event_tx
            .send(OrchestratorEvent::new(job.path().clone(), total_size))
            .await?;

        let content = data.slice(data_offset as usize..);
        writers.write(job.path(), job.index(), content).await?;
    } else {
        writers.write(job.path(), job.index(), data).await?;
    }

    Ok(())
}
