use std::path::PathBuf;

use crate::nntp::error::NntpError;
use crate::nntp::NntpClient;
use nzb_rs::{File, Segment};
use tokio::sync::Mutex;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::nntp::config::DownloadStatus;

pub struct MockNntpClient {
    mock_dir: Option<PathBuf>,
}

impl MockNntpClient {
    pub fn new(mock_dir: Option<PathBuf>) -> Self {
        Self { mock_dir }
    }
}

#[async_trait::async_trait]
impl NntpClient for MockNntpClient {
    async fn download_segment(&self, segment: &Segment) -> Result<Vec<u8>, NntpError> {
        todo!()
    }

    async fn download_files_with_throttle(
        &self,
        files: Vec<File>,
        output_dir: &Path,
        status: Arc<Mutex<DownloadStatus>>,
    ) -> Result<HashMap<String, PathBuf>, NntpError> {
        todo!()
    }

    async fn download_file(&self, file: &File, output_path: &Path) -> Result<Vec<u8>, NntpError> {
        todo!()
    }
}
