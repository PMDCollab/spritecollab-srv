//! The actual client implementation for SpriteCollab.
use std::cell::{BorrowError, Ref, RefCell};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::future::Future;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard};
use std::time::Duration;

use anyhow::{anyhow, Error};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use fred::prelude::*;
use fred::types::RedisKey;
use git2::build::CheckoutBuilder;
use git2::{Repository, ResetType};
use log::{debug, error, info, warn};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::fs::{create_dir_all, remove_dir_all};
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::cache::{CacheBehaviour, ScCache};
use crate::config::Config;
use crate::datafiles::credit_names::{read_credit_names, CreditNames};
use crate::datafiles::group_id::GroupId;
use crate::datafiles::sprite_config::{read_sprite_config, SpriteConfig};
use crate::datafiles::tracker::{read_tracker, Group, MapImpl, Tracker};
use crate::datafiles::{read_and_report_error, try_read_in_anim_data_xml};

const GIT_REPO_DIR: &str = "spritecollab";

#[derive(Eq, PartialEq)]
enum State {
    Refreshing,
    Ready,
}

#[derive(Eq, PartialEq)]
pub struct SpriteCollabData {
    pub sprite_config: SpriteConfig,
    pub tracker: Arc<Tracker>,
    pub credit_names: CreditNames,
}

impl SpriteCollabData {
    fn new(
        sprite_config: SpriteConfig,
        mut tracker: Tracker,
        credit_names: CreditNames,
    ) -> SpriteCollabData {
        Self::sort_tracker_by_sprite_config(&mut tracker, &sprite_config);
        Self {
            sprite_config,
            tracker: Arc::new(tracker),
            credit_names,
        }
    }
}

