use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::par2::packet::{FileDescPacket, Packet};
use crate::par2::par2_crc32;
use crate::par2::{error::Par2Error, packet::parse_packet};

#[derive(Debug, Clone)]
pub struct FilePar2Info {
    pub hash16k: Vec<u8>,
    pub filesize: u64,
    pub filehash: u32,
}

pub fn parse_file(path: &Path) -> Result<HashMap<String, FilePar2Info>, Par2Error> {
    let mut buffer = Vec::new();
    File::open(path)?.read_to_end(&mut buffer)?;
    parse_buffer(&buffer)
}

pub fn parse_buffer(buffer: &[u8]) -> Result<HashMap<String, FilePar2Info>, Par2Error> {
    let packets = scan_for_packets(buffer);
    build_file_info(packets)
}

fn scan_for_packets(buffer: &[u8]) -> Vec<Packet> {
    let mut packets = Vec::new();
    let mut cursor = 0;

    while cursor < buffer.len() {
        if let Some((packet, length)) = parse_packet(&buffer[cursor..]) {
            packets.push(packet);
            cursor += length;
        } else {
            cursor += 1;
        }
    }

    packets
}

fn build_file_info(packets: Vec<Packet>) -> Result<HashMap<String, FilePar2Info>, Par2Error> {
    let mut file_info: HashMap<String, FileDescPacket> = HashMap::new();
    let mut file_crcs: HashMap<String, Vec<u32>> = HashMap::new();
    let mut slice_size = None;

    for packet in packets {
        match packet {
            Packet::Main(main) => slice_size = Some(main.slice_size),
            Packet::FileDesc(desc) => {
                file_info.insert(desc.file_id.clone(), desc);
            }
            Packet::IFSC(ifsc) => {
                file_crcs.entry(ifsc.file_id).or_default().extend(ifsc.crcs);
            }
        }
    }

    let slice_size = slice_size.ok_or(Par2Error::MissingMainPacket)?;
    let coeff = par2_crc32::xpow8n(slice_size);
    let out = file_info
        .into_iter()
        .filter_map(|(file_id, desc)| {
            file_crcs.get(&file_id).and_then(|crcs| {
                (!crcs.is_empty()).then(|| {
                    let filehash = compute_file_hash(desc.filesize, crcs, slice_size, coeff);
                    (
                        desc.filename.clone(),
                        FilePar2Info {
                            hash16k: desc.hash16k.to_vec(),
                            filesize: desc.filesize,
                            filehash,
                        },
                    )
                })
            })
        })
        .collect();

    Ok(out)
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
        let path = Path::new(
            "/tmp/nzb/downloads/downloads/inter/Foundation.S03E03.When.a.Book.Finds.You.1080p.ATVP.WEB-DL.DDP5.1.H.264-NTb.#4/0ae5e6761c69051d401054734c174e91.par2",
        );

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
