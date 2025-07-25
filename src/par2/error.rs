use thiserror::Error;

#[derive(Error, Debug)]
pub enum Par2Error {
    #[error("error reading par2 file")]
    IoError(#[from] std::io::Error),
}
