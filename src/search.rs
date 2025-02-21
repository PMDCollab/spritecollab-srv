use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use itertools::Itertools;
use num_traits::PrimInt;
use std::borrow::Cow;
use std::hash::Hash;

pub fn fuzzy_find<V, I, N, S1, S2>(iter: I, query: S2) -> impl Iterator<Item = N>
where
    I: Iterator<Item = (S1, V)>,
    S1: AsRef<str>,
    S2: AsRef<str>,
    // XXX: not ideal, ideally we would just accept an Iterator over usize,
    // but I fought with the borrow checker for long enough now...
    V: CloneToVec<N>,
    N: PrimInt + Hash,
{
    let matcher = SkimMatcherV2::default();
    let mut search_result = iter
        .filter_map(|(k, v)| do_fuzzy_match(k, v.clone_to_vec(), &query, &matcher))
        .flatten()
        .collect::<Vec<(i64, N)>>();

    search_result.sort_by(|(score_a, _), (score_b, _)| score_b.cmp(score_a));

    search_result.into_iter().map(|(_score, val)| val).unique()
}

fn do_fuzzy_match<S1, S2, II, I>(
    key: S1,
    vals_brw: II,
    query: &S2,
    matcher: &SkimMatcherV2,
) -> Option<Vec<(i64, I)>>
where
    S1: AsRef<str>,
    S2: AsRef<str>,
    II: IntoIterator<Item = I>,
    I: PrimInt,
{
    match matcher.fuzzy_match(&key.as_ref().to_lowercase(), &query.as_ref().to_lowercase()) {
        None => None,
        Some(score) => {
            if score <= 0 {
                None
            } else {
                Some(
                    vals_brw
                        .into_iter()
                        .map(|val| (score, val))
                        .collect::<Vec<_>>(),
                )
            }
        }
    }
}

pub trait CloneToVec<T> {
    fn clone_to_vec(&self) -> Vec<T>;
}

impl<T: ToOwned + Copy> CloneToVec<T> for Cow<'_, [T]>
where
    [T]: ToOwned,
{
    fn clone_to_vec(&self) -> Vec<T> {
        self.to_vec()
    }
}

impl<T: Copy> CloneToVec<T> for Vec<T> {
    fn clone_to_vec(&self) -> Vec<T> {
        self.to_vec()
    }
}

impl<T: Copy> CloneToVec<T> for &Vec<T> {
    fn clone_to_vec(&self) -> Vec<T> {
        self.to_vec()
    }
}
