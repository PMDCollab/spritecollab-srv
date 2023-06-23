#![cfg_attr(not(feature = "discord"), allow(unused_variables))]

use crate::assets::fs_check::{
    get_existing_portrait_file, get_existing_sprite_file, iter_existing_portrait_files,
    iter_existing_sprite_files,
};
use crate::assets::url::{get_url, AssetType};
use crate::cache::ScCache;
use crate::config::Config as SystemConfig;
use crate::datafiles::anim_data_xml::AnimDataXml;
use crate::datafiles::credit_names::{parse_credit_id, CreditNamesRow};
use crate::datafiles::sprite_config::SpriteConfig;
use crate::datafiles::tracker::{fuzzy_find_tracker, FormMatch, Group, MonsterFormCollector};
use crate::reporting::Reporting;
use crate::sprite_collab::{CacheBehaviour, SpriteCollab};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fred::types::RedisKey;
use itertools::Itertools;
use juniper::{
    graphql_object, graphql_value, FieldError, FieldResult, GraphQLEnum, GraphQLObject,
    GraphQLUnion,
};
#[allow(unused_imports)]
use log::warn;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fmt::Debug;
use std::future::Future;
use std::iter::once;
use std::sync::Arc;

/// Maximum length for search query strings
const MAX_QUERY_LEN: usize = 75;
const API_VERSION: &str = "1.3";

#[repr(i64)]
#[derive(GraphQLEnum)]
#[graphql(description = "The current phase of the sprite or portrait.")]
pub enum Phase {
    Incomplete = 0,
    Exists = 1,
    Full = 2,
    #[graphql(
        description = "Returned if the phase value is non-standard. Use phaseRaw to get the raw ID."
    )]
    Unknown = -1,
}

impl From<i64> for Phase {
    fn from(phase: i64) -> Self {
        match phase {
            0 => Phase::Incomplete,
            1 => Phase::Exists,
            2 => Phase::Full,
            _ => Phase::Unknown,
        }
    }
}

#[derive(GraphQLObject)]
#[graphql(description = "A single sprite for a single action.")]
pub struct Sprite {
    #[graphql(description = "Action of this sprite.")]
    action: String,
    #[graphql(
        description = "Whether or not this sprite is locked and requires special permissions to be updated."
    )]
    locked: bool,
    #[graphql(
        description = "URL to the sprite sheet containing the actual frames for the animation."
    )]
    anim_url: String,
    #[graphql(
        description = "URL to the sprite sheet containing the sprite offset pixels for each frame."
    )]
    offsets_url: String,
    #[graphql(
        description = "URL to the sprite sheet containing the shadow placeholders for each frame."
    )]
    shadows_url: String,
}

#[derive(GraphQLObject)]
#[graphql(description = "A sprite, which is a copy of another sprite.")]
pub struct CopyOf {
    #[graphql(description = "Action of this sprite.")]
    action: String,
    #[graphql(
        description = "Whether or not this sprite is locked and requires special permissions to be updated."
    )]
    locked: bool,
    #[graphql(description = "Which action this sprite is a copy of.")]
    copy_of: String,
}

#[derive(GraphQLUnion)]
#[graphql(
    description = "A single sprite for a single action that is either a copy of another sprite (as defined in the AnimData.xml) or has it's own sprite data."
)]
enum SpriteUnion {
    Sprite(Sprite),
    CopyOf(CopyOf),
}

#[derive(GraphQLObject)]
#[graphql(description = "A single portrait for a single emotion.")]
pub struct Portrait {
    #[graphql(description = "Name of the emotion.")]
    emotion: String,
    #[graphql(
        description = "Whether or not this sprite is locked and requires special permissions to be updated."
    )]
    locked: bool,
    #[graphql(description = "URL to the portraits.")]
    url: String,
}

#[derive(GraphQLObject)]
#[graphql(description = "A bounty for a non-standard phase.")]
pub struct OtherBounty {
    phase: i32,
    bounty: i32,
}

#[derive(GraphQLObject)]
#[graphql(
    description = "A SkyTemple Discord Server Guild Point bounty that will be rewarded, if the portrait or sprite has transitioned into a phase."
)]
pub struct MonsterBounty {
    #[graphql(
        description = "If true, SpriteBot will not automatically hand out the Guild Point bounty."
    )]
    modreward: bool,
    #[graphql(description = "Amount of points to reward if the phase changes to Incomplete.")]
    incomplete: Option<i32>,
    #[graphql(description = "Amount of points to reward if the phase changes to Exists.")]
    exists: Option<i32>,
    #[graphql(description = "Amount of points to reward if the phase changes to Full.")]
    full: Option<i32>,
    other: Vec<OtherBounty>,
}

