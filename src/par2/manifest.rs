use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use crate::{nntp::simple::FirstSegment, par2::rar::RarExt};

// par2/manifest.rs
#[derive(Debug)]
pub struct Par2Manifest {
    pub files: HashMap<String, FileInfo>,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub real_filename: String,
    pub hash16k: Vec<u8>,
}

#[derive(Debug)]
pub struct DownloadTask {
    pub nzb: nzb_rs::File,
    pub obfuscated_path: PathBuf,
    pub real_name: String,
}

impl Par2Manifest {
    pub fn new(files: HashMap<String, FileInfo>) -> Self {
        Self { files }
    }

    pub fn create_download_tasks(&self, downloads: &[FirstSegment]) -> Vec<DownloadTask> {
        let hash_to_real: HashMap<_, _> = self
            .files
            .values()
            .map(|info| (info.hash16k.as_slice(), info.real_filename.as_str()))
            .collect();

        let mut linked: Vec<_> = downloads
            .iter()
            .filter_map(|segment| {
                hash_to_real
                    .get(segment.hash16k.as_slice())
                    .filter(|&&name| RarExt::from_filename(name).is_some())
                    .map(|&name| DownloadTask {
                        nzb: segment.nzb.clone(),
                        obfuscated_path: segment.path.clone(),
                        real_name: name.to_string(),
                    })
            })
            .collect();

        linked.sort_by_key(|task| RarExt::from_filename(&task.real_name).unwrap());

        linked
    }

    pub fn find_missing_files(&self, hashes: &[Vec<u8>]) -> Vec<String> {
        let downloaded: HashSet<_> = hashes.iter().map(|h| h.as_slice()).collect();

        self.files
            .values()
            .filter(|info| !downloaded.contains(info.hash16k.as_slice()))
            .map(|info| info.real_filename.clone())
            .collect()
    }
}
