use thiserror::Error;

#[derive(Error, Debug)]
pub enum BorisError {
    #[error("config error: {0}")]
    ConfigError(String),

    #[error("audio error: {0}")]
    AudioError(String),
}

pub type BorisResult<T> = std::result::Result<T, BorisError>;
