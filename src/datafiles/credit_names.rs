use crate::datafiles::{DataReadError, DataReadResult, cleanup_discord_id};
use crate::search::fuzzy_find;
use csv::ReaderBuilder;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub async fn read_credit_names<P: AsRef<Path>>(path: P) -> DataReadResult<CreditNames> {
    let input = File::open(path)?;
    let mut rdr = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(true)
        .from_reader(BufReader::new(input));

    let mut data = Vec::with_capacity(1000);
    let mut keys_credit_ids: HashMap<String, usize> = HashMap::with_capacity(1000);
    let mut keys_names: HashMap<String, Vec<usize>> = HashMap::with_capacity(1000);

    for (idx, result) in rdr.deserialize().enumerate() {
        let record: CreditNamesRow = result?;
        if let Some(name) = record.name.clone() {
            match keys_names.entry(name) {
                std::collections::hash_map::Entry::Occupied(mut v) => {
                    v.get_mut().push(idx);
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(vec![idx]);
                }
            }
        }
        if keys_credit_ids.contains_key(&record.credit_id) {
            return Err(DataReadError::CreditsDuplicateCreditId(record.credit_id));
        }
        keys_credit_ids.insert(record.credit_id.clone(), idx);
        data.push(record);
    }
    Ok(CreditNames {
        data,
        keys_credit_ids,
        keys_names,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreditNames {
    /// Vector that contains all rows.
    data: Vec<CreditNamesRow>,
    /// Unique keys over the credit_id field.
    keys_credit_ids: HashMap<String, usize>,
    /// Non-unique keys over the "Names" field.
    keys_names: HashMap<String, Vec<usize>>,
}

impl CreditNames {
    pub fn iter(&self) -> impl Iterator<Item = &CreditNamesRow> {
        self.data.iter()
    }
    pub fn fuzzy_find<S: AsRef<str>>(&self, query: S) -> impl Iterator<Item = &CreditNamesRow> {
        fuzzy_find(
            self.keys_credit_ids
                .iter()
                .map(|(key, val)| (key, Cow::from(vec![*val])))
                .chain(self.keys_names.iter().map(|(kn, kv)| (kn, Cow::from(kv)))),
            query,
        )
        .map(|val| &self.data[val])
    }
    pub fn get(&self, credit_id: &str) -> Option<&CreditNamesRow> {
        self.keys_credit_ids
            .get(credit_id)
            .map(|idx| &self.data[*idx])
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct CreditNamesRow {
    #[serde(
        deserialize_with = "cleanup_discord_id",
        rename(deserialize = "Discord")
    )]
    pub credit_id: String,
    #[serde(rename(deserialize = "Name"))]
    pub name: Option<String>,
    #[serde(rename(deserialize = "Contact"))]
    pub contact: Option<String>,
}
