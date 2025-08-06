use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum NntpError {
    #[error("Error reading config from environment")]
    Config(#[from] config::ConfigError),

    #[error("Error reading body: {0}")]
    Read(String),

    #[error("I/O error")]
    Io(#[from] io::Error),

    #[error("Error constructing pool")]
    CreatePool(#[from] deadpool::managed::BuildError),

    #[error("Error acquiring thread")] // TODO: why is this causing problems
    AcquirePool(#[from] deadpool::managed::PoolError<NntpPoolError>),
}

#[derive(Error, Debug)]
pub enum NntpPoolError {
    #[error("Authentication error: {0}")]
    Authentication(String),
}
