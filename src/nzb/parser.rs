use crate::{error::NzbStreamerError, nzb::error::NzbError};
use nzb_rs::{File, Nzb as RawNzb, ParseNzbError};
use tracing::{debug, info, warn};

pub struct Nzb {
    pub par2: Vec<File>,
    pub rar: Vec<File>,
    pub obfuscated: Vec<File>,
}

pub struct NzbParser;

impl NzbParser {
    pub fn parse(content: &str) -> Result<Nzb, NzbError> {
        debug!("parsing nzb");

        let raw_nzb = RawNzb::parse(content)?;
        let (par2, rar, obfuscated) = raw_nzb.files.into_iter().fold(
            (Vec::new(), Vec::new(), Vec::new()),
            |(mut par2, mut rar, mut obf), file| {
                if file.is_par2() {
                    par2.push(file);
                } else if file.is_rar() {
                    rar.push(file);
                } else if file.is_obfuscated() {
                    obf.push(file);
                }
                (par2, rar, obf)
            },
        );

        info!(
            "successfully parsed nzb (par2: {}, rar: {}, obfuscated: {})",
            par2.len(),
            rar.len(),
            obfuscated.len(),
        );

        let nzb = Nzb {
            par2,
            rar,
            obfuscated,
        };

        Ok(nzb)
    }
}