impl MonsterBounty {
    pub fn new(modreward: bool, bounty_spec: &HashMap<i64, i64>) -> Self {
        Self {
            modreward,
            incomplete: bounty_spec
                .get(&(Phase::Incomplete as i64))
                .map(|x| *x as i32),
            exists: bounty_spec.get(&(Phase::Exists as i64)).map(|x| *x as i32),
            full: bounty_spec.get(&(Phase::Full as i64)).map(|x| *x as i32),
            other: bounty_spec
                .iter()
                .filter(|(&k, _)| {
                    k != (Phase::Incomplete as i64)
                        && k != (Phase::Exists as i64)
                        && k != (Phase::Full as i64)
                })
                .map(|(k, v)| OtherBounty {
                    phase: *k as i32,
                    bounty: *v as i32,
                })
                .collect(),
        }
    }
}

// TODO: Once async works better with references in Juniper, switch back to this:
//pub struct MonsterFormPortraits<'a>(&'a Group, i32, &'a [i32]);
pub struct MonsterFormPortraits(Arc<Group>, i32, Vec<i32>);

#[graphql_object(Context = Context)]
#[graphql(description = "Portraits for a single monster form.")]
impl MonsterFormPortraits {
    #[graphql(description = "Whether or not this form should have portraits.")]
    fn required(&self) -> bool {
        self.0.portrait_required
    }

    #[graphql(description = "Guild Point bounty for this portrait set.")]
    fn bounty(&self) -> MonsterBounty {
        MonsterBounty::new(self.0.modreward, &self.0.portrait_bounty)
    }

    #[graphql(description = "Current completion phase of the portraits.")]
    fn phase(&self) -> Phase {
        Phase::from(self.0.portrait_complete)
    }

    #[graphql(description = "Current completion phase of the portraits (raw ID).")]
    fn phase_raw(&self) -> i32 {
        self.0.portrait_complete as i32
    }

    #[graphql(description = "Primary artist credits.")]
    fn credit_primary(&self, context: &Context) -> FieldResult<Option<Credit>> {
        let credit_id = parse_credit_id(&self.0.portrait_credit.primary);
        if credit_id.is_empty() {
            Ok(None)
        } else {
            Credit::new(
                context.collab.data().credit_names.get(&credit_id),
                &credit_id,
            )
            .map(Some)
        }
    }

    #[graphql(description = "All other artists credited.")]
    fn credit_secondary(&self, context: &Context) -> FieldResult<Vec<Credit>> {
        let names = &context.collab.data().credit_names;
        self.0
            .portrait_credit
            .secondary
            .iter()
            .map(parse_credit_id)
            .map(|v| Credit::new(names.get(&v), &v))
            .collect()
    }

    #[graphql(description = "URL to a SpriteBot format sheet of all portraits.")]
    fn sheet_url(&self, context: &Context) -> String {
        get_url(
            AssetType::PortraitSheet,
            &context.this_server_url,
            self.1,
            &self.2,
        )
    }

    #[graphql(description = "URL to a SpriteBot format recolor sheet.")]
    fn recolor_sheet_url(&self, context: &Context) -> String {
        get_url(
            AssetType::PortraitRecolorSheet,
            &context.this_server_url,
            self.1,
            &self.2,
        )
    }

    #[graphql(description = "A list of all existing portraits for the emotions.")]
    async fn emotions(&self, context: &Context) -> FieldResult<Vec<Portrait>> {
        Ok(
            iter_existing_portrait_files(&context, &self.0.portrait_files, false, self.1, &self.2)
                .await?
                .into_iter()
                .map(|(emotion, locked)| Portrait {
                    emotion: emotion.clone(),
                    locked,
                    url: get_url(
                        AssetType::Portrait(&emotion),
                        &context.this_server_url,
                        self.1,
                        &self.2,
                    ),
                })
                .collect(),
        )
    }

    #[graphql(description = "A single portrait for a given emotion.")]
    async fn emotion(&self, context: &Context, emotion: String) -> FieldResult<Option<Portrait>> {
        Ok(get_existing_portrait_file(
            &context,
            &self.0.portrait_files,
            &emotion,
            false,
            self.1,
            &self.2,
        )
        .await?
        .map(|locked| Portrait {
            emotion: emotion.clone(),
            locked,
            url: get_url(
                AssetType::Portrait(&emotion),
                &context.this_server_url,
                self.1,
                &self.2,
            ),
        }))
    }

