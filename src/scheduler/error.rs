use std::path::PathBuf;

use bytes::Bytes;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::{
    archive::error::ArchiveError, nntp::error::NntpError, stream::orchestrator::OrchestratorEvent,
};

#[derive(Error, Debug)]
pub enum SchedulerError {
    #[error("Configuration error")]
    Config(#[from] NntpError),

    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("Error in channel communication on write")]
    Write(#[from] mpsc::error::SendError<(usize, Bytes)>),

    // #[error("Error in channel communication during job dispatch")]
    // Dispatch(#[from] async_channel::SendError<Job>),
    #[error("Error communicating with event channel")]
    Communication(#[from] mpsc::error::SendError<OrchestratorEvent>),

    #[error("File '{0}' contained no subjects, source NZB may be malformed")]
    EmptyFile(String),

    #[error("Tried to write file to path '{0}', but does not exist")]
    FileNotFound(PathBuf),

    #[error("Error getting offset and length from file")]
    Archive(#[from] ArchiveError),
}
