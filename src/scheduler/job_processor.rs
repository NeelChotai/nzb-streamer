// scheduler/job_processor.rs
use bytes::Bytes;
use futures::{StreamExt, stream};
use memmap2::MmapMut;
use ordered_stream::OrderedStreamExt;
use parking_lot::RwLock;
use std::sync::Arc;
use tracing::{debug, error};

use crate::nntp::client::NntpClient;
use crate::scheduler::batch::Job;
use crate::scheduler::error::SchedulerError;

pub async fn process_job(
    job: Job,
    client: Arc<NntpClient>,
    mmap: Arc<RwLock<MmapMut>>,
    segment_parallelism: usize,
) -> Result<(), SchedulerError> {
    let segments = job.task.nzb().segments.iter().cloned();

    debug!(
        "Processing job at offset {} with {} segments",
        job.offset,
        segments.len()
    );

    let mut downloads = stream::iter(segments)
        .map(|segment| {
            let client = client.clone();
            async move { client.download(&segment).await }
        })
        .buffered(segment_parallelism);

    let mut write_offset = job.offset;
    while let Some(download) = downloads.next().await {
        match download {
            Ok(data) => {
                write_to_mmap(&mmap, write_offset, &data);
                write_offset += data.len() as u64;

                debug!(
                    "Wrote segment at offset {}",
                    write_offset - data.len() as u64
                );
            }
            Err(e) => error!("Encountered error during download: {}", e),
        }
    }

    Ok(())
}

fn write_to_mmap(mmap: &Arc<RwLock<MmapMut>>, offset: u64, data: &Bytes) {
    let mut guard = mmap.write();
    let start = offset as usize;
    let end = (start + data.len()).min(guard.len());

    if end > start {
        guard[start..end].copy_from_slice(&data[..end - start]);
    }
}
