use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nzb {
    pub meta: NzbMeta,
    pub files: Vec<NzbFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct NzbMeta {
    pub title: Option<String>,
    pub category: Option<String>,
    pub poster: Option<String>,
    pub date: Option<DateTime<Utc>>,
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NzbFile {
    pub subject: String,
    pub poster: String,
    pub date: DateTime<Utc>,
    pub groups: Vec<String>,
    pub segments: Vec<NzbSegment>,
    pub path: String, // Normalized path with forward slashes
    pub size: u64,    // Total size of all segments
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NzbSegment {
    pub message_id: String,
    pub number: u32,
    pub bytes: u64,
}

impl Default for Nzb {
    fn default() -> Self {
        Self::new()
    }
}

impl Nzb {
    pub fn new() -> Self {
        Self {
            meta: NzbMeta::default(),
            files: Vec::new(),
        }
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn find_file(&self, path: &str) -> Option<&NzbFile> {
        let normalized = normalize_path(path);
        self.files.iter().find(|f| f.path == normalized)
    }

    pub fn find_video_files(&self) -> Vec<&NzbFile> {
        self.files
            .iter()
            .filter(|f| is_video_file(&f.path))
            .collect()
    }
}


impl NzbFile {
    pub fn new(subject: String, poster: String, date: DateTime<Utc>) -> Self {
        Self {
            subject,
            poster,
            date,
            groups: Vec::new(),
            segments: Vec::new(),
            path: String::new(),
            size: 0,
        }
    }

    pub fn add_segment(&mut self, segment: NzbSegment) {
        self.size += segment.bytes;
        self.segments.push(segment);
    }

    pub fn sort_segments(&mut self) {
        self.segments.sort_by(|a, b| a.number.cmp(&b.number));
    }

    pub fn is_complete(&self) -> bool {
        if self.segments.is_empty() {
            return false;
        }
        
        // Check if segments are numbered consecutively starting from 1
        for (i, segment) in self.segments.iter().enumerate() {
            if segment.number != (i + 1) as u32 {
                return false;
            }
        }
        true
    }

    pub fn extension(&self) -> Option<&str> {
        self.path.rsplit('.').next()
    }
}

impl NzbSegment {
    pub fn new(message_id: String, number: u32, bytes: u64) -> Self {
        Self {
            message_id,
            number,
            bytes,
        }
    }
}

// Utility functions

pub fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .to_string()
}

pub fn is_video_file(path: &str) -> bool {
    if let Some(ext) = path.rsplit('.').next() {
        matches!(
            ext.to_lowercase().as_str(),
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg"
        )
    } else {
        false
    }
}

pub fn is_rar_file(path: &str) -> bool {
    if let Some(ext) = path.rsplit('.').next() {
        let ext = ext.to_lowercase();
        ext == "rar" || (ext.len() >= 3 && &ext[..1] == "r" && ext[1..].parse::<u32>().is_ok())
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path("folder\\file.txt"), "folder/file.txt");
        assert_eq!(normalize_path("/folder/file.txt"), "folder/file.txt");
        assert_eq!(normalize_path("folder/file.txt"), "folder/file.txt");
    }

    #[test]
    fn test_is_video_file() {
        assert!(is_video_file("movie.mp4"));
        assert!(is_video_file("MOVIE.MP4"));
        assert!(is_video_file("video.mkv"));
        assert!(!is_video_file("archive.rar"));
        assert!(!is_video_file("document.txt"));
    }

    #[test]
    fn test_is_rar_file() {
        assert!(is_rar_file("archive.rar"));
        assert!(is_rar_file("archive.r00"));
        assert!(is_rar_file("archive.r01"));
        assert!(!is_rar_file("video.mp4"));
        assert!(!is_rar_file("archive.zip"));
    }

    #[test]
    fn test_nzb_file_segments() {
        let mut file = NzbFile::new(
            "Test File".to_string(),
            "poster@example.com".to_string(),
            Utc::now(),
        );

        file.add_segment(NzbSegment::new("msg2".to_string(), 2, 1000));
        file.add_segment(NzbSegment::new("msg1".to_string(), 1, 1000));
        
        assert_eq!(file.size, 2000);
        assert!(!file.is_complete()); // Not complete because segments are not in order
        
        file.sort_segments();
        assert!(file.is_complete());
        assert_eq!(file.segments[0].number, 1);
        assert_eq!(file.segments[1].number, 2);
    }
}