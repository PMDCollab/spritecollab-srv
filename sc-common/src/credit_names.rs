use crate::credit_row::CreditRow;
use crate::search::fuzzy_find;
use crate::{DataReadError, DataReadResult};
use csv::ReaderBuilder;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;

pub fn read_credit_names<R: Read>(reader: R) -> DataReadResult<CreditNames> {
    let mut rdr = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(true)
        .from_reader(reader);

    let mut data = Vec::with_capacity(1000);
    let mut keys_credit_ids: HashMap<String, usize> = HashMap::with_capacity(1000);
    let mut keys_names: HashMap<String, Vec<usize>> = HashMap::with_capacity(1000);

    for (idx, result) in rdr.deserialize().enumerate() {
        let record: CreditRow = result?;
        if let Some(name) = record.name.as_ref().map(|v| v.clone().into_owned()) {
            match keys_names.entry(name) {
                std::collections::hash_map::Entry::Occupied(mut v) => {
                    v.get_mut().push(idx);
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(vec![idx]);
                }
            }
        }
        if keys_credit_ids.contains_key(record.credit_id.as_ref()) {
            return Err(DataReadError::CreditsDuplicateCreditId(
                record.credit_id.clone().into_owned(),
            ));
        }
        keys_credit_ids.insert(record.credit_id.clone().into_owned(), idx);
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
    data: Vec<CreditRow<'static>>,
    /// Unique keys over the credit_id field.
    keys_credit_ids: HashMap<String, usize>,
    /// Non-unique keys over the "Names" field.
    keys_names: HashMap<String, Vec<usize>>,
}

impl CreditNames {
    pub fn iter(&self) -> impl Iterator<Item = &CreditRow> {
        self.data.iter()
    }
    pub fn fuzzy_find<S: AsRef<str>>(&self, query: S) -> impl Iterator<Item = &CreditRow> {
        fuzzy_find(
            self.keys_credit_ids
                .iter()
                .map(|(key, val)| (key, Cow::from(vec![*val])))
                .chain(self.keys_names.iter().map(|(kn, kv)| (kn, Cow::from(kv)))),
            query,
        )
        .map(|val| &self.data[val])
    }
    pub fn get(&self, credit_id: &str) -> Option<&CreditRow> {
        self.keys_credit_ids
            .get(credit_id)
            .map(|idx| &self.data[*idx])
    }
}
