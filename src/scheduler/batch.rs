use crate::scheduler::queue::FileQueue;
use crate::stream::orchestrator::BufferHealth;
use derive_more::Constructor;
use nzb_rs::Segment;
use std::path::PathBuf;
use tokio::sync::watch;

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
    segment: Segment,
    path: PathBuf,
    index: usize,
    total_segments: usize,
}

impl Job {
    pub fn segment(&self) -> &Segment {
        &self.segment
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn total_segments(&self) -> usize {
        self.total_segments
    }
}

#[derive(Debug)]
pub struct Batch {
    pub jobs: Vec<Job>,
    pub priority: Priority,
}

#[derive(Debug, Constructor)]
pub struct BatchGenerator {
    files: Vec<FileQueue>,
    batch_size: usize,
    health_rx: watch::Receiver<BufferHealth>,
}

impl BatchGenerator {
    fn generate_batch(&mut self, priority: Priority) -> Option<Batch> {
        let jobs = match priority {
            Priority::Critical => self.take_critical(),
            Priority::Balanced => self.take_balanced(),
            Priority::Parallel => self.take_parallel(),
        };

        (!jobs.is_empty()).then_some(Batch { jobs, priority })
    }

    fn take_critical(&mut self) -> Vec<Job> {
        self.files
            .iter_mut()
            .find(|f| !f.is_empty())
            .map(|f| f.take(self.batch_size))
            .unwrap_or_default()
    }

    fn take_balanced(&mut self) -> Vec<Job> {
        const FILES_PER_WAVE: usize = 3;
        let per_file = self.batch_size / FILES_PER_WAVE;

        let jobs: Vec<_> = self
            .files
            .iter_mut()
            .filter(|f| !f.is_empty())
            .take(FILES_PER_WAVE)
            .flat_map(|f| f.take(per_file))
            .collect();

        if jobs.len() < self.batch_size / 2 {
            let remaining = self.batch_size - jobs.len();
            let extra: Vec<_> = self
                .files
                .iter_mut()
                .filter(|f| !f.is_empty())
                .flat_map(|f| f.take(remaining))
                .collect();
            [jobs, extra].concat()
        } else {
            jobs
        }
    }

    fn take_parallel(&mut self) -> Vec<Job> {
        let active = self.files.iter().filter(|f| !f.is_empty()).count();
        if active == 0 {
            return Vec::new();
        }

        let per_file = (self.batch_size / active).max(1);
        self.files
            .iter_mut()
            .filter(|f| !f.is_empty())
            .flat_map(|f| f.take(per_file))
            .collect()
    }
}

impl Iterator for BatchGenerator {
    type Item = Batch;

    fn next(&mut self) -> Option<Self::Item> {
        let priority = (*self.health_rx.borrow()).into();
        self.generate_batch(priority)
    }
}
