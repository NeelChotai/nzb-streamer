use thiserror::Error;

use crate::{nntp::error::NntpError, scheduler::batch::Job};

#[derive(Error, Debug)]
pub enum SchedulerError {
    #[error("Configuration error")]
    Config(#[from] NntpError),

    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("Error in channel communication on write")]
    Write(#[from] tokio::sync::mpsc::error::SendError<(usize, Vec<u8>)>),

    #[error("Error in channel communication during job dispatch")]
    Dispatch(#[from] async_channel::SendError<Job>),

    #[error("File '{0}' contained no subjects, source NZB may be malformed")]
    EmptyFile(String),
}
