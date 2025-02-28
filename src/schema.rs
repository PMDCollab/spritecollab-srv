use std::collections::HashMap;
use std::env;
use std::fmt::Debug;
use std::future::Future;
use std::iter::once;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fred::types::Key;
use itertools::Itertools;
use juniper::{
    FieldError, FieldResult, GraphQLEnum, GraphQLObject, GraphQLUnion, graphql_object,
    graphql_value,
};
#[allow(unused_imports)]
use log::warn;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::assets::fs_check::{
    AssetCategory, get_existing_portrait_file, get_existing_sprite_file, get_local_credits_file,
    iter_existing_portrait_files, iter_existing_sprite_files,
};
use crate::assets::url::{AssetType, get_url};
use crate::cache::{CacheBehaviour, ScCache};
use crate::config::Config as SystemConfig;
use crate::datafiles::anim_data_xml::AnimDataXml;
use crate::datafiles::credit_names::CreditNamesRow;
use crate::datafiles::group_id::GroupId;
use crate::datafiles::local_credits_file::LocalCreditRow;
use crate::datafiles::parse_credit_id;
use crate::datafiles::sprite_config::SpriteConfig;
use crate::datafiles::tracker::{
    FormMatch, Group, MapImpl, MonsterFormCollector, fuzzy_find_tracker,
};
use crate::sprite_collab::SpriteCollab;

/// Maximum length for search query strings
const MAX_QUERY_LEN: usize = 75;
const API_VERSION: &str = "1.6";

#[derive(GraphQLEnum)]
#[graphql(description = "A known license from a common list of options.")]
pub enum KnownLicenseType {
    #[graphql(description = "The license could not be determined.")]
    Unknown,
    #[graphql(description = "The license is not specified / the work is unlicensed.")]
    Unspecified,
    #[graphql(description = "Original license: When using, you must credit the contributors.")]
    PMDCollab1,
    #[graphql(
        description = "License for works between May 2023 - March 2024: You are free to use, copy redistribute or modify sprites and portraits from this repository for your own projects and contributions. When using portraits or sprites from this repository, you must credit the contributors for each portrait and sprite you use."
    )]
    PMDCollab2,
    #[graphql(
        description = "Licensed under Creative Commons Attribution-NonCommercial 4.0 International"
    )]
    CcByNc4,
}

#[derive(GraphQLObject)]
#[graphql(description = "A known license from a common list of options.")]
pub struct KnownLicense {
    license: KnownLicenseType,
}

#[derive(GraphQLObject)]
#[graphql(description = "An unknown license. The name is the identifier for the license.")]
pub struct OtherLicense {
    name: String,
}

#[derive(GraphQLUnion)]
#[graphql(
    description = "The license that applies to the image of a sprite action or portrait emotion."
)]
pub enum License {
    KnownLicense(KnownLicense),
    Other(OtherLicense),
}

impl From<String> for License {
    fn from(value: String) -> Self {
        match &*value {
            "Unknown" => License::KnownLicense(KnownLicense {
                license: KnownLicenseType::Unknown,
            }),
            "Unspecified" => License::KnownLicense(KnownLicense {
                license: KnownLicenseType::Unspecified,
            }),
            "PMDCollab_1" => License::KnownLicense(KnownLicense {
                license: KnownLicenseType::PMDCollab1,
            }),
            "PMDCollab_2" => License::KnownLicense(KnownLicense {
                license: KnownLicenseType::PMDCollab2,
            }),
            "CC_BY-NC_4" => License::KnownLicense(KnownLicense {
                license: KnownLicenseType::CcByNc4,
            }),
            _ => License::Other(OtherLicense { name: value }),
        }
    }
}

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

pub struct MonsterHistory {
    credit: Option<Credit>,
    modified_date: DateTime<Utc>,
    modifications: Vec<String>,
    obsolete: bool,
    license: License,
}

impl MonsterHistory {
    fn try_from_credit_row(context: &Context, value: LocalCreditRow) -> Result<Self, FieldError> {
        let credit_id = parse_credit_id(value.credit_id);
        let credit = if credit_id.is_empty() {
            None
        } else {
            Some(Credit::new(
                context.collab.data().credit_names.get(&credit_id),
                &credit_id,
            )?)
        };
        Ok(Self {
            credit,
            modified_date: value.date,
            modifications: value.items,
            obsolete: value.obsolete,
            license: value.license.into(),
        })
    }
}

