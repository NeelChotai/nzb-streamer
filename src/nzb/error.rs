use thiserror::Error;

#[derive(Error, Debug)]
pub enum NzbError {
    #[error("error parsing nzb")]
    Parsing(#[from] nzb_rs::ParseNzbError),
}
