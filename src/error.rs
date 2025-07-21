use thiserror::Error;

#[derive(Error, Debug)]
pub enum NzbStreamerError {
    #[error("NZB parsing error: {0}")]
    NzbParsing(String),
    
    #[error("NNTP connection error: {0}")]
    NntpConnection(String),
    
    #[error("NNTP authentication failed: {0}")]
    NntpAuth(String),
    
    #[error("Article not found: {message_id}")]
    ArticleNotFound { message_id: String },
    
    #[error("Article corrupt or invalid: {message_id}")]
    ArticleCorrupt { message_id: String },
    
    #[error("yEnc decoding error: {0}")]
    YencDecoding(String),
    
    #[error("RAR parsing error: {0}")]
    RarParsing(String),
    
    #[error("RAR file not found in archive: {filename}")]
    RarFileNotFound { filename: String },
    
    #[error("Unsupported RAR compression method: {method}")]
    UnsupportedRarMethod { method: u8 },
    
    #[error("HTTP range request error: {0}")]
    HttpRange(String),
    
    #[error("Invalid byte range: start={start}, end={end}, file_size={file_size}")]
    InvalidRange { start: u64, end: u64, file_size: u64 },
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("Configuration error: {0}")]
    Config(String),
    
    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },
    
    #[error("Cache error: {0}")]
    Cache(String),
}

pub type Result<T> = std::result::Result<T, NzbStreamerError>;

impl From<quick_xml::Error> for NzbStreamerError {
    fn from(err: quick_xml::Error) -> Self {
        NzbStreamerError::NzbParsing(err.to_string())
    }
}

impl From<native_tls::Error> for NzbStreamerError {
    fn from(err: native_tls::Error) -> Self {
        NzbStreamerError::NntpConnection(err.to_string())
    }
}

impl From<http_range_header::RangeUnsatisfiableError> for NzbStreamerError {
    fn from(err: http_range_header::RangeUnsatisfiableError) -> Self {
        NzbStreamerError::HttpRange(err.to_string())
    }
}