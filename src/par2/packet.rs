use byteorder::{ByteOrder, LittleEndian};

const PAR_PKT_ID: &[u8] = b"PAR2\x00PKT";
const PAR_MAIN_ID: &[u8] = b"PAR 2.0\x00Main\x00\x00\x00\x00";
const PAR_FILE_ID: &[u8] = b"PAR 2.0\x00FileDesc";
const PAR_SLICE_ID: &[u8] = b"PAR 2.0\x00IFSC\x00\x00\x00\x00";

const HEADER_SIZE: usize = 32;
const HEADER_FIELD_SIZE: usize = 16;
const CRC_ENTRY_SIZE: usize = 20;

#[derive(Debug, Clone)]
pub struct MainPacket {
    pub slice_size: u64,
}

#[derive(Debug, Clone)]
pub struct FileDescPacket {
    pub file_id: String,
    pub filename: String,
    pub hash16k: Vec<u8>,
    pub filesize: u64,
}

#[derive(Debug, Clone)]
pub struct IFSCPacket {
    pub file_id: String,
    pub crcs: Vec<u32>,
}

#[derive(Debug, Clone)]
pub enum Packet {
    Main(MainPacket),
    FileDesc(FileDescPacket),
    IFSC(IFSCPacket),
}

/// Parses a potential PAR2 packet at the start of the buffer.
/// Checks magic ID, validates packet length, then extracts
/// the packet body.
///
/// # Returns
/// - `Some((packet, length))` on valid packet
/// - `None` if not a PAR2 packet or invalid packet structure
pub fn parse_packet(input: &[u8]) -> Option<(Packet, usize)> {
    let mut cursor = input;

    if !cursor.starts_with(PAR_PKT_ID) {
        return None;
    }

    take(&mut cursor, PAR_PKT_ID.len())?;

    let pack_bytes = take(&mut cursor, 8)?;
    let pack_len = LittleEndian::read_u64(pack_bytes) as usize;

    if pack_len < HEADER_SIZE || pack_len > input.len() {
        return None;
    }

    let mut body = input.get(HEADER_SIZE..pack_len)?;

    take(&mut body, HEADER_FIELD_SIZE)?; // Skip recovery set ID
    let packet_type = take(&mut body, HEADER_FIELD_SIZE)?;

    let packet = match packet_type {
        PAR_MAIN_ID => parse_main_packet(body),
        PAR_FILE_ID => parse_file_packet(body),
        PAR_SLICE_ID => parse_slice_packet(body),
        _ => return None,
    }?;

    Some((packet, pack_len))
}

/// Parses a Main packet body
///
/// # Structure
/// ```text
/// 0         16        32        40
/// +---------+---------+---------+
/// | Recovery| Packet  | Slice   |
/// | Set ID  | Type    | Size    |
/// | (16B)   | (16B)   | (8B)    |
/// +---------+---------+---------+
/// ```
fn parse_main_packet(mut body: &[u8]) -> Option<Packet> {
    let bytes = take(&mut body, 8)?;
    let slice_size = LittleEndian::read_u64(bytes);

    Some(Packet::Main(MainPacket { slice_size }))
}

/// Parses a FileDesc packet body
///
/// # Structure
/// ```text
/// 0         16        32        48        64        80        88        ...
/// +---------+---------+---------+---------+---------+---------+---------+
/// | Recovery| Packet  | File ID | MD5     | MD5     | File    | File    |
/// | Set ID  | Type    | (16B)   | (Full)  | (16k)   | Size    | Name    |
/// | (16B)   | (16B)   |         | (16B)   | (16B)   | (8B)    | (NUL)   |
/// +---------+---------+---------+---------+---------+---------+---------+
/// ```
fn parse_file_packet(mut body: &[u8]) -> Option<Packet> {
    let file_id = hex::encode(take(&mut body, HEADER_FIELD_SIZE)?);
    take(&mut body, HEADER_FIELD_SIZE)?; // skip MD5 hash
    let hash16k = take(&mut body, HEADER_FIELD_SIZE)?.to_vec();
    let filesize = LittleEndian::read_u64(take(&mut body, 8)?);

    // find the NUL terminator
    let filename_offset = body.split(|&b| b == 0).next()?;
    let filename = String::from_utf8(filename_offset.to_vec()).ok()?;

    Some(Packet::FileDesc(FileDescPacket {
        file_id,
        filename,
        hash16k,
        filesize,
    }))
}

/// Parses an IFSC (slice checksums) packet body
///
/// # Structure
/// ```text
/// 0         16        32        48        ...
/// +---------+---------+---------+---------+---------+
/// | Recovery| Packet  | File ID | Slice   | ...     |
/// | Set ID  | Type    | (16B)   | CRC[0]  |         |
/// | (16B)   | (16B)   |         | (20B)   |         |
/// +---------+---------+---------+---------+---------+
/// ```
fn parse_slice_packet(mut body: &[u8]) -> Option<Packet> {
    let file_id = hex::encode(take(&mut body, HEADER_FIELD_SIZE)?);

    if body.len() % CRC_ENTRY_SIZE != 0 {
        return None;
    }

    let crcs = body
        .chunks_exact(CRC_ENTRY_SIZE)
        .map(|chunk| LittleEndian::read_u32(&chunk[16..20]))
        .collect();

    Some(Packet::IFSC(IFSCPacket { file_id, crcs }))
}

fn take<'a>(slice: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    let (head, tail) = slice.get(..len).map(|h| (h, &slice[len..]))?;

    *slice = tail;
    Some(head)
}
