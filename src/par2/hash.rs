use md5::{Digest, Md5};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const SIXTEEN_KB: usize = 16384;

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

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::par2::parse_file;

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

        let par2 = parse_file(&directory.join("db5839e271decea10e9e713aa0e20573.par2")).unwrap();
        //let matches = match_files_to_par2(directory, &par2).unwrap();

        //println!("{matches:#?}");
    }

    #[test]
    fn test_find_file_extended() {
        let directory = Path::new("/tmp/binzb/c312fa70-1b42-4cab-9372-723c89db0c46");

        let parsed = parse_file(&directory.join("0ae5e6761c69051d401054734c174e91.par2")).unwrap();
        println!("parsed: {parsed:?}");
        let target_hash = &parsed.files.get("yay.rar").unwrap().hash16k;

        match find_file_by_hash16k(directory, target_hash) {
            Ok(Some(path)) => println!("Found file: {}", path.display()),
            Ok(None) => println!("No file found with that hash"),
            Err(e) => eprintln!("Error: {e}"),
        }
    }
}
