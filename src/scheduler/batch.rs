use crate::archive::par2::DownloadTask;
use crate::stream::orchestrator::BufferHealth;
use derive_more::Constructor;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Priority {
    Critical,
    Balanced,
    Parallel,
}

impl From<BufferHealth> for Priority {
    fn from(health: BufferHealth) -> Self {
        match health {
            BufferHealth::Critical => Priority::Critical,
            BufferHealth::Poor => Priority::Balanced,
            _ => Priority::Parallel,
        }
    }
}

#[derive(Debug, Clone, Constructor)]
pub struct Job {
    pub task: Arc<DownloadTask>,
    pub offset: u64,
}

#[derive(Debug)]
pub struct Batch {
    pub jobs: Vec<Job>,
    pub health: BufferHealth,
}

#[derive(Debug)]
pub struct BatchGenerator {
    jobs: Vec<Job>,
    consumed: Vec<bool>,
    health_rx: watch::Receiver<BufferHealth>,
}

impl BatchGenerator {
    pub fn new(tasks: Vec<DownloadTask>, health_rx: watch::Receiver<BufferHealth>) -> Self {
        let mut offset = tasks.first().unwrap().bytes().len();
        let mut jobs = Vec::new();
        for task in tasks.into_iter().skip(1) {
            // skip first
            let length = task.bytes().len();
            println!("Writing {} bytes at offset {}", task.bytes().len(), offset);

            let job = Job {
                task: task.into(),
                offset: offset as u64,
            };
            jobs.push(job);
            offset += length;
        }

        let consumed = vec![false; jobs.len()];

        Self {
            jobs,
            consumed,
            health_rx,
        }
    }

    /// Get indices of available (not consumed) jobs
    fn available_indices(&self) -> Vec<usize> {
        self.consumed
            .iter()
            .enumerate()
            .filter_map(|(i, &consumed)| (!consumed).then_some(i))
            .collect()
    }
}

impl Iterator for BatchGenerator {
    type Item = Batch;

    fn next(&mut self) -> Option<Self::Item> {
        let available = self.available_indices();
        if available.is_empty() {
            return None;
        }

        let health = *self.health_rx.borrow();
        let concurrent = health.concurrent_jobs(available.len());

        // Select indices based on strategy
        let selected_indices: Vec<_> = match health {
            BufferHealth::Critical | BufferHealth::Poor => {
                // Sequential: take first N available
                available.into_iter().take(concurrent).collect()
            }
            BufferHealth::Good | BufferHealth::Excellent => {
                // Distributed: sample evenly from available
                let step = available.len() / concurrent.max(1);
                if step <= 1 {
                    available.into_iter().take(concurrent).collect()
                } else {
                    (0..concurrent).map(|i| available[i * step]).collect()
                }
            }
        };

        // Mark as consumed and collect jobs
        let jobs: Vec<Job> = selected_indices
            .into_iter()
            .map(|i| {
                self.consumed[i] = true;
                self.jobs[i].clone()
            })
            .collect();

        debug!(
            "Generated batch of {} jobs (health: {:?})",
            jobs.len(),
            health
        );

        Some(Batch { jobs, health })
    }
}
