use thiserror::Error;
use std::fmt;

#[derive(Error, Debug)]
pub enum NzbStreamerError {
    #[error("NZB parsing error: {0}")]
    NzbParsing(String),
    
    #[error("NNTP connection error: {0}")]
    NntpConnection(String),
    
    #[error("NNTP protocol error: {0}")]
    NntpProtocol(String),
    
    #[error("NNTP authentication failed: {0}")]
    NntpAuth(String),
    
    #[error("Article not found: {0}")]
    ArticleNotFound(String),
    
    #[error("Article corrupt or invalid: {message_id}")]
    ArticleCorrupt { message_id: String },
    
    #[error("yEnc decoding error: {0}")]
    YencDecoding(String),
    
    #[error("yEnc validation error: {0}")]
    YencValidation(String),
    
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
    
    #[error("Connection pool error: {0}")]
    ConnectionPool(String),
    
    #[error("Timeout error: {0}")]
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, NzbStreamerError>;

// Removed quick_xml error handling - using nzb-rs instead

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

// Enhanced error mapping for better integration with external crates
impl NzbStreamerError {
    /// Create an NNTP connection error from any error type
    pub fn nntp_connection<E: fmt::Display>(err: E) -> Self {
        NzbStreamerError::NntpConnection(err.to_string())
    }
    
    /// Create an NNTP protocol error from any error type
    pub fn nntp_protocol<E: fmt::Display>(err: E) -> Self {
        NzbStreamerError::NntpProtocol(err.to_string())
    }
    
    /// Create a yEnc decoding error from any error type
    pub fn yenc_decoding<E: fmt::Display>(err: E) -> Self {
        NzbStreamerError::YencDecoding(err.to_string())
    }
    
    /// Create a connection pool error from any error type
    pub fn connection_pool<E: fmt::Display>(err: E) -> Self {
        NzbStreamerError::ConnectionPool(err.to_string())
    }
    
    /// Create a timeout error from any error type
    pub fn timeout<E: fmt::Display>(err: E) -> Self {
        NzbStreamerError::Timeout(err.to_string())
    }
    
    /// Check if this error is retryable (network/connection issues)
    pub fn is_retryable(&self) -> bool {
        matches!(self, 
            NzbStreamerError::NntpConnection(_) |
            NzbStreamerError::Network(_) |
            NzbStreamerError::Timeout(_) |
            NzbStreamerError::ConnectionPool(_)
        )
    }
    
    /// Check if this error indicates a missing article
    pub fn is_article_missing(&self) -> bool {
        matches!(self, NzbStreamerError::ArticleNotFound(_))
    }
    
    /// Check if this error indicates data corruption
    pub fn is_corruption(&self) -> bool {
        matches!(self, 
            NzbStreamerError::ArticleCorrupt { .. } |
            NzbStreamerError::YencValidation(_) |
            NzbStreamerError::RarParsing(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_error_classification() {
        let conn_error = NzbStreamerError::NntpConnection("test".to_string());
        assert!(conn_error.is_retryable());
        assert!(!conn_error.is_article_missing());
        assert!(!conn_error.is_corruption());
        
        let missing_error = NzbStreamerError::ArticleNotFound("test".to_string());
        assert!(!missing_error.is_retryable());
        assert!(missing_error.is_article_missing());
        assert!(!missing_error.is_corruption());
        
        let corrupt_error = NzbStreamerError::ArticleCorrupt { message_id: "test".to_string() };
        assert!(!corrupt_error.is_retryable());
        assert!(!corrupt_error.is_article_missing());
        assert!(corrupt_error.is_corruption());
    }
    
    #[test]
    fn test_error_helper_methods() {
        let err = NzbStreamerError::nntp_connection("test error");
        assert!(matches!(err, NzbStreamerError::NntpConnection(_)));
        
        let err = NzbStreamerError::yenc_decoding(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad data"));
        assert!(matches!(err, NzbStreamerError::YencDecoding(_)));
    }
}