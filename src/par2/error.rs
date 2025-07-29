use thiserror::Error;

#[derive(Error, Debug)]
pub enum Par2Error {
    #[error("error reading par2 file")]
    Io(#[from] std::io::Error),

    #[error("missing main packet")]
    MissingMainPacket,

    #[error("received corrupted or incomplete packet")]
    IncompleteData,

    #[error("par2 parsing error")]
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