    #[graphql(
        description = "A single portrait. Return the 'Normal' portrait if avalaible, but may return another one if not present."
    )]
    fn preview_emotion(&self, context: &Context) -> Option<Portrait> {
        if let Some(locked) = self.0.portrait_files.get("Normal") {
            Some(Portrait {
                emotion: "Normal".to_string(),
                locked: *locked,
                url: get_url(
                    AssetType::Portrait("Normal"),
                    &context.this_server_url,
                    self.1,
                    &self.2,
                ),
            })
        } else {
            self.0
                .portrait_files
                .iter()
                .sorted()
                .next()
                .map(|(emotion, locked)| Portrait {
                    emotion: emotion.clone(),
                    locked: *locked,
                    url: get_url(
                        AssetType::Portrait(emotion),
                        &context.this_server_url,
                        self.1,
                        &self.2,
                    ),
                })
        }
    }

    #[graphql(description = "A list of all existing flipped portraits for the emotions.")]
    async fn emotions_flipped(&self, context: &Context) -> FieldResult<Vec<Portrait>> {
        Ok(
            iter_existing_portrait_files(&context, &self.0.portrait_files, true, self.1, &self.2)
                .await?
                .into_iter()
                .map(|(emotion, locked)| Portrait {
                    emotion: emotion.clone(),
                    locked,
                    url: get_url(
                        AssetType::PortraitFlipped(&emotion),
                        &context.this_server_url,
                        self.1,
                        &self.2,
                    ),
                })
                .collect(),
        )
    }

    #[graphql(description = "A single flipped portrait for a given emotion.")]
    async fn emotion_flipped(
        &self,
        context: &Context,
        emotion: String,
    ) -> FieldResult<Option<Portrait>> {
        Ok(get_existing_portrait_file(
            &context,
            &self.0.portrait_files,
            &emotion,
            true,
            self.1,
            &self.2,
        )
        .await?
        .map(|locked| Portrait {
            emotion: emotion.clone(),
            locked,
            url: get_url(
                AssetType::PortraitFlipped(&emotion),
                &context.this_server_url,
                self.1,
                &self.2,
            ),
        }))
    }

    #[graphql(description = "The date and time this portrait set was last updated.")]
    fn modified_date(&self) -> Option<DateTime<Utc>> {
        self.0.portrait_modified
    }
}

// TODO: Once async works better with references in Juniper, switch back to this:
//pub struct MonsterFormSprites<'a>(&'a Group, i32, &'a [i32]);
pub struct MonsterFormSprites(Arc<Group>, i32, Vec<i32>);

impl MonsterFormSprites {
    fn process_sprite_action(
        &self,
        action: &str,
        locked: bool,
        action_copy_map: &HashMap<String, String>,
        this_server_url: &str,
    ) -> SpriteUnion {
        match action_copy_map.get(action) {
            Some(copy_of) => SpriteUnion::CopyOf(CopyOf {
                action: action.to_string(),
                locked,
                copy_of: copy_of.to_string(),
            }),
            None => SpriteUnion::Sprite(Sprite {
                anim_url: get_url(
                    AssetType::SpriteAnim(action),
                    this_server_url,
                    self.1,
                    &self.2,
                ),
                offsets_url: get_url(
                    AssetType::SpriteOffsets(action),
                    this_server_url,
                    self.1,
                    &self.2,
                ),
                shadows_url: get_url(
                    AssetType::SpriteShadows(action),
                    this_server_url,
                    self.1,
                    &self.2,
                ),
                action: action.to_string(),
                locked,
            }),
        }
    }

    async fn fetch_xml_and_make_action_map(
        monster_idx: i32,
        path_to_form: &[i32],
    ) -> FieldResult<CacheBehaviour<HashMap<String, String>>> {
        let xml = AnimDataXml::open_for_form(monster_idx, path_to_form)
            .map_err(Self::failed_xml_fetch)?;
        Ok(CacheBehaviour::Cache(xml.get_action_copies()))
    }

    fn failed_xml_fetch<E: Debug>(e: E) -> FieldError {
        let e_as_str = format!("{:?}", e);
        FieldError::new(
            "Internal Server Error: Failed processing the animation data from the AnimData.xml."
                .to_string(),
            graphql_value!({ "details": e_as_str }),
        )
    }

    /// XXX: This isn't ideal, but Juniper is a bit silly about it's Sync requirements, so there's
    /// currently no way to do this truly async as far as I can tell.
    async fn get_action_map(&self, context: &Context) -> FieldResult<HashMap<String, String>> {
        context
            .cached_may_fail_chain(format!("/monster_actions|{}/{:?}", self.1, self.2), || {
                Self::fetch_xml_and_make_action_map(self.1, &self.2)
            })
            .await
    }
}

#[graphql_object(Context = Context)]
#[graphql(description = "Sprites for a single monster form.")]
impl MonsterFormSprites {
    #[graphql(description = "Whether or not this form should have sprites.")]
    fn required(&self) -> bool {
        self.0.sprite_required
    }

    #[graphql(description = "Guild Point bounty for this sprite set.")]
    fn bounty(&self) -> MonsterBounty {
        MonsterBounty::new(self.0.modreward, &self.0.sprite_bounty)
    }

