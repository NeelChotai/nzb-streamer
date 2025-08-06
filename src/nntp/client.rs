use std::sync::Arc;
use std::time::Duration;

use crate::nntp::pool::NntpPool;
use crate::nntp::yenc::extract_yenc_data;
use crate::nntp::{config::NntpConfig, error::NntpError};
use backoff::ExponentialBackoff;
use backoff::exponential::ExponentialBackoffBuilder;
use backoff::future::retry;
use bytes::Bytes;
use nzb_rs::Segment;
use rek2_nntp::body_bytes;
use tracing::{debug, warn};

pub struct NntpClient {
    pool: Arc<NntpPool>,
}

impl NntpClient {
    pub fn new(config: NntpConfig) -> Result<Self, NntpError> {
        let pool = NntpPool::new(config)?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    pub async fn download(&self, segment: &Segment) -> Result<Bytes, NntpError> {
        let backoff: ExponentialBackoff = ExponentialBackoffBuilder::new()
            .with_max_elapsed_time(Some(Duration::from_secs(30)))
            .build();

        retry(backoff, || async {
            match self.download_segment(segment).await {
                Ok(data) => Ok(data),
                Err(e) => {
                    warn!("Download attempt failed: {}", e);
                    Err(backoff::Error::transient(e)) // TODO: for now, some errors ARE permenant
                }
            }
        })
        .await
    }

    async fn download_segment(&self, segment: &Segment) -> Result<Bytes, NntpError> {
        let mut conn = self.pool.get().await?;

        let message_id = format!("<{}>", segment.message_id);
        let raw_data = body_bytes(&mut conn, &message_id)
            .await
            .map_err(|e| NntpError::Read(e.to_string()))?; // TODO: permenant error

        drop(conn); // TODO: quit logic when dropping connection (this is recycle though)

        let yenc_data = extract_yenc_data(&raw_data);
        let decoded = yenc::decode_buffer(&yenc_data).unwrap(); // TODO: permenant error

        debug!(
            "Downloaded segment {} ({} bytes raw, {} decoded)",
            segment.message_id,
            raw_data.len(),
            decoded.len()
        );

        Ok(decoded.into()) // TODO: fix type constraints
    }
}
