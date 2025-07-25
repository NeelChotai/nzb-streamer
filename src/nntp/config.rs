use crate::nntp::error::NntpError;

#[derive(Debug, Clone)]
pub struct NntpConfig {
    pub host: String,
    pub username: String,
    pub password: String,
    pub port: u16,
    pub use_ssl: bool,
}

impl NntpConfig {
    pub async fn from_env() -> Result<Self, NntpError> {
        dotenvy::dotenv().ok();
        
        let host = std::env::var("NNTP_HOST")
            .map_err(|_| NntpError::Config("NNTP_HOST not set".to_string()))?;
        let username = std::env::var("NNTP_USER")
            .map_err(|_| NntpError::Config("NNTP_USER not set".to_string()))?;
        let password = std::env::var("NNTP_PASS")
            .map_err(|_| NntpError::Config("NNTP_PASS not set".to_string()))?;
        
        let port = std::env::var("NNTP_PORT")
            .unwrap_or_else(|_| "563".to_string())
            .parse()
            .map_err(|_| NntpError::Config("NNTP_PORT must be a valid port number".to_string()))?;
            
        let use_ssl = std::env::var("NNTP_SSL")
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .map_err(|_| NntpError::Config("NNTP_SSL must be true or false".to_string()))?;
        
        Ok(Self { host, username, password, port, use_ssl })
    }

    pub fn new(host: String, username: String, password: String, port: u16, use_ssl: bool) -> Self {
        Self { host, username, password, port, use_ssl }
    }
}