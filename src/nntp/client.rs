use crate::nntp::{config::NntpConfig, error::NntpError};
use nzb_rs::File;
use rek2_nntp::{authenticate, body};
use tracing::debug;

pub struct NntpClient {
    config: NntpConfig
}

impl NntpClient {
    pub fn new(config: NntpConfig) -> Self {
        Self { config }
    }

    pub async fn get_file(&self, file: File) -> Result<Vec<u8>, NntpError> {
        debug!("authenticating with nntp server");
        let mut conn = authenticate(&self.config.host, &self.config.username, &self.config.password).await.map_err(|e| NntpError::ClientAuthentication(e.to_string()))?;

        debug!("getting file: {}", file.subject);
        
        let mut data = Vec::new();

        for seg in file.segments.iter() {
            data.extend(body(&mut conn, &seg.message_id).await.unwrap().into_bytes()) // TODO: functional + unwrap
        }

        debug!("got {} segments", data.len());
        Ok(data)
    }
}