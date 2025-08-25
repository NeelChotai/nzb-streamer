use crate::archive::par2::DownloadTask;
use crate::nntp::client::NntpClient;
use crate::nntp::config::NntpConfig;
use crate::nntp::yenc::compute_hash16k;
use crate::scheduler::batch::BatchGenerator;
use crate::scheduler::error::SchedulerError;
use crate::scheduler::job_processor::process_job;
use crate::stream::orchestrator::BufferHealth;
use bytes::Bytes;
use futures::TryStreamExt;
use futures::stream::{self, StreamExt};
use memmap2::MmapMut;
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::watch;

use tracing::info;

pub struct AdaptiveScheduler {
    client: Arc<NntpClient>,
    max_workers: usize,
}

#[derive(Debug)]
pub struct FirstSegment {
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
    ) -> Result<Vec<FirstSegment>, SchedulerError> {
        info!("Downloading first segments of {} files", files.len());

        stream::iter(files.iter().cloned())
            .map(|file| self.download_first_segment(file))
            .buffer_unordered(self.max_workers.min(20)) // TODO: workers
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect()
    }

    pub async fn download_first_segment(
        &self,
        file: nzb_rs::File,
    ) -> Result<FirstSegment, SchedulerError> {
        let first_segment = file
            .segments
            .first()
            .ok_or_else(|| SchedulerError::EmptyFile(file.subject.clone()))?;

        let data = self.client.download(first_segment).await?;
        let hash16k = compute_hash16k(&data);

        Ok(FirstSegment {
            nzb: file,
            hash16k: hash16k.into(),
            bytes: data,
        })
    }

    pub async fn schedule_downloads(
        &self,
        tasks: Vec<DownloadTask>,
        mmap: Arc<RwLock<MmapMut>>,
        health_rx: watch::Receiver<BufferHealth>,
    ) -> Result<(), SchedulerError> {
        info!("Starting adaptive scheduling for {} tasks", tasks.len());

        let total_tasks = tasks.len();

        let generator = BatchGenerator::new(tasks, health_rx);
        let client = self.client.clone();
        let max_workers = self.max_workers;

        stream::iter(generator)
            .then(|batch| {
                let client = client.clone();
                let mmap = mmap.clone();
                let job_parallelism = batch.health.concurrent_jobs(total_tasks);
                let segment_parallelism = batch
                    .health
                    .segment_parallelism(max_workers, job_parallelism);

                async move {
                    stream::iter(batch.jobs)
                        .map(|job| {
                            process_job(job, client.clone(), mmap.clone(), segment_parallelism)
                        })
                        .buffer_unordered(job_parallelism)
                        .try_collect::<Vec<_>>()
                        .await
                }
            })
            .try_collect::<Vec<_>>()
            .await?;

        info!("All downloads complete");

        Ok(())
    }
}
