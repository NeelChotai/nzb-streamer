use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use byteorder::{ByteOrder, LittleEndian};
use nom::{
    bytes::complete::{tag, take},
    number::complete::le_u64,
    IResult,
};

use crate::par2::error::Par2Error;
use crate::par2::packet::{FileDescPacket, IFSCPacket, MainPacket, Packet};
use crate::par2::par2_crc32;

const PAR_PKT_ID: &[u8] = b"PAR2\x00PKT";
const PAR_MAIN_ID: &[u8] = b"PAR 2.0\x00Main\x00\x00\x00\x00";
const PAR_FILE_ID: &[u8] = b"PAR 2.0\x00FileDesc";
const PAR_SLICE_ID: &[u8] = b"PAR 2.0\x00IFSC\x00\x00\x00\x00";

const HEADER_SIZE: u64 = 64;
const MD5_SIZE: usize = 16;
const FILE_ID_SIZE: usize = 16;
const RECOVERY_SET_SIZE: usize = 16;
const PACKET_TYPE_SIZE: usize = 16;
const CRC_ENTRY_SIZE: usize = 20;

#[derive(Debug, Clone)]
pub struct FilePar2Info {
    pub hash16k: Vec<u8>,
    pub filesize: u64,
    pub filehash: u32,
}

pub fn parse_par2_file(path: &Path) -> Result<HashMap<String, FilePar2Info>, Par2Error> {
    let mut file = File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let packets = scan_for_packets(&buffer)?;
    build_file_info(packets)
}

fn scan_for_packets(buffer: &[u8]) -> Result<Vec<Packet>, Par2Error> {
    let mut packets = Vec::new();
    let mut pos = 0;

    while pos + HEADER_SIZE as usize <= buffer.len() {
        // Look for packet header
        if &buffer[pos..pos + PAR_PKT_ID.len()] != PAR_PKT_ID {
            pos += 1;
            continue;
        }

        // Parse packet at this position
        match parse_packet(&buffer[pos..]) {
            Ok((remaining, Some(packet))) => {
                packets.push(packet);
                pos = buffer.len() - remaining.len();
            }
            Ok((_, None)) => {
                // Invalid packet, skip
                pos += 1;
            }
            Err(_) => {
                // Parse error, skip
                pos += 1;
            }
        }
    }

    Ok(packets)
}

fn parse_packet(input: &[u8]) -> IResult<&[u8], Option<Packet>> {
    let (input, _) = tag(PAR_PKT_ID)(input)?;
    let (input, pack_len) = le_u64(input)?;

    if pack_len < HEADER_SIZE {
        return Ok((input, None));
    }

    let pack_len = pack_len as usize;
    let total_needed = pack_len - MD5_SIZE - PAR_PKT_ID.len() - 8; // Already consumed header + length

    if input.len() < total_needed {
        return Ok((&input[input.len()..], None));
    }

    // Skip MD5 checksum
    let (input, _md5) = take(MD5_SIZE)(input)?;

    // Get the data portion
    let data_len = pack_len - (PAR_PKT_ID.len() + 8 + MD5_SIZE); // Total packet minus what we've consumed
    let (remaining, data) = take(data_len)(input)?;

    // Skip Recovery Set ID and get packet type
    if data.len() < RECOVERY_SET_SIZE + PACKET_TYPE_SIZE {
        return Ok((remaining, None));
    }

    let packet_type = &data[RECOVERY_SET_SIZE..RECOVERY_SET_SIZE + PACKET_TYPE_SIZE];

    let packet = match packet_type {
        PAR_MAIN_ID => parse_main_packet(data).ok(),
        PAR_FILE_ID => parse_file_packet(data).ok(),
        PAR_SLICE_ID => parse_slice_packet(data).ok(),
        _ => None,
    };

    Ok((remaining, packet))
}

fn parse_main_packet(data: &[u8]) -> Result<Packet, Par2Error> {
    let offset = RECOVERY_SET_SIZE + PACKET_TYPE_SIZE;
    let (_, slice_size) =
        le_u64::<_, nom::error::Error<_>>(&data[offset..]).map_err(|_| Par2Error::Parse)?;

    Ok(Packet::Main(MainPacket { slice_size }))
}

fn parse_file_packet(data: &[u8]) -> Result<Packet, Par2Error> {
    let min_size = RECOVERY_SET_SIZE + PACKET_TYPE_SIZE + FILE_ID_SIZE + MD5_SIZE + MD5_SIZE + 8; // 8 for filesize
    if data.len() < min_size {
        return Err(Par2Error::Parse);
    }

    let offset = RECOVERY_SET_SIZE + PACKET_TYPE_SIZE;
    let file_id = hex::encode(&data[offset..offset + FILE_ID_SIZE]);
    let hash16k_offset = offset + FILE_ID_SIZE + MD5_SIZE;
    let hash16k = data[hash16k_offset..hash16k_offset + MD5_SIZE].to_vec();
    let filesize_offset = hash16k_offset + MD5_SIZE;
    let filesize = LittleEndian::read_u64(&data[filesize_offset..filesize_offset + 8]);

    // Parse filename
    let filename_offset = filesize_offset + 8;
    let filename_bytes = &data[filename_offset..];
    let filename_end = filename_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(filename_bytes.len());
    let filename = String::from_utf8_lossy(&filename_bytes[..filename_end]).to_string();

    Ok(Packet::FileDesc(FileDescPacket {
        file_id,
        filename,
        hash16k,
        filesize,
    }))
}

