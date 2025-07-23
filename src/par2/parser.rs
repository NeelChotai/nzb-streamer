use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use byteorder::{ByteOrder, LittleEndian};

use crate::par2::par2_crc32;

#[derive(Debug, Clone)]
pub struct FilePar2Info {
    pub hash16k: Vec<u8>,
    pub filesize: u64,
    pub filehash: u32,
}

// Constants
const PAR_PKT_ID: &[u8] = b"PAR2\x00PKT";
const PAR_MAIN_ID: &[u8] = b"PAR 2.0\x00Main\x00\x00\x00\x00";
const PAR_FILE_ID: &[u8] = b"PAR 2.0\x00FileDesc";
const PAR_SLICE_ID: &[u8] = b"PAR 2.0\x00IFSC\x00\x00\x00\x00";

// Just a free function - no need for a struct
pub fn parse_par2_file(path: &Path) -> io::Result<HashMap<String, FilePar2Info>> {
    let mut file = File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    
    let mut result = HashMap::new();
    let mut file_info: HashMap<String, (String, Vec<u8>, u64)> = HashMap::new();
    let mut file_crcs: HashMap<String, Vec<u32>> = HashMap::new();
    let mut slice_size = 0u64;
    let mut coeff = 0u32;
    
    let mut pos = 0;
    while pos + 64 <= buffer.len() {
        // Check for PAR2 packet header
        if &buffer[pos..pos + 8] != PAR_PKT_ID {
            pos += 1;
            continue;
        }
        
        // Read packet length
        let pack_len = LittleEndian::read_u64(&buffer[pos + 8..pos + 16]) as usize;
        if pack_len < 64 || pos + pack_len > buffer.len() {
            pos += 1;
            continue;
        }
        
        // Skip MD5 checksum (16 bytes) - we're not validating it
        let data = &buffer[pos + 32..pos + pack_len];
        
        // Skip Recovery Set ID (16 bytes) and get packet type
        let packet_type = &data[16..32];
        
        match packet_type {
            PAR_FILE_ID => {
                // FileDesc packet
                let fileid = hex::encode(&data[32..48]);
                let hash16k = data[64..80].to_vec();
                let filesize = LittleEndian::read_u64(&data[80..88]);
                
                // Extract filename (null-terminated)
                let filename_bytes = &data[88..];
                let filename_end = filename_bytes.iter().position(|&b| b == 0).unwrap_or(filename_bytes.len());
                let filename = String::from_utf8_lossy(&filename_bytes[..filename_end]).to_string();
                
                file_info.insert(fileid, (filename, hash16k, filesize));
            }
            PAR_MAIN_ID => {
                // Main packet
                slice_size = LittleEndian::read_u64(&data[32..40]);
                coeff = par2_crc32::xpow8n(slice_size);
            }
            PAR_SLICE_ID => {
                // IFSC (Input File Slice Checksum) packet
                let fileid = hex::encode(&data[32..48]);
                let crcs = file_crcs.entry(fileid).or_default();
                
                // Each CRC entry is 20 bytes
                for chunk in data[48..].chunks_exact(20) {
                    let crc = LittleEndian::read_u32(&chunk[16..20]);
                    crcs.push(crc);
                }
            }
            _ => {}
        }
        
        pos += pack_len;
    }
    
    // Calculate file hashes
    for (fileid, (filename, hash16k, filesize)) in file_info {
        if let Some(crcs) = file_crcs.get(&fileid) {
            if slice_size == 0 || crcs.is_empty() {
                continue;
            }
            
            let slices = (filesize / slice_size) as usize;
            let mut crc32 = 0u32;
            
            // Process full slices
            for i in 0..slices.min(crcs.len()) {
                crc32 = par2_crc32::multiply(crc32, coeff) ^ crcs[i];
            }
            
            // Handle tail if exists
            let tail_size = filesize % slice_size;
            if tail_size > 0 && slices < crcs.len() {
                let tail_crc = par2_crc32::zero_unpad(crcs[slices], slice_size - tail_size);
                crc32 = par2_crc32::combine(crc32, tail_crc, tail_size);
            }
            
            result.insert(filename, FilePar2Info {
                hash16k,
                filesize,
                filehash: crc32,
            });
        }
    }
    
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_par2() {
        let path = Path::new("/tmp/nzb/downloads/downloads/inter/almost_downloaded/db5839e271decea10e9e713aa0e20573.par2");
        
        match parse_par2_file(path) {
            Ok(table) => {
                for (filename, info) in &table {
                    println!("{filename}: FilePar2Info {{");
                    println!("    hash16k: {:?},", info.hash16k);
                    println!("    filesize: {},", info.filesize);
                    println!("    filehash: {},", info.filehash);
                    println!("}}");
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }
}