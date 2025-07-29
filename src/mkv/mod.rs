use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub mod mkv;

const RAR_SIGNATURE: [u8; 7] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const MKV_SIGNATURE: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

fn read_u16_le<R: Read>(reader: &mut R) -> io::Result<u16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32_le<R: Read>(reader: &mut R) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn extract_file_data(path: &Path, output: &mut File, is_first: bool) -> io::Result<u64> {
    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();

    // Find RAR signature
    let mut sig_buf = vec![0u8; 1024.min(file_size as usize)];
    file.read(&mut sig_buf)?;

    let rar_offset = sig_buf
        .windows(7)
        .position(|w| w == RAR_SIGNATURE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "No RAR signature"))?;

    file.seek(SeekFrom::Start(rar_offset as u64 + 7))?; // Skip signature

    let mut total_copied = 0u64;

    // Parse volume headers
    loop {
        // Read base header
        let header_crc = match read_u16_le(&mut file) {
            Ok(v) => v,
            Err(_) => break, // End of file
        };

        let header_type = {
            let mut buf = [0u8; 1];
            if file.read_exact(&mut buf).is_err() {
                break;
            }
            buf[0]
        };

        let header_flags = read_u16_le(&mut file)?;
        let header_size = read_u16_le(&mut file)?;

        println!(
            "    Header: type=0x{header_type:02X}, flags=0x{header_flags:04X}, size={header_size}"
        );

        match header_type {
            0x73 => {
                // MAIN_HEAD - skip remaining bytes
                file.seek(SeekFrom::Current(header_size as i64 - 7))?;
            }
            0x74 => {
                // FILE_HEAD - this contains our data
                let pack_size = read_u32_le(&mut file)?;
                let unp_size = read_u32_le(&mut file)?;

                // Calculate remaining header bytes to skip
                let remaining_header = header_size as i64 - 7 - 8; // Already read 7 + 8 bytes
                file.seek(SeekFrom::Current(remaining_header))?;

                println!("    FILE_HEAD: pack_size={pack_size}, unp_size={unp_size}");

                // For the first volume with MKV, find where MKV actually starts
                if is_first && total_copied == 0 {
                    // Read a bit to find MKV signature
                    let mut search_buf = vec![0u8; 1024.min(pack_size as usize)];
                    let current_pos = file.stream_position()?;
                    file.read(&mut search_buf)?;

                    if let Some(mkv_offset) = search_buf.windows(4).position(|w| w == MKV_SIGNATURE)
                    {
                        println!("    Found MKV at offset {mkv_offset} within packed data");
                        file.seek(SeekFrom::Start(current_pos + mkv_offset as u64))?;

                        // Copy from MKV start
                        let to_copy = pack_size as u64 - mkv_offset as u64;
                        let mut limited = file.take(to_copy);
                        let copied = io::copy(&mut limited, output)?;
                        file = limited.into_inner();

                        total_copied += copied;
                        println!("    Copied {copied} bytes (MKV data)");
                    } else {
                        file.seek(SeekFrom::Start(current_pos))?;
                    }
                } else {
                    // Copy the packed data
                    let mut limited = file.take(pack_size as u64);
                    let copied = io::copy(&mut limited, output)?;
                    file = limited.into_inner();

                    total_copied += copied;
                    println!("    Copied {copied} bytes");
                }
            }
            0x7B => {
                // ENDARC_HEAD - end of archive
                println!("    End of archive marker");
                break;
            }
            _ => {
                // Other header - skip
                file.seek(SeekFrom::Current(header_size as i64 - 7))?;
            }
        }
    }

    Ok(total_copied)
}

fn main() -> io::Result<()> {
    let input_dir = Path::new("/tmp/iamtesting");
    let output_path = Path::new("/tmp/test_rust.mkv");

    // Collect volumes
    let mut volumes = vec![];
    if input_dir.join("yay.rar").exists() {
        volumes.push("yay.rar".to_string());
    }
    for i in 0..=48 {
        let name = format!("yay.r{i:02}");
        if input_dir.join(&name).exists() {
            volumes.push(name);
        }
    }

    println!("Processing {} volumes\n", volumes.len());

    let mut output = File::create(output_path)?;
    let mut total_written = 0;

    for (idx, volume) in volumes.iter().enumerate() {
        println!("{volume}:");
        let path = input_dir.join(volume);

        match extract_file_data(&path, &mut output, idx == 0) {
            Ok(bytes) => {
                total_written += bytes;
                println!("  Total from this volume: {bytes} bytes\n");
            }
            Err(e) => {
                println!("  ERROR: {e}\n");
            }
        }
    }

    println!(
        "Total written: {} bytes ({:.2} MB)",
        total_written,
        total_written as f64 / 1024.0 / 1024.0
    );

    // Verify
    drop(output);
    if let Ok(mut check) = File::open(output_path) {
        let mut header = [0u8; 4];
        if check.read_exact(&mut header).is_ok() && header == MKV_SIGNATURE {
            println!("✓ Valid MKV file created");
        } else {
            println!("✗ Invalid MKV file");
        }
    }

    println!(
        "\nTest: mpv {} 2>&1 | grep -c 'Invalid'",
        output_path.display()
    );

    Ok(())
}
