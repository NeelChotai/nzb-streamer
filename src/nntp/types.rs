use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NntpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub use_tls: bool,
    pub connection_timeout: u64, // seconds
    pub read_timeout: u64,       // seconds
    pub max_connections: usize,
}

impl Default for NntpConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 563, // Default NNTP SSL port
            username: String::new(),
            password: String::new(),
            use_tls: true,
            connection_timeout: 30,
            read_timeout: 60,
            max_connections: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NntpResponse {
    pub code: u16,
    pub message: String,
    pub lines: Vec<String>,
}

impl NntpResponse {
    pub fn new(code: u16, message: String) -> Self {
        Self {
            code,
            message,
            lines: Vec::new(),
        }
    }

    pub fn with_lines(mut self, lines: Vec<String>) -> Self {
        self.lines = lines;
        self
    }

    pub fn is_success(&self) -> bool {
        self.code < 400
    }

    pub fn is_error(&self) -> bool {
        self.code >= 400
    }
}

#[derive(Debug, Clone)]
pub struct Article {
    pub message_id: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Article {
    pub fn new(message_id: String) -> Self {
        Self {
            message_id,
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }

    pub fn subject(&self) -> Option<&String> {
        self.headers.get("Subject")
    }

    pub fn from(&self) -> Option<&String> {
        self.headers.get("From")
    }

    pub fn date(&self) -> Option<&String> {
        self.headers.get("Date")
    }

    pub fn is_yenc(&self) -> bool {
        self.body.windows(5).any(|w| w == b"=ybegin")
    }
}

#[derive(Debug, Clone)]
pub struct YencHeader {
    pub line: u32,
    pub size: u64,
    pub part: Option<u32>,
    pub total: Option<u32>,
    pub name: String,
    pub crc32: Option<u32>,
}

impl YencHeader {
    pub fn parse(line: &str) -> Option<Self> {
        if !line.starts_with("=ybegin") {
            return None;
        }

        let mut header = YencHeader {
            line: 128, // Default line length
            size: 0,
            part: None,
            total: None,
            name: String::new(),
            crc32: None,
        };

        // Parse key=value pairs
        for token in line.split_whitespace().skip(1) {
            if let Some((key, value)) = token.split_once('=') {
                match key {
                    "line" => header.line = value.parse().unwrap_or(128),
                    "size" => header.size = value.parse().unwrap_or(0),
                    "part" => header.part = value.parse().ok(),
                    "total" => header.total = value.parse().ok(),
                    "name" => header.name = value.to_string(),
                    "crc32" => header.crc32 = u32::from_str_radix(value, 16).ok(),
                    _ => {}
                }
            }
        }

        Some(header)
    }
}

#[derive(Debug, Clone)]
pub struct YencFooter {
    pub size: u64,
    pub part: Option<u32>,
    pub pcrc32: Option<u32>, // Part CRC32
    pub crc32: Option<u32>,  // Total CRC32
}

impl YencFooter {
    pub fn parse(line: &str) -> Option<Self> {
        if !line.starts_with("=yend") {
            return None;
        }

        let mut footer = YencFooter {
            size: 0,
            part: None,
            pcrc32: None,
            crc32: None,
        };

        // Parse key=value pairs
        for token in line.split_whitespace().skip(1) {
            if let Some((key, value)) = token.split_once('=') {
                match key {
                    "size" => footer.size = value.parse().unwrap_or(0),
                    "part" => footer.part = value.parse().ok(),
                    "pcrc32" => footer.pcrc32 = u32::from_str_radix(value, 16).ok(),
                    "crc32" => footer.crc32 = u32::from_str_radix(value, 16).ok(),
                    _ => {}
                }
            }
        }

        Some(footer)
    }
}

#[derive(Debug)]
pub enum NntpCommand {
    Connect,
    Authenticate { username: String, password: String },
    ModeReader,
    Article { message_id: String },
    Body { message_id: String },
    Head { message_id: String },
    Quit,
}

impl NntpCommand {
    pub fn as_string(&self) -> String {
        match self {
            NntpCommand::Connect => "CONNECT".to_string(),
            NntpCommand::Authenticate { username, password } => {
                format!("AUTHINFO USER {username}\r\nAUTHINFO PASS {password}")
            }
            NntpCommand::ModeReader => "MODE READER".to_string(),
            NntpCommand::Article { message_id } => format!("ARTICLE {message_id}"),
            NntpCommand::Body { message_id } => format!("BODY {message_id}"),
            NntpCommand::Head { message_id } => format!("HEAD {message_id}"),
            NntpCommand::Quit => "QUIT".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yenc_header_parsing() {
        let line = "=ybegin line=128 size=123456 part=1 total=3 name=movie.mp4";
        let header = YencHeader::parse(line).unwrap();
        
        assert_eq!(header.line, 128);
        assert_eq!(header.size, 123456);
        assert_eq!(header.part, Some(1));
        assert_eq!(header.total, Some(3));
        assert_eq!(header.name, "movie.mp4");
    }

    #[test]
    fn test_yenc_footer_parsing() {
        let line = "=yend size=123456 part=1 pcrc32=12345678";
        let footer = YencFooter::parse(line).unwrap();
        
        assert_eq!(footer.size, 123456);
        assert_eq!(footer.part, Some(1));
        assert_eq!(footer.pcrc32, Some(0x12345678));
    }

    #[test]
    fn test_nntp_response() {
        let response = NntpResponse::new(200, "OK".to_string());
        assert!(response.is_success());
        assert!(!response.is_error());

        let error_response = NntpResponse::new(430, "No such article".to_string());
        assert!(!error_response.is_success());
        assert!(error_response.is_error());
    }
}