use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Internal Server Error: {0}")]
    Internal(String),

    #[error("Not Found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Bad Request: {0}")]
    BadRequest(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Database error: {0}")]
    Database(String),
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err.to_string())
    }
}

#[cfg(feature = "sqlx")]
impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::Database(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