#[graphql_object(Context = Context)]
#[graphql(description = "An entry in the history log for a monster sprite or portrait.")]
impl MonsterHistory {
    #[graphql(description = "The author that contributed for this history entry.")]
    pub fn credit(&self) -> Option<&Credit> {
        self.credit.as_ref()
    }

    #[graphql(description = "The date of the history entry submission.")]
    pub fn modified_date(&self) -> DateTime<Utc> {
        self.modified_date
    }

    #[graphql(
        description = "A list of emotions or actions that were changed in this history entry."
    )]
    pub fn modifications(&self) -> &[String] {
        &self.modifications
    }

    #[graphql(
        description = "True if the credit for this history entry was marked as no longer relevant for the current portraits or sprites."
    )]
    pub fn obsolete(&self) -> bool {
        self.obsolete
    }

    #[graphql(description = "The license applying to this modification.")]
    pub fn license(&self) -> &License {
        &self.license
    }
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
    pub fn new(modreward: bool, bounty_spec: &MapImpl<i64, i64>) -> Self {
        Self {
            modreward,
            incomplete: bounty_spec
                .get(&(Phase::Incomplete as i64))
                .map(|x| *x as i32),
            exists: bounty_spec.get(&(Phase::Exists as i64)).map(|x| *x as i32),
            full: bounty_spec.get(&(Phase::Full as i64)).map(|x| *x as i32),
            other: bounty_spec
                .iter()
                .filter(|&(&k, _)| {
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

    #[graphql(
        description = "List of all modifications made to those portraits since its creation."
    )]
    async fn history(&self, context: &Context) -> FieldResult<Vec<MonsterHistory>> {
        get_local_credits_file(&context, AssetCategory::Portrait, self.1, &self.2)
            .await??
            .into_iter()
            .map(|i| MonsterHistory::try_from_credit_row(context, i))
            .collect::<Result<Vec<_>, _>>()
    }

    #[graphql(
        description = "Returns a URL to retrieve the credits text file for the portraits for this form."
    )]
    fn history_url(&self, context: &Context) -> Option<String> {
        Some(get_url(
            AssetType::PortraitCreditsTxt,
            &context.this_server_url,
            self.1,
            &self.2,
        ))
    }
}

// TODO: Once async works better with references in Juniper, switch back to this:
//pub struct MonsterFormSprites<'a>(&'a Group, i32, &'a [i32]);
pub struct MonsterFormSprites(Arc<Group>, i32, Vec<i32>);