    #[graphql(description = "Current completion phase of the sprites.")]
    fn phase(&self) -> Phase {
        Phase::from(self.0.sprite_complete)
    }

    #[graphql(description = "Current completion phase of the sprites (raw ID).")]
    fn phase_raw(&self) -> i32 {
        self.0.sprite_complete as i32
    }

    #[graphql(description = "Primary artist credits.")]
    fn credit_primary(&self, context: &Context) -> FieldResult<Option<Credit>> {
        let credit_id = parse_credit_id(&self.0.sprite_credit.primary);
        if credit_id.is_empty() {
            Ok(None)
        } else {
            Credit::new(
                context.collab.data().credit_names.get(&credit_id),
                &credit_id,
            )
            .map(Some)
        }
    }

    #[graphql(description = "All other artists credited.")]
    fn credit_secondary(&self, context: &Context) -> FieldResult<Vec<Credit>> {
        let names = &context.collab.data().credit_names;
        self.0
            .sprite_credit
            .secondary
            .iter()
            .map(parse_credit_id)
            .map(|v| Credit::new(names.get(&v), &v))
            .collect()
    }

    #[graphql(description = "URL to the AnimData XML file for this sprite set.")]
    fn anim_data_xml(&self, context: &Context) -> Option<String> {
        if self.0.sprite_complete == Phase::Incomplete as i64 {
            return None;
        }
        Some(get_url(
            AssetType::SpriteAnimDataXml,
            &context.this_server_url,
            self.1,
            &self.2,
        ))
    }

    #[graphql(description = "URL to a SpriteBot format ZIP archive of all sprites.")]
    fn zip_url(&self, context: &Context) -> Option<String> {
        if self.0.sprite_complete == Phase::Incomplete as i64 {
            return None;
        }
        Some(get_url(
            AssetType::SpriteZip,
            &context.this_server_url,
            self.1,
            &self.2,
        ))
    }

    #[graphql(description = "URL to a SpriteBot format recolor sheet.")]
    fn recolor_sheet_url(&self, context: &Context) -> Option<String> {
        if self.0.sprite_complete == Phase::Incomplete as i64 {
            return None;
        }
        Some(get_url(
            AssetType::SpriteRecolorSheet,
            &context.this_server_url,
            self.1,
            &self.2,
        ))
    }

    #[graphql(description = "A list of all existing sprites for the actions.")]
    async fn actions(&self, context: &Context) -> FieldResult<Vec<SpriteUnion>> {
        if self.0.sprite_complete == Phase::Incomplete as i64 {
            return Ok(vec![]);
        }
        let action_copy_map = self.get_action_map(context).await?;
        Ok(
            iter_existing_sprite_files(&context, &self.0.sprite_files, self.1, &self.2)
                .await?
                .into_iter()
                .map(|(action, locked)| {
                    self.process_sprite_action(
                        &action,
                        locked,
                        &action_copy_map,
                        &context.this_server_url,
                    )
                })
                .collect(),
        )
    }

    #[graphql(description = "A single sprite for a given action.")]
    async fn action(&self, context: &Context, action: String) -> FieldResult<Option<SpriteUnion>> {
        if self.0.sprite_complete == Phase::Incomplete as i64 {
            return Ok(None);
        }
        let action_copy_map = self.get_action_map(context).await?;
        Ok(
            get_existing_sprite_file(&context, &self.0.sprite_files, &action, self.1, &self.2)
                .await?
                .map(|locked| {
                    self.process_sprite_action(
                        &action,
                        locked,
                        &action_copy_map,
                        &context.this_server_url,
                    )
                }),
        )
    }

    #[graphql(description = "The date and time this sprite set was last updated.")]
    fn modified_date(&self) -> Option<DateTime<Utc>> {
        self.0.sprite_modified
    }
}

pub struct MonsterForm {
    id: i32,
    form_id: Vec<i32>,
    name_path: Vec<String>,
    data: Arc<Group>,
}

#[graphql_object(Context = Context)]
impl MonsterForm {
    #[graphql(description = "The ID of the monster, that this form belongs to.")]
    fn monster_id(&self) -> i32 {
        self.id
    }

