use crate::datafiles::{DataReadResult, cleanup_discord_id};
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

    for result in rdr.deserialize::<LocalCreditRow>() {
        let record = {
            match result {
                Ok(record) => record,
                Err(initial_err) => {
                    // If this fails, try to read as old credits file and convert
                    return match get_credits_old(input) {
                        Ok(old_records) => {
                            Ok(old_records.into_iter().map(convert_old_credits).collect())
                        }
                        // If that also fails, return initial error
                        Err(_) => Err(initial_err.into()),
                    };
                }
            }
        };
        credits.push(record);
    }
    Ok(credits)
}

fn get_credits_old<I: AsRef<[u8]>>(input: I) -> DataReadResult<Vec<LocalCreditRowV0>> {
    let mut rdr = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(false)
        .from_reader(BufReader::new(input.as_ref()));

    let mut credits: Vec<LocalCreditRowV0> = Vec::with_capacity(50);

    for result in rdr.deserialize() {
        let record: LocalCreditRowV0 = result?;
        credits.push(record);
    }
    Ok(credits)
}

fn convert_old_credits(old_record: LocalCreditRowV0) -> LocalCreditRow {
    LocalCreditRow {
        date: old_record.date,
        credit_id: old_record.credit_id,
        obsolete: old_record.obsolete,
        license: "Unknown".to_string(),
        items: old_record.items,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct LocalCreditRow {
    #[serde(deserialize_with = "parse_time")]
    pub date: DateTime<Utc>,
    #[serde(deserialize_with = "cleanup_discord_id")]
    pub credit_id: String,
    #[serde(deserialize_with = "parse_obsolete")]
    pub obsolete: bool,
    pub license: String,
    #[serde(deserialize_with = "parse_items")]
    pub items: Vec<String>,
}

// Old version of the credits rows, for backwards compat.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct LocalCreditRowV0 {
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
