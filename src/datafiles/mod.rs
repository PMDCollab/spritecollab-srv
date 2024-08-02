use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use log::error;
use once_cell::sync::OnceCell;
use regex::Regex;
use serde::{Deserialize, Deserializer};
use thiserror::Error;

use crate::datafiles::anim_data_xml::{AnimDataXml, AnimDataXmlOpenError};
use crate::datafiles::tracker::{MonsterFormCollector, Tracker};

pub mod anim_data_xml;
pub mod credit_names;
pub mod group_id;
pub mod local_credits_file;
pub mod sprite_config;
pub mod tracker;

pub type DataReadResult<T> = Result<T, DataReadError>;

static DISCORD_REGEX: OnceCell<Regex> = OnceCell::new();

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
    #[error("Errors reading AnimData.xmls.")]
    AnimDataXmlErrors(Vec<(i32, Vec<i32>, Arc<AnimDataXmlOpenError>)>),
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

/// Reads the given file and returns the result of `generate_fn`.
/// If there was an error, it tries to process and log it.
pub async fn read_and_report_error<P, FN, FT, T>(path: P, generate_fn: FN) -> DataReadResult<T>
where
    P: AsRef<Path> + Copy,
    FN: FnOnce(P) -> FT,
    FT: Future<Output = DataReadResult<T>>,
{
    let out = generate_fn(path).await;
    match &out {
        Ok(_) => {}
        Err(e) => {
            error!("Failed reading {}: {}", path.as_ref().display(), e);
        }
    }
    out
}

pub async fn try_read_in_anim_data_xml(tracker: &Tracker) -> Result<(), DataReadError> {
    let errs = tracker
        .keys()
        .flat_map(|group_id| {
            let group_id = **group_id as i32;
            #[allow(clippy::map_flatten)] // See comment at MonsterFormCollector::map
            MonsterFormCollector::collect(tracker, group_id)
                .unwrap()
                .map(|(path, _, group)| {
                    if group.sprite_complete == 0 {
                        return None;
                    }
                    if let Err(e) = AnimDataXml::open_for_form(group_id, &path) {
                        Some((group_id, path, Arc::new(e)))
                    } else {
                        None
                    }
                })
                .flatten()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    if !errs.is_empty() {
        for (monster, form, error) in &errs {
            error!("Failed reading AnimData.xml for {monster}/{form:?}: {error}");
        }
        Err(DataReadError::AnimDataXmlErrors(errs))
    } else {
        Ok(())
    }
}

fn cleanup_discord_id<'de, D>(deser: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(parse_credit_id(String::deserialize(deser)?))
}

pub fn parse_credit_id<S: AsRef<str> + ToString>(credit_id_raw: S) -> String {
    let cell = &DISCORD_REGEX;
    let regex = cell.get_or_init(|| Regex::new(r"<@!(\d+)>").unwrap());

    if let Some(discord_id) = regex.captures(credit_id_raw.as_ref()) {
        discord_id[1].to_string()
    } else {
        credit_id_raw.to_string()
    }
}
