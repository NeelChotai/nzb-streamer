use crate::error::{NzbStreamerError, Result};
use nzb_rs::{Nzb, ParseNzbError};
use tracing::{debug, info, warn};

pub struct NzbParser;

impl NzbParser {
    pub fn parse(content: &str) -> Result<Nzb> {
        debug!("parsing nzb");
        
        let nzb = Nzb::parse(content)
            .map_err(|e: ParseNzbError| {
                warn!("nzb parsing failed: {}", e);
                NzbStreamerError::NzbParsing(format!("invalid nzb format: {e}"))
            })?;
        
        info!(
            "successfully parsed nzb with {} files", 
            nzb.files.len()
        );
        
        Ok(nzb)
    }
}