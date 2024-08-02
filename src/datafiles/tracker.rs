use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::BufReader;
use std::iter::Peekable;
use std::path::Path;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::cache::CacheBehaviour;
use crate::cache::ScCache;
use crate::datafiles::DataReadResult;
use crate::datafiles::group_id::GroupId;
use crate::search::fuzzy_find;

pub async fn read_tracker<P: AsRef<Path>>(path: P) -> DataReadResult<Tracker> {
    let input = File::open(path)?;
    Ok(serde_json::from_reader(BufReader::new(input))?)
}

pub type Tracker = HashMap<GroupId, Group>;

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
pub struct Credit {
    pub primary: String,
    pub secondary: Vec<String>,
    pub total: i64,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
pub struct Group {
    pub canon: bool,
    pub modreward: bool,
    pub name: String,
    pub portrait_bounty: HashMap<i64, i64>,
    pub portrait_complete: i64,
    pub portrait_credit: Credit,
    pub portrait_files: HashMap<String, bool>,
    pub portrait_link: String,
    #[serde(deserialize_with = "parse_datetime")]
    pub portrait_modified: Option<DateTime<Utc>>,
    pub portrait_pending: Value,
    pub portrait_recolor_link: String,
    pub portrait_required: bool,
    pub sprite_bounty: HashMap<i64, i64>,
    pub sprite_complete: i64,
    pub sprite_credit: Credit,
    pub sprite_files: HashMap<String, bool>,
    pub sprite_link: String,
    #[serde(deserialize_with = "parse_datetime")]
    pub sprite_modified: Option<DateTime<Utc>>,
    pub sprite_pending: Value,
    pub sprite_recolor_link: String,
    pub sprite_required: bool,
    pub subgroups: HashMap<GroupId, Group>,
}

fn parse_datetime<'de, D>(deser: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    let as_str = String::deserialize(deser)?;
    if as_str.is_empty() {
        Ok(None)
    } else {
        NaiveDateTime::parse_from_str(&as_str, "%Y-%m-%d %H:%M:%S%.f")
            .map(|datetime| Some(Utc.from_utc_datetime(&datetime)))
            .map_err(serde::de::Error::custom)
    }
}

pub async fn fuzzy_find_tracker<S, C, E, T, F>(
    tracker: &Tracker,
    monster_name: S,
    cache: &C,
    consume: F,
) -> Result<Vec<T>, E>
where
    S: AsRef<str>,
    C: ScCache<Error = E>,
    F: Fn(i64) -> T,
{
    let index: HashMap<String, Vec<i64>> = cache
        .cached("fuzzy_find_tracker", || async {
            let mut names: HashMap<String, Vec<i64>> = HashMap::with_capacity(tracker.len() * 10);
            for (monster_idx, monster) in tracker.iter() {
                fft_insert(&mut names, **monster_idx, &monster.name);
                fft_recurse(&mut names, **monster_idx, &monster.subgroups);
            }
            CacheBehaviour::Cache(names)
        })
        .await?;
    Ok(fuzzy_find(index.iter(), monster_name)
        .map(consume)
        .collect())
}

fn fft_insert(names: &mut HashMap<String, Vec<i64>>, monster_idx: i64, name: &str) {
    names
        .entry(name.to_lowercase())
        .or_default()
        .push(monster_idx);
}

fn fft_recurse(
    names: &mut HashMap<String, Vec<i64>>,
    monster_idx: i64,
    subgroups: &HashMap<GroupId, Group>,
) {
    for grp in subgroups.values() {
        fft_insert(names, monster_idx, &grp.name);
        fft_recurse(names, monster_idx, &grp.subgroups);
    }
}

#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum FormMatch {
    /// Look exactly for this form ID.
    Exact(i32),
    /// Look for this form ID or fall back to 0 if it doesn't exist.
    Fallback(i32),
}

trait IntoFormMatchIterator {
    fn form_match_combinations(self) -> Vec<Vec<i32>>;
}

impl<T> IntoFormMatchIterator for T
where
    T: Iterator<Item = FormMatch>,
{
    fn form_match_combinations(self) -> Vec<Vec<i32>> {
        let mut combinations: Vec<Vec<i32>> = vec![Vec::new()];
        for form_match in self {
            match form_match {
                FormMatch::Exact(form_id) => {
                    combinations
                        .iter_mut()
                        .for_each(|combination| combination.push(form_id));
                }
                FormMatch::Fallback(form_id) => {
                    // Generate the 0-fallback combinations.
                    let mut new_combinations = combinations.to_vec();
                    combinations
                        .iter_mut()
                        .for_each(|combination| combination.push(form_id));
                    new_combinations
                        .iter_mut()
                        .for_each(|combination| combination.push(0));
                    combinations.append(&mut new_combinations);
                }
            }
        }
        combinations
    }
}

// collapse entries to -> 0000 -> / if they don't exist to find the form.
pub struct MonsterFormCollector<'a>(&'a Group);

impl<'a> MonsterFormCollector<'a> {
    pub fn collect(tracker: &'a Tracker, monster_idx: i32) -> Option<MonsterFormCollector> {
        tracker
            .get(&GroupId(monster_idx as i64))
            .map(MonsterFormCollector)
    }

    pub fn is_female<'b, P>(form: P) -> bool
    where
        P: IntoIterator<Item = &'b i32>,
    {
        form.into_iter()
            .nth(2)
            .map(|&idx| idx == 2)
            .unwrap_or(false)
    }