impl SpriteCollabData {
    fn sort_tracker_by_sprite_config(
        tracker: &mut MapImpl<GroupId, Group>,
        sprite_config: &SpriteConfig,
    ) {
        let mut action_indices = BTreeMap::new();
        for (i, action) in sprite_config.actions.iter().enumerate() {
            action_indices.insert(action, i);
        }
        let mut emotion_indices = BTreeMap::new();
        for (i, emotion) in sprite_config.emotions.iter().enumerate() {
            emotion_indices.insert(emotion, i);
        }
        for group in tracker.values_mut() {
            group.sprite_files.sort_by(|k1, _, k2, _| {
                match (action_indices.get(k1), action_indices.get(k2)) {
                    (Some(i1), Some(i2)) => i1.cmp(i2),
                    (None, Some(_)) => Ordering::Greater,
                    (Some(_), None) => Ordering::Less,
                    _ => k1.cmp(k2),
                }
            });
            group.portrait_files.sort_by(|k1, _, k2, _| {
                match (emotion_indices.get(k1), emotion_indices.get(k2)) {
                    (Some(i1), Some(i2)) => i1.cmp(i2),
                    (None, Some(_)) => Ordering::Greater,
                    (Some(_), None) => Ordering::Less,
                    _ => k1.cmp(k2),
                }
            });
            Self::sort_tracker_by_sprite_config(&mut group.subgroups, sprite_config);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Meta {
    pub assets_commit: String,
    pub assets_update_date: DateTime<Utc>,
    pub update_checked_date: DateTime<Utc>,
}

impl Meta {
    fn new() -> Self {
        Self {
            assets_commit: "".to_string(),
            assets_update_date: Utc::now(),
            update_checked_date: Utc::now(),
        }
    }
}

pub struct SpriteCollab {
    state: Mutex<State>,
    meta: Mutex<RefCell<Meta>>,
    current_data: RwLock<SpriteCollabData>,
    redis: RedisClient,
}

impl SpriteCollab {
    pub async fn new((redis_url, redis_port): (String, u16)) -> Arc<Self> {
        let config = RedisConfig::from_url(&format!("redis://{}:{}", redis_url, redis_port))
            .expect("Invalid Redis config.");
        let policy = ReconnectPolicy::new_linear(10, 10000, 1000);
        let client = RedisClient::new(config, None, None, Some(policy));
        client.connect();
        client
            .wait_for_connect()
            .await
            .expect("Failed to connect to Redis.");
        let _: Option<()> = client.flushall(false).await.ok();
        info!("Connected to Redis.");

        let meta = Mutex::new(RefCell::new(Meta::new()));

        // First try an ordinary data update.
        let current_data = match refresh_data(&meta).await {
            Some(v) => RwLock::new(v),
            None => {
                // Try going back in time in the repo and updating.
                error!("Failed getting the newest data. Checking out old data until data processing works.");
                let repo_path = PathBuf::from(Config::Workdir.get()).join(GIT_REPO_DIR);
                loop {
                    let new_commit = try_checkout_previous_commit(&repo_path)
                        .expect("Failed checking out old commit.");
                    warn!("Checked out old commit: {}", new_commit);
                    if let Ok(value) = refresh_data_internal(&meta, false).await {
                        break RwLock::new(value);
                    }
                }
            }
        };

        Arc::new(Self {
            state: Mutex::new(State::Ready),
            current_data,
            redis: client,
            meta,
        })
    }

    /// Refreshes the data. Does nothing if already refreshing.
    pub async fn refresh(slf: Arc<Self>) {
        let state_lock_result = timeout(Duration::from_secs(360), slf.state.lock()).await;
        match state_lock_result {
            Ok(mut state_lock) => {
                if state_lock.deref() == &State::Refreshing {
                    return;
                }
                if let Some(new_data) = refresh_data(&slf.meta).await {
                    let changed;
                    {
                        let mut lock_data = slf.current_data.write().unwrap();
                        changed = lock_data.deref() == &new_data;
                        *lock_data = new_data;
                        *state_lock = State::Ready;
                    }
                    if changed {
                        let _: Option<()> = slf.redis.flushall(false).await.ok();
                    }
                }
            }
            Err(_) => warn!("BUG: State lock could not be acquired in SpriteCollab::refresh!"),
        }
    }

    pub fn data(&self) -> RwLockReadGuard<'_, SpriteCollabData> {
        self.current_data.read().unwrap()
    }

    pub async fn with_meta<F: FnOnce(Result<Ref<'_, Meta>, BorrowError>) -> R, R>(
        &self,
        cb: F,
    ) -> R {
        cb(self.meta.lock().await.deref().try_borrow())
    }
}

#[async_trait]
impl ScCache for SpriteCollab {
    type Error = Error;

    async fn cached_may_fail<S, Fn, Ft, T, E>(
        &self,
        cache_key: S,
        func: Fn,
    ) -> Result<Result<T, E>, Self::Error>
    where
        S: AsRef<str> + Into<RedisKey> + Send + Sync,
        Fn: (FnOnce() -> Ft) + Send,
        Ft: Future<Output = Result<CacheBehaviour<T>, E>> + Send,
        T: DeserializeOwned + Serialize + Send + Sync,
        E: Send,
    {
        let red_val: Option<String> = self.redis.get(cache_key.as_ref()).await?;
        if let Some(red_val) = red_val {
            Ok(Ok(serde_json::from_str(&red_val)?))
        } else {
            match func().await {
                Ok(CacheBehaviour::Cache(v)) => {
                    let save_string = serde_json::to_string(&v);
                    match save_string {
                        Ok(save_string) => {
                            let r: Result<(), RedisError> = self
                                .redis
                                .set(cache_key.as_ref(), save_string, None, None, false)
                                .await;
                            if let Err(err) = r {
                                warn!(
                                    "Failed writing cache entry for '{}' to Redis (stage 2): {:?}",
                                    cache_key.as_ref(),
                                    err
                                );
                            }
                        }
                        Err(err) => {
                            warn!(
                                "Failed writing cache entry for '{}' to Redis (stage 1): {:?}",
                                cache_key.as_ref(),
                                err
                            );
                        }
                    }
                    Ok(Ok(v))
                }
                Ok(CacheBehaviour::NoCache(v)) => Ok(Ok(v)),
                Err(e) => Ok(Err(e)),
            }
        }
    }
}

async fn refresh_data(meta: &Mutex<RefCell<Meta>>) -> Option<SpriteCollabData> {
    debug!("Refreshing data...");
    match refresh_data_internal(meta, true).await {
        Ok(v) => Some(v),
        Err(e) => {
            error!("Error refreshing data: {}. Gave up.", e);
            None
        }
    }
}

async fn refresh_data_internal(
    meta: &Mutex<RefCell<Meta>>,
    update: bool,
) -> Result<SpriteCollabData, Error> {
    match refresh_data_internal_do(meta, update).await {
        Ok(v) => Ok(v),
        Err(e) => {
            // Update at least the scan time
            let meta_acq = meta.lock().await;
            let mut meta_brw = meta_acq.try_borrow_mut()?;
            meta_brw.update_checked_date = Utc::now();
            Err(e)
        }
    }
}

async fn refresh_data_internal_do(
    meta: &Mutex<RefCell<Meta>>,
    update: bool,
) -> Result<SpriteCollabData, Error> {
    let repo_path = PathBuf::from(Config::Workdir.get()).join(GIT_REPO_DIR);
    let repo;
    if repo_path.exists() {
        if update {
            match try_update_repo(&repo_path) {
                Ok(v) => repo = Some(v),
                Err(clone_e) => {
                    // If this fails, throw the repo away (if applicable) and clone it new.
                    warn!(
                        "Failed to update repo, deleting and cloning it again: {}",
                        clone_e
                    );
                    if let Err(e) = remove_dir_all(&repo_path).await {
                        warn!("Failed to delete repo directory: {}", e);
                    }
                    repo = Some(create_repo(&repo_path, &Config::GitRepo.get())?);
                }
            }
        } else {
            if !repo_path.join(".git").exists() {
                return Err(anyhow!("Missing .git directory"));
            }
            repo = Some(Repository::open(&repo_path)?);
        }
    } else {
        create_dir_all(&repo_path).await?;
        repo = Some(create_repo(&repo_path, &Config::GitRepo.get())?);
    }

    let scd = SpriteCollabData::new(
        read_and_report_error(&repo_path.join("sprite_config.json"), read_sprite_config).await?,
        read_and_report_error(&repo_path.join("tracker.json"), read_tracker).await?,
        read_and_report_error(&repo_path.join("credit_names.txt"), read_credit_names).await?,
    );

    // Also try to recursively read in all AnimData.xml files, for validation.
    try_read_in_anim_data_xml(&scd.tracker).await?;

    // Update metadata
    let meta_acq = meta.lock().await;
    let mut meta_brw = meta_acq.try_borrow_mut()?;
    let commit = repo.as_ref().unwrap().head()?.peel_to_commit()?;
    let commit_time_raw = commit.time();
    let commit_time = FixedOffset::east_opt(commit_time_raw.offset_minutes() * 60)
        .unwrap()
        .from_local_datetime(
            &DateTime::from_timestamp(commit_time_raw.seconds(), 0)
                .ok_or_else(|| anyhow!("Invalid Git Commit date."))?
                .naive_utc(),
        )
        .unwrap();

    *meta_brw = Meta {
        assets_commit: commit.id().to_string(),
        assets_update_date: Utc.from_utc_datetime(&commit_time.naive_utc()),
        update_checked_date: Utc::now(),
    };

    Ok(scd)
}

fn try_checkout_previous_commit(path: &Path) -> Result<String, Error> {
    let repo = Repository::open(path)?;
    let reference = repo.head()?.peel_to_commit()?.parent(0)?;
    let name = reference.id().to_string();
    repo.reset(
        reference.as_object(),
        ResetType::Hard,
        Some(CheckoutBuilder::default().force()),
    )?;
    Ok(name)
}

fn try_update_repo(path: &Path) -> Result<Repository, Error> {
    if !path.join(".git").exists() {
        return Err(anyhow!("Missing .git directory"));
    }
    let repo = Repository::open(path)?;
    let mut remote = repo.find_remote("origin")?;
    remote.fetch(&["master"], None, None)?;
    let reference = repo.find_reference("FETCH_HEAD")?;
    repo.set_head(reference.name().unwrap())?;
    repo.checkout_head(Some(CheckoutBuilder::default().force()))?;
    Ok(Repository::open(path)?) // libgit2's borrowing code is a bit dumb
}

fn create_repo(path: &Path, clone_url: &str) -> Result<Repository, Error> {
    info!("Cloning SpriteCollab repo...");
    let repo = Repository::clone(clone_url, path)?;
    info!("Cloning SpriteCollab repo. Done!");
    Ok(repo)
}
