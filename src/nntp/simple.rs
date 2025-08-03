use nzb_rs::{File as NzbFile, Segment};
use rek2_nntp::{authenticate, body_bytes, quit};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tracing::{debug, error, info};

use crate::nntp::config::NntpConfig;
use crate::nntp::error::NntpError;
use crate::par2::hash::compute_hash16k_from_bytes;

pub struct SimpleNntpClient {
    config: NntpConfig,
    semaphore: Arc<Semaphore>,
}

pub struct FirstSegment {
    pub path: PathBuf,
    pub nzb: nzb_rs::File,
    pub hash16k: Vec<u8>,
}

impl SimpleNntpClient {
    pub fn new(config: NntpConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_connections.unwrap_or(40)));
        Self { config, semaphore }
    }

    pub async fn download_first_segment_to_file(
        &self,
        file: &NzbFile,
        output_dir: &Path,
    ) -> Result<(PathBuf, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
        if file.segments.is_empty() {
            return Err("No segments in file".into());
        }

        let first_segment = &file.segments[0];
        let data = self.download_segment(first_segment).await?;
        let hash16k = compute_hash16k_from_bytes(&data);

        let filename = extract_filename(&file.subject);
        let output_path = output_dir.join(&filename);

        tokio::fs::write(&output_path, &data).await?;
        debug!("Saved first segment to: {}", output_path.display());

        Ok((output_path, hash16k))
    }

    // Download first segments of multiple files concurrently and save them
    pub async fn download_first_segments_to_files(
        &self,
        files: &[NzbFile],
        output_dir: &Path,
    ) -> Result<Vec<FirstSegment>, Box<dyn std::error::Error + Send + Sync>> {
        info!("Downloading first segments of {} files", files.len());

        let mut handles = Vec::new();

        for file in files {
            let client = self.clone();
            let nzb = file.clone();
            let output_dir = output_dir.to_path_buf();

            let handle = tokio::spawn(async move {
                match client
                    .download_first_segment_to_file(&nzb, &output_dir)
                    .await
                {
                    Ok((path, hash16k)) => Ok(FirstSegment { path, nzb, hash16k }),
                    Err(e) => {
                        error!("Failed to download first segment: {}", e);
                        Err(e)
                    }
                }
            });

            handles.push(handle);
        }

        // Collect results
        let mut results = Vec::new();
        for handle in handles {
            if let Ok(Ok(result)) = handle.await {
                results.push(result);
            }
        }

        Ok(results)
    }

    // Download remaining segments of a partially downloaded file
    pub async fn download_remaining_segments(
        &self,
        file: &NzbFile,
        existing_path: &Path,
        real_path: &Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // rename to deobfuscated name
        tokio::fs::rename(existing_path, real_path).await?;

        // Read existing data
        let mut output = OpenOptions::new()
            .append(true)
            .open(real_path)
            .await
            .unwrap();

        for segment in file.segments.iter().skip(1) {
            let data = self.download_segment(segment).await?;

            output.write_all(&data).await.unwrap();
            output.flush().await.unwrap();
        }

        info!("Completed download of {}", existing_path.display());

        Ok(())
    }

    // Download a single segment
    async fn download_segment(
        &self,
        segment: &Segment,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        // Acquire permit
        let _permit = self.semaphore.acquire().await?;

        // Create new connection
        let mut conn = authenticate(
            &self.config.host,
            &self.config.username,
            &self.config.password,
        )
        .await
        .map_err(|e| NntpError::ClientAuthentication(e.to_string()))?;

        // Download body
        let message_id = &segment.message_id;
        let bytes = body_bytes(&mut conn, &format!("<{message_id}>"))
            .await
            .map_err(|e| NntpError::BodyRead(e.to_string()))?;
        quit(&mut conn).await.unwrap();
        let extracted = extract_yenc_data(&bytes);
        let decoded = yenc::decode_buffer(&extracted).unwrap();

        debug!(
            "Downloaded segment {} ({} bytes)",
            segment.message_id,
            bytes.len()
        );

        Ok(decoded)
    }
}

impl Clone for SimpleNntpClient {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            semaphore: self.semaphore.clone(),
        }
    }
}

fn extract_filename(subject: &str) -> String {
    // Extract filename from subject: "filename.rar" yEnc (1/1)
    if let Some(start) = subject.find('"') {
        if let Some(end) = subject[start + 1..].find('"') {
            return subject[start + 1..start + 1 + end].to_string();
        }
    }
    // Fallback: use first word
    subject
        .split_whitespace()
        .next()
        .unwrap_or("unknown")
        .to_string()
}

fn extract_yenc_data(article: &[u8]) -> Vec<u8> {
    let mut in_body = false;
    article
        .split(|&b| b == b'\n')
        .filter_map(|line| match line {
            _ if line.starts_with(b"=ybegin") => {
                in_body = true;
                None
            }
            _ if line.starts_with(b"=ypart") => {
                in_body = true;
                None
            }
            _ if line.starts_with(b"=yend") => {
                in_body = false;
                None
            }
            _ if in_body => Some(trim_line_endings(line)),
            _ => None,
        })
        .flatten()
        .copied()
        .collect()
}

fn trim_line_endings(line: &[u8]) -> &[u8] {
    match line {
        [.., b'\r', b'\n'] => &line[..line.len() - 2],
        [.., b'\n'] => &line[..line.len() - 1],
        _ => line,
    }
}
