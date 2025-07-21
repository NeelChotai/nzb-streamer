use nzb_streamer::nzb::NzbParser;
use std::io::Cursor;

#[tokio::test]
async fn test_parse_simple_nzb() {
    let nzb_content = include_str!("./fixtures/simple.nzb");
    let cursor = Cursor::new(nzb_content);
    
    let nzb = NzbParser::parse(cursor).unwrap();
    
    // Check basic structure
    assert_eq!(nzb.files.len(), 2);
    assert_eq!(nzb.total_size(), 350000); // 100000 + 50000 + 200000
    
    // Check first file
    let first_file = &nzb.files[0];
    assert!(first_file.path.contains("Test Movie.mp4") || first_file.path.contains("mp4"));
    assert_eq!(first_file.segments.len(), 2);
    assert_eq!(first_file.size, 150000); // 100000 + 50000
    assert!(first_file.is_complete());
    
    // Check segments are sorted
    assert_eq!(first_file.segments[0].number, 1);
    assert_eq!(first_file.segments[1].number, 2);
    assert_eq!(first_file.segments[0].message_id, "<msg1@server.com>");
    
    // Check second file
    let second_file = &nzb.files[1];
    assert!(second_file.path.contains("r00") || second_file.path.contains("Test Movie"));
    assert_eq!(second_file.segments.len(), 1);
    assert_eq!(second_file.size, 200000);
    
    // Test file finding
    assert!(nzb.find_video_files().len() > 0);
}

#[test]
fn test_nzb_file_types() {
    use nzb_streamer::nzb::types::{is_video_file, is_rar_file, normalize_path};
    
    // Test video file detection
    assert!(is_video_file("movie.mp4"));
    assert!(is_video_file("MOVIE.MP4"));
    assert!(is_video_file("video.mkv"));
    assert!(is_video_file("film.avi"));
    assert!(!is_video_file("archive.rar"));
    assert!(!is_video_file("document.txt"));
    
    // Test RAR file detection
    assert!(is_rar_file("archive.rar"));
    assert!(is_rar_file("archive.r00"));
    assert!(is_rar_file("archive.r01"));
    assert!(is_rar_file("archive.r99"));
    assert!(!is_rar_file("video.mp4"));
    assert!(!is_rar_file("archive.zip"));
    
    // Test path normalization
    assert_eq!(normalize_path("folder\\file.txt"), "folder/file.txt");
    assert_eq!(normalize_path("/folder/file.txt"), "folder/file.txt");
    assert_eq!(normalize_path("folder/file.txt"), "folder/file.txt");
}

#[test]
fn test_empty_nzb() {
    let empty_nzb = r#"<?xml version="1.0" encoding="utf-8" ?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
</nzb>"#;
    
    let cursor = Cursor::new(empty_nzb);
    let result = NzbParser::parse(cursor);
    
    // Should fail validation due to no files
    assert!(result.is_err());
}

#[test]
fn test_invalid_xml() {
    let invalid_xml = "This is not XML at all";
    let cursor = Cursor::new(invalid_xml);
    let result = NzbParser::parse(cursor);
    
    assert!(result.is_err());
}