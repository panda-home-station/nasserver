use thiserror::Error;

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Execution error: {0}")]
    Execution(String),
}

pub type Result<T> = std::result::Result<T, TerminalError>;
