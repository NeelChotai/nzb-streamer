use crate::error::{NzbStreamerError, Result};
use crate::nzb::types::{Nzb, NzbFile};
use crate::nntp::client::NntpClient;
use crate::par2::parser::Par2Parser;
use crate::par2::types::{DeobfuscationMapping, Par2Archive};
use std::collections::HashMap;
use tracing::{debug, info, warn};

pub struct Par2Deobfuscator;

impl Par2Deobfuscator {
    pub async fn parse_with_deobfuscation(
        nzb_content: &[u8],
        nntp_client: &NntpClient,
    ) -> Result<Nzb> {
        // First parse the NZB normally
        let content = std::str::from_utf8(nzb_content)
            .map_err(|e| crate::error::NzbStreamerError::NzbParsing(format!("Invalid UTF-8: {}", e)))?;
        let mut nzb = crate::nzb::parser::NzbParser::parse(content)?;
        
        // Check if deobfuscation is needed
        if !Self::needs_deobfuscation(&nzb) {
            info!("NZB doesn't need deobfuscation - files have clear names");
            return Ok(nzb);
        }
        
        info!("NZB has obfuscated filenames, attempting deobfuscation...");
        
        // Find the main PAR2 file
        let main_par2 = Self::find_main_par2(&nzb)?;
        info!("Found main PAR2 file: {}", main_par2.path);
        
        // Download PAR2 headers (first ~32KB)
        let par2_data = Self::fetch_par2_headers(main_par2, nntp_client).await?;
        
        // Parse PAR2 to get file descriptions
        let par2_archive = Par2Parser::parse(&par2_data)?;
        
        // Create filename mappings
        let mappings = Self::create_deobfuscation_mappings(&nzb, &par2_archive)?;
        
        // Apply mappings to NZB files
        Self::apply_deobfuscation(&mut nzb, &mappings)?;
        
        info!("Deobfuscation complete - {} files mapped", mappings.len());
        
        Ok(nzb)
    }
    
    fn needs_deobfuscation(nzb: &Nzb) -> bool {
        // Check if files have obfuscated names (no extensions, hash-like names)
        let obfuscated_count = nzb.files.iter()
            .filter(|f| Self::is_obfuscated_filename(&f.path))
            .count();
        
        // If more than 50% of files appear obfuscated, we need deobfuscation
        obfuscated_count > nzb.files.len() / 2
    }
    
    fn is_obfuscated_filename(filename: &str) -> bool {
        // Check for common obfuscation patterns:
        // - No file extension
        // - Hash-like names (long hex strings)
        // - Random alphanumeric strings
        
        let name = filename.trim();
        
        // Has no extension
        if !name.contains('.') {
            return true;
        }
        
        // Check if it's a hash-like string (32 or 64 character hex)
        if name.len() == 32 || name.len() == 64 {
            if name.chars().all(|c| c.is_ascii_hexdigit()) {
                return true;
            }
        }
        
        // Check for random-looking strings without meaningful extensions
        let extension = name.split('.').last().unwrap_or("");
        if extension.len() > 10 || extension.chars().all(|c| c.is_ascii_hexdigit()) {
            return true;
        }
        
        false
    }
    
    fn find_main_par2(nzb: &Nzb) -> Result<&NzbFile> {
        // Find PAR2 files (they usually have clear names)
        let par2_files: Vec<&NzbFile> = nzb.files.iter()
            .filter(|f| f.path.to_lowercase().ends_with(".par2"))
            .collect();
        
        if par2_files.is_empty() {
            return Err(NzbStreamerError::NzbParsing("No PAR2 files found for deobfuscation".to_string()));
        }
        
        // Find the main PAR2 file (smallest one without .volXX)
        let main_par2 = par2_files.iter()
            .filter(|f| !f.path.contains(".vol"))
            .min_by_key(|f| f.size)
            .or_else(|| par2_files.iter().min_by_key(|f| f.size))
            .ok_or_else(|| NzbStreamerError::NzbParsing("No suitable PAR2 file found".to_string()))?;
        
        Ok(main_par2)
    }
    
