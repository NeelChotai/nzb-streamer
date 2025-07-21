// Placeholder for RAR data structures

#[derive(Debug, Clone)]
pub struct RarArchive {
    pub files: Vec<RarFile>,
}

#[derive(Debug, Clone)]
pub struct RarFile {
    pub name: String,
    pub size: u64,
    pub offset: u64,
}

impl Default for RarArchive {
    fn default() -> Self {
        Self::new()
    }
}

impl RarArchive {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }
}