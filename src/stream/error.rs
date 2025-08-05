use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum StreamError {
    #[error("No RAR signature for file '{0}'")]
    MalformedRar(String),

    #[error("Error reading file")]
    Read(#[from] io::Error),
}
