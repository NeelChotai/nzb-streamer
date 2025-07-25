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
