use std::path::Path;

use crate::nntp::config::DownloadStatus;
use crate::nntp::pool::ConnectionPool;
use crate::nntp::NntpClient;
use crate::nntp::{config::NntpConfig, error::NntpError};
use async_trait::async_trait;
use backoff::ExponentialBackoff;
use nzb_rs::{File, Segment};
use rek2_nntp::body;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::{debug, info};

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

    async fn download_file(&self, file: &File, output_path: &Path) -> Result<Vec<u8>, NntpError> {
        let mut data = Vec::new();
        let total_segments = file.segments.len();

        info!(
            "Downloading file: {} ({} segments)",
            file.subject, total_segments
        );

        // Download segments in order
        for (idx, segment) in file.segments.iter().enumerate() {
            debug!(
                "Downloading segment {}/{}: {}",
                idx + 1,
                total_segments,
                segment.message_id
            );

            let segment_data = self.download_segment(segment).await?;
            data.extend_from_slice(&segment_data);

            // Progress update
            if (idx + 1) % 10 == 0 || idx + 1 == total_segments {
                debug!("Progress: {}/{} segments", idx + 1, total_segments);
            }
        }

        // Write to file
        let mut output = fs::File::create(output_path).await.unwrap();
        output.write_all(&data).await.unwrap();
        output.flush().await.unwrap();

        info!(
            "Downloaded {} to {:?} ({} bytes)",
            file.subject,
            output_path,
            data.len()
        );
        Ok(data)
    }

    async fn download_files_with_throttle(
        &self,
        files: Vec<File>,
        output_dir: &Path,
        status: Arc<Mutex<DownloadStatus>>,
    ) -> Result<HashMap<String, PathBuf>, NntpError> {
        let mut results = HashMap::new();
        let total_files = files.len();

        for (idx, file) in files.into_iter().enumerate() {
            // Update status
            {
                let mut status = status.lock().await;
                status.current_file = Some(file.subject.clone());
                status.downloaded_files = idx;
            }

            // Determine output path
            let filename = extract_filename(&file.subject);
            let output_path = output_dir.join(&filename);
            let data = self.download_file(&file, &output_path).await?;

            results.insert(filename.clone(), output_path.clone());

            info!(
                "Downloaded {}/{}: {} ({} bytes)",
                idx + 1,
                total_files,
                filename,
                data.len()
            );
        }

        // Mark as complete
        {
            let mut status = status.lock().await;
            status.is_complete = true;
        }

        Ok(results)
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

fn extract_filename(subject: &str) -> String {
    // Extract filename from subject line
    // Example: "filename.rar" yEnc (1/10)
    subject
        .split('"')
        .nth(1)
        .or_else(|| subject.split(' ').next())
        .unwrap_or(subject)
        .to_string()
}
