use bytes::Bytes;
use std::{
    io::{Cursor, SeekFrom},
    path::Path,
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
};

use crate::archive::error::ArchiveError;

const RAR_SIGNATURE: [u8; 7] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const MKV_SIGNATURE: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

const RAR_MAIN_HEAD: u8 = 0x73;
const RAR_FILE_HEAD: u8 = 0x74;
const RAR_ENDARC_HEAD: u8 = 0x7B;

pub async fn analyse_rar_volume(path: &Path, is_first: bool) -> Result<(u64, u64), ArchiveError> {
    let mut file = File::open(path).await?;
    let file_size = file.metadata().await?.len();

    let mut buffer = vec![0u8; 1024.min(file_size as usize)]; // TODO: how much of the file do we need?
    file.read_exact(&mut buffer).await?;

    analyse_rar_buffer(&buffer.into(), is_first).await
}

pub async fn analyse_rar_buffer(
    buffer: &Bytes,
    is_first: bool,
) -> Result<(u64, u64), ArchiveError> {
    let rar_offset = buffer
        .windows(7)
        .position(|w| w == RAR_SIGNATURE)
        .map(|offset| offset as u64 + 7)
        .ok_or(ArchiveError::MalformedRar)?;

    let mut cursor = Cursor::new(buffer);
    cursor.seek(SeekFrom::Start(rar_offset)).await?;

    let mut data_offset = 0;
    let mut data_length = 0;

    while let Ok(_header_crc) = cursor.read_u16_le().await {
        let header_type = cursor.read_u8().await?;
        let _ = cursor.read_u16_le().await?; // flags
        let header_size = cursor.read_u16_le().await?;

        match header_type {
            RAR_MAIN_HEAD => {
                cursor
                    .seek(SeekFrom::Current(header_size as i64 - 7))
                    .await?;
            }
            RAR_FILE_HEAD => {
                let pack_size = cursor.read_u32_le().await? as u64;
                let _ = cursor.read_u32_le().await?; // unpacked size

                // Skip remaining header
                let skip_size = header_size as i64 - 7 - 8;
                cursor.seek(SeekFrom::Current(skip_size)).await?;

                data_offset = cursor.stream_position().await?;
                data_length = pack_size;

                // For first volume, find MKV signature
                if is_first {
                    let mut search_buf = vec![0u8; 1024.min(pack_size as usize)];
                    let current_pos = data_offset;
                    cursor.read_exact(&mut search_buf).await?;

                    if let Some(mkv_offset) = search_buf.windows(4).position(|w| w == MKV_SIGNATURE)
                    {
                        data_offset = current_pos + mkv_offset as u64;
                        data_length = pack_size - mkv_offset as u64;
                    }
                }

                break;
            }
            RAR_ENDARC_HEAD => break,
            _ => {
                cursor
                    .seek(SeekFrom::Current(header_size as i64 - 7))
                    .await?;
            }
        }
    }

    Ok((data_offset, data_length))
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum RarExt {
    Main,
    Part(u32),
}

impl RarExt {
    pub fn from_filename(filename: &Path) -> Option<Self> {
        if filename.extension().is_some_and(|ext| ext == "rar") {
            Some(RarExt::Main)
        } else {
            extract_rar_number(filename.file_name().unwrap().to_str().unwrap()).map(RarExt::Part)
        }
    }
}

impl Ord for RarExt {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use RarExt::*;
        match (self, other) {
            (Main, Main) => std::cmp::Ordering::Equal,
            (Main, _) => std::cmp::Ordering::Less,
            (_, Main) => std::cmp::Ordering::Greater,
            (Part(a), Part(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for RarExt {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn extract_rar_number(filename: &str) -> Option<u32> {
    filename.rsplit_once('.').and_then(|(_, ext)| {
        ext.strip_prefix('r')
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .and_then(|s| s.parse().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rar_number() {
        let cases = [
            ("file.rar", None),
            ("file.r00", Some(0)),
            ("file.r01", Some(1)),
            ("file.r99", Some(99)),
            ("file.r9999", Some(9999)),
            ("roll.roll.on.r00", Some(0)),
            ("path/to/file.r05", Some(5)),
            ("file.ra0", None), // Not a number
            ("file.r", None),   // No digits
            ("r00", None),      // No dot
            ("file.txt", None), // Wrong extension
        ];

        for (input, expected) in cases {
            assert_eq!(
                extract_rar_number(input),
                expected,
                "Failed for input: {input}"
            );
        }
    }

    #[test]
    fn test_rar_sorting() {
        let mut files = vec!["file.r02", "file.r00", "file.rar", "file.r10", "file.r01"];

        files.sort_by_key(|name| {
            if name.ends_with(".rar") {
                (0, 0)
            } else {
                (1, extract_rar_number(name).unwrap_or(999))
            }
        });

        assert_eq!(
            files,
            vec!["file.rar", "file.r00", "file.r01", "file.r02", "file.r10",]
        );
    }
}
