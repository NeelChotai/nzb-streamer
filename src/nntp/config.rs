use std::time::Duration;

use config::{Config, Environment};
use derive_more::Constructor;
use serde::Deserialize;
use shrinkwraprs::Shrinkwrap;
use tracing::warn;

use crate::nntp::error::NntpError;

#[derive(Debug, Clone, Deserialize, Constructor)]
pub struct NntpConfig {
    pub host: String,
    pub username: String,
    pub password: String,

    #[serde(default)] // TODO: not working?
    pub max_connections: MaxConnections,

    #[serde(default)]
    pub idle_timeout: IdleTimeout,
}

#[derive(Deserialize, Debug, Clone, Shrinkwrap)]
pub struct MaxConnections(usize);

impl Default for MaxConnections {
    fn default() -> Self {
        const DEFAULT: usize = 50;
        warn!("Max connections not provided. Using default ({}).", DEFAULT);
        MaxConnections(DEFAULT)
    }
}

#[derive(Deserialize, Debug, Clone, Shrinkwrap)]
pub struct IdleTimeout(Duration);

impl Default for IdleTimeout {
    fn default() -> Self {
        const DEFAULT: usize = 10;
        warn!(
            "Idle timeout not provided. Using default ({} secs).",
            DEFAULT
        );
        IdleTimeout(Duration::from_secs(DEFAULT as u64))
    }
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
