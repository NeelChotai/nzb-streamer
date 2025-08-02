use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::par2::parser::FilePar2Info;

const SIXTEEN_KB: usize = 16384;

#[derive(Debug, Clone)]
pub struct FileMatch {
    pub path: PathBuf,
    pub hash16k: Vec<u8>,
    pub size: u64,
    pub real_name: Option<String>,
}

#[derive(Debug)]
pub struct MatchResult {
    pub matched: Vec<FileMatch>,
    pub missing: Vec<(String, Vec<u8>)>, // (filename, hash16k)
}

pub fn compute_hash16k_from_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; SIXTEEN_KB];
    let bytes_read = file.read(&mut buffer)?;
    buffer.truncate(bytes_read);

    Ok(compute_hash16k_from_bytes(&buffer))
}

pub fn compute_hash16k_from_bytes(bytes: &[u8]) -> Vec<u8> {
    let len = bytes.len().min(SIXTEEN_KB);

    Md5::new().chain_update(&bytes[..len]).finalize().to_vec()
}

pub fn scan_directory(directory: &Path) -> io::Result<Vec<(PathBuf, Vec<u8>, u64)>> {
    let mut files = Vec::new();

    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() || path.extension().is_some_and(|ext| ext == "par2") {
            continue;
        }

        match compute_hash16k_from_file(&path) {
            Ok(hash) => {
                let size = entry.metadata()?.len();
                files.push((path, hash, size));
            }
            Err(e) => eprintln!("Error reading {}: {}", path.display(), e),
        }
    }

    Ok(files)
}

pub fn match_files_to_par2(
    directory: &Path,
    par2_info: &HashMap<String, FilePar2Info>,
) -> io::Result<MatchResult> {
    let mut hash_to_real: HashMap<Vec<u8>, (&str, u64)> = HashMap::new();
    for (filename, info) in par2_info {
        hash_to_real.insert(info.hash16k.clone(), (filename.as_str(), info.filesize));
    }

    let scanned_files = scan_directory(directory)?;
    let mut matched = Vec::new();
    let mut found_hashes = HashMap::new();

    for (path, hash, size) in scanned_files {
        if let Some((real_name, _)) = hash_to_real.get(&hash) {
            found_hashes.insert(hash.clone(), ());
            matched.push(FileMatch {
                path,
                hash16k: hash,
                size,
                real_name: Some(real_name.to_string()),
            });
        }
    }

    let missing: Vec<_> = par2_info
        .iter()
        .filter(|(_, info)| !found_hashes.contains_key(&info.hash16k))
        .map(|(name, info)| {
            println!("{name} not found");
            (name.clone(), info.hash16k.clone())
        })
        .collect();

    Ok(MatchResult {
        matched,
        missing,
    })
}

#[cfg(test)]
mod tests {
    use crate::par2::{self, parser::parse_par2_file};

    use super::*;

    fn find_file_by_hash16k(directory: &Path, target_hash: &[u8]) -> io::Result<Option<PathBuf>> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                continue;
            }

            if let Ok(hash) = compute_hash16k_from_file(&path) {
                if hash == target_hash {
                    return Ok(Some(path));
                }
            }
        }

        Ok(None)
    }

    #[test]
    fn test_find_file() {
        let target_hash = vec![
            174, 93, 175, 166, 105, 86, 69, 252, 44, 193, 189, 17, 136, 199, 90, 69,
        ];
        let directory = Path::new("/tmp/downloaded/");

        match find_file_by_hash16k(directory, &target_hash) {
            Ok(Some(path)) => println!("Found file: {}", path.display()),
            Ok(None) => println!("No file found with that hash"),
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    #[test]
    fn test_find_and_match() {
        let directory = Path::new("/tmp/downloaded/");

        let par2 =
            parse_par2_file(&directory.join("db5839e271decea10e9e713aa0e20573.par2")).unwrap();
        let matches = match_files_to_par2(directory, &par2).unwrap();

        println!("{matches:#?}");
    }

    #[test]
    fn test_find_file_extended() {
        let directory = Path::new("/tmp/binzb/c312fa70-1b42-4cab-9372-723c89db0c46");

        let parsed =
            par2::parser::parse_par2_file(&directory.join("0ae5e6761c69051d401054734c174e91.par2"))
                .unwrap();
        println!("parsed: {parsed:?}");
        let target_hash = &parsed.get("yay.rar").unwrap().hash16k;

        match find_file_by_hash16k(directory, target_hash) {
            Ok(Some(path)) => println!("Found file: {}", path.display()),
            Ok(None) => println!("No file found with that hash"),
            Err(e) => eprintln!("Error: {e}"),
        }
    }
}
