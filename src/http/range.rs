use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use std::ops::Range;

#[derive(Debug, Clone)]
pub struct RangeRequest {
    pub start: u64,
    pub end: Option<u64>,
}

impl RangeRequest {
    pub fn parse_range_header(
        headers: &HeaderMap,
        content_length: u64,
    ) -> Result<Option<RangeRequest>, StatusCode> {
        let range_header = match headers.get(header::RANGE) {
            Some(range) => range,
            None => return Ok(None),
        };

        let range_str = range_header.to_str().map_err(|_| StatusCode::BAD_REQUEST)?;

        if !range_str.starts_with("bytes=") {
            return Err(StatusCode::BAD_REQUEST);
        }

        let range_spec = &range_str[6..]; // Remove "bytes="

        // Handle first range only for now (HTTP/1.1 allows multiple ranges)
        let first_range = range_spec.split(',').next().unwrap().trim();

        let (start_str, end_str) = if let Some((start, end)) = first_range.split_once('-') {
            (start.trim(), end.trim())
        } else {
            return Err(StatusCode::BAD_REQUEST);
        };

        let (start, end) = if start_str.is_empty() {
            // Suffix range: "-500" means last 500 bytes
            let suffix_length: u64 = end_str.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
            let start = content_length.saturating_sub(suffix_length);
            (start, None)
        } else {
            let start = start_str.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
            let end = if end_str.is_empty() {
                None // Read until end
            } else {
                Some(end_str.parse().map_err(|_| StatusCode::BAD_REQUEST)?)
            };
            (start, end)
        };

        // Validate range
        if start >= content_length {
            return Err(StatusCode::RANGE_NOT_SATISFIABLE);
        }

        if let Some(end_pos) = end {
            if end_pos >= content_length || start > end_pos {
                return Err(StatusCode::RANGE_NOT_SATISFIABLE);
            }
        }

        Ok(Some(RangeRequest { start, end }))
    }

    pub fn to_range(&self, content_length: u64) -> Range<u64> {
        let end = self
            .end
            .unwrap_or(content_length - 1)
            .min(content_length - 1);
        self.start..end + 1
    }

    pub fn content_length(&self, total_length: u64) -> u64 {
        let end = self.end.unwrap_or(total_length - 1).min(total_length - 1);
        end - self.start + 1
    }
}

pub fn create_range_response_headers(
    range_request: &RangeRequest,
    content_length: u64,
    total_length: u64,
    content_type: &str,
) -> Result<HeaderMap, StatusCode> {
    let mut headers = HeaderMap::new();

    // Calculate actual end position
    let end = range_request
        .end
        .unwrap_or(total_length - 1)
        .min(total_length - 1);

    // Content-Range: bytes start-end/total
    let content_range = format!("bytes {}-{}/{}", range_request.start, end, total_length);
    headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&content_range).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );

    // Content-Length: actual bytes being sent
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&content_length.to_string())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );

    // Content-Type
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );

    // Accept-Ranges: bytes
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn test_parse_full_range() {
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-1023"));

        let range = RangeRequest::parse_range_header(&headers, 2048)
            .unwrap()
            .unwrap();
        assert_eq!(range.start, 0);
        assert_eq!(range.end, Some(1023));
    }

    #[test]
    fn test_parse_open_ended_range() {
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=1024-"));

        let range = RangeRequest::parse_range_header(&headers, 2048)
            .unwrap()
            .unwrap();
        assert_eq!(range.start, 1024);
        assert_eq!(range.end, None);
    }

    #[test]
    fn test_parse_suffix_range() {
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=-500"));

        let range = RangeRequest::parse_range_header(&headers, 2048)
            .unwrap()
            .unwrap();
        assert_eq!(range.start, 1548); // 2048 - 500
        assert_eq!(range.end, None);
    }

    #[test]
    fn test_to_range() {
        let range = RangeRequest {
            start: 100,
            end: Some(199),
        };
        let byte_range = range.to_range(1000);
        assert_eq!(byte_range, 100..200);
    }
}
