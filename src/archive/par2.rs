use bytes::Bytes;
use derive_more::Constructor;
use itertools::Itertools;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

use crate::{archive::rar::RarExt, scheduler::adaptive::FirstSegment};

#[derive(Debug)]
pub struct Par2Manifest {
    pub files: HashMap<String, FileInfo>,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub real_filename: String,
    pub hash16k: Bytes,
}

#[derive(Debug)]
pub enum ArchiveType {
    Plain,
    Obfuscated,
}

#[derive(Debug, Constructor)]
pub struct DownloadTask {
    path: PathBuf,
    nzb: nzb_rs::File,
    archive_type: ArchiveType,
}

impl DownloadTask {
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn nzb(&self) -> &nzb_rs::File {
        &self.nzb
    }

    pub fn start_segment(&self) -> usize {
        match self.archive_type {
            ArchiveType::Plain => 0,
            ArchiveType::Obfuscated => 1,
        }
    }
}

impl Par2Manifest {
    pub fn new(files: HashMap<String, FileInfo>) -> Self {
        Self { files }
    }

    pub fn create_download_tasks(&self, downloads: &[FirstSegment]) -> Vec<DownloadTask> {
        let hash_to_real: HashMap<_, _> = self
            .files
            .values()
            .map(|info| (info.hash16k.clone(), info.real_filename.as_str()))
            .collect();

        downloads
            .iter()
            .filter_map(|segment| {
                let real_name = hash_to_real.get(&segment.hash16k)?;
                let obfuscated = &segment.path;
                let path = obfuscated.parent()?.join(real_name);

                match fs::rename(obfuscated, &path) {
                    // TODO: side effect not clear
                    Ok(_) => Some(DownloadTask::new(
                        path,
                        segment.nzb.clone(),
                        ArchiveType::Obfuscated,
                    )), // TODO: don't clone
                    Err(e) => {
                        eprintln!("Failed to rename {obfuscated:?} → {path:?}: {e}");
                        None
                    }
                }
            })
            .sorted_by_key(|task| RarExt::from_filename(task.path()).unwrap())
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
