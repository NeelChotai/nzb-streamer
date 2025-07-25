use thiserror::Error;

#[derive(Error, Debug)]
pub enum NntpError {
    #[error("error reading config from environment")]
    Config(#[from] config::ConfigError),

    #[error("client authentication error: {0}")]
    ClientAuthentication(String),

    #[error("error reading body: {0}")]
    BodyRead(String),

    #[error("connection pool error: {0}")]
    ConnectionPool(String),

    #[error("segment download failed: {0}")]
    SegmentDownload(String),

    #[error("timeout error: {0}")]
    Timeout(String),

    #[error("network error: {0}")]
    Network(String),
}
