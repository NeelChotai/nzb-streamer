use thiserror::Error;

#[derive(Error, Debug)]
pub enum Par2Error {
    #[error("Error reading PAR2 file")]
    Io(#[from] std::io::Error),

    #[error("No files found")]
    NoFiles,

    #[error("Received corrupted or incomplete packet")]
    IncompleteData,

    #[error("PAR2 parsing error")]
    Parse,
}
