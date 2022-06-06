use crate::assets::img_util::{add_palette_to, to_png};
use crate::datafiles::tracker::Group;
use crate::sprite_collab::CacheBehaviour;
use image::{GenericImage, RgbaImage};
use std::cmp::max;
use std::collections::HashMap;
use std::path::Path;

/// Maps known emotions from the sprite config to positions in the sheets.
/// All positions, widths and heights here use the portraits as units, so they must
/// be multiplied by the dimensions of a portrait for the actual coordinates / sizes.
pub struct PortraitSheetEmotions {
    emotion_positions: HashMap<String, (i32, i32)>,
    max_width: i32,
    max_height: i32,
}

impl PortraitSheetEmotions {
    pub fn new(emotion_cfg: Vec<String>, width_sheet: i32) -> PortraitSheetEmotions {
        let mut current_row = 0;
        let mut max_width = 0;
        let mut emotion_positions = HashMap::with_capacity(emotion_cfg.len());
        for (idx, emotion) in emotion_cfg.into_iter().enumerate() {
            let current_col = (idx as i32) % width_sheet;
            emotion_positions.insert(emotion, (current_col, current_row));
            if current_col == width_sheet - 1 {
                current_row += 1;
            }
            max_width = max(max_width, current_col + 1);
        }
        Self {
            emotion_positions,
            max_height: current_row,
            max_width,
        }
    }
}

pub async fn make_portrait_sheet(
    group: &Group,
    emotions: PortraitSheetEmotions,
    portrait_base_path: &Path,
    portrait_size: i32,
) -> Result<CacheBehaviour<Vec<u8>>, anyhow::Error> {
    Ok(CacheBehaviour::Cache(to_png(
        do_make_portrait_sheet(0, group, emotions, portrait_base_path, portrait_size).await?,
    )?))
}

pub async fn make_portrait_recolor_sheet(
    group: &Group,
    emotions: PortraitSheetEmotions,
    portrait_base_path: &Path,
    portrait_size: i32,
) -> Result<CacheBehaviour<Vec<u8>>, anyhow::Error> {
    let mut img =
        do_make_portrait_sheet(1, group, emotions, portrait_base_path, portrait_size).await?;
    add_palette_to(&mut img);
    Ok(CacheBehaviour::Cache(to_png(img)?))
}

async fn do_make_portrait_sheet(
    padding_top: i32,
    group: &Group,
    emotions: PortraitSheetEmotions,
    portrait_base_path: &Path,
    portrait_size: i32,
) -> Result<RgbaImage, anyhow::Error> {
    let mut img = RgbaImage::new(
        (emotions.max_width * portrait_size) as u32,
        (emotions.max_height * portrait_size + padding_top) as u32,
    );
    for grp_emotion in group.portrait_files.keys() {
        if emotions.emotion_positions.contains_key(grp_emotion) {
            let (x, y) = emotions.emotion_positions.get(grp_emotion).unwrap();
            let portrait_path = portrait_base_path.join(&format!("{}.png", grp_emotion));
            if let Ok(portrait_img) = image::open(&portrait_path) {
                img.copy_from(
                    &portrait_img,
                    (x * portrait_size) as u32,
                    ((y * portrait_size) + padding_top) as u32,
                )?;
            }
        }
    }
    Ok(img)
}
