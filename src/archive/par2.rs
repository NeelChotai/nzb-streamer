use bytes::Bytes;
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
pub enum DownloadTask {
    New {
        path: PathBuf,
        nzb: nzb_rs::File,
    },
    FromFirstSegment {
        path: PathBuf,
        nzb: nzb_rs::File,
        data: Bytes,
    },
}

impl DownloadTask {
    pub fn path(&self) -> &PathBuf {
        match self {
            DownloadTask::New { path, .. } => path,
            DownloadTask::FromFirstSegment { path, .. } => path,
        }
    }

    pub fn nzb(&self) -> &nzb_rs::File {
        match self {
            DownloadTask::New { nzb, .. } => nzb,
            DownloadTask::FromFirstSegment { nzb, .. } => nzb,
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
                    Ok(_) => Some(DownloadTask::FromFirstSegment {
                        path,
                        nzb: segment.nzb.clone(),
                        data: segment.bytes.clone(),
                    }), // TODO: don't clone
                    Err(e) => {
                        eprintln!("Failed to rename {obfuscated:?} → {path:?}: {e}");
                        None
                    }
                }
            })
            .sorted_by_key(|task| {
                RarExt::from_filename(task.path().file_name().unwrap().to_str().unwrap()).unwrap()
            })
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
