use crate::archive::par2::DownloadTask;
use crate::nntp::client::NntpClient;
use crate::nntp::config::NntpConfig;
use crate::nntp::yenc::{compute_hash16k, extract_filename};
use crate::scheduler::batch::{self, BatchGenerator, Priority};
use crate::scheduler::{error::SchedulerError, queue::FileQueue, writer::FileWriterPool};
use crate::stream::orchestrator::{BufferHealth, StreamOrchestrator};
use bytes::Bytes;
use dashmap::DashMap;
use futures::future::join_all;
use futures::stream::{self, StreamExt};
use std::path::PathBuf;
use std::{path::Path, sync::Arc};

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
        })
    }

    pub async fn schedule_downloads(
        &self,
        tasks: Vec<DownloadTask>,
        session_id: Uuid,
        orchastrator: Arc<StreamOrchestrator>,
    ) -> Result<(), SchedulerError> {
        info!("Starting adaptive download for session {}", session_id);

        let session = self.prepare_session(tasks).await?;
        self.process_batches(session, orchastrator).await?;

        info!("Download complete for session {}", session_id);
        Ok(())
    }

    async fn prepare_session(&self, tasks: Vec<DownloadTask>) -> Result<Session, SchedulerError> {
        let writers = FileWriterPool::new();
        let file_queues = stream::iter(tasks)
            .then(|task| {
                let writers = &writers;
                async move {
                    writers.add_file(&task.path).await?;
                    FileQueue::new(task)
                }
            })
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Session {
            file_queues,
            writers,
        })
    }

    async fn process_batches(
        &self,
        session: Session,
        orchestrator: Arc<StreamOrchestrator>,
    ) -> Result<(), SchedulerError> {
        let (tx, rx) = async_channel::bounded(self.max_workers * 2);

        let file_segments = Arc::new(dashmap::DashMap::from_iter(
            session
                .file_queues
                .iter()
                .map(|q| (q.path().clone(), (q.total_segments(), 1))), // TODO: first segment done, is this the right place to put this?
        ));

        let workers = (0..self.max_workers)
            .map(|id| {
                spawn_worker(
                    id,
                    rx.clone(),
                    self.client.clone(),
                    session.writers.clone(),
                    orchestrator.clone(),
                    file_segments.clone(),
                )
            })
            .collect::<Vec<_>>();

        let generator =
            BatchGenerator::new(session.file_queues, self.max_workers, orchestrator.clone());

        for batch in generator {
            debug!(
                "Processing batch with {} jobs, priority {:?}",
                batch.jobs.len(),
                batch.priority
            );

            for job in batch.jobs {
                tx.send(job).await?;
            }

            // Adaptive backpressure
            match orchestrator.get_buffer_health() {
                BufferHealth::Critical => {
                    // Prioritise playback area
                    while tx.len() > 1 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    }
                }
                BufferHealth::Poor => {
                    while tx.len() > self.max_workers / 2 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
                    }
                }
                _ => {
                    // Normal operation
                    if batch.priority == Priority::Critical {
                        while tx.len() > self.max_workers {
                            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                        }
                    }
                }
            }
        }

        drop(tx);
        join_all(workers).await;
        session.writers.close_all().await;

        Ok(())
    }
}

struct Session {
    file_queues: Vec<FileQueue>,
    writers: FileWriterPool,
}

fn spawn_worker(
    id: usize,
    rx: async_channel::Receiver<batch::Job>,
    client: Arc<NntpClient>,
    writers: FileWriterPool,
    orchestrator: Arc<StreamOrchestrator>,
    file_segments: Arc<DashMap<PathBuf, (usize, usize)>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        debug!("Worker {} started", id);

        while let Ok(job) = rx.recv().await {
            match process_job(&job, &client, &writers).await {
                Ok(_) => {
                    let path = job.path().clone();
                    let mut should_notify = false;

                    if let Some(mut entry) = file_segments.get_mut(&path) {
                        entry.1 += 1; // Increment completed
                        debug!("File {} progress: {}/{}", path.display(), entry.1, entry.0);

                        if entry.1 == entry.0 {
                            should_notify = true;
                            info!("File {} fully downloaded", path.display());
                        }
                    }

                    // Notify orchestrator only when file is complete
                    if should_notify {
                        if let Err(e) = orchestrator.mark_file_complete(&path).await {
                            error!("Failed to mark file complete: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Worker {} failed: {}", id, e);
                }
            }
        }

        debug!("Worker {} finished", id);
    })
}

async fn process_job(
    job: &batch::Job,
    client: &NntpClient,
    writers: &FileWriterPool,
) -> Result<(), SchedulerError> {
    let data = client.download(job.segment()).await?;
    writers.write(job.path(), job.index(), data).await?;
    Ok(())
}
