use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::par2::parser::FilePar2Info;

const SIXTEEN_KB: usize = 16384;

/// Information about a file found in the directory
#[derive(Debug, Clone)]
pub struct FileMatch {
    pub path: PathBuf,
    pub hash16k: Vec<u8>,
    pub size: u64,
    pub expected_name: Option<String>,
}

/// Result of matching files against PAR2 info
#[derive(Debug)]
pub struct MatchResult {
    pub matched_files: Vec<FileMatch>,
    pub missing_files: Vec<(String, Vec<u8>)>, // (filename, hash16k)
}

/// Compute MD5 hash of first 16KB of a file
pub fn compute_hash16k(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; SIXTEEN_KB];

    let bytes_read = file.read(&mut buffer)?;
    buffer.truncate(bytes_read);

    let mut hasher = Md5::new();
    hasher.update(&buffer);
    Ok(hasher.finalize().to_vec())
}

/// Scan directory and compute hash16k for all non-PAR2 files
pub fn scan_directory(directory: &Path) -> io::Result<Vec<(PathBuf, Vec<u8>, u64)>> {
    let mut files = Vec::new();
    
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();

        // Skip directories and PAR2 files
        if path.is_dir() || path.extension().is_some_and(|ext| ext == "par2") {
            continue;
        }

        match compute_hash16k(&path) {
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
    // Build lookup tables
    let mut hash_to_expected: HashMap<Vec<u8>, (&str, u64)> = HashMap::new();
    for (filename, info) in par2_info {
        hash_to_expected.insert(info.hash16k.clone(), (filename.as_str(), info.filesize));
    }
    
    // Scan directory once
    let scanned_files = scan_directory(directory)?;
    
    // Match files
    let mut matched_files = Vec::new();
    let mut found_hashes = HashMap::new();
    
    for (path, hash, size) in scanned_files {
        if let Some((expected_name, expected_size)) = hash_to_expected.get(&hash) {
            let current_name = path.file_name().unwrap().to_string_lossy();
            
            // Verify size matches for extra validation
            if size != *expected_size {
                eprintln!(
                    "Warning: {current_name} matches hash but size differs ({size}b vs {expected_size}b expected)"
                );
                continue;
            }
            
            found_hashes.insert(hash.clone(), ());
            
            let needs_rename = current_name != *expected_name;
            if needs_rename {
                println!("✓ {current_name} matches hash for '{expected_name}' (needs rename)");
            } else {
                println!("✓ {current_name} matches (correct name)");
            }
            
            matched_files.push(FileMatch {
                path,
                hash16k: hash,
                size,
                expected_name: Some(expected_name.to_string()),
            });
        }
    }
    
    // Find missing files
    let missing_files: Vec<_> = par2_info
        .iter()
        .filter(|(_, info)| !found_hashes.contains_key(&info.hash16k))
        .map(|(name, info)| {
            println!("✗ {name} not found");
            (name.clone(), info.hash16k.clone())
        })
        .collect();
    
    Ok(MatchResult {
        matched_files,
        missing_files,
    })
}

pub fn rename_files_to_match_par2(
    directory: &Path,
    par2_info: &HashMap<String, FilePar2Info>,
    dry_run: bool,
) -> io::Result<u32> {
    let match_result = match_files_to_par2(directory, par2_info)?;
    let mut rename_count = 0;
    
    for file_match in match_result.matched_files {
        if let Some(expected_name) = file_match.expected_name {
            let current_name = file_match.path.file_name().unwrap().to_string_lossy();
            
            if current_name != expected_name {
                let new_path = directory.join(&expected_name);
                
                if dry_run {
                    println!("Would rename: {current_name} -> {expected_name}");
                } else {
                    // Check if target exists
                    if new_path.exists() {
                        eprintln!("Error: Target file {expected_name} already exists");
                        continue;
                    }
                    
                    fs::rename(&file_match.path, &new_path)?;
                    println!("Renamed: {current_name} -> {expected_name}");
                }
                rename_count += 1;
            }
        }
    }
    
    Ok(rename_count)
}

pub fn find_file_by_hash16k(
    directory: &Path,
    target_hash: &[u8],
) -> io::Result<Option<PathBuf>> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            continue;
        }

        if let Ok(hash) = compute_hash16k(&path) {
            if hash == target_hash {
                return Ok(Some(path));
            }
        }
    }

    Ok(None)
}

pub fn create_par2_to_actual_mapping(
    directory: &Path,
    par2_info: &HashMap<String, FilePar2Info>,
) -> io::Result<HashMap<String, PathBuf>> {
    let match_result = match_files_to_par2(directory, par2_info)?;
    
    let mut mapping = HashMap::new();
    for file_match in match_result.matched_files {
        if let Some(expected_name) = file_match.expected_name {
            mapping.insert(expected_name, file_match.path);
        }
    }
    
    Ok(mapping)
}

// Example usage
#[cfg(test)]
mod tests {
    use super::*;

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
}
