pub mod parser;
pub mod matcher;
//pub mod deobfuscate;
pub mod par2_crc32;

pub use parser::{FilePar2Info, parse_par2_file};