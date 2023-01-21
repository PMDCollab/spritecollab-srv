mod local_credits_file;
mod serialize_oid;

use crate::local_credits_file::{
    get_credits_until, get_last_credits_old_format, get_latest_credits,
};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use csv::DeserializeErrorKind;
use git2::{Blob, Commit, Delta, Deltas, Oid, Repository, Time, Tree};
use lazy_static::lazy_static;
use log::warn;
use sc_common::credit_names::{read_credit_names, CreditNames};
use sc_common::DataReadError;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io;
use std::io::BufReader;
use std::mem::discriminant;
use std::num::ParseIntError;
use std::path::{Path, PathBuf};
use thiserror::Error;

lazy_static! {
    static ref CREDIT_CONSISTENCY_TIME: DateTime<Utc> = {
        let time = NaiveDate::from_ymd_opt(2022, 5, 7)
            .unwrap()
            .and_hms_opt(19, 29, 49)
            .unwrap();
        DateTime::<Utc>::from_utc(time, Utc)
    };
}

#[derive(Error, Debug)]
pub enum ActivityRecError {
    #[error("Git internal error: {0}")]
    GitError(#[from] git2::Error),
    #[error("I/O error: {0}")]
    IoError(#[from] io::Error),
    #[error("Failed reading file: {0}")]
    DataReadError(#[from] DataReadError),
    #[error("Git internal error: Expected metadata for delta ({1}) but got None. Commit: {0}")]
    MissingDeltaMetadata(Oid, &'static str),
    #[error("Can not process Git Delta type {1:?} (commit {0}, path: {2})")]
    UnprocessableDelta(Oid, Delta, PathBuf),
    #[error("An asset was moved in a way that makes no sense: {0} -> {1}")]
    InvalidMove(PathBuf, PathBuf),
    #[error(
        "Expected a path containing valid monster and farm ids, was unable to parse ('{0}'): {1}"
    )]
    InvalidNumberInPath(PathBuf, ParseIntError),
    #[error("No credits found for an asset: {1:?} at commit {0}")]
    MissingCredits(Oid, Box<SpritePathInfo>),
    #[error("The Activities instance tied to the Activity was dropped")]
    StaleActivityReference,
    #[error("Lock was poisoned while trying to read data from Activities")]
    PoisonError,
}

#[derive(Debug, Clone)]
pub struct SpritePathInfo {
    monster_idx: i32,
    path_to_form: Vec<i32>,
    asset: Asset,
    base_path: PathBuf,
}

impl SpritePathInfo {
    /// Returns None if the path does not appear to be a sprite action or portrait emotion.
    /// Otherwise tries to figure out what it is and returns it's info.
    pub fn try_from_path(tree: &Tree, path: &Path) -> Result<Option<Self>, ActivityRecError> {
        let Some(base_path) = path.parent() else {
            return Ok(None);
        };
        let mut path_parts = path
            .components()
            .map(|v| v.as_os_str().to_string_lossy())
            .collect::<VecDeque<_>>();
        let first_path_part = path_parts.pop_front();
        let asset = match first_path_part.as_ref() {
            Some(Cow::Borrowed("portrait")) => {
                let Some(file_name) = path_parts.pop_back() else {
                    return Ok(None);
                };
                if !file_name.ends_with(".png") {
                    return Ok(None);
                }
                let emotion_name = &file_name[..file_name.len() - 4];
                Asset::Portrait {
                    name: emotion_name.to_string(),
                    file: File {
                        file_name: file_name.to_string().into(),
                        oid: Self::get_blob_oid(tree, path),
                    },
                }
            }
            Some(Cow::Borrowed("sprite")) => {
                let Some(file_name) = path_parts.pop_back() else {
                    return Ok(None);
                };

                // Currently the sprites are split into three files:
                // ...-Anim.png, ...-Shadow.png, ...-Offsets.png
                // If the file contains -Anim at the end, strip that. Skip offsets and shadows, so
                // we don't generate duplicate activities.
                if !file_name.ends_with("-Anim.png") {
                    return Ok(None);
                }
                let action_name = &file_name[..file_name.len() - 9];

                // Check if an AnimData.xml and the other two sprite files exists
                static ANIM_DATA_XML_NAME: &str = "AnimData.xml";
                let anim_data_xml = base_path.join(ANIM_DATA_XML_NAME);
                let anim_png = path;
                let shadow_png_name = format!("{action_name}-Shadow.png");
                let shadow_png = base_path.join(&shadow_png_name);
                let offsets_png_name = format!("{action_name}-Offsets.png");
                let offsets_png = base_path.join(&offsets_png_name);

                Asset::Sprite {
                    name: action_name.to_string(),
                    anim_sprite: File {
                        file_name: file_name.to_string().into(),
                        oid: Self::get_blob_oid(tree, anim_png),
                    },
                    shadow_sprite: File {
                        file_name: shadow_png_name.into(),
                        oid: Self::get_blob_oid(tree, &shadow_png),
                    },
                    offsets_sprite: File {
                        file_name: offsets_png_name.into(),
                        oid: Self::get_blob_oid(tree, &offsets_png),
                    },
                    anim_xml: File {
                        file_name: ANIM_DATA_XML_NAME.into(),
                        oid: Self::get_blob_oid(tree, &anim_data_xml),
                    },
                }
            }
            _ => return Ok(None),
        };

        let Some(monster_idx) = path_parts.pop_front() else {
            return Ok(None);
        };
        let monster_idx = monster_idx
            .parse::<i32>()
            .map_err(|err| ActivityRecError::InvalidNumberInPath(path.to_path_buf(), err))?;
        let path_to_form = path_parts
            .into_iter()
            .map(|v| v.parse::<i32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| ActivityRecError::InvalidNumberInPath(path.to_path_buf(), err))?;

        Ok(Some(Self {
            monster_idx,
            path_to_form,
            asset,
            base_path: base_path.to_path_buf(),
        }))
    }

    fn get_blob_oid(tree: &Tree, path: &Path) -> Option<Oid> {
        tree.get_path(path).ok().map(|te| te.id())
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum Action {
    Add,
    Remove,
    Update,
    MoveAndUpdate {
        new_monster_idx: i32,
        new_path_to_form: Vec<i32>,
    },
}

impl Action {
    /// Any action that does not delete an asset has content.
    pub fn has_content(&self) -> bool {
        match self {
            Action::Add => true,
            Action::Remove => false,
            Action::Update => true,
            Action::MoveAndUpdate { .. } => true,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, Serialize)]
pub struct File {
    pub file_name: Cow<'static, str>,
    #[serde(with = "serialize_oid::option")]
    pub oid: Option<Oid>, // None = deleted
}

impl File {
    pub fn contents<'a>(
        &self,
        repo: &'a Repository,
    ) -> Result<Option<impl AsRef<[u8]> + 'a>, ActivityRecError> {
        self.oid
            .map::<Result<_, ActivityRecError>, _>(|oid| Ok(WrappedBlob(repo.find_blob(oid)?)))
            .transpose()
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, Serialize)]
pub enum Asset {
    Portrait {
        name: String,
        file: File,
    },
    Sprite {
        name: String,
        anim_sprite: File,
        shadow_sprite: File,
        offsets_sprite: File,
        anim_xml: File,
    },
}

impl Asset {
    pub fn files<'a>(&'a self) -> Box<dyn Iterator<Item = &'a File> + 'a> {
        match self {
            Asset::Portrait { file, .. } => Box::new([file].into_iter()),
            Asset::Sprite {
                anim_sprite,
                shadow_sprite,
                offsets_sprite,
                anim_xml,
                ..
            } => Box::new([anim_sprite, shadow_sprite, offsets_sprite, anim_xml].into_iter()),
        }
    }
}

impl Asset {
    pub fn name(&self) -> &str {
        match self {
            Asset::Portrait { name, .. } => name.as_ref(),
            Asset::Sprite { name, .. } => name.as_ref(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Activity<'c> {
    monster_idx: i32,
    path_to_form: Vec<i32>,
    asset: Asset,
    action: Action,
    /// None for removals.
    credit_id: Option<Cow<'c, str>>,
    /// In it's early days SpriteCollab did not track which emotion / action was made by who.
    /// As such we are using the latest credits entry for these submissions, however they may
    /// not be 100% correct, since if multiple authors submitted emotions/actions in a single
    /// commit for the same form, it's impossible to know who did what. In these cases, the
    /// latest author is returned.
    ///
    /// There are also a few even odder edge cases where this may be true.
    author_uncertain: bool,
}

impl<'c> Activity<'c> {
    pub fn monster_idx(&self) -> i32 {
        self.monster_idx
    }
    pub fn path_to_form(&self) -> &[i32] {
        &self.path_to_form
    }
    pub fn asset(&self) -> &Asset {
        &self.asset
    }
    pub fn action(&self) -> &Action {
        &self.action
    }
    pub fn credit_id(&self) -> Option<&Cow<'c, str>> {
        self.credit_id.as_ref()
    }
    pub fn author_uncertain(&self) -> bool {
        self.author_uncertain
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CommitData {
    #[serde(with = "serialize_oid")]
    id: Oid,
    time: DateTime<Utc>,
    msg: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExportedActivity {
    commit: CommitData,
    activity: Activity<'static>,
}

/// See note for [`Activity::author_uncertain`]
#[derive(Debug, Clone)]
enum CreditCertainty {
    Certain(String),
    Maybe(String),
}

impl CreditCertainty {
    pub fn into_id(self) -> String {
        match self {
            CreditCertainty::Certain(v) => v,
            CreditCertainty::Maybe(v) => v,
        }
    }
    pub fn certain(&self) -> bool {
        match self {
            CreditCertainty::Certain(_) => true,
            CreditCertainty::Maybe(_) => false,
        }
    }
}

pub struct Activities<'c> {
    c_oid: Oid,
    c_time: Time,
    c_msg: String,
    credit_names: CreditNames,
    acts: Vec<Activity<'c>>,
}

impl<'c> Activities<'c> {
    /// head_commit is the latest commit in the repo, used for credits lookups after May 7th 2022.
    pub fn load(
        repository: &Repository,
        commit: Commit,
        head_commit: Commit,
        deltas: Deltas<'_>,
    ) -> Result<Activities<'c>, ActivityRecError> {
        let credits_file = read_file_at_commit(repository, &commit, Path::new("credit_names.txt"))?;
        let credit_names = read_credit_names(BufReader::new(credits_file.as_ref()))?;

        let mut slf = Self {
            c_oid: commit.id(),
            c_time: commit.time(),
            c_msg: commit.message().unwrap_or_default().to_string(),
            credit_names,
            acts: Vec::with_capacity(deltas.len()),
        };

        let tree = commit.tree()?;

        for delta in deltas {
            let old_file_path =
                delta
                    .old_file()
                    .path()
                    .ok_or(ActivityRecError::MissingDeltaMetadata(
                        commit.id(),
                        "old_file().path()",
                    ))?;

            let Some(old_info) = SpritePathInfo::try_from_path(&tree, old_file_path)? else {
                // This doesn't appear to be a sprite/portrait. Log a warning if we didn't expect this file.
                let file_name = old_file_path.file_name().unwrap_or_default().to_string_lossy();
                match file_name.as_ref() {
                    "credit_names.txt" | "tracker.json" | "credits.txt" |
                    "AnimData.xml" | "Animations.xml"| "animations.xml" | 
                    "FrameData.xml" | "sheet.png" | "LICENSE" | "README.md" => {},
                    v => {
                        if !v.ends_with("-Shadow.png") && !v.ends_with("-Offsets.png") {
                            warn!("Unexpected file in repository commit, skipped: {}", old_file_path.as_os_str().to_string_lossy())
                        }
                    }
                }
                continue;
            };

            let activity =
                match delta.status() {
                    Delta::Added | Delta::Copied => Self::record_activity(
                        Action::Add,
                        Some(Self::get_credit_id(
                            repository,
                            &old_info,
                            &commit,
                            &head_commit,
                        )?),
                        old_info,
                    )?,
                    Delta::Deleted => Self::record_activity(Action::Remove, None, old_info)?,
                    Delta::Modified => Self::record_activity(
                        Action::Update,
                        Some(Self::get_credit_id(
                            repository,
                            &old_info,
                            &commit,
                            &head_commit,
                        )?),
                        old_info,
                    )?,
                    Delta::Renamed => {
                        let new_file_path = delta.new_file().path().ok_or(
                            ActivityRecError::MissingDeltaMetadata(
                                commit.id(),
                                "new_file().path()",
                            ),
                        )?;

                        let new_info = SpritePathInfo::try_from_path(&tree, new_file_path)?
                            .ok_or_else(|| {
                                ActivityRecError::InvalidMove(
                                    old_file_path.to_path_buf(),
                                    new_file_path.to_path_buf(),
                                )
                            })?;

                        if discriminant(&old_info.asset) != discriminant(&new_info.asset) {
                            return Err(ActivityRecError::InvalidMove(
                                old_file_path.to_path_buf(),
                                new_file_path.to_path_buf(),
                            ));
                        }

                        Self::record_activity(
                            Action::MoveAndUpdate {
                                new_monster_idx: new_info.monster_idx,
                                new_path_to_form: new_info.path_to_form,
                            },
                            Some(Self::get_credit_id(
                                repository,
                                &old_info,
                                &commit,
                                &head_commit,
                            )?),
                            old_info,
                        )?
                    }
                    other => {
                        return Err(ActivityRecError::UnprocessableDelta(
                            commit.id(),
                            other,
                            old_file_path.to_path_buf(),
                        ));
                    }
                };

            slf.acts.push(activity);
        }
        Ok(slf)
    }

    pub fn export(&self) -> Vec<ExportedActivity> {
        self.acts
            .iter()
            .map(|act| ExportedActivity {
                commit: CommitData {
                    id: self.c_oid,
                    time: Utc.timestamp(self.c_time.seconds(), 0),
                    msg: self.c_msg.clone(),
                },
                activity: Activity {
                    monster_idx: act.monster_idx,
                    path_to_form: act.path_to_form.clone(),
                    asset: act.asset.clone(),
                    action: act.action.clone(),
                    credit_id: act.credit_id.as_ref().map(|cid| cid.to_string().into()),
                    author_uncertain: act.author_uncertain,
                },
            })
            .collect()
    }

    fn record_activity(
        action: Action,
        credit_id: Option<CreditCertainty>,
        info: SpritePathInfo,
    ) -> Result<Activity<'c>, ActivityRecError> {
        Ok(Activity {
            monster_idx: info.monster_idx,
            path_to_form: info.path_to_form,
            asset: info.asset,
            action,
            author_uncertain: credit_id
                .as_ref()
                .map(|cid| !cid.certain())
                .unwrap_or_default(),
            credit_id: credit_id.map(|cid| cid.into_id().into()),
        })
    }

    fn get_credit_id<'a>(
        repo: &Repository,
        info: &'a SpritePathInfo,
        commit: &Commit,
        head_commit: &Commit,
    ) -> Result<CreditCertainty, ActivityRecError> {
        // After May 7th 2022 we can look at the current origin/master version of the credits file
        // to find the proper author.
        let commit_time = Utc.timestamp(commit.time().seconds(), 0);
        if &commit_time > &CREDIT_CONSISTENCY_TIME {
            Self::new_credit_lookup(repo, info, commit, commit_time, head_commit)
        } else {
            // Before that, we determine it from the commit
            Self::old_credit_lookup(repo, info, commit)
        }
    }

    /// Tries to:
    /// - Lookup newest author in current HEAD at the commit time
    ///   -> If the file does not exist at HEAD falls back to old method
    ///   -> If that fails: fails
    /// - Falls back to ? newest author at the commit time
    /// - Then falls back to old method
    /// - Then falls back to newest author in current HEAD RIGHT NOW
    /// - Then falls back to ? newest author in current HEAD  RIGHT NOW
    /// - Then fails
    fn new_credit_lookup<'a>(
        repo: &Repository,
        info: &'a SpritePathInfo,
        commit: &Commit,
        time: DateTime<Utc>,
        head_commit: &Commit,
    ) -> Result<CreditCertainty, ActivityRecError> {
        let mut path_to_credits = info.base_path.clone();
        path_to_credits.push("credits.txt");
        let Ok(credit_file_head) = read_file_at_commit(repo, head_commit, &path_to_credits) else {
            // The entry was removed or moved in HEAD. Fall back to old method.
            return Self::old_credit_lookup(repo, info, commit);
        };
        let Ok(mut current_credits) = get_credits_until(credit_file_head.as_ref(), time) else {
            return Err(ActivityRecError::MissingCredits(
                commit.id(),
                Box::new(info.clone()),
            ));
        };
        // New credits format:
        let credit_id = {
            if let Some(credit_id) = current_credits.remove(info.asset.name()) {
                Ok(CreditCertainty::Certain(credit_id))
            }
            // Try to get the latest ? from the old format instead.
            else if let Some(question_credit_id) = current_credits.remove("?") {
                Ok(CreditCertainty::Maybe(question_credit_id))
            } else {
                // uhhh help? Let's fall back to old method.
                match Self::old_credit_lookup(repo, info, commit) {
                    Ok(v) => Ok(v),
                    Err(_) => {
                        // Okay hm. In that case as a last resort, try to get current author and hope.
                        match get_latest_credits(credit_file_head.as_ref()) {
                            Ok(mut current_credits) => {
                                if let Some(credit_id) = current_credits.remove(info.asset.name()) {
                                    Ok(CreditCertainty::Maybe(credit_id))
                                } else if let Some(question_credit_id) = current_credits.remove("?")
                                {
                                    Ok(CreditCertainty::Maybe(question_credit_id))
                                } else {
                                    // We tried everything!
                                    Err(ActivityRecError::MissingCredits(
                                        commit.id(),
                                        Box::new(info.clone()),
                                    ))
                                }
                            }
                            Err(_) => Err(ActivityRecError::MissingCredits(
                                commit.id(),
                                Box::new(info.clone()),
                            )),
                        }
                    }
                }
            }
        }?;
        Ok(credit_id)
    }

    /// Tries to
    /// - Handle some edge cases
    /// - Lookup newest author in that commit
    /// - Falls back to ? newest author at that commit
    /// - Falls back to reading old credits file, taking the newest author at that commit
    /// - Fails
    fn old_credit_lookup<'a>(
        repo: &Repository,
        info: &'a SpritePathInfo,
        commit: &Commit,
    ) -> Result<CreditCertainty, ActivityRecError> {
        // EXCEPTIONS
        // This commit contains portraits that should have been included in one commit later.
        if commit.id() == Oid::from_str("99a41c3c379300aefa42f95568b658c3b9986057")?
            && info.monster_idx == 222
            && info.path_to_form == [1]
        {
            return Ok(CreditCertainty::Certain("356635814668664832".to_string()));
        }
        if commit.id() == Oid::from_str("366d2dbceb2736bd5316c9e492ddfa6c7cdc8fab")?
            && info.monster_idx == 150
            && info.path_to_form == [2, 1]
        {
            return Ok(CreditCertainty::Certain("593113130213572610".to_string()));
        }

        let mut path_to_credits = info.base_path.clone();
        path_to_credits.push("credits.txt");
        let credit_file_at_commit = read_file_at_commit(repo, commit, &path_to_credits)?;
        let credit_id = {
            match get_latest_credits(credit_file_at_commit.as_ref()) {
                Ok(mut current_credits) => {
                    // New credits format:
                    if let Some(credit_id) = current_credits.remove(info.asset.name()) {
                        Ok(CreditCertainty::Certain(credit_id))
                    }
                    // Try to get the latest ? from the old format instead.
                    else if let Some(question_credit_id) = current_credits.remove("?") {
                        Ok(CreditCertainty::Maybe(question_credit_id))
                    } else {
                        Err(ActivityRecError::MissingCredits(
                            commit.id(),
                            Box::new(info.clone()),
                        ))
                    }
                }
                Err(DataReadError::SerdeCsv(err)) => {
                    let source_err = DataReadError::SerdeCsv(err.clone());
                    match err.kind() {
                        csv::ErrorKind::Deserialize { err, .. } => {
                            match err.kind() {
                                DeserializeErrorKind::Message(_) => {
                                    // Try reading in the older format.
                                    if let Some(credit_id) =
                                        get_last_credits_old_format(credit_file_at_commit)?
                                    {
                                        Ok(CreditCertainty::Maybe(credit_id))
                                    } else {
                                        Err(ActivityRecError::MissingCredits(
                                            commit.id(),
                                            Box::new(info.clone()),
                                        ))
                                    }
                                }
                                _ => Err(source_err.into()),
                            }
                        }
                        _ => Err(source_err.into()),
                    }
                }
                Err(err) => Err(err.into()),
            }
        }?;
        Ok(credit_id)
    }

    pub fn c_oid(&self) -> Oid {
        self.c_oid
    }
    pub fn c_time(&self) -> Time {
        self.c_time
    }
    pub fn c_msg(&self) -> &str {
        &self.c_msg
    }
    pub fn acts(&self) -> &[Activity<'c>] {
        &self.acts
    }
    pub fn credit_names(&self) -> &CreditNames {
        &self.credit_names
    }
}

struct WrappedBlob<'a>(Blob<'a>);

