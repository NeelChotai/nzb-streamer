use std::path::PathBuf;

use crate::nntp::NntpClient;
use crate::nntp::error::NntpError;
use nzb_rs::Segment;

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
}
