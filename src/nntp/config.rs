use crate::nntp::error::NntpError;

pub struct NntpConfig {
    pub host: String,
    pub username: String,
    pub password: String
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
        
        Ok(Self { host, username, password })
    }

    pub fn new(host: String, username: String, password: String) -> Self {
        Self { host, username, password }
    }
}