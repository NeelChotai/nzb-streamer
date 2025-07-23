use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory,
    ReplyEntry, Request,
};
use md5::{Digest, Md5};

use crate::par2::{parse_par2_file, FilePar2Info};

pub mod copy;

const TTL: Duration = Duration::from_secs(1);

#[derive(Debug)]
struct FileMapping {
    real_name: String,
    obfuscated_files: Vec<PathBuf>,
    total_size: u64,
    part_sizes: Vec<u64>,
}

pub struct MultiPartRarVfs {
    base_dir: PathBuf,
    file_mappings: HashMap<String, FileMapping>, // main filename -> mapping
    inodes: HashMap<u64, String>, // inode -> main filename
    next_inode: u64,
}

impl MultiPartRarVfs {
    pub fn new(base_dir: PathBuf, par2_path: &Path) -> io::Result<Self> {
        let par2_info = parse_par2_file(par2_path)?;
        
        // Build hash -> real filename mapping
        let mut hash_to_real: HashMap<Vec<u8>, (String, u64)> = HashMap::new();
        for (filename, info) in &par2_info {
            hash_to_real.insert(info.hash16k.clone(), (filename.clone(), info.filesize));
        }
        
        // Scan directory and map obfuscated files
        let mut obfuscated_to_real: HashMap<PathBuf, (String, u64)> = HashMap::new();
        for entry in std::fs::read_dir(&base_dir)? {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().map_or(false, |ext| ext == "par2") {
                continue;
            }
            
            if let Ok(hash) = compute_hash16k(&path) {
                if let Some((real_name, size)) = hash_to_real.get(&hash) {
                    obfuscated_to_real.insert(path, (real_name.clone(), *size));
                }
            }
        }
        
        // Group files by their base name (without .rar, .r00, etc)
        let mut file_groups: HashMap<String, Vec<(PathBuf, String, u64)>> = HashMap::new();
        for (obfuscated_path, (real_name, size)) in obfuscated_to_real {
            let base_name = if real_name.ends_with(".rar") {
                real_name.trim_end_matches(".rar").to_string()
            } else if let Some(pos) = real_name.rfind(".r") {
                real_name[..pos].to_string()
            } else {
                continue; // Not a RAR file
            };
            
            file_groups.entry(base_name).or_default().push((obfuscated_path, real_name, size));
        }
        
        // Create file mappings
        let mut file_mappings = HashMap::new();
        let mut inodes = HashMap::new();
        let mut next_inode = 2;
        
        for (base_name, mut files) in file_groups {
            // Sort files by their extension (rar, r00, r01, etc)
            files.sort_by(|a, b| {
                let ext_a = get_rar_part_number(&a.1);
                let ext_b = get_rar_part_number(&b.1);
                ext_a.cmp(&ext_b)
            });
            
            let main_filename = format!("{}.rar", base_name);
            let obfuscated_files: Vec<PathBuf> = files.iter().map(|(p, _, _)| p.clone()).collect();
            let part_sizes: Vec<u64> = files.iter().map(|(_, _, s)| *s).collect();
            let total_size: u64 = part_sizes.iter().sum();
            
            let mapping = FileMapping {
                real_name: main_filename.clone(),
                obfuscated_files,
                total_size,
                part_sizes,
            };
            
            file_mappings.insert(main_filename.clone(), mapping);
            inodes.insert(next_inode, main_filename);
            next_inode += 1;
        }
        
        Ok(Self {
            base_dir,
            file_mappings,
            inodes,
            next_inode,
        })
    }
    
    fn get_file_attr(&self, inode: u64, size: u64) -> FileAttr {
        FileAttr {
            ino: inode,
            size,
            blocks: (size + 511) / 512,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            crtime: SystemTime::now(),
            kind: FileType::RegularFile,
            perm: 0o444,
            nlink: 1,
            uid: 0,
            gid: 0,
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }
    
    /// Read data from multi-part files
    fn read_multipart(&self, mapping: &FileMapping, offset: u64, size: usize) -> io::Result<Vec<u8>> {
        let mut result = Vec::with_capacity(size);
        let mut current_offset = offset;
        let mut remaining = size;
        
        // Find which part to start reading from
        let mut part_start_offset = 0u64;
        for (part_idx, part_size) in mapping.part_sizes.iter().enumerate() {
            let part_end_offset = part_start_offset + part_size;
            
            if current_offset < part_end_offset {
                // This is our starting part
                let mut parts_to_read = vec![(part_idx, current_offset - part_start_offset)];
                
                // Calculate how many more parts we need
                let mut bytes_from_first_part = (part_end_offset - current_offset).min(remaining as u64) as usize;
                remaining -= bytes_from_first_part;
                
                // Add subsequent parts if needed
                for next_idx in (part_idx + 1)..mapping.part_sizes.len() {
                    if remaining == 0 {
                        break;
                    }
                    parts_to_read.push((next_idx, 0));
                    remaining = remaining.saturating_sub(mapping.part_sizes[next_idx] as usize);
                }
                
                // Read from all necessary parts
                for (idx, start_in_part) in parts_to_read {
                    let mut file = File::open(&mapping.obfuscated_files[idx])?;
                    file.seek(SeekFrom::Start(start_in_part))?;
                    
                    let to_read = if idx == part_idx {
                        bytes_from_first_part
                    } else {
                        (size - result.len()).min(mapping.part_sizes[idx] as usize)
                    };
                    
                    let mut buffer = vec![0u8; to_read];
                    file.read_exact(&mut buffer)?;
                    result.extend_from_slice(&buffer);
                }
                
                break;
            }
            
            part_start_offset = part_end_offset;
        }
        
        Ok(result)
    }
}

/// Get RAR part number for sorting (rar=0, r00=1, r01=2, etc)
fn get_rar_part_number(filename: &str) -> u32 {
    if filename.ends_with(".rar") {
        0
    } else if let Some(pos) = filename.rfind(".r") {
        filename[pos + 2..].parse().unwrap_or(999)
    } else {
        999
    }
}

/// Compute MD5 hash of first 16KB of a file
pub fn compute_hash16k(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; 16384]; // 16KB
    