    #[graphql(
        description = "The path to this form (without the monster ID) as it's specified in the SpriteCollab tracker.json file and repository file structure."
    )]
    fn path(&self) -> String {
        let mut path = self.form_id.iter().map(|v| format!("{:04}", v)).join("/");
        if path.ends_with('/') {
            path.truncate(path.len() - 1);
        }
        path
    }

    #[graphql(
        description = "The path to this form (including the monster ID) as it's specified in the SpriteCollab tracker.json file and repository file structure."
    )]
    fn full_path(&self) -> String {
        let mut path = once(format!("{:04}", self.id))
            .chain(self.form_id.iter().map(|v| format!("{:04}", v)))
            .join("/");
        if path.ends_with('/') {
            path.truncate(path.len() - 1);
        }
        path
    }

    #[graphql(description = "Human-readable name of this form.")]
    fn name(&self) -> String {
        self.data.name.clone()
    }

    #[graphql(
        description = "Human-readable full name of this form (excluding the monster name itself)."
    )]
    fn full_name(&self) -> String {
        self.name_path.iter().cloned().join(" ")
    }

    #[graphql(description = "Whether or not this form is considered for a shiny.")]
    fn is_shiny(&self) -> bool {
        MonsterFormCollector::is_shiny(&self.form_id)
    }

    #[graphql(description = "Whether or not this form is considered for a female monsters.")]
    fn is_female(&self) -> bool {
        MonsterFormCollector::is_female(&self.form_id)
    }

    #[graphql(description = "Whether or not this form is canon.")]
    fn canon(&self) -> bool {
        self.data.canon
    }

    #[graphql(description = "Portraits for this form.")]
    fn portraits(&self) -> MonsterFormPortraits {
        MonsterFormPortraits(self.data.clone(), self.id, self.form_id.clone())
    }

    #[graphql(description = "Sprites for this form.")]
    fn sprites(&self) -> MonsterFormSprites {
        MonsterFormSprites(self.data.clone(), self.id, self.form_id.clone())
    }
}

#[derive(Deserialize, Serialize)]
pub struct Monster {
    id: i32,
}

fn monster_not_found(id: i32) -> FieldError {
    FieldError::new("Monster not found", graphql_value!({ "id": id }))
}

#[graphql_object(Context = Context)]
impl Monster {
    #[graphql(description = "ID of this monster.")]
    async fn id(&self) -> FieldResult<i32> {
        Ok(self.id)
    }

    #[graphql(
        description = "Raw ID of this monster, as a string. This is a 4-character numeric string, padded with leading zeroes."
    )]
    async fn raw_id(&self) -> FieldResult<String> {
        Ok(format!("{:04}", self.id))
    }

    #[graphql(description = "Human-readable name of this monster.")]
    fn name(&self, context: &Context) -> FieldResult<String> {
        context
            .collab
            .data()
            .tracker
            .get(&(self.id as i64))
            .ok_or_else(|| monster_not_found(self.id))
            .map(|monster| monster.name.clone())
    }

    #[graphql(description = "All forms that exist for this monster.")]
    fn forms(&self, context: &Context) -> FieldResult<Vec<MonsterForm>> {
        match MonsterFormCollector::collect(&context.collab.data().tracker, self.id) {
            Some(collector) => Ok(collector
                .map(|(k, name_path, v)| MonsterForm {
                    id: self.id,
                    form_id: k,
                    name_path,
                    data: Arc::new(v.clone()),
                })
                .collect()),
            None => Err(FieldError::new(
                "Monster not found",
                graphql_value!({ "id": (self.id) }),
            )),
        }
    }

    #[graphql(description = "Get a specific form for this monster.")]
    fn get(
        &self,
        context: &Context,
        form_id: i32,
        shiny: bool,
        female: bool,
    ) -> FieldResult<Option<MonsterForm>> {
        // <poke id>/<form index>/<shiny? - yes: 0001, no: 0000>/<female? - yes: 0002, no: 0001>
        match MonsterFormCollector::collect(&context.collab.data().tracker, self.id) {
            Some(collector) => Ok(collector
                .find_form([
                    FormMatch::Exact(form_id),
                    FormMatch::Exact(if shiny { 1 } else { 0 }),
                    if female {
                        FormMatch::Exact(2)
                    } else {
                        FormMatch::Fallback(1)
                    },
                ])
                .map(|(path, name_path, v)| MonsterForm {
                    id: self.id,
                    form_id: path,
                    name_path,
                    data: Arc::new(v.clone()),
                })),
            None => Err(FieldError::new(
                "Monster not found",
                graphql_value!({ "id": (self.id) }),
            )),
        }
    }

    #[graphql(
        description = "Manually enter the path to a monster, seperated by /. This should match the path as it is stored in SpriteCollab, however the path passed in might be collapsed until a unique form is found."
    )]
    fn manual(&self, context: &Context, path: String) -> FieldResult<Option<MonsterForm>> {
        let form_needle: Result<Vec<i32>, _> = path
            .split('/')
            .filter(|v| !v.is_empty())
            .map(|v| v.parse::<i32>())
            .collect();
        match form_needle {
            Ok(form_needle) => {
                match MonsterFormCollector::collect(&context.collab.data().tracker, self.id) {
                    Some(collector) => Ok(collector
                        .find_form(form_needle.into_iter().map(FormMatch::Exact))
                        .map(|(path, name_path, v)| MonsterForm {
                            id: self.id,
                            form_id: path,
                            name_path,
                            data: Arc::new(v.clone()),
                        })),
                    None => Err(FieldError::new(
                        "Monster not found",
                        graphql_value!({ "id": (self.id) }),
                    )),
                }
            }
            Err(e) => {
                let e_dbg = format!("{:?}", e);
                Err(FieldError::new(
                    "Invalid path.",
                    graphql_value!({ "details": e_dbg }),
                ))
            }
        }
    }
}

