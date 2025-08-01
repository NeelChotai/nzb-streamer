use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::nntp::config::DownloadStatus;
use crate::nntp::live::LiveNntpClient;
use crate::nntp::mock::MockNntpClient;
use crate::nntp::{config::NntpConfig, error::NntpError};
use async_trait::async_trait;
use nzb_rs::{File, Segment};
use tokio::sync::Mutex;
use tracing::{info, warn};

pub fn nntp_client(
    live: bool,
    mock_dir: Option<std::path::PathBuf>,
) -> Box<dyn NntpClient + Send + Sync> {
    if live {
        info!("Configuring live NNTP downloads");
        let config = NntpConfig::from_env()
            .unwrap_or_else(|err| panic!("Failed to load NNTP config: {err}"));
        info!("NNTP configuration loaded");

        Box::new(LiveNntpClient::new(config))
    } else {
        info!("Using mock mode - no live downloads");
        if let Some(ref dir) = mock_dir {
            if !dir.exists() {
                warn!("Mock data directory doesn't exist: {}", dir.display());
            }
        }

        Box::new(MockNntpClient::new(mock_dir))
    }
}

#[async_trait]
pub trait NntpClient: Send + Sync {
    async fn download_segment(&self, segment: &Segment) -> Result<Vec<u8>, NntpError>;

    async fn download_files_with_throttle(
        &self,
        files: Vec<File>,
        output_dir: &Path,
        status: Arc<Mutex<DownloadStatus>>,
    ) -> Result<HashMap<String, PathBuf>, NntpError>;

    async fn download_file(&self, file: &File, output_path: &Path) -> Result<Vec<u8>, NntpError>;
}
