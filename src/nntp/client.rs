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
use tokio::time;
use tracing::{debug, info, warn};

pub struct NntpClient {
    pool: NntpPool,
}

impl NntpClient {
    pub fn new(config: NntpConfig) -> Result<Self, NntpError> {
        Ok(Self {
            pool: NntpPool::new(config)?,
        })
    }

    pub async fn warm_pool(&self) {
        let target = 50; // TODO: hardcoded for now
        info!("Pre-warming connection pool with {} connections", target);

        for i in 0..target {
            tokio::spawn({
                let client = self.pool.clone();
                async move {
                    time::sleep(Duration::from_millis(i as u64 * 50)).await;

                    match client.get().await {
                        Ok(_) => {
                            debug!("Pre-warmed connection {}", i);
                        }
                        Err(e) => {
                            warn!("Failed to pre-warm connection {}: {}", i, e);
                        }
                    };
                }
            });
        }

        info!("Connection pool warmed");
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

        //drop(conn); // TODO: quit logic when dropping connection (this is recycle though)

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
