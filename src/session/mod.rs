use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::Mutex;

use nzb_rs::{File as NzbFile, Nzb, Segment};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{NzbStreamerError};
use crate::nntp::{config::NntpConfig, NntpClient};
use crate::nzb::NzbParser;
use crate::par2::{FilePar2Info};

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadTask {
    Par2 {
        file: NzbFile,
        priority: u32,
    },
    RarSegment {
        segment: Segment,
        file_index: usize,
        segment_index: usize,
        priority: u32,
        byte_range: std::ops::Range<u64>,
    },
}

impl DownloadTask {
    pub fn priority(&self) -> u32 {
        match self {
            DownloadTask::Par2 { priority, .. } => *priority,
            DownloadTask::RarSegment { priority, .. } => *priority,
        }
    }

    pub fn message_id(&self) -> &str {
        match self {
            DownloadTask::Par2 { file, .. } => &file.segments[0].message_id,
            DownloadTask::RarSegment { segment, .. } => &segment.message_id,
        }
    }
}

/// Session state for tracking NZB processing and streaming
#[derive(Debug)]
pub struct Session {
    pub id: Uuid,
    pub created_at: SystemTime,
    pub nzb: Nzb,
    pub par2_files: Vec<NzbFile>,
    pub rar_files: Vec<NzbFile>,
    pub video_files: Vec<String>, // Real filenames after deobfuscation
    pub par2_mappings: HashMap<String, FilePar2Info>,
    pub obfuscated_to_real: HashMap<String, String>,
    pub download_queue: Arc<Mutex<VecDeque<DownloadTask>>>,
    pub downloaded_segments: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    pub stream_ready: Arc<AtomicBool>,
    pub par2_complete: Arc<AtomicBool>,
    pub deobfuscation_complete: Arc<AtomicBool>,
    pub cache_dir: PathBuf,
}

impl Session {
    /// Create new session from NZB data
    pub fn new(nzb_data: &str, cache_dir: PathBuf) -> Result<Self> {
        let nzb = NzbParser::parse(nzb_data)?;
        let session_id = Uuid::new_v4();

        // Separate PAR2 files from other files
        let mut par2_files = Vec::new();
        let mut rar_files = Vec::new();

        for file in &nzb.files {
            let subject_lower = file.subject.to_lowercase();
            if subject_lower.contains(".par2") || subject_lower.contains(".vol") {
                par2_files.push(file.clone());
            } else {
                // For obfuscated downloads, treat all non-PAR2 files as potential RAR files
                // The PAR2 deobfuscation will tell us which ones are actually video files
                rar_files.push(file.clone());
            }
        }

        info!(
            "Created session {}: {} PAR2 files, {} RAR files",
            session_id,
            par2_files.len(),
            rar_files.len()
        );

        let session_cache_dir = cache_dir.join(session_id.to_string());
        std::fs::create_dir_all(&session_cache_dir)?;

        Ok(Session {
            id: session_id,
            created_at: SystemTime::now(),
            nzb,
            par2_files,
            rar_files,
            video_files: Vec::new(),
            par2_mappings: HashMap::new(),
            obfuscated_to_real: HashMap::new(),
            download_queue: Arc::new(Mutex::new(VecDeque::new())),
            downloaded_segments: Arc::new(Mutex::new(HashMap::new())),
            stream_ready: Arc::new(AtomicBool::new(false)),
            par2_complete: Arc::new(AtomicBool::new(false)),
            deobfuscation_complete: Arc::new(AtomicBool::new(false)),
            cache_dir: session_cache_dir,
        })
    }

    /// Initialize download queue with PAR2 files first
    pub async fn queue_par2_downloads(&mut self) -> Result<()> {
        let mut queue = self.download_queue.lock().await;

        // Add PAR2 files with highest priority
        for (index, file) in self.par2_files.iter().enumerate() {
            queue.push_back(DownloadTask::Par2 {
                file: file.clone(),
                priority: 10000 + index as u32, // Highest priority
            });
        }

        info!("Queued {} PAR2 files for download", self.par2_files.len());
        Ok(())
    }

