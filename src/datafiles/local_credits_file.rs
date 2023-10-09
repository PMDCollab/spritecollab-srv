use crate::datafiles::{cleanup_discord_id, DataReadResult};
use chrono::{DateTime, NaiveDateTime, Utc};
use csv::ReaderBuilder;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use std::io::BufReader;

/// Parse local credits
pub fn get_credits<I: AsRef<[u8]>>(input: I) -> DataReadResult<Vec<LocalCreditRow>> {
    let mut rdr = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(false)
        .from_reader(BufReader::new(input.as_ref()));

    let mut credits: Vec<LocalCreditRow> = Vec::with_capacity(50);

    for result in rdr.deserialize() {
        let record: LocalCreditRow = result?;
        credits.push(record);
    }
    Ok(credits)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct LocalCreditRow {
    #[serde(deserialize_with = "parse_time")]
    pub date: DateTime<Utc>,
    #[serde(deserialize_with = "cleanup_discord_id")]
    pub credit_id: String,
    #[serde(deserialize_with = "parse_obsolete")]
    pub obsolete: bool,
    #[serde(deserialize_with = "parse_items")]
    pub items: Vec<String>,
}

pub fn parse_items<'de, D>(deser: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(String::deserialize(deser)?
        .split(',')
        .map(Into::into)
        .collect())
}

pub fn parse_obsolete<'de, D>(deser: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deser)?;
    Ok(&s == "OLD")
}

pub fn parse_time<'de, D>(deser: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deser)?;
    let t = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f")
        .map_err(|e| Error::custom(e.to_string()))?;
    Ok(DateTime::<Utc>::from_naive_utc_and_offset(t, Utc))
}
