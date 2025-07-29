
use crate::nntp::pool::ConnectionPool;
use crate::nntp::NntpClient;
use crate::nntp::{config::NntpConfig, error::NntpError};
use async_trait::async_trait;
use backoff::ExponentialBackoff;
use nzb_rs::Segment;
use rek2_nntp::body;
use tokio::time::Duration;

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(1);

pub struct LiveNntpClient {
    pub config: NntpConfig,
    connection_pool: ConnectionPool,
}

#[async_trait]
impl NntpClient for LiveNntpClient {
    async fn download_segment(&self, segment: &Segment) -> Result<Vec<u8>, NntpError> {
        backoff::future::retry(ExponentialBackoff::default(), || async {
            Ok(self.download_segment_internal(segment).await?)
        })
        .await
    }
}

impl LiveNntpClient {
    pub fn new(config: NntpConfig) -> Self {
        let pool = ConnectionPool::new(config.clone());
        Self {
            config,
            connection_pool: pool,
        }
    }

    async fn download_segment_internal(&self, segment: &Segment) -> Result<Vec<u8>, NntpError> {
        let mut conn = self.connection_pool.get_connection().await?;

        let result = body(&mut conn, &segment.message_id)
            .await
            .map_err(|e| NntpError::BodyRead(e.to_string()))?;

        self.connection_pool.return_connection(conn).await;
        Ok(result.into_bytes())
    }

    // pub async fn download_segments_prioritised(
    //     &self,
    //     segments: Vec<&Segment>
    // ) -> HashMap<String, Vec<u8>> {
    //     let mut results = HashMap::new();

    //     // Use semaphore to limit concurrent downloads
    //     let semaphore = Arc::new(tokio::sync::Semaphore::new(3)); // Max 3 concurrent downloads
    //     let mut handles = Vec::new();

    //     let segment_count = segments.len();
    //     for segment in segments {
    //         let segment_clone = segment.clone();
    //         let client = self.clone_for_async();
    //         let permit = semaphore.clone();

    //         let handle = tokio::spawn(async move {
    //             let _permit = permit.acquire().await.unwrap();
    //             let message_id = segment_clone.message_id.clone();

    //             match client.download_segment(&segment_clone).await {
    //                 Ok(data) => Some((message_id, data)),
    //                 Err(e) => {
    //                     warn!("Failed to download segment {}: {}", message_id, e);
    //                     None
    //                 }
    //             }
    //         });

    //         handles.push(handle);
    //     }

    //     // Collect results
    //     for handle in handles {
    //         if let Ok(Some((message_id, data))) = handle.await {
    //             results.insert(message_id, data);
    //         }
    //     }

    //     debug!("Downloaded {}/{} segments", results.len(), segment_count);
    //     results
    // }

    // /// Download only PAR2 files from a list of files
    // pub async fn download_par2_files(&self, files: &[File]) -> HashMap<String, Vec<u8>> {
    //     let par2_files: Vec<&File> = files
    //         .iter()
    //         .filter(|f| f.subject.to_lowercase().contains(".par2"))
    //         .collect();

    //     debug!("Downloading {} PAR2 files", par2_files.len());
    //     let mut results = HashMap::new();

    //     let par2_count = par2_files.len();
    //     for file in par2_files {
    //         match self.get_file((*file).clone()).await {
    //             Ok(data) => {
    //                 let data_len = data.len();
    //                 results.insert(file.subject.clone(), data);
    //                 debug!("Downloaded PAR2: {} ({} bytes)", file.subject, data_len);
    //             }
    //             Err(e) => {
    //                 warn!("Failed to download PAR2 {}: {}", file.subject, e);
    //             }
    //         }
    //     }

    //     debug!("Downloaded {}/{} PAR2 files", results.len(), par2_count);
    //     results
    // }
}