#[derive(GraphQLObject)]
#[graphql(description = "An action mapped uniquely to an ID.")]
pub struct ActionId {
    id: i32,
    name: String,
}

#[derive(GraphQLObject)]
#[graphql(description = "Configuration for this instance of SpriteCollab.")]
pub struct Config {
    #[graphql(description = "The portrait width and height in pixels.")]
    portrait_size: i32,
    #[graphql(description = "How many portraits per row a portrait sheet contains.")]
    portrait_tile_x: i32,
    #[graphql(description = "How many rows a portrait sheet contains.")]
    portrait_tile_y: i32,
    #[graphql(description = "A list of known emotions. The position is the ID of the emotion.")]
    emotions: Vec<String>,
    #[graphql(description = "A list of known action. The position is the ID of the action.")]
    actions: Vec<String>,
    #[graphql(
        description = "Returns a list, that for each phase contains a list of emotions (by index) that need to exist for this phase to be considered completed."
    )]
    completion_emotions: Vec<Vec<i32>>,
    #[graphql(
        description = "Returns a list, that for each phase contains a list of actions (by index) that need to exist for this phase to be considered completed."
    )]
    completion_actions: Vec<Vec<i32>>,
    #[graphql(description = "A mapping of actions to EoS action indices.")]
    action_map: Vec<ActionId>,
}

impl From<&SpriteConfig> for Config {
    fn from(c: &SpriteConfig) -> Self {
        Self {
            portrait_size: c.portrait_size,
            portrait_tile_x: c.portrait_tile_x,
            portrait_tile_y: c.portrait_tile_y,
            emotions: c.emotions.clone(),
            actions: c.actions.clone(),
            completion_emotions: c.completion_emotions.clone(),
            completion_actions: c.completion_actions.clone(),
            action_map: c
                .action_map
                .iter()
                .map(|(idx, act)| ActionId {
                    id: *idx,
                    name: act.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct Credit {
    id: String,
    name: Option<String>,
    contact: Option<String>,
}

#[graphql_object(Context = Context)]
impl Credit {
    #[graphql(description = "Discord ID or absentee ID. Guaranteed to be an ASCII string.")]
    fn id(&self) -> String {
        self.id.clone()
    }

    #[graphql(
        description = "The human-readable name of the author. Guaranteed to be an ASCII string."
    )]
    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    #[graphql(description = "Contact information for this author.")]
    fn contact(&self) -> Option<String> {
        self.contact.clone()
    }

    #[graphql(
        description = "Discord username or old-style name and discriminator (in the form Name#Discriminator [eg. Capypara#7887)), if this is a credit for a Discord profile, and the server can resolve the ID to a Discord profile."
    )]
    async fn discord_handle(&self, context: &Context) -> FieldResult<Option<String>> {
        #[cfg(feature = "discord")]
        {
            if let Some(discord) = &context.discord {
                context
                    .cached_may_fail_chain(format!("discord_user|{}", self.id), || async {
                        let id = self.id.parse().ok();
                        if id.is_none() {
                            return Ok(CacheBehaviour::NoCache(None));
                        }
                        let id = id.unwrap();
                        let response = tokio::time::timeout(
                            // We don't wait here for long. If we can't get it that quick,
                            // it's not worth it.
                            std::time::Duration::from_millis(500),
                            discord.get_user(id),
                        )
                        .await;
                        match response {
                            Err(_) => Ok(CacheBehaviour::NoCache(None)),
                            Ok(Ok(profile)) => {
                                Ok(CacheBehaviour::Cache(profile.map(|user| {
                                    // If the API reports a discriminator of "0", then this is a new-style username.
                                    // XXX: Should probably update Discord API crate and use whatever mechanism they provide.
                                    if &user.discriminator == "0" {
                                        user.name.clone()
                                    } else {
                                        format!("{}#{}", user.name, user.discriminator)
                                    }
                                })))
                            }
                            Ok(Err(e)) => Err(FieldError::new(
                                "Internal Server Error trying to resolve Discord ID",
                                graphql_value!({
                                    "details": (e.to_string())
                                }),
                            )),
                        }
                    })
                    .await
            } else {
                Ok(None)
            }
        }
        #[cfg(not(feature = "discord"))]
        {
            Ok(None)
        }
    }

    #[graphql(
        description = "This may return some sort of point value for reputation for this user, if applicable and the source to get these points from is returning them properly. The value may be cached/stale for a while."
    )]
    async fn reputation(&self, context: &Context) -> FieldResult<Option<i32>> {
        #[cfg(feature = "discord-reputation")]
        {
            // We only keep the cache valid for this time, by using new keys
            // every time wer pass this threshold. This will leak cache entries, but that's
            // probably ok.
            const REPUTATION_CACHE_TIMEOUT: i64 = 10800000;
            let timekey = Utc::now().timestamp_millis() % REPUTATION_CACHE_TIMEOUT;
            context
                .cached_may_fail_chain(format!("discord_reputation|{}", timekey), || async {
                    tokio::time::timeout(
                        // We don't wait here for long. If we can't get it that quick,
                        // it's not worth it.
                        std::time::Duration::from_millis(20000),
                        crate::discord_reputation::fetch_reputation(
                            &SystemConfig::DiscordReputationFetchUrl
                                .get_or_none()
                                .ok_or_else(|| {
                                    FieldError::new(
                                        "Internal Server Error: Misconfigured reputation endpoint.",
                                        graphql_value!(None),
                                    )
                                })?,
                        ),
                    )
                    .await
                    .map_err(|_| {
                        FieldError::new(
                            "Internal Server Error: Timeout tring to fetch reputation.",
                            graphql_value!(None),
                        )
                    })
                    .and_then(|success_cache| {
                        success_cache
                            .map_err(|e| {
                                FieldError::new(
                                    "Internal Server Error: Failed fetching the map of reputation.",
                                    graphql_value!({
                                        "details": (e.to_string())
                                    }),
                                )
                            })
                            .map(CacheBehaviour::Cache)
                    })
                })
                .await
                .map(|reputation_list| reputation_list.get(&self.id).copied())
        }
        #[cfg(not(feature = "discord-reputation"))]
        {
            Ok(None)
        }
    }
}

