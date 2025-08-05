use crate::scheduler::queue::FileQueue;
use crate::stream::segment_tracker::{BufferHealth, SegmentTracker};
use nzb_rs::Segment;
use std::path::PathBuf;
use std::sync::Arc;

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

#[derive(Debug, Clone)]
pub struct Job {
    segment: Segment,
    path: PathBuf,
    index: usize,
}

impl Job {
    pub fn new(segment: Segment, path: PathBuf, index: usize) -> Self {
        Self {
            segment,
            path,
            index,
        }
    }

    pub fn segment(&self) -> &Segment {
        &self.segment
    }
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
    pub fn index(&self) -> usize {
        self.index
    }
}

#[derive(Debug)]
pub struct Batch {
    pub jobs: Vec<Job>,
    pub priority: Priority,
}

pub struct BatchGenerator {
    files: Vec<FileQueue>,
    wave_size: usize,
}

impl BatchGenerator {
    pub fn new(files: Vec<FileQueue>, wave_size: usize) -> Self {
        Self { files, wave_size }
    }

    pub fn into_iter_with_tracker(
        self,
        tracker: Arc<SegmentTracker>,
    ) -> BatchIterator<impl Fn() -> Priority> {
        let priority_fn = move || tracker.get_buffer_health().into();
        BatchIterator {
            generator: self,
            priority_fn,
        }
    }

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
            .map(|f| f.take(self.wave_size))
            .unwrap_or_default()
    }

    fn take_balanced(&mut self) -> Vec<Job> {
        const FILES_PER_WAVE: usize = 3;
        let per_file = self.wave_size / FILES_PER_WAVE;

        let jobs: Vec<_> = self
            .files
            .iter_mut()
            .filter(|f| !f.is_empty())
            .take(FILES_PER_WAVE)
            .flat_map(|f| f.take(per_file))
            .collect();

        if jobs.len() < self.wave_size / 2 {
            let remaining = self.wave_size - jobs.len();
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

        let per_file = (self.wave_size / active).max(1);
        self.files
            .iter_mut()
            .filter(|f| !f.is_empty())
            .flat_map(|f| f.take(per_file))
            .collect()
    }
}

pub struct BatchIterator<F> {
    generator: BatchGenerator,
    priority_fn: F,
}

impl<F> Iterator for BatchIterator<F>
where
    F: Fn() -> Priority,
{
    type Item = Batch;

    fn next(&mut self) -> Option<Self::Item> {
        let priority = (self.priority_fn)();
        self.generator.generate_batch(priority)
    }
}
