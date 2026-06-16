use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("config error: {0}")]
    ConfigError(String),

    #[error("audio error: {0}")]
    AudioError(String),
}

pub type Result<T> = std::result::Result<T, Error>;
