//! This module double checks if sprite and portrait files actually exist.

use crate::assets::util::join_monster_and_form;
use crate::cache::CacheBehaviour;
use crate::cache::ScCache;
use crate::datafiles::local_credits_file::{get_credits, LocalCreditRow};
use crate::datafiles::{DataReadError, DataReadResult};
use crate::Config;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub enum AssetCategory {
    Sprite,
    Portrait,
}

impl Display for AssetCategory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetCategory::Sprite => write!(f, "Sprite"),
            AssetCategory::Portrait => write!(f, "Portrait"),
        }
    }
}

enum FileLookup<'a, I: Iterator<Item = &'a String> + Clone> {
    Sprite(I, i32, &'a [i32]),
    Portrait(I, i32, &'a [i32]),
}

impl<'a, C> FileLookup<'a, C>
where
    C: Iterator<Item = &'a String> + Clone,
{
    async fn lookup(&self) -> CacheBehaviour<Vec<String>> {
        CacheBehaviour::Cache(self.all().flat_map(|a| self.do_single_lookup(a)).collect())
    }

    fn all(&self) -> C {
        match self {
            FileLookup::Sprite(all, _, _) => all.clone(),
            FileLookup::Portrait(all, _, _) => all.clone(),
        }
    }

    fn path(&self, act: &str) -> PathBuf {
        match self {
            FileLookup::Sprite(_, mon, path) => {
                let joined_p = join_monster_and_form(*mon, path, '/');
                PathBuf::from(Config::Workdir.get())
                    .join(format!("spritecollab/sprite/{}/{}-Anim.png", joined_p, act))
            }
            FileLookup::Portrait(_, mon, path) => {
                let joined_p = join_monster_and_form(*mon, path, '/');
                PathBuf::from(Config::Workdir.get())
                    .join(format!("spritecollab/portrait/{}/{}.png", joined_p, act))
            }
        }
    }

    fn do_single_lookup(&self, act: &str) -> Option<String> {
        if self.path(act).exists() {
            Some(act.to_string())
        } else {
            None
        }
    }
}

struct FileLookupCache(Vec<String>);

impl FileLookupCache {
    async fn new<'a, C, I>(cache: &C, lookup: FileLookup<'a, I>) -> Result<Self, C::Error>
    where
        C: ScCache,
        I: Iterator<Item = &'a String> + Send + Sync + Clone,
    {
        let data = match lookup {
            FileLookup::Sprite(_, mon, pat) => {
                cache
                    .cached(format!("spr_files|{}/{:?}", mon, pat), || lookup.lookup())
                    .await
            }
            FileLookup::Portrait(_, mon, pat) => {
                cache
                    .cached(format!("prt_files|{}/{:?}", mon, pat), || lookup.lookup())
                    .await
            }
        }?;
        Ok(Self(data))
    }

    fn if_has<T, S: AsRef<str>>(&self, needle: S, then_return: T) -> Option<T> {
        match self.0.iter().any(|x| x == needle.as_ref()) {
            true => Some(then_return),
            false => None,
        }
    }

    fn take_out_if_has<T, S: AsRef<str>>(
        &mut self,
        needle: S,
        then_return: T,
    ) -> Option<(String, T)> {
        match self.0.iter().position(|x| x == needle.as_ref()) {
            Some(pos) => Some((self.0.remove(pos), then_return)),
            None => None,
        }
    }
}

pub async fn iter_existing_sprite_files<C: ScCache + Send + Sync>(
    cache: &C,
    sprite_files: &HashMap<String, bool>,
    monster_idx: i32,
    form_path: &[i32],
) -> Result<impl IntoIterator<Item = (String, bool)>, C::Error> {
    let mut lookup_cache = FileLookupCache::new(
        cache,
        FileLookup::Sprite(sprite_files.keys(), monster_idx, form_path),
    )
    .await?;
    Ok(sprite_files
        .iter()
        .filter_map(|(action, locked)| lookup_cache.take_out_if_has(action, *locked))
        .collect::<Vec<_>>())
}

pub async fn get_existing_sprite_file<C: ScCache + Send + Sync>(
    cache: &C,
    sprite_files: &HashMap<String, bool>,
    action: &str,
    monster_idx: i32,
    form_path: &[i32],
) -> Result<Option<bool>, C::Error> {
    let lookup_cache = FileLookupCache::new(
        cache,
        FileLookup::Sprite(sprite_files.keys(), monster_idx, form_path),
    )
    .await?;
    Ok(sprite_files
        .get(action)
        .and_then(|locked| lookup_cache.if_has(action, *locked)))
}

pub async fn iter_existing_portrait_files<C: ScCache + Send + Sync>(
    cache: &C,
    portrait_files: &HashMap<String, bool>,
    flipped: bool,
    monster_idx: i32,
    form_path: &[i32],
) -> Result<impl IntoIterator<Item = (String, bool)>, C::Error> {
    let mut lookup_cache = FileLookupCache::new(
        cache,
        FileLookup::Portrait(portrait_files.keys(), monster_idx, form_path),
    )
    .await?;
    Ok(portrait_files
        .iter()
        .filter(|(emotion, _)| {
            if flipped {
                emotion.ends_with('^')
            } else {
                !emotion.ends_with('^')
            }
        })
        .filter_map(|(emotion, locked)| lookup_cache.take_out_if_has(emotion, *locked))
        .collect::<Vec<_>>())
}

pub async fn get_existing_portrait_file<C: ScCache + Send + Sync>(
    cache: &C,
    portrait_files: &HashMap<String, bool>,
    emotion: &str,
    flipped: bool,
    monster_idx: i32,
    form_path: &[i32],
) -> Result<Option<bool>, C::Error> {
    let lookup_cache = FileLookupCache::new(
        cache,
        FileLookup::Portrait(portrait_files.keys(), monster_idx, form_path),
    )
    .await?;
    let emotion = if flipped {
        Cow::from(format!("{}^", emotion))
    } else {
        Cow::from(emotion)
    };
    Ok(portrait_files
        .get(emotion.as_ref())
        .and_then(|locked| lookup_cache.if_has(&emotion, *locked)))
}

pub async fn get_local_credits_file<C: ScCache + Send + Sync>(
    cache: &C,
    asset_type: AssetCategory,
    monster_idx: i32,
    form_path: &[i32],
) -> Result<DataReadResult<Vec<LocalCreditRow>>, C::Error> {
    let content_result: DataReadResult<Option<Vec<u8>>> = cache
        .cached_may_fail(
            format!("credits_{}|{}/{:?}", asset_type, monster_idx, form_path),
            || async {
                let joined_p = join_monster_and_form(monster_idx, form_path, '/');
                let path = match asset_type {
                    AssetCategory::Sprite => PathBuf::from(Config::Workdir.get())
                        .join(format!("spritecollab/sprite/{}/credits.txt", joined_p)),
                    AssetCategory::Portrait => PathBuf::from(Config::Workdir.get())
                        .join(format!("spritecollab/portrait/{}/credits.txt", joined_p)),
                };
                if path.exists() {
                    Ok(CacheBehaviour::Cache(Some(tokio::fs::read(path).await?)))
                } else {
                    Ok(CacheBehaviour::Cache(None))
                }
            },
        )
        .await?
        .map_err(|e| DataReadError::Io(Arc::new(e)));
    match content_result {
        Ok(Some(content)) => Ok(get_credits(content)),
        Ok(None) => Ok(Ok(Vec::new())),
        Err(e) => Ok(Err(e)),
    }
}
