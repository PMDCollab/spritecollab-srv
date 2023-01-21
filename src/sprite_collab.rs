//! The actual client implementation for SpriteCollab.
use crate::cache::ScCache;
use crate::datafiles::sprite_config::{read_sprite_config, SpriteConfig};
use crate::datafiles::tracker::{read_tracker, Tracker};
use crate::datafiles::{read_and_report_error, try_read_in_anim_data_xml, DatafilesReport};
use crate::reporting::Reporting;
use crate::{Config, ReportingEvent};
use anyhow::{anyhow, Error};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use fred::prelude::*;
use fred::types::RedisKey;
use git2::build::CheckoutBuilder;
use git2::{Oid, Repository, ResetType};
use log::{debug, error, info, warn};
use sc_common::credit_names::{read_credit_names, CreditNames};
use sc_common::DataReadResult;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::cell::{BorrowError, Ref, RefCell};
use std::fmt::{Debug, Formatter};
use std::fs::File;
use std::future::Future;
use std::io::BufReader;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard};
use std::time::Duration;
use tokio::fs::{create_dir_all, remove_dir_all};
use tokio::sync::Mutex;
use tokio::time::timeout;

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

pub enum CacheBehaviour<T> {
    /// Cache this value.
    Cache(T),
    /// Do not cache this value.
    NoCache(T),
}

