use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use bytes::Bytes;
use mime_guess::from_path;

pub fn create_streaming_response(
    data: Bytes,
    range_headers: Option<HeaderMap>,
    filename: &str,
) -> Response<Body> {
    let content_type = from_path(filename).first_or_octet_stream().to_string();

    let mut response = Response::builder();

    // Set status code based on whether it's a range request
    let status = if range_headers.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };

    response = response.status(status);

    // Add range headers if provided
    if let Some(headers) = range_headers {
        for (key, value) in headers {
            if let Some(key) = key {
                response = response.header(key, value);
            }
        }
    } else {
        // Add basic headers for non-range responses
        response = response
            .header("Content-Type", content_type)
            .header("Content-Length", data.len().to_string())
            .header("Accept-Ranges", "bytes");
    }

    // Create the response body
    let body = Body::from(data);

    response.body(body).unwrap_or_else(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::empty())
            .unwrap()
    })
}

pub fn create_error_response(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(format!(r#"{{"error": "{message}"}}"#)))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .unwrap()
        })
}

// Helper to create a mock file response for testing
pub fn create_test_file_response(size: u64, _filename: &str) -> Bytes {
    // Create test data - alternating pattern for easy identification
    let mut data = Vec::with_capacity(size as usize);
    for i in 0..size {
        data.push((i % 256) as u8);
    }
    Bytes::from(data)
}