    async fn fetch_par2_headers(
        par2_file: &NzbFile,
        nntp_client: &NntpClient,
    ) -> Result<Vec<u8>> {
        // We only need the first ~32KB to get file descriptions
        const MAX_HEADER_SIZE: usize = 32 * 1024;
        
        let mut data = Vec::new();
        let mut bytes_fetched = 0;
        
        for segment in &par2_file.segments {
            if bytes_fetched >= MAX_HEADER_SIZE {
                break;
            }
            
            debug!("Fetching PAR2 segment: {}", segment.message_id);
            let decoded = nntp_client.fetch_article(&segment.message_id.0).await?;
            
            // yEnc decoding is now handled in the NNTP client
            data.extend_from_slice(&decoded);
            
            bytes_fetched += decoded.len();
            
            // Stop if we have enough data
            if bytes_fetched >= MAX_HEADER_SIZE {
                data.truncate(MAX_HEADER_SIZE);
                break;
            }
        }
        
        if data.is_empty() {
            return Err(NzbStreamerError::NzbParsing("Failed to fetch PAR2 data".to_string()));
        }
        
        info!("Fetched {} bytes of PAR2 data", data.len());
        Ok(data)
    }
    
    fn create_deobfuscation_mappings(
        nzb: &Nzb,
        par2_archive: &Par2Archive,
    ) -> Result<HashMap<String, DeobfuscationMapping>> {
        let mut mappings = HashMap::new();
        
        // Create a map of file sizes to help match obfuscated files
        let mut size_to_nzb: HashMap<u64, Vec<&NzbFile>> = HashMap::new();
        for file in &nzb.files {
            let size = file.size;
            size_to_nzb.entry(size).or_default().push(file);
        }
        
        // Match PAR2 file descriptions to NZB files
        for file_desc in &par2_archive.file_descriptions {
            // Skip PAR2 files themselves
            if file_desc.filename.ends_with(".par2") {
                continue;
            }
            
            // Find NZB files with matching size
            if let Some(candidates) = size_to_nzb.get(&file_desc.file_size) {
                for candidate in candidates {
                    // Prefer obfuscated names for mapping
                    if Self::is_obfuscated_filename(&candidate.path) {
                        mappings.insert(
                            candidate.path.clone(),
                            DeobfuscationMapping {
                                obfuscated_name: candidate.path.clone(),
                                real_name: file_desc.filename.clone(),
                                file_size: file_desc.file_size,
                                md5_hash: file_desc.md5_hash,
                            },
                        );
                        break;
                    }
                }
            }
        }
        
        if mappings.is_empty() {
            warn!("No filename mappings created - may not be properly obfuscated");
        } else {
            info!("Created {} filename mappings", mappings.len());
        }
        
        Ok(mappings)
    }
    
    fn apply_deobfuscation(
        nzb: &mut Nzb,
        mappings: &HashMap<String, DeobfuscationMapping>,
    ) -> Result<()> {
        for file in &mut nzb.files {
            if let Some(mapping) = mappings.get(&file.path) {
                debug!("Mapping {} -> {}", file.path, mapping.real_name);
                file.path = mapping.real_name.clone();
            }
        }
        
        Ok(())
    }
    
    pub fn identify_video_files(nzb: &Nzb) -> Vec<&NzbFile> {
        nzb.files.iter()
            .filter(|f| Self::is_video_file(&f.path))
            .collect()
    }
    
    pub fn identify_first_rar_part(nzb: &Nzb) -> Option<&NzbFile> {
        let mut rar_files: Vec<&NzbFile> = nzb.files.iter()
            .filter(|f| Self::is_rar_file(&f.path))
            .collect();
        
        rar_files.sort_by(|a, b| a.path.cmp(&b.path));
        
        // Look for .part001.rar or .part01.rar or just .rar
        rar_files.iter()
            .find(|f| f.path.contains("part001") || f.path.contains("part01") || f.path.contains(".rar"))
            .copied()
    }
    
    fn is_video_file(filename: &str) -> bool {
        if let Some(ext) = filename.rsplit('.').next() {
            matches!(
                ext.to_lowercase().as_str(),
                "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg"
            )
        } else {
            false
        }
    }
    
    fn is_rar_file(filename: &str) -> bool {
        if let Some(ext) = filename.rsplit('.').next() {
            let ext = ext.to_lowercase();
            ext == "rar" || (ext.len() >= 3 && &ext[..1] == "r" && ext[1..].parse::<u32>().is_ok())
        } else {
            false
        }
    }
}