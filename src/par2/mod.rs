use std::{collections::HashMap, path::Path};

use tracing::warn;

use crate::par2::{
    error::Par2Error,
    manifest::{FileInfo, Par2Manifest},
    packet::{Packet, parse_packet},
};

pub mod error;
pub mod hash;
pub mod manifest;
pub mod packet;
pub mod rar;

pub fn parse_file(path: &Path) -> Result<Par2Manifest, Par2Error> {
    let buffer = std::fs::read(path)?;
    parse_buffer(&buffer)
}

pub fn parse_buffer(buffer: &[u8]) -> Result<Par2Manifest, Par2Error> {
    let packets = scan_for_packets(buffer);

    let mut files = HashMap::new();
    let mut found_main = false;

    for packet in packets {
        match packet {
            Packet::Main(_) => found_main = true,
            Packet::FileDesc(desc) => {
                files.insert(
                    desc.filename.clone(),
                    FileInfo {
                        real_filename: desc.filename,
                        hash16k: desc.hash16k,
                    },
                );
            }
            _ => {} // ignore IFSC packets - we're not doing recovery for now
        }
    }

    if files.is_empty() {
        return Err(Par2Error::NoFiles);
    }

    if !found_main {
        warn!(
            "PAR2 file missing Main packet. The data may still be usable, but this indicates file corruption."
        );
    }

    Ok(Par2Manifest::new(files))
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