    // Read up to 16KB
    let bytes_read = file.read(&mut buffer)?;
    buffer.truncate(bytes_read);
    
    // Compute MD5
    let mut hasher = Md5::new();
    hasher.update(&buffer);
    Ok(hasher.finalize().to_vec())
}

impl Filesystem for MultiPartRarVfs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent != 1 {
            reply.error(libc::ENOENT);
            return;
        }
        
        let name_str = name.to_str().unwrap_or("");
        for (inode, filename) in &self.inodes {
            if filename == name_str {
                if let Some(mapping) = self.file_mappings.get(filename) {
                    reply.entry(&TTL, &self.get_file_attr(*inode, mapping.total_size), 0);
                    return;
                }
            }
        }
        
        reply.error(libc::ENOENT);
    }
    
    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        if ino == 1 {
            let attr = FileAttr {
                ino: 1,
                size: 0,
                blocks: 0,
                atime: SystemTime::now(),
                mtime: SystemTime::now(),
                ctime: SystemTime::now(),
                crtime: SystemTime::now(),
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 2,
                uid: 0,
                gid: 0,
                rdev: 0,
                blksize: 512,
                flags: 0,
            };
            reply.attr(&TTL, &attr);
            return;
        } else if let Some(filename) = self.inodes.get(&ino) {
            if let Some(mapping) = self.file_mappings.get(filename) {
                reply.attr(&TTL, &self.get_file_attr(ino, mapping.total_size));
                return;
            }
        }
        
        reply.error(libc::ENOENT);
    }
    
    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        if let Some(filename) = self.inodes.get(&ino) {
            if let Some(mapping) = self.file_mappings.get(filename) {
                match self.read_multipart(mapping, offset as u64, size as usize) {
                    Ok(data) => reply.data(&data),
                    Err(_) => reply.error(libc::EIO),
                }
                return;
            }
        }
        
        reply.error(libc::ENOENT);
    }
    
    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != 1 {
            reply.error(libc::ENOENT);
            return;
        }
        
        let entries = vec![
            (1, FileType::Directory, "."),
            (1, FileType::Directory, ".."),
        ];
        
        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        
        let mut file_entries: Vec<_> = self.inodes.iter().collect();
        file_entries.sort_by_key(|(inode, _)| **inode);
        
        for (i, (inode, filename)) in file_entries.into_iter().enumerate() {
            let index = i + 3;
            if index > offset as usize {
                if reply.add(*inode, index as i64, FileType::RegularFile, filename) {
                    break;
                }
            }
        }
        
        reply.ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;
    
    #[test]
    fn test_mount_multipart_rar() {
        // Path to your test directory
        let base_dir = Path::new("/tmp/nzb/downloads/downloads/inter/almost_downloaded");
        let par2_path = base_dir.join("db5839e271decea10e9e713aa0e20573.par2");
        let mount_point = "/tmp/test-mount";
        
        // Create mount point
        std::fs::create_dir_all(mount_point).ok();
        
        // Create and mount filesystem
        let fs = MultiPartRarVfs::new(base_dir.to_path_buf(), &par2_path).unwrap();
        
        println!("Found {} files to expose:", fs.file_mappings.len());
        for (name, mapping) in &fs.file_mappings {
            println!("  {} ({} bytes, {} parts)", name, mapping.total_size, mapping.part_sizes.len());
        }
        
        // Mount in background thread
        let handle = thread::spawn(move || {
            let options = vec![
                MountOption::RO,
                MountOption::FSName("multipart-rar".to_string()),
                MountOption::AutoUnmount,
            ];
            
            if let Err(e) = fuser::mount2(fs, mount_point, &options) {
                eprintln!("Mount failed: {}", e);
            }
        });

        thread::sleep(Duration::from_secs(1000));
        
        // Test reading from mounted file
        println!("\nTesting file access...");
        
        // List files
        if let Ok(entries) = std::fs::read_dir(mount_point) {
            for entry in entries {
                if let Ok(entry) = entry {
                    println!("Found: {}", entry.path().display());
                    
                    // Test seeking (simulating video player)
                    if let Ok(mut file) = File::open(entry.path()) {
                        // Seek to middle
                        if let Ok(metadata) = file.metadata() {
                            let mid_point = metadata.len() / 2;
                            println!("  Seeking to byte {}", mid_point);
                            file.seek(SeekFrom::Start(mid_point)).unwrap();
                            
                            // Read some data
                            let mut buffer = vec![0u8; 1024];
                            if let Ok(bytes) = file.read(&mut buffer) {
                                println!("  Read {} bytes from middle", bytes);
                            }
                        }
                    }
                }
            }
        }
        
        println!("\nUnmounting...");
        drop(handle);
        
        thread::sleep(Duration::from_secs(1));
        std::fs::remove_dir(mount_point).ok();
    }
}