    /// Process downloaded PAR2 files to build filename mappings
    pub async fn process_par2_files(&mut self) -> Result<()> {
        // Collect all PAR2 data first to avoid borrow checker issues
        let mut par2_data_map = HashMap::new();
        {
            let downloaded = self.downloaded_segments.lock().await;
            for par2_file in &self.par2_files {
                if let Some(data) = downloaded.get(&par2_file.segments[0].message_id) {
                    par2_data_map.insert(par2_file.segments[0].message_id.clone(), data.clone());
                }
            }
        }

        // Process each PAR2 file
        for par2_file in &self.par2_files {
            if let Some(par2_data) = par2_data_map.get(&par2_file.segments[0].message_id) {
                // Save PAR2 data to temp file for parsing
                let par2_path = self
                    .cache_dir
                    .join(format!("{}.par2", par2_file.segments[0].message_id));
                tokio::fs::write(&par2_path, par2_data).await?;

                // Parse PAR2 file
                match parse_par2_file(&par2_path) {
                    Ok(mappings) => {
                        for (filename, info) in mappings {
                            self.par2_mappings.insert(filename.clone(), info);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse PAR2 file {}: {}", par2_file.subject, e);
                    }
                }
            }
        }

        // After all PAR2 files are processed, match filenames
        let filenames: Vec<String> = self.par2_mappings.keys().cloned().collect();
        for filename in filenames {
            self.match_obfuscated_filename(&filename);
        }

        self.deobfuscation_complete.store(true, Ordering::Relaxed);
        info!(
            "PAR2 processing complete. Found {} real filenames, {} video files",
            self.obfuscated_to_real.len(),
            self.video_files.len()
        );

        Ok(())
    }

    /// Queue RAR files for priority download
    pub async fn queue_priority_download(&mut self) -> Result<()> {
        let mut queue = self.download_queue.lock().await;

        // Sort RAR files by name (.rar, .r00, .r01, etc.)
        let mut sorted_rars = self.rar_files.clone();
        sorted_rars.sort_by(|a, b| {
            let a_name = &a.subject;
            let b_name = &b.subject;

            // Extract numeric suffix
            let get_number = |name: &str| -> u32 {
                if name.ends_with(".rar") {
                    return 0;
                }
                if let Some(pos) = name.rfind(".r") {
                    if let Ok(num) = name[pos + 2..].parse::<u32>() {
                        return num + 1;
                    }
                }
                999
            };

            get_number(a_name).cmp(&get_number(b_name))
        });

        // Queue segments with decreasing priority
        for (file_index, file) in sorted_rars.iter().enumerate() {
            let mut byte_offset = 0u64;

            for (seg_index, segment) in file.segments.iter().enumerate() {
                let priority = 5000 - (file_index * 100 + seg_index) as u32; // Decreasing priority
                let byte_range = byte_offset..(byte_offset + segment.size as u64);

                queue.push_back(DownloadTask::RarSegment {
                    segment: segment.clone(),
                    file_index,
                    segment_index: seg_index,
                    priority,
                    byte_range,
                });

                byte_offset += segment.size as u64;
            }
        }

        info!(
            "Queued {} RAR segments for priority download",
            sorted_rars.iter().map(|f| f.segments.len()).sum::<usize>()
        );

        // Signal that streaming can potentially start after first few segments
        if !sorted_rars.is_empty() && sorted_rars[0].segments.len() >= 2 {
            self.stream_ready.store(true, Ordering::Relaxed);
            info!("Stream ready - sufficient RAR data queued");
        }

        Ok(())
    }

    /// Get next download task from priority queue
    pub async fn get_next_download_task(&self) -> Option<DownloadTask> {
        let mut queue = self.download_queue.lock().await;

        // Sort by priority (higher first)
        let mut tasks: Vec<_> = queue.drain(..).collect();
        tasks.sort_by(|a, b| b.priority().cmp(&a.priority()));

        if let Some(task) = tasks.pop() {
            // Put remaining tasks back
            for t in tasks {
                queue.push_back(t);
            }
            Some(task)
        } else {
            None
        }
    }

    /// Mark segment as downloaded
    pub async fn mark_segment_downloaded(&self, message_id: String, data: Vec<u8>) -> Result<()> {
        let mut downloaded = self.downloaded_segments.lock().await;

        downloaded.insert(message_id, data);
        Ok(())
    }

    /// Check if session is ready for streaming
    pub fn is_ready_for_streaming(&self) -> bool {
        self.stream_ready.load(Ordering::Relaxed)
    }

    /// Get session age in seconds
    pub fn age_seconds(&self) -> u64 {
        self.created_at.elapsed().unwrap_or_default().as_secs()
    }

    /// Advanced filename matching for PAR2 deobfuscation
    fn match_obfuscated_filename(&mut self, real_filename: &str) {
        let real_lower = real_filename.to_lowercase();

        // Only process video files
        if !self.is_video_file(&real_lower) {
            return;
        }

        // Extract base name without extension for matching
        let real_base = self.extract_base_name(&real_lower);

        for rar_file in &self.rar_files {
            if self.filenames_match(&rar_file.subject, real_filename, &real_base) {
                debug!(
                    "Matched obfuscated '{}' to real '{}'",
                    rar_file.subject, real_filename
                );
                self.obfuscated_to_real
                    .insert(rar_file.subject.clone(), real_filename.to_string());

                // Add to video files if not already present
                if !self.video_files.contains(&real_filename.to_string()) {
                    self.video_files.push(real_filename.to_string());
                    info!("Added video file: {}", real_filename);
                }
                break; // Only match to first RAR file found
            }
        }
    }

    /// Check if filename represents a video file
    fn is_video_file(&self, filename: &str) -> bool {
        filename.ends_with(".mkv")
            || filename.ends_with(".mp4")
            || filename.ends_with(".avi")
            || filename.ends_with(".mov")
            || filename.ends_with(".wmv")
            || filename.ends_with(".m4v")
    }

    /// Extract base filename without extension and common prefixes
    fn extract_base_name(&self, filename: &str) -> String {
        let without_ext = filename.split('.').next().unwrap_or(filename);

        // Remove common prefixes that might not match
        let prefixes_to_remove = ["www.", "rarbg", "yts", "eztv"];
        let mut base = without_ext.to_string();

        for prefix in &prefixes_to_remove {
            if base.starts_with(prefix) {
                base = base
                    .trim_start_matches(prefix)
                    .trim_start_matches('.')
                    .to_string();
            }
        }

        // Clean up common separators
        base.replace(&['-', '_', '.', ' '][..], "")
    }

    /// Advanced filename matching algorithm
    fn filenames_match(&self, obfuscated: &str, real_filename: &str, real_base: &str) -> bool {
        let obfuscated_lower = obfuscated.to_lowercase();
        let real_lower = real_filename.to_lowercase();

        // Method 1: Direct substring matching
        if obfuscated_lower.contains(&real_lower) || real_lower.contains(&obfuscated_lower) {
            return true;
        }

        // Method 2: Base name matching (without extensions)
        let obfuscated_base = self.extract_base_name(&obfuscated_lower);
        if obfuscated_base.len() > 5 && real_base.len() > 5 {
            // Check for significant overlap
            let similarity = self.calculate_similarity(&obfuscated_base, real_base);
            if similarity > 0.7 {
                return true;
            }
        }

        // Method 3: Check if both have similar patterns (year, quality indicators)
        if self.has_similar_patterns(&obfuscated_lower, &real_lower) {
            return true;
        }

        // Method 4: Size-based matching as fallback
        // This would require file size information from PAR2 and NZB

        false
    }

    /// Calculate string similarity (Levenshtein-like)
    fn calculate_similarity(&self, s1: &str, s2: &str) -> f32 {
        if s1.is_empty() || s2.is_empty() {
            return 0.0;
        }

        let longer = if s1.len() > s2.len() { s1 } else { s2 };
        let shorter = if s1.len() > s2.len() { s2 } else { s1 };

        if longer.is_empty() {
            return 1.0;
        }

        // Simple overlap ratio
        let mut matches = 0;
        for c in shorter.chars() {
            if longer.contains(c) {
                matches += 1;
            }
        }

        matches as f32 / longer.len() as f32
    }

    /// Check for similar patterns in filenames (years, quality, etc.)
    fn has_similar_patterns(&self, s1: &str, s2: &str) -> bool {
        // Check for year patterns (simple approach without regex crate)
        let years = [
            "2020", "2021", "2022", "2023", "2024", "2025", "1990", "1995", "2000", "2005", "2010",
            "2015",
        ];
        let mut s1_year = None;
        let mut s2_year = None;

        for year in &years {
            if s1.contains(year) {
                s1_year = Some(*year);
            }
            if s2.contains(year) {
                s2_year = Some(*year);
            }
        }

        if let (Some(y1), Some(y2)) = (s1_year, s2_year) {
            if y1 == y2 {
                return true;
            }
        }

        // Check for quality indicators
        let quality_indicators = [
            "720p", "1080p", "2160p", "4k", "hdr", "x264", "x265", "hevc",
        ];
        let mut s1_qualities = Vec::new();
        let mut s2_qualities = Vec::new();

        for quality in &quality_indicators {
            if s1.contains(quality) {
                s1_qualities.push(*quality);
            }
            if s2.contains(quality) {
                s2_qualities.push(*quality);
            }
        }

        if !s1_qualities.is_empty() && !s2_qualities.is_empty() {
            for q1 in &s1_qualities {
                for q2 in &s2_qualities {
                    if q1 == q2 {
                        return true;
                    }
                }
            }
        }

        false
    }
}

/// Manager for multiple streaming sessions
#[derive(Debug)]
pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<Uuid, Arc<Mutex<Session>>>>>,
    cache_base_dir: PathBuf,
    nntp_config: Option<NntpConfig>,
}

