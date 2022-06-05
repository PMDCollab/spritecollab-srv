use crate::datafiles::DataReadResult;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub async fn read_sprite_config<P: AsRef<Path>>(path: P) -> DataReadResult<SpriteConfig> {
    let input = File::open(path)?;
    Ok(serde_json::from_reader(BufReader::new(input))?)
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
pub struct SpriteConfig {
    pub portrait_size: i32,
    pub portrait_tile_x: i32,
    pub portrait_tile_y: i32,
    pub completion_emotions: Vec<Vec<i32>>,
    pub emotions: Vec<String>,
    pub completion_actions: Vec<Vec<i32>>,
    pub actions: Vec<String>,
    pub action_map: HashMap<i32, String>,
}
