use std::sync::Arc;
use thiserror::Error;

pub mod credit_names;
pub mod credit_row;
pub mod search;

pub struct AssetActivity {}

pub type DataReadResult<T> = Result<T, DataReadError>;

#[derive(Error, Debug, Clone)]
pub enum DataReadError {
    #[error("JSON deserialization error: {0}")]
    SerdeJson(Arc<serde_json::Error>),
    #[error("CSV deserialization error: {0}")]
    SerdeCsv(Arc<csv::Error>),
    #[error("I/O error: {0}")]
    Io(Arc<std::io::Error>),
    #[error("Duplicate credit id while trying to read credit names: {0}")]
    CreditsDuplicateCreditId(String),
}

impl From<serde_json::Error> for DataReadError {
    fn from(e: serde_json::Error) -> Self {
        DataReadError::SerdeJson(Arc::new(e))
    }
}

impl From<csv::Error> for DataReadError {
    fn from(e: csv::Error) -> Self {
        DataReadError::SerdeCsv(Arc::new(e))
    }
}

impl From<std::io::Error> for DataReadError {
    fn from(e: std::io::Error) -> Self {
        DataReadError::Io(Arc::new(e))
    }
}
