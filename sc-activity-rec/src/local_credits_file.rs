use chrono::{DateTime, NaiveDateTime, Utc};
use csv::ReaderBuilder;
use sc_common::credit_row::cleanup_discord_id;
use sc_common::DataReadResult;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::BufReader;

/// Reads a form local credits.txt and returns the newest author credits ID for each item.
pub fn get_latest_credits<I: AsRef<[u8]>>(input: I) -> DataReadResult<HashMap<String, String>> {
    do_get_credits(input, None)
}

/// Reads local credits up until a given timestamp.
pub fn get_credits_until<I: AsRef<[u8]>>(
    input: I,
    until: DateTime<Utc>,
) -> DataReadResult<HashMap<String, String>> {
    do_get_credits(input, Some(until))
}

fn do_get_credits<I: AsRef<[u8]>>(
    input: I,
    until: Option<DateTime<Utc>>,
) -> DataReadResult<HashMap<String, String>> {
    let mut rdr = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(false)
        .from_reader(BufReader::new(input.as_ref()));

    let mut latest_credits: HashMap<String, String> = HashMap::with_capacity(50);

    for result in rdr.deserialize() {
        let record: LocalCreditRow = result?;
        if let Some(until) = until {
            if until < record.date {
                break;
            }
        }

        for item in record.items {
            latest_credits.insert(item, record.credit_id.to_string());
        }
    }
    Ok(latest_credits)
}

/// Gets the last credit row from the old format.
pub fn get_last_credits_old_format<I: AsRef<[u8]>>(input: I) -> DataReadResult<Option<String>> {
    let mut rdr = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(false)
        .from_reader(BufReader::new(input.as_ref()));

    Ok(rdr
        .deserialize::<LocalCreditRowOld>()
        .last()
        .transpose()?
        .map(|c| c.credit_id.to_string()))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct LocalCreditRow<'a> {
    #[serde(deserialize_with = "parse_time")]
    pub date: DateTime<Utc>,
    #[serde(deserialize_with = "cleanup_discord_id")]
    pub credit_id: Cow<'a, str>,
    #[serde(deserialize_with = "parse_items")]
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct LocalCreditRowOld<'a> {
    #[serde(deserialize_with = "parse_time")]
    pub date: DateTime<Utc>,
    #[serde(deserialize_with = "cleanup_discord_id")]
    pub credit_id: Cow<'a, str>,
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

pub fn parse_time<'de, D>(deser: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deser)?;
    let t = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f")
        .map_err(|e| Error::custom(e.to_string()))?;
    Ok(DateTime::<Utc>::from_utc(t, Utc))
}
