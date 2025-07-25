use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::par2::parser::FilePar2Info;

const SIXTEEN_KB: usize = 16384;

pub struct Par2Matcher {
    par2_file: PathBuf,
}

impl Par2Matcher {
    pub fn compute_hash16k(path: &Path) -> io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        let mut buffer = vec![0u8; SIXTEEN_KB];

        let bytes_read = file.read(&mut buffer)?;
        buffer.truncate(bytes_read);

        let mut hasher = Md5::new();
        hasher.update(&buffer);
        Ok(hasher.finalize().to_vec())
    }

    pub fn match_files_to_par2(
        directory: &Path,
        par2_info: &HashMap<String, FilePar2Info>,
    ) -> io::Result<HashMap<Vec<u8>, PathBuf>> {
        let mut hash_to_path = HashMap::new();
        let mut hash_to_expected_name: HashMap<Vec<u8>, String> = HashMap::new();
        for (filename, info) in par2_info {
            hash_to_expected_name.insert(info.hash16k.clone(), filename.clone());
        }

        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();

            // Skip directories and PAR2 files
            if path.is_dir() || path.extension().is_some_and(|ext| ext == "par2") {
                continue;
            }

            // Compute hash16k for this file
            match Self::compute_hash16k(&path) {
                Ok(hash) => {
                    // Check if this hash matches any PAR2 entry
                    if hash_to_expected_name.contains_key(&hash) {
                        hash_to_path.insert(hash.clone(), path.clone());

                        let expected_name = &hash_to_expected_name[&hash];
                        let actual_name = path.file_name().unwrap().to_string_lossy();

                        if expected_name == &actual_name {
                            println!("✓ {actual_name} matches (correct name)");
                        } else {
                            println!(
                                "✓ {actual_name} matches hash for '{expected_name}' (needs rename)"
                            );
                        }
                    }
                }
                Err(e) => eprintln!("Error reading {}: {}", path.display(), e),
            }
        }

        // Report missing files
        for (hash, expected_name) in &hash_to_expected_name {
            if !hash_to_path.contains_key(hash) {
                println!("✗ {expected_name} not found (hash: {hash:?})");
            }
        }

        Ok(hash_to_path)
    }

    pub fn rename_files_to_match_par2(
        directory: &Path,
        par2_info: &HashMap<String, FilePar2Info>,
        dry_run: bool,
    ) -> io::Result<()> {
        // Create hash -> (expected_name, filesize) lookup
        let mut hash_to_info: HashMap<Vec<u8>, (&str, u64)> = HashMap::new();
        for (filename, info) in par2_info {
            hash_to_info.insert(info.hash16k.clone(), (filename.as_str(), info.filesize));
        }

        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() || path.extension().is_some_and(|ext| ext == "par2") {
                continue;
            }

            // Get file size for additional validation
            let metadata = path.metadata()?;
            let file_size = metadata.len();

            if let Ok(hash) = Self::compute_hash16k(&path) {
                if let Some((expected_name, expected_size)) = hash_to_info.get(&hash) {
                    let current_name = path.file_name().unwrap().to_string_lossy();

                    if current_name != *expected_name {
                        // Verify size matches too for extra safety
                        if file_size == *expected_size {
                            let new_path = directory.join(expected_name);

                            if dry_run {
                                println!("Would rename: {current_name} -> {expected_name}");
                            } else {
                                std::fs::rename(&path, &new_path)?;
                                println!("Renamed: {current_name} -> {expected_name}");
                            }
                        } else {
                            println!("Warning: {current_name} matches hash but size differs ({file_size}b vs {expected_size}b expected)");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    // Convenience function to find a single file by hash
    pub fn find_file_by_hash16k(
        directory: &Path,
        target_hash: &[u8],
    ) -> io::Result<Option<PathBuf>> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                continue;
            }

            if let Ok(hash) = Self::compute_hash16k(&path) {
                if hash == target_hash {
                    return Ok(Some(path));
                }
            }
        }

        Ok(None)
    }
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

        match Par2Matcher::find_file_by_hash16k(directory, &target_hash) {
            Ok(Some(path)) => println!("Found file: {}", path.display()),
            Ok(None) => println!("No file found with that hash"),
            Err(e) => eprintln!("Error: {e}"),
        }
    }
}