fn parse_slice_packet(data: &[u8]) -> Result<Packet, Par2Error> {
    let min_size = RECOVERY_SET_SIZE + PACKET_TYPE_SIZE + FILE_ID_SIZE;
    if data.len() < min_size {
        return Err(Par2Error::Parse);
    }

    let offset = RECOVERY_SET_SIZE + PACKET_TYPE_SIZE;
    let file_id = hex::encode(&data[offset..offset + FILE_ID_SIZE]);
    let mut crcs = Vec::new();

    let crc_offset = offset + FILE_ID_SIZE;
    for chunk in data[crc_offset..].chunks_exact(CRC_ENTRY_SIZE) {
        let crc = LittleEndian::read_u32(&chunk[16..20]);
        crcs.push(crc);
    }

    Ok(Packet::IFSC(IFSCPacket { file_id, crcs }))
}

fn build_file_info(packets: Vec<Packet>) -> Result<HashMap<String, FilePar2Info>, Par2Error> {
    let mut file_info: HashMap<String, FileDescPacket> = HashMap::new();
    let mut file_crcs: HashMap<String, Vec<u32>> = HashMap::new();
    let mut slice_size = 0u64;

    for packet in packets {
        match packet {
            Packet::Main(main) => {
                slice_size = main.slice_size;
            }
            Packet::FileDesc(desc) => {
                file_info.insert(desc.file_id.clone(), desc);
            }
            Packet::IFSC(ifsc) => {
                file_crcs.entry(ifsc.file_id).or_default().extend(ifsc.crcs);
            }
        }
    }

    if slice_size == 0 {
        return Err(Par2Error::MissingMainPacket);
    }

    let coeff = par2_crc32::xpow8n(slice_size);
    let mut result = HashMap::new();

    for (file_id, desc) in file_info {
        if let Some(crcs) = file_crcs.get(&file_id) {
            if crcs.is_empty() {
                continue;
            }

            let filehash = compute_file_hash(desc.filesize, crcs, slice_size, coeff);

            result.insert(
                desc.filename,
                FilePar2Info {
                    hash16k: desc.hash16k,
                    filesize: desc.filesize,
                    filehash,
                },
            );
        }
    }

    Ok(result)
}

fn compute_file_hash(filesize: u64, crcs: &[u32], slice_size: u64, coeff: u32) -> u32 {
    let slices = (filesize / slice_size) as usize;
    let mut crc32 = 0u32;

    for &crc in crcs.iter().take(slices) {
        crc32 = par2_crc32::multiply(crc32, coeff) ^ crc;
    }

    let tail_size = filesize % slice_size;
    if tail_size > 0 && slices < crcs.len() {
        let tail_crc = par2_crc32::zero_unpad(crcs[slices], slice_size - tail_size);
        crc32 = par2_crc32::combine(crc32, tail_crc, tail_size);
    }

    crc32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_par2() {
        let path = Path::new("/tmp/downloaded/db5839e271decea10e9e713aa0e20573.par2");

        match parse_par2_file(path) {
            Ok(table) => {
                for (filename, info) in &table {
                    println!("{filename}: FilePar2Info {{");
                    println!("    hash16k: {:?},", info.hash16k);
                    println!("    filesize: {},", info.filesize);
                    println!("    filehash: {},", info.filehash);
                    println!("}}");
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    #[test]
    fn test_parse_par2_again() {
        //let path = Path::new("/tmp/test_nntp/downloads/0ae5e6761c69051d401054734c174e91.par2");
        let path = Path::new("/tmp/nzb/downloads/downloads/inter/Foundation.S03E03.When.a.Book.Finds.You.1080p.ATVP.WEB-DL.DDP5.1.H.264-NTb.#4/0ae5e6761c69051d401054734c174e91.par2");

        match parse_par2_file(path) {
            Ok(table) => {
                for (filename, info) in &table {
                    println!("{filename}: FilePar2Info {{");
                    println!("    hash16k: {:?},", info.hash16k);
                    println!("    filesize: {},", info.filesize);
                    println!("    filehash: {},", info.filehash);
                    println!("}}");
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    #[test]
    fn test_parse_par2_again_again() {
        let path = Path::new("/tmp/test_nntp/downloads/test.par2");

        match parse_par2_file(path) {
            Ok(table) => {
                for (filename, info) in &table {
                    println!("{filename}: FilePar2Info {{");
                    println!("    hash16k: {:?},", info.hash16k);
                    println!("    filesize: {},", info.filesize);
                    println!("    filehash: {},", info.filehash);
                    println!("}}");
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }
}
