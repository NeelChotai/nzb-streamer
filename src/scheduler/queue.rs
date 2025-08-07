use crate::archive::par2::DownloadTask;
use crate::scheduler::{batch::Job, error::SchedulerError};
use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Debug)]
pub struct FileQueue {
    path: PathBuf,
    segments: VecDeque<Job>,
    total: usize,
}

impl FileQueue {
    pub fn new(task: DownloadTask) -> Result<Self, SchedulerError> {
        let total = task.nzb.segments.len() - 1;
        let path = task.path;

        let segments: VecDeque<Job> = task
            .nzb
            .segments
            .into_iter()
            .enumerate()
            .skip(1)
            .map(|(idx, segment)| Job::new(segment, path.clone(), idx))
            .collect();

        Ok(Self {
            path,
            segments,
            total,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn take(&mut self, count: usize) -> Vec<Job> {
        self.segments
            .drain(..count.min(self.segments.len()))
            .collect()
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn total_segments(&self) -> usize {
        self.total
    }
}