impl Credit {
    fn new(credit_entry: Option<&CreditNamesRow>, credit_id: &str) -> FieldResult<Credit> {
        credit_entry
            .map(|v| Self {
                id: v.credit_id.clone(),
                name: v.name.as_ref().cloned(),
                contact: v.contact.as_ref().cloned(),
            })
            .ok_or_else(|| {
                FieldError::new(
                    "Internal error. Could not resolved credit ID.",
                    graphql_value!({ "credit_id": (credit_id) }),
                )
            })
    }
}

impl From<&CreditNamesRow> for Credit {
    fn from(c: &CreditNamesRow) -> Self {
        Self {
            id: c.credit_id.clone(),
            name: c.name.clone(),
            contact: c.contact.clone(),
        }
    }
}

pub struct Context {
    this_server_url: String,
    collab: Arc<SpriteCollab>,
    #[allow(dead_code)] // potentially for future use.
    reporting: Arc<Reporting>,
    #[cfg(feature = "discord")]
    discord: Option<Arc<crate::reporting::DiscordBot>>,
}

impl Context {
    pub fn new(collab: Arc<SpriteCollab>, reporting: Arc<Reporting>) -> Self {
        #[cfg(feature = "discord")]
        let discord = reporting.discord_bot.clone();
        Context {
            this_server_url: SystemConfig::Address.get_or_none().unwrap_or_default(),
            collab,
            reporting,
            #[cfg(feature = "discord")]
            discord,
        }
    }
}

#[async_trait]
impl ScCache for Context {
    type Error = FieldError;

    async fn cached_may_fail<S, Fn, Ft, T, E>(
        &self,
        cache_key: S,
        func: Fn,
    ) -> FieldResult<Result<T, E>>
    where
        S: AsRef<str> + Into<RedisKey> + Send + Sync,
        Fn: (FnOnce() -> Ft) + Send,
        Ft: Future<Output = Result<CacheBehaviour<T>, E>> + Send,
        T: DeserializeOwned + Serialize + Send + Sync,
        E: Send,
    {
        self.collab
            .cached_may_fail(cache_key, func)
            .await
            .map_err(|_e| {
                FieldError::new(
                    "Internal lookup error.",
                    graphql_value!({ "reason": "redis lookup failed. try again." }),
                )
            })
    }
}

pub struct Meta;

#[graphql_object(Context = Context)]
impl Meta {
    #[graphql(description = "Version of this API.")]
    fn api_version(_context: &Context) -> &str {
        API_VERSION
    }

