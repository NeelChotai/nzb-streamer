use std::path::PathBuf;

use bytes::Bytes;
use thiserror::Error;

use crate::{archive::error::ArchiveError, nntp::error::NntpError, scheduler::batch::Job};

#[derive(Error, Debug)]
pub enum SchedulerError {
    #[error("Configuration error")]
    Config(#[from] NntpError),

    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("Error in channel communication on write")]
    Write(#[from] tokio::sync::mpsc::error::SendError<(usize, Bytes)>),

    #[error("Error in channel communication during job dispatch")]
    Dispatch(#[from] async_channel::SendError<Job>),

    #[error("File '{0}' contained no subjects, source NZB may be malformed")]
    EmptyFile(String),

    #[error("Tried to write file to path '{0}', but does not exist")]
    FileNotFound(PathBuf),

    #[error("Error getting offset and length from file")]
    Archive(#[from] ArchiveError),
}