impl SessionManager {
    /// Create new session manager
    pub fn new(cache_base_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&cache_base_dir).ok();

        SessionManager {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            cache_base_dir,
            nntp_config: None,
        }
    }

    /// Set NNTP configuration for downloading
    pub fn set_nntp_config(&mut self, config: NntpConfig) {
        self.nntp_config = Some(config);
    }

    /// Create new session from NZB data
    pub async fn create_session(&self, nzb_data: &str) -> Result<Uuid> {
        let session_cache_dir = self.cache_base_dir.join("sessions");
        let mut session = Session::new(nzb_data, session_cache_dir)?;
        let session_id = session.id;

        // Initialize PAR2 download queue
        session.queue_par2_downloads().await?;

        // Store session
        let mut sessions = self.sessions.lock().await;

        sessions.insert(session_id, Arc::new(Mutex::new(session)));

        info!("Created session {} with PAR2 downloads queued", session_id);
        Ok(session_id)
    }

    /// Get session by ID
    pub async fn get_session(&self, session_id: Uuid) -> Result<Arc<Mutex<Session>>> {
        let sessions = self.sessions.lock().await;

        sessions
            .get(&session_id)
            .cloned()
            .ok_or_else(|| NzbStreamerError::SessionNotFound {
                session_id: session_id.to_string(),
            })
    }

    /// Start mock processor for a session (loads pre-downloaded files)
    pub async fn start_mock_processor(
        &self,
        session_id: Uuid,
        mock_data_dir: PathBuf,
    ) -> Result<()> {
        let session = self.get_session(session_id).await?;

        // Spawn background task for mock processing
        let session_clone = session.clone();
        tokio::spawn(async move {
            info!("🔄 Starting mock processor for session {}", session_id);

            // Load PAR2 files that actually exist in mock data directory
            let par2_files = {
                let session_guard = session_clone.lock().await;
                session_guard.par2_files.clone()
            };

            // Load the actual PAR2 files directly and process them
            if let Ok(entries) = tokio::fs::read_dir(&mock_data_dir).await {
                let mut entries = entries;
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                        if filename.ends_with(".par2") && !filename.contains(".vol") {
                            // This is a main PAR2 file - parse it directly
                            match crate::par2::parse_par2_file(&path) {
                                Ok(mappings) => {
                                    info!(
                                        "✅ Parsed PAR2: {} -> {} files",
                                        filename,
                                        mappings.len()
                                    );

                                    // Store mappings directly in session
                                    {
                                        let mut session_guard = session_clone.lock().await;
                                        for (real_filename, info) in mappings {
                                            session_guard
                                                .par2_mappings
                                                .insert(real_filename.clone(), info);
                                            // Immediately process filename matching
                                            session_guard.match_obfuscated_filename(&real_filename);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to parse PAR2 file {}: {}", filename, e);
                                }
                            }
                            break; // Only process one PAR2 file for now
                        }
                    }
                }
            }

            // Mark PAR2 complete and process them
            {
                let session_guard = session_clone.lock().await;
                session_guard
                    .par2_complete
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }

            // Process PAR2 files
            {
                let mut session_guard = session_clone.lock().await;
                if let Err(e) = session_guard.process_par2_files().await {
                    warn!("Failed to process mock PAR2 files: {}", e);
                } else {
                    info!("✅ Mock PAR2 processing complete");

                    // Queue RAR downloads
                    if let Err(e) = session_guard.queue_priority_download().await {
                        warn!("Failed to queue mock RAR downloads: {}", e);
                    } else {
                        info!("✅ Mock RAR queue ready");
                    }
                }
            }

            // Load some RAR segments to enable streaming
            let rar_files = {
                let session_guard = session_clone.lock().await;
                session_guard.rar_files.clone()
            };

            for (file_index, rar_file) in rar_files.iter().take(3).enumerate() {
                // Only load first 3 files
                for (seg_index, segment) in rar_file.segments.iter().take(5).enumerate() {
                    // Only load first 5 segments
                    let message_id = &segment.message_id;
                    let segment_path = mock_data_dir.join(message_id);

                    if segment_path.exists() {
                        match tokio::fs::read(&segment_path).await {
                            Ok(data) => {
                                let session_guard = session_clone.lock().await;
                                if let Err(e) = session_guard
                                    .mark_segment_downloaded(message_id.clone(), data)
                                    .await
                                {
                                    warn!("Failed to mark mock segment downloaded: {}", e);
                                } else {
                                    debug!(
                                        "Loaded mock segment: {} (file {}, seg {})",
                                        message_id, file_index, seg_index
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to read mock segment file {}: {}",
                                    segment_path.display(),
                                    e
                                );
                            }
                        }
                    } else {
                        debug!("Mock segment file not found: {}", segment_path.display());
                    }
                }
            }

            // For mock mode, create a fake video file since PAR2 only contains RAR names
            {
                let mut session_guard = session_clone.lock().await;
                if session_guard.video_files.is_empty() {
                    // The video is what would be extracted from the RAR files
                    session_guard.video_files.push("test_video.mkv".to_string());
                    info!("✅ Added mock video file for streaming: test_video.mkv");
                }
            }

            info!("✅ Mock processor complete for session {}", session_id);
        });

        Ok(())
    }

    /// Start background downloader for a session
    pub async fn start_session_downloader(&self, session_id: Uuid) -> Result<()> {
        if self.nntp_config.is_none() {
            return Err(NzbStreamerError::Config(
                "NNTP configuration not set".to_string(),
            ));
        }

        let session = self.get_session(session_id).await?;
        let nntp_config = self.nntp_config.as_ref().unwrap().clone();

        // Spawn background task for downloading
        let session_clone = session.clone();
        tokio::spawn(async move {
            let nntp_client = NntpClient::new(nntp_config);

            loop {
                let task = {
                    let session_guard = session_clone.lock().await;
                    session_guard.get_next_download_task().await
                };

                match task {
                    Some(DownloadTask::Par2 { file, .. }) => {
                        debug!("Downloading PAR2 file: {}", file.subject);

                        match nntp_client.get_file(file.clone()).await {
                            Ok(data) => {
                                let session_guard = session_clone.lock().await;
                                if let Err(e) = session_guard
                                    .mark_segment_downloaded(
                                        file.segments[0].message_id.clone(),
                                        data,
                                    )
                                    .await
                                {
                                    warn!("Failed to mark PAR2 downloaded: {}", e);
                                }
                            }
                            Err(e) => {
                                warn!("Failed to download PAR2 {}: {}", file.subject, e);
                            }
                        }

                        // Check if all PAR2s are downloaded
                        let all_par2_downloaded = {
                            let session_guard = session_clone.lock().await;
                            let downloaded = session_guard.downloaded_segments.lock().await;
                            session_guard
                                .par2_files
                                .iter()
                                .all(|f| downloaded.contains_key(&f.segments[0].message_id))
                        };

                        if all_par2_downloaded {
                            {
                                let session_guard = session_clone.lock().await;
                                session_guard.par2_complete.store(true, Ordering::Relaxed);
                            }

                            // Process PAR2 files
                            {
                                let mut session_guard = session_clone.lock().await;
                                if let Err(e) = session_guard.process_par2_files().await {
                                    warn!("Failed to process PAR2 files: {}", e);
                                } else {
                                    // Queue RAR downloads
                                    if let Err(e) = session_guard.queue_priority_download().await {
                                        warn!("Failed to queue RAR downloads: {}", e);
                                    }
                                }
                            }
                        }
                    }
                    Some(DownloadTask::RarSegment {
                        segment,
                        file_index,
                        segment_index,
                        ..
                    }) => {
                        debug!(
                            "Downloading RAR segment: {} (file {}, segment {})",
                            segment.message_id, file_index, segment_index
                        );

                        match nntp_client.download_segment(&segment).await {
                            Ok(data) => {
                                let session_guard = session_clone.lock().await;
                                if let Err(e) = session_guard
                                    .mark_segment_downloaded(segment.message_id.clone(), data)
                                    .await
                                {
                                    warn!("Failed to mark RAR segment downloaded: {}", e);
                                }
                                debug!(
                                    "Successfully downloaded RAR segment: {}",
                                    segment.message_id
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to download RAR segment {}: {}",
                                    segment.message_id, e
                                );

                                // Check if error is retryable and re-queue task if needed
                                if e.is_retryable() {
                                    let session_guard = session_clone.lock().await;
                                    let mut queue = session_guard.download_queue.lock().await;
                                    queue.push_back(DownloadTask::RarSegment {
                                        segment: segment.clone(),
                                        file_index,
                                        segment_index,
                                        priority: 1000, // Lower priority for retry
                                        byte_range: 0..segment.size as u64, // Approximate range
                                    });
                                    debug!("Re-queued failed segment: {}", segment.message_id);
                                }
                            }
                        }
                    }
                    None => {
                        // No more tasks, sleep and check again
                        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                    }
                }
            }
        });

        Ok(())
    }

    /// Clean up old sessions
    pub async fn cleanup_sessions(&self, max_age_seconds: u64) -> Result<usize> {
        let mut sessions = self.sessions.lock().await;

        let mut to_remove = Vec::new();

        for (id, session_arc) in sessions.iter() {
            let session = session_arc.lock().await;
            if session.age_seconds() > max_age_seconds {
                to_remove.push(*id);
            }
        }

        for id in &to_remove {
            sessions.remove(id);
        }

        Ok(to_remove.len())
    }

    /// Get number of active sessions
    pub async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_session_creation() {
        let temp_dir = TempDir::new().unwrap();
        let nzb_data = r#"<?xml version="1.0" encoding="utf-8"?>
            <nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
                <file subject="test.par2">
                    <segments>
                        <segment id="test@example.com" size="1000"/>
                    </segments>
                </file>
                <file subject="test.rar">
                    <segments>
                        <segment id="test2@example.com" size="2000"/>
                    </segments>
                </file>
            </nzb>"#;

        let session = Session::new(nzb_data, temp_dir.path().to_path_buf()).unwrap();

        assert_eq!(session.par2_files.len(), 1);
        assert_eq!(session.rar_files.len(), 1);
        assert!(!session.is_ready_for_streaming());
    }

    #[tokio::test]
    async fn test_session_manager() {
        let temp_dir = TempDir::new().unwrap();
        let manager = SessionManager::new(temp_dir.path().to_path_buf());

        let nzb_data = r#"<?xml version="1.0" encoding="utf-8"?>
            <nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
                <file subject="test.par2">
                    <segments>
                        <segment id="test@example.com" size="1000"/>
                    </segments>
                </file>
            </nzb>"#;

        let session_id = manager.create_session(nzb_data).await.unwrap();
        assert_eq!(manager.session_count().await, 1);

        let session = manager.get_session(session_id).await.unwrap();
        let session_guard = session.lock().await;
        assert_eq!(session_guard.par2_files.len(), 1);
    }
}
