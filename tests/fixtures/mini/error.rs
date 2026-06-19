//! Error handling types and conversions.

use std::fmt;

/// Custom error type for the application.
#[derive(Debug)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

/// Categories of errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    InvalidInput,
    Internal,
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError {
            kind: ErrorKind::Internal,
            message: err.to_string(),
        }
    }
}

/// Result type alias for convenience.
pub type Result<T> = std::result::Result<T, AppError>;
