use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Durations {
    #[serde(rename = "$value")]
    duration: Option<Vec<i64>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Anim {
    name: String,
    index: i64,
    frame_width: Option<i64>,
    frame_height: Option<i64>,
    durations: Option<Durations>,
    rush_frame: Option<i64>,
    hit_frame: Option<i64>,
    return_frame: Option<i64>,
    copy_of: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Anims {
    #[serde(rename = "$value")]
    anim: Vec<Anim>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct AnimDataXml {
    shadow_size: i64,
    anims: Anims,
}

impl AnimDataXml {
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
