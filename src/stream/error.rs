use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum StreamError {
    #[error("No RAR signature for file '{0}'")]
    MalformedRar(String),

    #[error("Error reading file")]
    Read(#[from] io::Error),

    #[error("Tried to mark file with path '{0}' as complete, but does not exist in this VolumeSegment")]
    FileNotFound(PathBuf)
}
