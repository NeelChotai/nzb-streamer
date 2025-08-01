use config::{Config, Environment};
use derive_more::Constructor;
use serde::Deserialize;

use crate::nntp::error::NntpError;

#[derive(Debug, Clone, Deserialize, Constructor)]
pub struct NntpConfig {
    pub host: String,
    pub username: String,
    pub password: String,
    pub max_connections: Option<usize>,
}

impl NntpConfig {
    pub fn from_env() -> Result<Self, NntpError> {
        let config: NntpConfig = Config::builder()
            .add_source(
                Environment::with_prefix("NNTP")
                    .separator("_")
                    .list_separator(" "),
            )
            .build()?
            .try_deserialize()?;

        Ok(config)
    }
}

#[derive(Debug, Clone)]
pub struct DownloadStatus {
    pub total_files: usize,
    pub downloaded_files: usize,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub current_file: Option<String>,
    pub is_complete: bool,
}
