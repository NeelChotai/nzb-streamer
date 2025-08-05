use bytes::Bytes;
use dashmap::DashMap;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tracing::{error, info};

#[derive(Clone)]
pub struct FileWriterPool {
    writers: Arc<DashMap<PathBuf, mpsc::Sender<(usize, Bytes)>>>,
}

impl Default for FileWriterPool {
    fn default() -> Self {
        Self::new()
    }
}

impl FileWriterPool {
    pub fn new() -> Self {
        Self {
            writers: Arc::new(DashMap::new()),
        }
    }

    pub async fn add_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let (tx, rx) = mpsc::channel(100);
        self.writers.insert(path.to_owned(), tx);

        let path = path.to_owned();
        tokio::spawn(async move {
            if let Err(e) = ordered_file_writer(path, rx).await {
                error!("File writer error: {}", e);
            }
        });

        Ok(())
    }

    pub async fn write(
        &self,
        path: &Path,
        index: usize,
        data: Bytes,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.writers
            .get(path)
            .ok_or("No writer for path")?
            .send((index, data))
            .await?;
        Ok(())
    }

    pub async fn close_all(&self) {
        self.writers.clear();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.writers
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }
}

async fn ordered_file_writer(
    path: PathBuf,
    mut rx: mpsc::Receiver<(usize, Bytes)>,
) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;

    let mut expected = 1; // Skip segment 0
    let mut pending: BTreeMap<usize, Bytes> = BTreeMap::new();

    while let Some((index, data)) = rx.recv().await {
        if index == expected {
            file.write_all(&data).await?;
            expected += 1;

            while let Some(data) = pending.remove(&expected) {
                file.write_all(&data).await?;
                expected += 1;
            }

            if expected % 10 == 0 {
                file.flush().await?;
            }
        } else {
            pending.insert(index, data);
        }
    }

    file.flush().await?;
    info!("Completed writing {}", path.display());
    Ok(())
}