    #[graphql(description = "Version of spritecollab-srv serving this API.")]
    fn server_version(_context: &Context) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    #[graphql(
        description = "Git Commit (https://github.com/PMDCollab/SpriteCollab/) currently checked out to serve the assets."
    )]
    async fn assets_commit(context: &Context) -> FieldResult<String> {
        context
            .collab
            .with_meta(|meta| {
                meta.map_err(|_| {
                    FieldError::new(
                        "Internal error while trying to load meta data.",
                        graphql_value!(None),
                    )
                })
                .map(|v| v.assets_commit.clone())
            })
            .await
    }

    #[graphql(
        description = "Date of the last commit in the assets repository (https://github.com/PMDCollab/SpriteCollab) that is currently checked out."
    )]
    async fn assets_update_date(context: &Context) -> FieldResult<DateTime<Utc>> {
        context
            .collab
            .with_meta(|meta| {
                meta.map_err(|_| {
                    FieldError::new(
                        "Internal error while trying to load meta data.",
                        graphql_value!(None),
                    )
                })
                .map(|v| v.assets_update_date)
            })
            .await
    }

    #[graphql(description = "Date that the server last checked for updates.")]
    async fn update_checked_date(context: &Context) -> FieldResult<DateTime<Utc>> {
        context
            .collab
            .with_meta(|meta| {
                meta.map_err(|_| {
                    FieldError::new(
                        "Internal error while trying to load meta data.",
                        graphql_value!(None),
                    )
                })
                .map(|v| v.update_checked_date)
            })
            .await
    }
}

// To make our context usable by Juniper, we have to implement a marker trait.
impl juniper::Context for Context {}

pub struct Query;

#[graphql_object(Context = Context)]
impl Query {
    #[graphql(
        description = "Version of this API.",
        deprecated = "Use `meta` instead."
    )]
    fn api_version(_context: &Context) -> &str {
        API_VERSION
    }

    #[graphql(description = "Meta information about the server and state of the assets.")]
    fn meta(_context: &Context) -> Meta {
        Meta
    }

    #[graphql(
        description = "Search for a monster by (parts) of its name. Results are sorted by best match."
    )]
    async fn search_monster(context: &Context, monster_name: String) -> FieldResult<Vec<Monster>> {
        if monster_name.len() > MAX_QUERY_LEN {
            Err(FieldError::new(
                "Search query too long",
                graphql_value!({ "max_length": (MAX_QUERY_LEN as i32) }),
            ))
        } else {
            let tracker = context.collab.data().tracker.clone();
            context
                .cached_may_fail_chain(format!("/search_monster|{}", &monster_name), || async {
                    let r: FieldResult<Vec<Monster>> =
                        fuzzy_find_tracker(&tracker, &monster_name, context, |idx| Monster {
                            id: idx as i32,
                        })
                        .await;
                    match r {
                        Ok(v) if !v.is_empty() => Ok(CacheBehaviour::Cache(v)),
                        Ok(v) => Ok(CacheBehaviour::NoCache(v)),
                        Err(e) => Err(e),
                    }
                })
                .await
        }
    }

    #[graphql(
        description = "Retrieve a list of monsters.",
        arguments(filter(description = "Monster IDs to limit the request to.",))
    )]
    fn monster(context: &Context, filter: Option<Vec<i32>>) -> FieldResult<Vec<Monster>> {
        Ok(context
            .collab
            .data()
            .tracker
            .keys()
            .filter(|v| {
                if let Some(filter) = &filter {
                    filter.contains(&(**v as i32))
                } else {
                    true
                }
            })
            .map(|idx| Monster { id: *idx as i32 })
            .collect())
    }

    #[graphql(
        description = "Search for a credit entry by (parts) of the ID, the author name or the contact info. Results are sorted by best match."
    )]
    async fn search_credit(context: &Context, query: String) -> FieldResult<Vec<Credit>> {
        if query.len() > MAX_QUERY_LEN {
            Err(FieldError::new(
                "Search query too long",
                graphql_value!({ "max_length": (MAX_QUERY_LEN as i32) }),
            ))
        } else {
            context
                .cached(format!("/search_credit|{}", &query), || async {
                    let r: Vec<Credit> = context
                        .collab
                        .data()
                        .credit_names
                        .fuzzy_find(&query)
                        .map(Credit::from)
                        .collect();
                    if !r.is_empty() {
                        CacheBehaviour::Cache(r)
                    } else {
                        CacheBehaviour::NoCache(r)
                    }
                })
                .await
        }
    }

    #[graphql(
        description = "Retrieve a list of credits.",
        arguments(filter(
            description = "Credit IDs (Discord ID or absentee ID) to limit the request to.",
        ))
    )]
    fn credit(context: &Context) -> FieldResult<Vec<Credit>> {
        Ok(context
            .collab
            .data()
            .credit_names
            .iter()
            .map(Credit::from)
            .collect())
    }

    #[graphql(description = "Configuration for this instance of SpriteCollab.")]
    fn config(context: &Context) -> FieldResult<Config> {
        Ok(Config::from(&context.collab.data().sprite_config))
    }
}
