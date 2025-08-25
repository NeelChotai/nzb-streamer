use bytes::Bytes;
use md5::{Digest, Md5};

pub fn extract_filename(subject: &str) -> Option<&str> {
    if let Some(start) = subject.find('"') {
        if let Some(end) = subject[start + 1..].find('"') {
            return Some(&subject[start + 1..start + 1 + end]);
        }
    }

    subject.split_whitespace().next()
}

pub fn extract_yenc_data(article: &[u8]) -> Bytes {
    let mut in_body = false;
    article
        .split(|&b| b == b'\n')
        .filter_map(|line| match line {
            _ if line.starts_with(b"=ybegin") => {
                in_body = true;
                None
            }
            _ if line.starts_with(b"=ypart") => {
                in_body = true;
                None
            }
            _ if line.starts_with(b"=yend") => {
                in_body = false;
                None
            }
            _ if in_body => Some(trim_line_endings(line)),
            _ => None,
        })
        .flatten()
        .copied()
        .collect()
}

fn trim_line_endings(line: &[u8]) -> &[u8] {
    match line {
        [.., b'\r', b'\n'] => &line[..line.len() - 2],
        [.., b'\n'] => &line[..line.len() - 1],
        _ => line,
    }
}

pub fn compute_hash16k(bytes: &[u8]) -> Vec<u8> {
    const SIXTEEN_KB: usize = 16384;
    let len = bytes.len().min(SIXTEEN_KB);

    Md5::new().chain_update(&bytes[..len]).finalize().to_vec()
}
