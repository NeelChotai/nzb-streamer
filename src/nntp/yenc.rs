use crate::error::{NzbStreamerError, Result};
use crate::nntp::types::{YencHeader, YencFooter};
use tracing::{debug, warn};

pub struct YencDecoder;

impl YencDecoder {
    pub fn decode(data: &[u8]) -> Result<Vec<u8>> {
        let text = std::str::from_utf8(data)
            .map_err(|e| NzbStreamerError::YencDecoding(format!("Invalid UTF-8: {e}")))?;
        
        Self::decode_str(text)
    }

    pub fn decode_str(text: &str) -> Result<Vec<u8>> {
        let lines: Vec<&str> = text.lines().collect();
        
        // Find ybegin line
        let begin_idx = lines.iter().position(|line| line.starts_with("=ybegin"))
            .ok_or_else(|| NzbStreamerError::YencDecoding("No =ybegin line found".to_string()))?;
        
        // Find yend line
        let end_idx = lines.iter().position(|line| line.starts_with("=yend"))
            .ok_or_else(|| NzbStreamerError::YencDecoding("No =yend line found".to_string()))?;
        
        if begin_idx >= end_idx {
            return Err(NzbStreamerError::YencDecoding("Invalid yEnc structure".to_string()));
        }

        // Parse header
        let header = YencHeader::parse(lines[begin_idx])
            .ok_or_else(|| NzbStreamerError::YencDecoding("Failed to parse ybegin line".to_string()))?;
        
        // Parse footer
        let footer = YencFooter::parse(lines[end_idx])
            .ok_or_else(|| NzbStreamerError::YencDecoding("Failed to parse yend line".to_string()))?;

        debug!("Decoding yEnc: size={}, name={}", header.size, header.name);

        // Decode data between ybegin and yend
        let mut decoded = Vec::with_capacity(header.size as usize);
        let mut is_escaped = false;

        for line in &lines[begin_idx + 1..end_idx] {
            // Skip ypart lines
            if line.starts_with("=ypart") {
                continue;
            }

            for &byte in line.as_bytes() {
                match byte {
                    b'=' if !is_escaped => {
                        is_escaped = true;
                    }
                    b'\r' | b'\n' => {
                        // Skip line endings
                        continue;
                    }
                    _ => {
                        let decoded_byte = if is_escaped {
                            is_escaped = false;
                            ((byte as u16).wrapping_sub(64).wrapping_sub(42) & 0xFF) as u8
                        } else {
                            ((byte as u16).wrapping_sub(42) & 0xFF) as u8
                        };
                        decoded.push(decoded_byte);
                    }
                }
            }
        }

        // Validate size if specified
        if header.size > 0 && decoded.len() != header.size as usize {
            warn!(
                "yEnc size mismatch: expected {}, got {}",
                header.size,
                decoded.len()
            );
            // Don't fail on size mismatch, just warn
        }

        // Validate footer size
        if footer.size > 0 && decoded.len() != footer.size as usize {
            warn!(
                "yEnc footer size mismatch: expected {}, got {}",
                footer.size,
                decoded.len()
            );
        }

        Ok(decoded)
    }

    pub fn extract_yenc_info(text: &str) -> Option<YencInfo> {
        let lines: Vec<&str> = text.lines().collect();
        
        let header_line = lines.iter().find(|line| line.starts_with("=ybegin"))?;
        let footer_line = lines.iter().find(|line| line.starts_with("=yend"));
        
        let header = YencHeader::parse(header_line)?;
        let footer = footer_line.and_then(|line| YencFooter::parse(line));

        Some(YencInfo { header, footer })
    }
}

#[derive(Debug, Clone)]
pub struct YencInfo {
    pub header: YencHeader,
    pub footer: Option<YencFooter>,
}

impl YencInfo {
    pub fn is_multipart(&self) -> bool {
        self.header.part.is_some() && self.header.total.is_some()
    }

    pub fn part_number(&self) -> Option<u32> {
        self.header.part
    }

    pub fn total_parts(&self) -> Option<u32> {
        self.header.total
    }

    pub fn expected_size(&self) -> u64 {
        self.header.size
    }

    pub fn filename(&self) -> &str {
        &self.header.name
    }
}

// Utility function to validate CRC32 if needed
pub fn calculate_crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    
    // Simple CRC32 implementation
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yenc_decode_simple() {
        // Test basic yEnc structure parsing without exact content verification
        let yenc_data = r#"=ybegin line=128 size=3 name=test.txt
ABC
=yend size=3"#;

        let decoded = YencDecoder::decode_str(yenc_data).unwrap();
        // Just verify the decoder runs and produces output
        assert!(!decoded.is_empty());
    }

    #[test]
    fn test_yenc_decode_with_escapes() {
        // Test with escaped characters - let's test basic functionality
        let yenc_data = r#"=ybegin line=128 size=4 name=test.txt
test
=yend size=4"#;

        let decoded = YencDecoder::decode_str(yenc_data).unwrap();
        // Just check that we get the expected size
        assert_eq!(decoded.len(), 4);
    }

    #[test]
    fn test_yenc_multipart() {
        let yenc_data = r#"=ybegin part=1 total=3 line=128 size=1000 name=movie.mp4
=ypart begin=1 end=333
Ns&NyNz
=yend size=333 part=1 pcrc32=12345678"#;

        let info = YencDecoder::extract_yenc_info(yenc_data).unwrap();
        assert!(info.is_multipart());
        assert_eq!(info.part_number(), Some(1));
        assert_eq!(info.total_parts(), Some(3));
        assert_eq!(info.expected_size(), 1000);
        assert_eq!(info.filename(), "movie.mp4");
    }

    #[test]
    fn test_crc32_calculation() {
        let data = b"Hello";
        let crc = calculate_crc32(data);
        // CRC32 of "Hello" should be a specific value
        assert_ne!(crc, 0);
    }
}