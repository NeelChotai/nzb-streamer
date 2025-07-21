use crate::error::{NzbStreamerError, Result};
use crate::nzb::types::*;
use chrono::{DateTime, Utc};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::Reader;
use std::io::BufRead;
use std::str;
use tracing::{debug, warn};

pub struct NzbParser;

impl NzbParser {
    pub fn parse<R: BufRead>(reader: R) -> Result<Nzb> {
        let mut xml_reader = Reader::from_reader(reader);
        xml_reader.trim_text(true);
        
        let mut nzb = Nzb::new();
        let mut buf = Vec::new();
        let mut current_file: Option<NzbFile> = None;
        let mut current_segment: Option<NzbSegment> = None;
        let mut current_groups = Vec::new();
        let mut in_head = false;
        let mut in_group = false;

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    match e.name() {
                        QName(b"nzb") => {
                            // Parse NZB root attributes if any
                            Self::parse_nzb_attributes(e, &mut nzb)?;
                        }
                        QName(b"head") => {
                            in_head = true;
                        }
                        QName(b"meta") if in_head => {
                            Self::parse_meta_tag(e, &mut nzb.meta)?;
                        }
                        QName(b"file") => {
                            current_file = Some(Self::parse_file_start(e)?);
                            current_groups.clear();
                        }
                        QName(b"group") => {
                            in_group = true;
                        }
                        QName(b"segment") => {
                            current_segment = Some(Self::parse_segment_start(e)?);
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(ref e)) => {
                    match e.name() {
                        QName(b"head") => {
                            in_head = false;
                        }
                        QName(b"file") => {
                            if let Some(mut file) = current_file.take() {
                                file.groups = current_groups.clone();
                                file.sort_segments();
                                
                                // Extract filename from subject if path is empty
                                if file.path.is_empty() {
                                    file.path = Self::extract_filename_from_subject(&file.subject);
                                }
                                
                                nzb.files.push(file);
                            }
                        }
                        QName(b"segment") => {
                            if let (Some(segment), Some(ref mut file)) = (current_segment.take(), current_file.as_mut()) {
                                file.add_segment(segment);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(e)) => {
                    let text = e.unescape()
                        .map_err(|e| NzbStreamerError::NzbParsing(e.to_string()))?;
                    let text = text.trim().to_string();
                    
                    if let Some(ref mut segment) = current_segment {
                        segment.message_id = text;
                    } else if in_group && !text.is_empty() {
                        current_groups.push(text);
                        in_group = false;
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(NzbStreamerError::NzbParsing(e.to_string())),
                _ => {}
            }
            buf.clear();
        }

        debug!("Parsed NZB with {} files", nzb.files.len());
        
        // Validate the parsed NZB
        Self::validate_nzb(&nzb)?;
        
        Ok(nzb)
    }

    fn parse_nzb_attributes(element: &BytesStart, nzb: &mut Nzb) -> Result<()> {
        for attr in element.attributes() {
            let attr = attr.map_err(|e| NzbStreamerError::NzbParsing(e.to_string()))?;
            let key = str::from_utf8(attr.key.as_ref())?;
            let value = attr.unescape_value()
                .map_err(|e| NzbStreamerError::NzbParsing(e.to_string()))?;
            
            nzb.meta.attributes.insert(key.to_string(), value.to_string());
        }
        Ok(())
    }

    fn parse_meta_tag(
        element: &BytesStart,
        meta: &mut NzbMeta,
    ) -> Result<()> {
        let mut type_attr = None;
        
        for attr in element.attributes() {
            let attr = attr.map_err(|e| NzbStreamerError::NzbParsing(e.to_string()))?;
            if attr.key == QName(b"type") {
                type_attr = Some(attr.unescape_value()
                    .map_err(|e| NzbStreamerError::NzbParsing(e.to_string()))?
                    .to_string());
                break;
            }
        }
        
        if let Some(meta_type) = type_attr {
            // The content is in the next text event, but we need to handle this differently
            // For now, just store in attributes
            meta.attributes.insert(meta_type, String::new());
        }
        
        Ok(())
    }

    fn parse_file_start(element: &BytesStart) -> Result<NzbFile> {
        let mut poster = String::new();
        let mut date = Utc::now();
        let mut subject = String::new();

        for attr in element.attributes() {
            let attr = attr.map_err(|e| NzbStreamerError::NzbParsing(e.to_string()))?;
            let key = str::from_utf8(attr.key.as_ref())?;
            let value = attr.unescape_value()
                .map_err(|e| NzbStreamerError::NzbParsing(e.to_string()))?;

            match key {
                "poster" => poster = value.to_string(),
                "date" => {
                    date = Self::parse_date(&value)?;
                }
                "subject" => subject = value.to_string(),
                _ => {}
            }
        }

        Ok(NzbFile::new(subject, poster, date))
    }

    fn parse_segment_start(element: &BytesStart) -> Result<NzbSegment> {
        let mut bytes = 0u64;
        let mut number = 0u32;

        for attr in element.attributes() {
            let attr = attr.map_err(|e| NzbStreamerError::NzbParsing(e.to_string()))?;
            let key = str::from_utf8(attr.key.as_ref())?;
            let value = attr.unescape_value()
                .map_err(|e| NzbStreamerError::NzbParsing(e.to_string()))?;

            match key {
                "bytes" => {
                    bytes = value.parse()
                        .map_err(|_| NzbStreamerError::NzbParsing(format!("Invalid bytes value: {value}")))?;
                }
                "number" => {
                    number = value.parse()
                        .map_err(|_| NzbStreamerError::NzbParsing(format!("Invalid number value: {value}")))?;
                }
                _ => {}
            }
        }

        Ok(NzbSegment::new(String::new(), number, bytes))
    }


    fn parse_date(date_str: &str) -> Result<DateTime<Utc>> {
        // Try parsing Unix timestamp first
        if let Ok(timestamp) = date_str.parse::<i64>() {
            DateTime::from_timestamp(timestamp, 0)
                .ok_or_else(|| NzbStreamerError::NzbParsing(format!("Invalid timestamp: {timestamp}")))
        } else {
            // Try parsing RFC 3339 format
            DateTime::parse_from_rfc3339(date_str)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|_| NzbStreamerError::NzbParsing(format!("Invalid date format: {date_str}")))
        }
    }

    fn extract_filename_from_subject(subject: &str) -> String {
        // Common patterns in Usenet subjects:
        // "filename.ext" yEnc (1/50)
        // [01/50] - "filename.ext" yEnc
        // filename.ext (1/50)
        
        // Look for quoted filename
        if let Some(start) = subject.find('"') {
            if let Some(end) = subject[start + 1..].find('"') {
                let filename = &subject[start + 1..start + 1 + end];
                return normalize_path(filename);
            }
        }
        
        // Look for filename before yEnc or (x/y) pattern
        let parts: Vec<&str> = subject.split_whitespace().collect();
        for part in &parts {
            if part.contains('.') && !part.contains('/') && !part.contains('(') {
                return normalize_path(part);
            }
        }
        
        // Fallback to cleaned subject
        normalize_path(subject)
    }

    fn validate_nzb(nzb: &Nzb) -> Result<()> {
        if nzb.files.is_empty() {
            return Err(NzbStreamerError::NzbParsing("NZB contains no files".to_string()));
        }

        for (i, file) in nzb.files.iter().enumerate() {
            if file.segments.is_empty() {
                warn!("File {} has no segments: {}", i, file.path);
                continue;
            }

            for segment in &file.segments {
                if segment.message_id.is_empty() {
                    return Err(NzbStreamerError::NzbParsing(
                        format!("Empty message ID in file: {}", file.path)
                    ));
                }
                if segment.bytes == 0 {
                    warn!("Zero-byte segment in file: {}", file.path);
                }
            }
        }

        Ok(())
    }
}

impl From<std::str::Utf8Error> for NzbStreamerError {
    fn from(err: std::str::Utf8Error) -> Self {
        NzbStreamerError::NzbParsing(err.to_string())
    }
}