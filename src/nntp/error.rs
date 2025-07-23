use thiserror::Error;

#[derive(Error, Debug)]
pub enum NntpError {
    #[error("error in config: {0}")]
    Config(String),

    #[error("client authentication error: {0}")]
    ClientAuthentication(String),

    #[error("error reading body: {0}")]
    BodyRead(String),
}
