use std::{
    io::SeekFrom,
    path::Path,
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
};

use crate::stream::error::StreamError;

const RAR_SIGNATURE: [u8; 7] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const MKV_SIGNATURE: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

const RAR_MAIN_HEAD: u8 = 0x73;
const RAR_FILE_HEAD: u8 = 0x74;
const RAR_ENDARC_HEAD: u8 = 0x7B;

pub async fn analyse_rar_volume(path: &Path, is_first: bool) -> Result<(u64, u64), StreamError> {
    let mut file = File::open(path).await?;
    let file_size = file.metadata().await?.len();

    // Find RAR signature
    let mut sig_buf = vec![0u8; 1024.min(file_size as usize)];
    file.read_exact(&mut sig_buf).await?;

    let rar_offset = sig_buf
        .windows(7)
        .position(|w| w == RAR_SIGNATURE)
        .ok_or_else(|| StreamError::MalformedRar(path.display().to_string()))?
        as u64;

    file.seek(SeekFrom::Start(rar_offset + 7)).await?;

    let mut data_offset = 0;
    let mut data_length = 0;

    // Parse volume headers
    // loop {
    //     let _header_crc = match file.read_u16_le().await {
    //         Ok(v) => v,
    //         Err(_) => break, // EOF
    //     };

    //     let header_type = {
    //         let mut buf = [0u8; 1];
    //         file.read_exact(&mut buf).await?;
    //         buf[0]
    //     };

    //     let _ = file.read_u16_le().await?; // flags
    //     let header_size = file.read_u16_le().await?;

    //     match header_type {
    //         RAR_MAIN_HEAD => {
    //             file.seek(SeekFrom::Current(header_size as i64 - 7)).await?;
    //         }
    //         RAR_FILE_HEAD => {
    //             let pack_size = file.read_u32_le().await? as u64;
    //             let _ = file.read_u32_le().await?; // unpacked size

    //             // Skip remaining header
    //             let skip_size = header_size as i64 - 7 - 8;
    //             file.seek(SeekFrom::Current(skip_size)).await?;

    //             data_offset = file.stream_position().await?;
    //             data_length = pack_size;

    //             // For first volume, find MKV signature
    //             if is_first {
    //                 let mut search_buf = vec![0u8; 1024.min(pack_size as usize)];
    //                 let current_pos = data_offset;
    //                 file.read_exact(&mut search_buf).await?;

    //                 if let Some(mkv_offset) = search_buf.windows(4).position(|w| w == MKV_SIGNATURE)
    //                 {
    //                     data_offset = current_pos + mkv_offset as u64;
    //                     data_length = pack_size - mkv_offset as u64;
    //                 }
    //             }
    //             break;
    //         }
    //         RAR_ENDARC_HEAD => {
    //             break;
    //         }
    //         _ => {
    //             file.seek(SeekFrom::Current(header_size as i64 - 7)).await?;
    //         }
    //     }
    // }

    while let Ok(_header_crc) = file.read_u16_le().await {
        let header_type = file.read_u8().await?;
        let _ = file.read_u16_le().await?; // flags
        let header_size = file.read_u16_le().await?;

        match header_type {
            RAR_MAIN_HEAD => {
                file.seek(SeekFrom::Current(header_size as i64 - 7)).await?;
            }
            RAR_FILE_HEAD => {
                let pack_size = file.read_u32_le().await? as u64;
                let _ = file.read_u32_le().await?; // unpacked size

                // Skip remaining header
                let skip_size = header_size as i64 - 7 - 8;
                file.seek(SeekFrom::Current(skip_size)).await?;

                data_offset = file.stream_position().await?;
                data_length = pack_size;

                // For first volume, find MKV signature
                if is_first {
                    let mut search_buf = vec![0u8; 1024.min(pack_size as usize)];
                    let current_pos = data_offset;
                    file.read_exact(&mut search_buf).await?;

                    if let Some(mkv_offset) = search_buf.windows(4).position(|w| w == MKV_SIGNATURE)
                    {
                        data_offset = current_pos + mkv_offset as u64;
                        data_length = pack_size - mkv_offset as u64;
                    }
                }

                break;
            }
            RAR_ENDARC_HEAD => {
                break;
            }
            _ => {
                file.seek(SeekFrom::Current(header_size as i64 - 7)).await?;
            }
        }
    }

    Ok((data_offset, data_length))
}