impl SpriteCollabData {
    fn new(
        sprite_config: SpriteConfig,
        tracker: Tracker,
        credit_names: CreditNames,
    ) -> SpriteCollabData {
        Self {
            sprite_config,
            tracker: Arc::new(tracker),
            credit_names,
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

pub struct RepositoryUpdate {
    pub(crate) repo: Repository,
    pub(crate) changelist: Vec<Oid>,
}

unsafe impl Sync for RepositoryUpdate {}

impl Debug for RepositoryUpdate {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepositoryUpdate")
            .field("<repo at>", &self.repo.path())
            .field("changelist", &self.changelist)
            .finish()
    }
}

impl RepositoryUpdate {
    fn new(repo: Repository, changelist: Vec<Oid>) -> Self {
        Self { repo, changelist }
    }

    fn new_no_changes(repo: Repository) -> Self {
        Self {
            repo,
            changelist: vec![],
        }
    }
}

pub struct SpriteCollab {
    state: Mutex<State>,
    meta: Mutex<RefCell<Meta>>,
    current_data: RwLock<SpriteCollabData>,
    reporting: Arc<Reporting>,
    redis: RedisClient,
}

impl SpriteCollab {
    pub async fn new(
        (redis_url, redis_port): (String, u16),
        reporting: Arc<Reporting>,
    ) -> Arc<Self> {
        let config = RedisConfig::from_url(&format!("redis://{}:{}", redis_url, redis_port))
            .expect("Invalid Redis config.");
        let policy = ReconnectPolicy::new_linear(10, 10000, 1000);
        let client = RedisClient::new(config, None, Some(policy));
        client.connect();
        client
            .wait_for_connect()
            .await
            .expect("Failed to connect to Redis.");
        let _: Option<()> = client.flushall(false).await.ok();
        info!("Connected to Redis.");

        let meta = Mutex::new(RefCell::new(Meta::new()));

        // First try an ordinary data update.
        let current_data = match refresh_data(reporting.clone(), &meta).await {
            Some(v) => RwLock::new(v),
            None => {
                // Try going back in time in the repo and updating.
                error!("Failed getting the newest data. Checking out old data until data processing works.");
                let repo_path = PathBuf::from(Config::Workdir.get()).join(GIT_REPO_DIR);
                let (value, new_commit) = loop {
                    let new_commit = try_checkout_previous_commit(&repo_path)
                        .expect("Failed checking out old commit.");
                    warn!("Checked out old commit: {}", new_commit);
                    if let Ok(value) = refresh_data_internal(reporting.clone(), &meta, false).await
                    {
                        break (RwLock::new(value), new_commit);
                    }
                };
                reporting
                    .send_event(ReportingEvent::StaleDatafiles(new_commit))
                    .await;
                value
            }
        };

        Arc::new(Self {
            state: Mutex::new(State::Ready),
            current_data,
            reporting,
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
                if let Some(new_data) = refresh_data(slf.reporting.clone(), &slf.meta).await {
                    let changed;
                    {
                        let mut lock_data = slf.current_data.write().unwrap();
                        changed = lock_data.deref() == &new_data;
                        *lock_data = new_data;
                        *state_lock = State::Ready;
                    }
                    if changed {
                        let _: Option<()> = slf.redis.flushall(false).await.ok();
                        #[cfg(feature = "discord")]
                        slf.pre_warm_discord().await;
                    }
                }
            }
            Err(_) => warn!("BUG: State lock could not be acquired in SpriteCollab::refresh!"),
        }
    }

    #[cfg(feature = "discord")]
    pub(crate) async fn pre_warm_discord(&self) {
        debug!("Asking Discord Bot to pre-warm user list...");
        if let Some(discord) = &self.reporting.discord_bot {
            let credit_names = self.current_data.read().unwrap().credit_names.clone();
            juniper::futures::future::try_join_all(credit_names.iter().filter_map(|credit| {
                if let Ok(id) = credit.credit_id.parse() {
                    Some(discord.pre_warm_get_user(id))
                } else {
                    None
                }
            }))
            .await
            .ok();
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

async fn refresh_data(
    reporting: Arc<Reporting>,
    meta: &Mutex<RefCell<Meta>>,
) -> Option<SpriteCollabData> {
    debug!("Refreshing data...");
    let r = match refresh_data_internal(reporting.clone(), meta, true).await {
        Ok(v) => Some(v),
        Err(e) => {
            error!("Error refreshing data: {}. Gave up.", e);
            None
        }
    };
    if r.is_some() {
        reporting
            .send_event(ReportingEvent::UpdateDatafiles(DatafilesReport::Ok))
            .await;
    }
    r
}

async fn refresh_data_internal(
    reporting: Arc<Reporting>,
    meta: &Mutex<RefCell<Meta>>,
    update: bool,
) -> Result<SpriteCollabData, Error> {
    match refresh_data_internal_do(reporting, meta, update).await {
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
    reporting: Arc<Reporting>,
    meta: &Mutex<RefCell<Meta>>,
    update: bool,
) -> Result<SpriteCollabData, Error> {
    let repo_path = PathBuf::from(Config::Workdir.get()).join(GIT_REPO_DIR);
    let repo_update;
    if repo_path.exists() {
        if update {
            match try_update_repo(&repo_path) {
                Ok(v) => repo_update = Some(v),
                Err(clone_e) => {
                    // If this fails, throw the repo away (if applicable) and clone it new.
                    warn!(
                        "Failed to update repo, deleting and cloning it again: {}",
                        clone_e
                    );
                    if let Err(e) = remove_dir_all(&repo_path).await {
                        warn!("Failed to delete repo directory: {}", e);
                    }
                    repo_update = Some(create_repo(&repo_path, &Config::GitRepo.get())?);
                }
            }
        } else {
            if !repo_path.join(".git").exists() {
                return Err(anyhow!("Missing .git directory"));
            }
            repo_update = Some(RepositoryUpdate::new_no_changes(Repository::open(
                &repo_path,
            )?));
        }
    } else {
        create_dir_all(&repo_path).await?;
        repo_update = Some(create_repo(&repo_path, &Config::GitRepo.get())?);
    }

    let repo_update =
        repo_update.ok_or_else(|| anyhow!("Internal Error: Repository was None during update."))?;

    if let Some(last) = repo_update.changelist.last() {
        info!(
            "Trying updating repo to commit {} ({} new commits)...",
            last,
            repo_update.changelist.len()
        )
    }

    let commit = repo_update.repo.head()?.peel_to_commit()?;
    let commit_id = commit.id();
    let commit_time_raw = commit.time();
    let commit_time = FixedOffset::east_opt(commit_time_raw.offset_minutes() * 60)
        .unwrap()
        .from_local_datetime(
            &NaiveDateTime::from_timestamp_opt(commit_time_raw.seconds(), 0)
                .ok_or_else(|| anyhow!("Invalid Git Commit date."))?,
        )
        .unwrap();
    drop(commit);

    #[cfg(feature = "activity")]
    reporting.update_activity(repo_update).await?;

    async fn do_read_credit_names(p: &PathBuf) -> DataReadResult<CreditNames> {
        read_credit_names(BufReader::new(File::open(p)?))
    }

    let scd = SpriteCollabData::new(
        read_and_report_error(
            &repo_path.join("sprite_config.json"),
            read_sprite_config,
            &reporting,
        )
        .await?,
        read_and_report_error(&repo_path.join("tracker.json"), read_tracker, &reporting).await?,
        read_and_report_error(
            &repo_path.join("credit_names.txt"),
            do_read_credit_names,
            &reporting,
        )
        .await?,
    );

    // Also try to recursively read in all AnimData.xml files, for validation.
    try_read_in_anim_data_xml(&scd.tracker, &reporting).await?;

    // Update metadata
    let meta_acq = meta.lock().await;
    let mut meta_brw = meta_acq.try_borrow_mut()?;

    *meta_brw = Meta {
        assets_commit: commit_id.to_string(),
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

fn try_update_repo(path: &Path) -> Result<RepositoryUpdate, Error> {
    if !path.join(".git").exists() {
        return Err(anyhow!("Missing .git directory"));
    }
    let repo = Repository::open(path)?;
    let mut remote = repo.find_remote("origin")?;
    remote.fetch(&["master"], None, None)?;
    let reference = repo.find_reference("FETCH_HEAD")?;
    let old_reference = repo.head()?.peel_to_commit()?.id();
    repo.set_head(reference.name().unwrap())?;
    repo.checkout_head(Some(CheckoutBuilder::default().force()))?;

    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    walk.hide(old_reference)?;
    let changes = walk.collect::<Result<Vec<Oid>, _>>()?;

    Ok(RepositoryUpdate::new(Repository::open(path)?, changes)) // libgit2's borrowing code is a bit dumb
}

fn create_repo(path: &Path, clone_url: &str) -> Result<RepositoryUpdate, Error> {
    info!("Cloning SpriteCollab repo...");
    let repo = Repository::clone(clone_url, path)?;
    info!("Cloning SpriteCollab repo. Done!");
    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    let changes = walk.collect::<Result<Vec<Oid>, _>>()?;
    Ok(RepositoryUpdate::new(repo, changes))
}
