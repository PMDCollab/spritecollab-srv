//! The actual client implementation for SpriteCollab.
use async_trait::async_trait;
use std::future::Future;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard};
use anyhow::Error;
use fred::prelude::*;
use fred::types::RedisKey;
use git2::build::CheckoutBuilder;
use git2::Repository;
use log::{debug, error, info, warn};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::fs::create_dir_all;
use tokio::sync::Mutex;
use crate::cache::ScCache;
use crate::{Config, ReportingEvent};
use crate::datafiles::credit_names::{CreditNames, read_credit_names};
use crate::datafiles::{DatafilesReport, read_and_report_error};
use crate::datafiles::sprite_config::{read_sprite_config, SpriteConfig};
use crate::datafiles::tracker::{read_tracker, Tracker};
use crate::reporting::Reporting;

const GIT_REPO_DIR: &str = "spritecollab";

#[derive(Eq, PartialEq)]
enum State {
    Refreshing, Ready
}

pub struct SpriteCollabData {
    pub sprite_config: SpriteConfig,
    pub tracker: Arc<Tracker>,
    pub credit_names: CreditNames
}

pub enum CacheBehaviour<T> {
    /// Cache this value.
    Cache(T),
    /// Do not cache this value.
    NoCache(T)
}

impl SpriteCollabData {
    fn new(sprite_config: SpriteConfig, tracker: Tracker, credit_names: CreditNames) -> SpriteCollabData {
        Self {
            sprite_config,
            tracker: Arc::new(tracker),
            credit_names
        }
    }
}

pub struct SpriteCollab {
    state: Mutex<State>,
    current_data: RwLock<SpriteCollabData>,
    reporting: Arc<Reporting>,
    redis: RedisClient
}

impl SpriteCollab {
    pub async fn new((redis_url, redis_port): (String, u16), reporting: Arc<Reporting>) -> Arc<Self> {
        let config = RedisConfig::from_url(
            &format!("redis://{}:{}", redis_url, redis_port)
        ).expect("Invalid Redis config.");
        let policy = ReconnectPolicy::new_linear(10, 10000, 1000);
        let client = RedisClient::new(config);
        client.connect(Some(policy));
        client.wait_for_connect().await.expect("Failed to connect to Redis.");
        let _: Option<()> = client.flushall(false).await.ok();
        info!("Connected to Redis.");

        let current_data = RwLock::new(
            refresh_data(reporting.clone()).await.unwrap_or_else(|| {
                panic!("Error initializing data.");
            })
        );

        let slf_arc = Arc::new(
            Self {
                state: Mutex::new(State::Ready),
                current_data,
                reporting,
                redis: client
            }
        );

        slf_arc
    }

    /// Refreshes the data. Does nothing if already refreshing.
    pub async fn refresh(slf: Arc<Self>) {
        if slf.state.lock().await.deref() == &State::Refreshing {
            return;
        }
        if let Some(new_data) = refresh_data(slf.reporting.clone()).await {
            let mut lock_data = slf.current_data.write().unwrap();
            let mut lock_state = slf.state.lock().await;
            *lock_data = new_data;
            let _: Option<()> = slf.redis.flushall(false).await.ok();
            *lock_state = State::Ready;
        }
    }

    pub fn data(&self) -> RwLockReadGuard<'_, SpriteCollabData> {
        self.current_data.read().unwrap()
    }
}

#[async_trait]
impl ScCache for SpriteCollab {
    type Error = Error;

    async fn cached_may_fail<S, Fn, Ft, T, E>(&self, cache_key: S, func: Fn) -> Result<Result<T, E>, Self::Error>
        where
            S: AsRef<str> + Into<RedisKey> + Send + Sync,
            Fn: (FnOnce() -> Ft) + Send,
            Ft: Future<Output = Result<CacheBehaviour<T>, E>> + Send,
            T: DeserializeOwned + Serialize + Send + Sync,
            E: Send
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
                            let r: Result<(), RedisError> = self.redis.set(
                                cache_key.as_ref(),
                                save_string,
                                None, None, false
                            ).await;
                            if let Err(err) = r {
                                warn!("Failed writing cache entry for '{}' to Redis (stage 2): {:?}", cache_key.as_ref(), err);
                            }
                        }
                        Err(err) => {
                            warn!("Failed writing cache entry for '{}' to Redis (stage 1): {:?}", cache_key.as_ref(), err);
                        }
                    }
                    Ok(Ok(v))
                },
                Ok(CacheBehaviour::NoCache(v)) => Ok(Ok(v)),
                Err(e) => Ok(Err(e))
            }
        }
    }
}

async fn refresh_data(reporting: Arc<Reporting>) -> Option<SpriteCollabData> {
    debug!("Refreshing data...");
    let r = match refresh_data_internal(reporting.clone()).await {
        Ok(v) => Some(v),
        Err(e) => {
            error!("Error refreshing data: {}. Gave up.", e);
            None
        }
    };
    reporting.send_event(ReportingEvent::UpdateDatafiles(DatafilesReport::Ok)).await;
    r
}

async fn refresh_data_internal(reporting: Arc<Reporting>) -> Result<SpriteCollabData, Error> {
    let repo_path = PathBuf::from(Config::Workdir.get()).join(GIT_REPO_DIR);
    create_dir_all(&repo_path).await?;
    if try_update_repo(&repo_path).is_err() {
        // If this fails, throw the repo away (if applicable) and clone it new.
        create_repo(&repo_path, &Config::GitRepo.get())?;
    }

    Ok(SpriteCollabData::new(
        read_and_report_error(&repo_path.join("sprite_config.json"), read_sprite_config, &reporting).await?,
        read_and_report_error(&repo_path.join("tracker.json"), read_tracker, &reporting).await?,
        read_and_report_error(&repo_path.join("credit_names.txt"), read_credit_names, &reporting).await?
    ))
}

fn try_update_repo(path: &Path) -> Result<(), Error> {
    let repo = Repository::open(path)?;
    let mut remote = repo.find_remote("origin")?;
    remote.fetch(&["master"], None, None)?;
    let reference = repo.find_reference("FETCH_HEAD")?;
    repo.set_head(reference.name().unwrap())?;
    repo.checkout_head(Some(CheckoutBuilder::default().force()))?;
    Ok(())
}

fn create_repo(path: &Path, clone_url: &str) -> Result<(), Error> {
    info!("Cloning SpriteCollab repo...");
    Repository::clone(clone_url, path)?;
    info!("Cloning SpriteCollab repo. Done!");
    Ok(())
}
