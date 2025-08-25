use bytes::Bytes;
use derive_more::Constructor;
use futures::StreamExt;
use itertools::Itertools;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    archive::{
        error::ArchiveError,
        rar::{RarExt, analyse_rar_buffer},
    },
    nntp::yenc::extract_filename,
    scheduler::adaptive::FirstSegment,
};

#[derive(Debug, Constructor)]
pub struct Par2Manifest {
    pub files: HashMap<String, FileInfo>,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub real_filename: String,
    pub hash16k: Bytes,
}

#[derive(Debug, Clone, Constructor)]
pub struct DownloadTask {
    path: PathBuf,
    nzb: nzb_rs::File,
    length: u64,
    offset: u64,
    bytes: Bytes,
}

impl DownloadTask {
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn nzb(&self) -> &nzb_rs::File {
        &self.nzb
    }

    pub fn length(&self) -> &u64 {
        &self.length
    }

    pub fn offset(&self) -> &u64 {
        &self.offset
    }

    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }
}

impl Par2Manifest {
    pub fn hash_to_filename(&self) -> HashMap<Bytes, &str> {
        self.files
            .values()
            .map(|info| (info.hash16k.clone(), info.real_filename.as_str()))
            .collect()
    }

    pub fn find_missing_files(&self, hashes: &[Bytes]) -> Vec<String> {
        let downloaded: HashSet<_> = hashes.iter().collect();

        self.files
            .values()
            .filter(|info| !downloaded.contains(&info.hash16k))
            .map(|info| info.real_filename.clone())
            .collect()
    }
}

pub async fn create_download_tasks(
    hash_to_real: HashMap<Bytes, &str>,
    downloads: &[FirstSegment],
    session_dir: &Path,
) -> Result<Vec<DownloadTask>, ArchiveError> {
    let mut tasks = Vec::new();
    for segment in downloads {
        let real_name = hash_to_real
            .get(&segment.hash16k)
            .ok_or(ArchiveError::FilenameNotFound(segment.nzb.subject.clone()))?;
        let path = session_dir.join(real_name);
        let is_first = RarExt::from_filename(&path).is_some_and(|ext| ext == RarExt::Main);
        let (offset, length) = analyse_rar_buffer(&segment.bytes, is_first).await?;
        let data = segment.bytes.slice(offset as usize..);
        tokio::fs::write(&path, &data).await?;

        tasks.push(DownloadTask::new(
            path,
            segment.nzb.clone(), // TODO: don't clone
            length,
            offset,
            data,
        ));
    }

    let tasks: Vec<_> = tasks
        .into_iter()
        .sorted_by_key(|task| RarExt::from_filename(task.path()).unwrap())
        .collect();

    println!(
        "tasks: {:?}",
        tasks.iter().map(|t| t.path.clone()).collect::<Vec<_>>()
    );
    Ok(tasks)
}

pub async fn create_download_tasks_plain(
    downloads: &[FirstSegment],
    session_dir: &Path,
) -> Result<Vec<DownloadTask>, ArchiveError> {
    let mut tasks = Vec::new();
    for segment in downloads {
        let filename = extract_filename(&segment.nzb.subject)
            .ok_or(ArchiveError::FilenameNotFound(segment.nzb.subject.clone()))?;
        let path = session_dir.join(filename);
        let is_first = RarExt::from_filename(&path).is_some_and(|ext| ext == RarExt::Main);
        let (offset, length) = analyse_rar_buffer(&segment.bytes, is_first).await?;
        let data = segment.bytes.clone(); //.slice(offset as usize..);
        //tokio::fs::write(&path, &data).await?;

        tasks.push(DownloadTask::new(
            path,
            segment.nzb.clone(), // TODO: don't clone
            length,
            offset,
            data,
        ));
    }

    let tasks: Vec<_> = tasks
        .into_iter()
        .sorted_by_key(|task| RarExt::from_filename(task.path()).unwrap())
        .collect();

    Ok(tasks)
}
