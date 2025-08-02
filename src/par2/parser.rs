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
