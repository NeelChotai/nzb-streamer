use thiserror::Error;

#[derive(Error, Debug)]
pub enum Par2Error {
    #[error("Error reading PAR2 file")]
    Io(#[from] std::io::Error),

    #[error("Missing main packet")]
    MissingMainPacket,

    #[error("Received corrupted or incomplete packet")]
    IncompleteData,

    #[error("PAR2 parsing error")]
    Parse,
}

impl<E> From<nom::Err<E>> for Par2Error {
    fn from(value: nom::Err<E>) -> Self {
        match value {
            nom::Err::Incomplete(_) => Par2Error::IncompleteData,
            nom::Err::Error(_) | nom::Err::Failure(_) => Par2Error::Parse,
        }
    }
}
