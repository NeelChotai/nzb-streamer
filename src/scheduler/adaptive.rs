use crate::nntp::client::NntpClient;
use crate::nntp::config::NntpConfig;
use crate::nntp::yenc::{compute_hash16k, extract_filename};
use crate::par2::manifest::DownloadTask;
use crate::scheduler::batch::{self, BatchGenerator, Priority};
use crate::scheduler::{error::SchedulerError, queue::FileQueue, writer::FileWriterPool};
use crate::stream::{VirtualFileStreamer, segment_tracker::SegmentTracker};
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
    pub hash16k: Vec<u8>,
}

impl AdaptiveScheduler {
    pub fn new(config: NntpConfig) -> Result<Self, SchedulerError> {
        let client = NntpClient::new(config.clone())?; // TODO: don't clone here

        Ok(Self {
            client: Arc::new(client),
            max_workers: *config.max_connections,
        })
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
            hash16k,
        })
    }

    pub async fn schedule_downloads(
        &self,
        tasks: Vec<DownloadTask>,
        session_dir: &Path,
        session_id: Uuid,
        streamer: Arc<VirtualFileStreamer>,
        tracker: Arc<SegmentTracker>,
    ) -> Result<(), SchedulerError> {
        info!("Starting adaptive download for session {}", session_id);

        let session = self.prepare_session(tasks, session_dir).await?;
        self.process_batches(session, tracker, streamer).await?;

        info!("Download complete for session {}", session_id);
        Ok(())
    }

    async fn prepare_session(
        &self,
        tasks: Vec<DownloadTask>,
        session_dir: &Path,
    ) -> Result<Session, SchedulerError> {
        let writers = FileWriterPool::new();
        let file_queues = stream::iter(tasks)
            .then(|task| {
                let writers = &writers;
                async move {
                    let real_path = session_dir.join(&task.real_name);
                    tokio::fs::rename(&task.obfuscated_path, &real_path).await?;
                    writers.add_file(&real_path).await?;
                    FileQueue::new(task, real_path)
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
        tracker: Arc<SegmentTracker>,
        streamer: Arc<VirtualFileStreamer>,
    ) -> Result<(), SchedulerError> {
        let (tx, rx) = async_channel::bounded(self.max_workers * 2);

        let file_segments = Arc::new(dashmap::DashMap::from_iter(
            session
                .file_queues
                .iter()
                .map(|q| (q.path().clone(), q.total_segments())),
        ));
        let completed = Arc::new(dashmap::DashMap::from_iter(
            session
                .file_queues
                .iter()
                .map(|q| (q.path().clone(), 0usize)),
        ));
        let workers = (0..self.max_workers)
            .map(|id| {
                spawn_worker(
                    id,
                    rx.clone(),
                    self.client.clone(),
                    session.writers.clone(),
                    completed.clone(),
                    file_segments.clone(),
                    streamer.clone(),
                    tracker.clone(),
                )
            })
            .collect::<Vec<_>>();

        let generator = BatchGenerator::new(session.file_queues, self.max_workers)
            .into_iter_with_tracker(tracker.clone());

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
            if batch.priority == Priority::Critical {
                while tx.len() > self.max_workers / 2 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
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
    completed: Arc<dashmap::DashMap<PathBuf, usize>>,
    file_totals: Arc<dashmap::DashMap<PathBuf, usize>>,
    streamer: Arc<VirtualFileStreamer>,
    tracker: Arc<SegmentTracker>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        debug!("Worker {} started", id);

        while let Ok(job) = rx.recv().await {
            match client.download(job.segment()).await {
                Ok(data) => {
                    if let Err(e) = writers.write(job.path(), job.index(), data).await {
                        error!("Worker {} write failed: {}", id, e);
                    } else {
                        let mut entry = completed.entry(job.path().clone()).or_insert(0);
                        *entry += 1;

                        if let Some(total) = file_totals.get(job.path()) {
                            if *entry == *total {
                                info!("File {} complete", job.path().display());
                                let _ = streamer.mark_segment_downloaded(job.path()).await;
                                tracker.mark_segment_complete(job.index()); // TODO : why two
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Worker {} download failed: {}", id, e);
                }
            }
        }

        debug!("Worker {} finished", id);
    })
}
