use thiserror::Error;

#[derive(Error, Debug)]
pub enum NzbError {
    #[error("Error parsing NZB file")]
    Parsing(#[from] nzb_rs::ParseNzbError),
}