    pub fn is_shiny<'b, P>(form: P) -> bool
    where
        P: IntoIterator<Item = &'b i32>,
    {
        form.into_iter()
            .nth(1)
            .map(|&idx| idx == 1)
            .unwrap_or(false)
    }

    pub fn find_form<N>(&'a self, needle: N) -> Option<(Vec<i32>, Vec<String>, &'a Group)>
    where
        N: IntoIterator<Item = FormMatch>,
    {
        for possibility in needle.into_iter().form_match_combinations() {
            // first collapse away all trailing zeroes path elements.
            let mut had_something_other_than_zero = false;
            let mut possibility_collapsed: Vec<i32> = possibility
                .into_iter()
                .rev()
                .filter(|n| {
                    if !had_something_other_than_zero {
                        if n != &0 {
                            had_something_other_than_zero = true;
                            true
                        } else {
                            false
                        }
                    } else {
                        true
                    }
                })
                .collect();
            if possibility_collapsed.is_empty() {
                possibility_collapsed.push(0);
            }
            if let Some(r) = Self::find_form_step(
                self.0,
                possibility_collapsed.into_iter().rev().peekable(),
                Vec::new(),
                Vec::new(),
            ) {
                return Some(r);
            }
        }
        None
    }

    fn find_form_step<N>(
        current_group: &'a Group,
        mut needle: Peekable<N>,
        mut collected: Vec<i32>,
        mut collected_names: Vec<String>,
    ) -> Option<(Vec<i32>, Vec<String>, &'a Group)>
    where
        N: Iterator<Item = i32>,
    {
        match needle.next() {
            Some(current) => {
                match needle.peek() {
                    Some(_) => {
                        // We will still have a path to process after this; we are not at the leaf yet.
                        // Try to find the group.
                        let sub_group = current_group.subgroups.get(&GroupId(current as i64));
                        match sub_group {
                            Some(sub_group) => {
                                // Return the sub-group.
                                collected.push(current);
                                if !sub_group.name.is_empty() {
                                    collected_names.push(sub_group.name.clone());
                                }
                                Self::find_form_step(sub_group, needle, collected, collected_names)
                            }
                            None => None,
                        }
                    }
                    None => {
                        if current == 0 {
                            // We have no more forms to check and are group 0 so look on (relative) root level
                            Some((collected, collected_names, current_group))
                        } else {
                            let sub_group = current_group.subgroups.get(&GroupId(current as i64));
                            match sub_group {
                                Some(sub_group) => {
                                    // Return the sub-group.
                                    collected.push(current);
                                    if !sub_group.name.is_empty() {
                                        collected_names.push(sub_group.name.clone());
                                    }
                                    Some((collected, collected_names, sub_group))
                                }
                                None => None,
                            }
                        }
                    }
                }
            }
            None => None,
        }
    }

    // TODO: This needs to be refactored so MonsterFormCollector just implements IntoIterator,
    //       and MappedFormIterator is just a "normal" iterator.
    pub fn map<F, T>(&'a self, map_fn: F) -> MappedFormIterator<'a, F, T>
    where
        F: Fn((Vec<i32>, Vec<String>, &'a Group)) -> T + 'a,
        T: 'a,
    {
        MappedFormIterator {
            map_fn,
            root: Some(self.0),
            remaining: Default::default(),
        }
    }
}

pub struct MappedFormIterator<'a, F, T>
where
    F: Fn((Vec<i32>, Vec<String>, &'a Group)) -> T + 'a,
    T: 'a,
{
    map_fn: F,
    // The root group. If not None, the first next call will yield it and fill
    // remaining with the sub groups.
    root: Option<&'a Group>,
    // A list of unprocessed groups and the paths to their parents (!)
    remaining: VecDeque<(Vec<i32>, Vec<String>, &'a Group)>,
}

impl<'a, F, T> Iterator for MappedFormIterator<'a, F, T>
where
    F: Fn((Vec<i32>, Vec<String>, &'a Group)) -> T + 'a,
    T: 'a,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        match self.root {
            Some(root) => {
                Self::add_all_sub_groups(&[], &[], root, &mut self.remaining);
                self.root
                    .take()
                    .map(|r| (self.map_fn)((Vec::new(), vec![r.name.clone()], r)))
            }
            None => self.remaining.pop_front().and_then(|(p, names, g)| {
                Self::add_all_sub_groups(&p, &names, g, &mut self.remaining);
                // if this is a 0 ID don't yield it.
                if p.last().map(|&idx| idx == 0).unwrap_or(false) {
                    self.next()
                } else {
                    Some((self.map_fn)((p, names, g)))
                }
            }),
        }
    }
}

impl<'a, F, T> MappedFormIterator<'a, F, T>
where
    F: Fn((Vec<i32>, Vec<String>, &'a Group)) -> T + 'a,
    T: 'a,
{
    fn add_all_sub_groups(
        path_to_root: &[i32],
        names_to_root: &[String],
        root: &'a Group,
        pending: &mut VecDeque<(Vec<i32>, Vec<String>, &'a Group)>,
    ) {
        for (subidx, subgroup) in &root.subgroups {
            let mut subpath = path_to_root.to_vec();
            subpath.push(**subidx as i32);
            let mut subpath_names = names_to_root.to_vec();
            if !subgroup.name.is_empty() {
                subpath_names.push(subgroup.name.clone());
            }
            pending.push_back((subpath, subpath_names, subgroup));
        }
    }
}
