use crate::Config;
use crate::assets::util::join_monster_and_form;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Durations {
    #[serde(rename = "$value")]
    pub duration: Option<Vec<i64>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Anim {
    pub name: String,
    pub index: Option<i64>,
    pub frame_width: Option<i64>,
    pub frame_height: Option<i64>,
    pub durations: Option<Durations>,
    pub rush_frame: Option<i64>,
    pub hit_frame: Option<i64>,
    pub return_frame: Option<i64>,
    pub copy_of: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Anims {
    #[serde(rename = "$value")]
    pub anim: Vec<Anim>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct AnimDataXml {
    pub shadow_size: i64,
    pub anims: Anims,
}

#[derive(Error, Debug)]
pub enum AnimDataXmlOpenError {
    #[error("Could not open file: {0:?}")]
    IoError(#[from] std::io::Error),
    #[error("Could not parse file: {0:?}")]
    SerdeXmlError(#[from] serde_xml_rs::Error),
}

impl AnimDataXml {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, AnimDataXmlOpenError> {
        let file = File::open(path)?;
        let file_reader = BufReader::new(file);
        Ok(Self::from_reader(file_reader)?)
    }

    pub fn open_for_form(
        monster_idx: i32,
        path_to_form: &[i32],
    ) -> Result<Self, AnimDataXmlOpenError> {
        let joined_f = join_monster_and_form(monster_idx, path_to_form, '/');
        let path = PathBuf::from(Config::Workdir.get())
            .join(format!("spritecollab/sprite/{}/AnimData.xml", joined_f));
        Self::open(path)
    }

    pub fn from_reader<R: Read>(r: R) -> Result<Self, serde_xml_rs::Error> {
        serde_xml_rs::from_reader(r)
    }

    pub fn get_action_copies(&self) -> HashMap<String, String> {
        self.anims
            .anim
            .iter()
            .filter_map(|anim| {
                anim.copy_of
                    .as_ref()
                    .map(|copy_of| (anim.name.clone(), copy_of.clone()))
            })
            .collect()
    }
}