impl MonsterFormSprites {
    fn process_sprite_action(&self, action: &str, locked: bool, this_server_url: &str) -> Sprite {
        Sprite {
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

    #[inline]
    fn sprites_available(&self) -> bool {
        !self.0.sprite_files.is_empty()
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
        if self.sprites_available() {
            Some(get_url(
                AssetType::SpriteAnimDataXml,
                &context.this_server_url,
                self.1,
                &self.2,
            ))
        } else {
            None
        }
    }

    #[graphql(description = "URL to a SpriteBot format ZIP archive of all sprites.")]
    fn zip_url(&self, context: &Context) -> Option<String> {
        if self.sprites_available() {
            Some(get_url(
                AssetType::SpriteZip,
                &context.this_server_url,
                self.1,
                &self.2,
            ))
        } else {
            None
        }
    }

    #[graphql(description = "URL to a SpriteBot format recolor sheet.")]
    fn recolor_sheet_url(&self, context: &Context) -> Option<String> {
        if self.sprites_available() {
            Some(get_url(
                AssetType::SpriteRecolorSheet,
                &context.this_server_url,
                self.1,
                &self.2,
            ))
        } else {
            None
        }
    }

    #[graphql(description = "A list of all existing sprites for the actions.")]
    async fn actions(&self, context: &Context) -> FieldResult<Vec<SpriteUnion>> {
        if self.sprites_available() {
            let action_copy_map = self.get_action_map(context).await?;
            // TODO: needed because of borrow in closure. can this be optimized?
            let action_copy_map_clone = action_copy_map.clone();
            let mut normal_sprites: HashMap<String, Sprite> =
                iter_existing_sprite_files(&context, &self.0.sprite_files, self.1, &self.2)
                    .await?
                    .into_iter()
                    .filter_map(|(action, locked)| {
                        // Copy ofs shouldn't appear here since they shouldn't have any sheets, but
                        // if they do, we filter them out, since we explicitly add them below.
                        if action_copy_map_clone.contains_key(&action) {
                            None
                        } else {
                            let action_clone = action.clone();
                            Some((
                                action,
                                self.process_sprite_action(
                                    &action_clone,
                                    locked,
                                    &context.this_server_url,
                                ),
                            ))
                        }
                    })
                    .collect();

            let mut copy_of_sprites: HashMap<String, CopyOf> = action_copy_map
                .into_iter()
                .map(|(action, copy_of)| {
                    let action_clone = action.clone();
                    (
                        action,
                        CopyOf {
                            locked: self
                                .0
                                .sprite_files
                                .get(&action_clone)
                                .copied()
                                .unwrap_or_default(),
                            action: action_clone,
                            copy_of: copy_of.to_string(),
                        },
                    )
                })
                .collect();

            let sprites_iter = self.0.sprite_files.keys().filter_map(|k| {
                if let Some(sprite) = normal_sprites.remove(k) {
                    Some(SpriteUnion::Sprite(sprite))
                } else {
                    copy_of_sprites.remove(k).map(SpriteUnion::CopyOf)
                }
            });

            Ok(sprites_iter.collect())
        } else {
            Ok(vec![])
        }
    }

    #[graphql(description = "A single sprite for a given action.")]
    async fn action(&self, context: &Context, action: String) -> FieldResult<Option<SpriteUnion>> {
        if self.sprites_available() {
            let action_copy_map = self.get_action_map(context).await?;
            if let Some(copy_of) = action_copy_map.get(&action) {
                // Copy of
                Ok(Some(SpriteUnion::CopyOf(CopyOf {
                    locked: self
                        .0
                        .sprite_files
                        .get(&action)
                        .copied()
                        .unwrap_or_default(),
                    action,
                    copy_of: copy_of.to_string(),
                })))
            } else {
                // Regular sprite
                Ok(get_existing_sprite_file(
                    &context,
                    &self.0.sprite_files,
                    &action,
                    self.1,
                    &self.2,
                )
                .await?
                .map(|locked| {
                    SpriteUnion::Sprite(self.process_sprite_action(
                        &action,
                        locked,
                        &context.this_server_url,
                    ))
                }))
            }
        } else {
            Ok(None)
        }
    }

    #[graphql(description = "The date and time this sprite set was last updated.")]
    fn modified_date(&self) -> Option<DateTime<Utc>> {
        self.0.sprite_modified
    }

    #[graphql(description = "List of all modifications made to those sprites since its creation.")]
    async fn history(&self, context: &Context) -> FieldResult<Vec<MonsterHistory>> {
        get_local_credits_file(&context, AssetCategory::Sprite, self.1, &self.2)
            .await??
            .into_iter()
            .map(|i| MonsterHistory::try_from_credit_row(context, i))
            .collect::<Result<Vec<_>, _>>()
    }

    #[graphql(
        description = "Returns a URL to retrieve the credits text file for the sprites for this form."
    )]
    fn history_url(&self, context: &Context) -> Option<String> {
        Some(get_url(
            AssetType::SpriteCreditsTxt,
            &context.this_server_url,
            self.1,
            &self.2,
        ))
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
            .get(&GroupId(self.id as i64))
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
        description = "This used to return the Discord handle of this author, if applicable and possible. It will now always return null.",
        deprecated = "This is no longer implemented and will always return null. It may or may not be re-introduced in future versions."
    )]
    async fn discord_handle(&self) -> FieldResult<Option<String>> {
        Ok(None)
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
}

impl Context {
    pub fn new(collab: Arc<SpriteCollab>) -> Self {
        Context {
            this_server_url: SystemConfig::Address.get_or_none().unwrap_or_default(),
            collab,
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
        S: AsRef<str> + Into<Key> + Send + Sync,
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

    #[graphql(description = "Retrieve a list of monsters.")]
    fn monster(
        context: &Context,
        #[graphql(description = "Monster IDs to limit the request to.")] filter: Option<Vec<i32>>,
    ) -> FieldResult<Vec<Monster>> {
        Ok(context
            .collab
            .data()
            .tracker
            .keys()
            .filter(|v| {
                if let Some(filter) = &filter {
                    filter.contains(&(***v as i32))
                } else {
                    true
                }
            })
            .map(|idx| Monster { id: **idx as i32 })
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

    #[graphql(description = "Retrieve a list of credits.")]
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
