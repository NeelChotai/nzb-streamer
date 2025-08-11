use std::{io, path::PathBuf};

use thiserror::Error;

use crate::archive::error::ArchiveError;

#[derive(Error, Debug)]
pub enum StreamError {
    #[error("Error reading file")]
    Read(#[from] io::Error),

    #[error(
        "Tried to mark file with path '{0}' as complete, but does not exist in this VolumeSegment"
    )]
    FileNotFound(PathBuf),

    #[error(transparent)]
    Archive(#[from] ArchiveError),
}
