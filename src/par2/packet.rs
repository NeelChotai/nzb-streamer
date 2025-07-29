#[derive(Debug)]
pub struct MainPacket {
    pub slice_size: u64,
}

#[derive(Debug)]
pub struct FileDescPacket {
    pub file_id: String,
    pub filename: String,
    pub hash16k: Vec<u8>,
    pub filesize: u64,
}

#[derive(Debug)]
pub struct IFSCPacket {
    pub file_id: String,
    pub crcs: Vec<u32>,
}

#[derive(Debug)]
pub enum Packet {
    Main(MainPacket),
    FileDesc(FileDescPacket),
    IFSC(IFSCPacket),
}