impl<'a> AsRef<[u8]> for WrappedBlob<'a> {
    fn as_ref(&self) -> &[u8] {
        self.0.content()
    }
}

/// Reads the file from the given commit.
fn read_file_at_commit<'a>(
    repo: &'a Repository,
    commit: &Commit<'a>,
    path: &Path,
) -> Result<WrappedBlob<'a>, ActivityRecError> {
    let blob_id = commit.tree()?.get_path(path)?.id();
    let blob = repo.find_blob(blob_id)?;
    Ok(WrappedBlob(blob))
}

pub fn get_activities<'o: 'c, 'c>(
    repo: &'o Repository,
    commit: Oid,
    head_commit: Oid,
) -> Result<Activities<'c>, ActivityRecError> {
    let commit_obj = repo.find_commit(commit)?;
    let head_commit_obj = repo.find_commit(head_commit)?;
    let parent_tree = commit_obj
        .parent(0)
        .ok()
        .map(|prnt| prnt.tree())
        .transpose()?;

    let changeset =
        repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_obj.tree()?), None)?;

    Activities::load(repo, commit_obj, head_commit_obj, changeset.deltas())
}

pub async fn process_commit(
    _repo: &Repository,
    _commit: Oid,
    _head_commit: Oid,
) -> Result<(), ActivityRecError> {
    todo!()
}
