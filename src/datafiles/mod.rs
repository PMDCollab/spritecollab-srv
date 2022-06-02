pub mod anim_data_xml;
pub mod credit_names;
pub mod sprite_config;
pub mod tracker;

use crate::reporting::Reporting;
use crate::ReportingEvent;
use ellipse::Ellipse;
use std::fs::read_to_string;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

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

#[derive(Clone, Debug)]
pub enum DatafilesReport {
    Ok,
    JsonDeserializeError(PathBuf, Arc<serde_json::Error>),
    CsvDeserializeError(PathBuf, Arc<csv::Error>),
    IoError(PathBuf, Arc<std::io::Error>),
    CreditsDuplicateCreditId(PathBuf, String),
}

const DISCORD_UPDATE_INFO: &str = "The data update failed. I will not send any messages about further failures for 12h. I will send a message when the update works again.";

impl DatafilesReport {
    pub fn from_data_read_error(file_path: PathBuf, err: DataReadError) -> Self {
        match err {
            DataReadError::SerdeJson(e) => Self::JsonDeserializeError(file_path, e),
            DataReadError::SerdeCsv(e) => Self::CsvDeserializeError(file_path, e),
            DataReadError::Io(e) => Self::IoError(file_path, e),
            DataReadError::CreditsDuplicateCreditId(e) => {
                Self::CreditsDuplicateCreditId(file_path, e)
            }
        }
    }

    pub fn format_short(&self) -> String {
        match self {
            DatafilesReport::JsonDeserializeError(file_path, err) => {
                let fname = file_path.file_name().unwrap().to_string_lossy();
                format!("Failed reading {}: {}", fname, err)
            }
            DatafilesReport::CsvDeserializeError(file_path, err) => {
                let fname = file_path.file_name().unwrap().to_string_lossy();
                format!("Failed reading {}: {}", fname, err)
            }
            DatafilesReport::IoError(file_path, err) => {
                let fname = file_path.file_name().unwrap().to_string_lossy();
                format!("Failed reading {}: {}", fname, err)
            }
            DatafilesReport::CreditsDuplicateCreditId(file_path, err) => {
                let fname = file_path.file_name().unwrap().to_string_lossy();
                format!("Failed reading {}: {}", fname, err)
            }
            DatafilesReport::Ok => "Success.".to_string(),
        }
    }

    pub fn format_for_discord(&self) -> (&'static str, String) {
        let title = match self {
            DatafilesReport::Ok => "Failed SpriteCollab Update",
            _ => "SpriteCollab Update Recovered",
        };
        (
            title,
            match self {
                DatafilesReport::JsonDeserializeError(file_path, err) => {
                    let fname = file_path.file_name().unwrap().to_string_lossy();
                    format!(
                        "*{}*\n\n**Description**:\nFailed reading {}: {}{}",
                        DISCORD_UPDATE_INFO,
                        fname,
                        err,
                        self._discord_preview(file_path, err.line())
                    )
                }
                DatafilesReport::CsvDeserializeError(file_path, err) => {
                    let fname = file_path.file_name().unwrap().to_string_lossy();
                    format!(
                        "*{}*\n\n**Description**:\nFailed reading {}: {}{}",
                        DISCORD_UPDATE_INFO,
                        fname,
                        err,
                        self._discord_preview(
                            file_path,
                            err.position().map_or(0, |p| p.line() as usize)
                        )
                    )
                }
                DatafilesReport::IoError(file_path, err) => {
                    let fname = file_path.file_name().unwrap().to_string_lossy();
                    format!(
                        "*{}*\n\n**Description**:\nFailed reading {}: {}",
                        DISCORD_UPDATE_INFO, fname, err
                    )
                }
                DatafilesReport::CreditsDuplicateCreditId(file_path, err) => {
                    let fname = file_path.file_name().unwrap().to_string_lossy();
                    format!(
                        "*{}*\n\n**Description**:\nFailed reading {}: {}",
                        DISCORD_UPDATE_INFO, fname, err
                    )
                }
                DatafilesReport::Ok => "The SpriteCollab data update is working again.".to_string(),
            },
        )
    }

    fn _discord_preview(&self, file_path: &PathBuf, line_n: usize) -> String {
        if line_n != 0 {
            if let Ok(content) = read_to_string(file_path) {
                if let Some(line) = content.lines().skip(line_n - 1).take(1).next() {
                    let truncated = line.truncate_ellipse(300);
                    return format!("\nLine {}:\n```\n{}\n```", line_n, truncated);
                }
            }
        }
        "".to_owned()
    }
}

/// Reads the given file and returns the result of `generate_fn`.
/// If there was an error, it tries to process and log it and report it.
pub async fn read_and_report_error<P, FN, FT, R, T>(
    path: P,
    generate_fn: FN,
    reporting: R,
) -> DataReadResult<T>
where
    P: AsRef<Path> + Copy,
    FN: FnOnce(P) -> FT,
    FT: Future<Output = DataReadResult<T>>,
    R: AsRef<Reporting>,
{
    let out = generate_fn(path).await;
    match &out {
        Ok(_) => {}
        Err(e) => {
            reporting
                .as_ref()
                .send_event(ReportingEvent::UpdateDatafiles(
                    DatafilesReport::from_data_read_error(path.as_ref().to_path_buf(), e.clone()),
                ))
                .await;
        }
    }
    out
}
