use crate::scheduler::{batch::Job, error::SchedulerError};
use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Debug)]
pub struct FileQueue {
    path: PathBuf,
    segments: VecDeque<Job>,
    total: usize,
    start_index: usize,
}

impl FileQueue {
    pub fn new(
        path: PathBuf,
        nzb: nzb_rs::File,
        start_index: usize,
    ) -> Result<Self, SchedulerError> {
        let total = nzb.segments.len() - start_index;

        let segments: VecDeque<Job> = nzb
            .segments
            .into_iter()
            .enumerate()
            .skip(start_index)
            .map(|(idx, segment)| Job::new(segment, path.clone(), idx, total))
            .collect();

        Ok(Self {
            path,
            segments,
            total,
            start_index,
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

    pub fn start_index(&self) -> usize {
        self.start_index
    }
}
