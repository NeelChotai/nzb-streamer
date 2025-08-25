use thiserror::Error;

#[derive(Error, Debug)]
pub enum ArchiveError {
    #[error("Error reading file")]
    Io(#[from] std::io::Error),

    #[error("No files found")]
    NoFiles,

    #[error("Received corrupted or incomplete packet")]
    IncompleteData,

    #[error("PAR2 parsing error")]
    Parse,

    #[error("RAR signature not found in buffer")]
    MalformedRar,

    #[error("Could not create download task for subject {0}")]
    FilenameNotFound(String),
}
