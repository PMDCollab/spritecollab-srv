use crate::assets::img_util::{add_palette_to, to_png};
use crate::datafiles::anim_data_xml::AnimDataXml;
use crate::sprite_collab::CacheBehaviour;
use anyhow::anyhow;
use image::{DynamicImage, GenericImage, GenericImageView, RgbaImage};
use indexmap::IndexMap;
use std::cmp::{max, min};
use std::path::{Path, PathBuf};

#[derive(Default)]
struct SpriteOffsets {
    head_x: i32,
    head_y: i32,
    lhand_x: i32,
    lhand_y: i32,
    rhand_x: i32,
    rhand_y: i32,
    center_x: i32,
    center_y: i32,
}

impl SpriteOffsets {
    fn add_loc(&mut self, (x, y): (i32, i32)) {
        self.head_x += x;
        self.head_y += y;
        self.lhand_x += x;
        self.lhand_y += y;
        self.rhand_x += x;
        self.rhand_y += y;
        self.center_x += x;
        self.center_y += y;
    }

    fn get_bounds(&self) -> (i32, i32, i32, i32) {
        let mut max_bounds = (10000, 10000, -10000, -10000);
        max_bounds = Self::combine_extents(max_bounds, self.get_bounds_head());
        max_bounds = Self::combine_extents(max_bounds, self.get_bounds_lhand());
        max_bounds = Self::combine_extents(max_bounds, self.get_bounds_rhand());
        max_bounds = Self::combine_extents(max_bounds, self.get_bounds_center());
        max_bounds
    }

    fn get_bounds_head(&self) -> (i32, i32, i32, i32) {
        (self.head_x, self.head_y, self.head_x + 1, self.head_y + 1)
    }

    fn get_bounds_lhand(&self) -> (i32, i32, i32, i32) {
        (
            self.lhand_x,
            self.lhand_y,
            self.lhand_x + 1,
            self.lhand_y + 1,
        )
    }

    fn get_bounds_rhand(&self) -> (i32, i32, i32, i32) {
        (
            self.rhand_x,
            self.rhand_y,
            self.rhand_x + 1,
            self.rhand_y + 1,
        )
    }

    fn get_bounds_center(&self) -> (i32, i32, i32, i32) {
        (
            self.center_x,
            self.center_y,
            self.center_x + 1,
            self.center_y + 1,
        )
    }

    fn get_centered_bounds(&self, (cx, cy): (i32, i32)) -> (i32, i32, i32, i32) {
        let (x, y, xm, ym) = self.get_bounds();
        let min_x = min(x - cx, cx - xm);
        let min_y = min(y - cy, cy - ym);
        let max_x = max(cx - x, xm - cx);
        let max_y = max(cy - y, ym - cy);
        add_to_bounds((min_x, min_y, max_x, max_y), (cx, cy))
    }

    fn combine_extents(
        (t0, t1, t2, t3): (i32, i32, i32, i32),
        (b0, b1, b2, b3): (i32, i32, i32, i32),
    ) -> (i32, i32, i32, i32) {
        (min(t0, b0), min(t1, b1), max(t2, b2), max(t3, b3))
    }
}

pub async fn make_sprite_recolor_sheet(
    sprite_base_path: &Path,
) -> Result<CacheBehaviour<Vec<u8>>, anyhow::Error> {
    let frames = get_sprite_frames(sprite_base_path).await?;
    for (idx, (frame, _)) in frames.iter().enumerate() {
        frame.save(format!("/workdir/{}.png", idx)).unwrap();
    }
    let (frame_size_x, frame_size_y) = get_sprite_frame_size_from_frames(&frames);

    let max_size = (frames.len() as f64).sqrt().ceil() as u32;
    let mut combined_img = RgbaImage::new(max_size * frame_size_x, max_size * frame_size_y);

    for (idx, (frame, _)) in frames.iter().enumerate() {
        let idx = idx as u32;
        let diff_pos_x = frame_size_x / 2 - frame.width() / 2;
        let diff_pos_y = frame_size_y / 2 - frame.height() / 2;
        let xx = idx % max_size;
        let yy = idx / max_size;
        let tile_pos_x = xx * frame_size_x;
        let tile_pos_y = yy * frame_size_y;
        combined_img.copy_from(frame, tile_pos_x + diff_pos_x, tile_pos_y + diff_pos_y)?;
    }
    add_palette_to(&mut combined_img);
    Ok(CacheBehaviour::Cache(to_png(combined_img)?))
}

async fn get_sprite_frames(
    sprite_base_path: &Path,
) -> Result<Vec<(DynamicImage, SpriteOffsets)>, anyhow::Error> {
    let mut anim_dims = IndexMap::new();

    let xml_path = PathBuf::from(sprite_base_path).join("AnimData.xml");
    let xml = AnimDataXml::open(xml_path)?;

    for anim_node in &xml.anims.anim {
        if anim_node.copy_of.is_none() {
            if anim_node.frame_width.is_none() || anim_node.frame_height.is_none() {
                return Err(anyhow!("The AnimData.xml for this sprite is invalid: FrameWidth or FrameHeight missing for {}", anim_node.name));
            }
            anim_dims.insert(
                &anim_node.name,
                (
                    *anim_node.frame_width.as_ref().unwrap() as i32,
                    *anim_node.frame_height.as_ref().unwrap() as i32,
                ),
            );
        }
    }

    let mut frames: Vec<(DynamicImage, SpriteOffsets)> = Vec::new();

    for (anim_name, (frame_size_x, frame_size_y)) in anim_dims {
        let img_path = sprite_base_path.join(format!("{}-Anim.png", anim_name));
        let c_img = image::open(img_path);
        let offset_img_path = sprite_base_path.join(format!("{}-Offsets.png", anim_name));
        let c_offset_img = image::open(offset_img_path);

        if let (Ok(mut img), Ok(offset_img)) = (c_img, c_offset_img) {
            for (base_yy, _) in (0..img.height()).step_by(frame_size_y as usize).enumerate() {
                let base_yy = base_yy as i32;
                // standardized to clockwise style
                let yy = ((8 - base_yy).rem_euclid(8)) * frame_size_y;
                for xx in (0..img.width()).step_by(frame_size_x as usize) {
                    let xx = xx as i32;
                    let tile_bounds = (xx, yy, xx + frame_size_x, yy + frame_size_y);
                    let (mut bounds_x, mut bounds_y, mut bounds_xm, mut bounds_ym) =
                        get_covered_bounds(&img, tile_bounds);
                    let mut missing_tex = if bounds_x >= bounds_xm {
                        (bounds_x, bounds_y, bounds_xm, bounds_ym) = (
                            frame_size_x / 2,
                            frame_size_y / 2,
                            frame_size_x / 2 + 1,
                            frame_size_y / 2 + 1,
                        );
                        true
                    } else {
                        false
                    };

                    let frame_offset = get_offset_from_rgb(
                        &offset_img,
                        tile_bounds,
                        true,
                        true,
                        true,
                        true,
                        false,
                    )?;

                    let mut offsets = SpriteOffsets::default();
                    if let Some((cx, cy)) = frame_offset[2] {
                        offsets.center_x = cx;
                        offsets.center_y = cy;
                    }
                    match frame_offset[0] {
                        Some((x, y)) => {
                            offsets.head_x = x;
                            offsets.head_y = y;
                            missing_tex = false;
                        }
                        None => {
                            offsets.head_x = offsets.center_x;
                            offsets.head_y = offsets.center_y;
                        }
                    }

                    // no texture OR offset means this frame is missing.  do not map it.  skip.
                    if missing_tex {
                        continue;
                    }

                    if let Some((cx, cy)) = frame_offset[1] {
                        offsets.lhand_x = cx;
                        offsets.lhand_y = cy;
                    }
                    if let Some((cx, cy)) = frame_offset[3] {
                        offsets.rhand_x = cx;
                        offsets.rhand_y = cy;
                    }

                    offsets.add_loc((-bounds_x, -bounds_y));

                    let (abs_bounds_x, abs_bounds_y, abs_bounds_xm, abs_bounds_ym) =
                        add_to_bounds((bounds_x, bounds_y, bounds_xm, bounds_ym), (xx, yy));
                    let frame_tex = img.crop(
                        abs_bounds_x as u32,
                        abs_bounds_y as u32,
                        (abs_bounds_xm - abs_bounds_x) as u32,
                        (abs_bounds_ym - abs_bounds_y) as u32,
                    );

                    let mut is_dupe = false;
                    for (final_frame, final_offset) in &frames {
                        if imgs_equal(final_frame, &frame_tex, false)
                            && offsets_equal(
                                final_offset,
                                &offsets,
                                frame_tex.width() as i32,
                                false,
                            )
                        {
                            is_dupe = true;
                            break;
                        }
                        if imgs_equal(final_frame, &frame_tex, true)
                            && offsets_equal(final_offset, &offsets, frame_tex.width() as i32, true)
                        {
                            is_dupe = true;
                            break;
                        }
                    }
                    if !is_dupe {
                        frames.push((frame_tex, offsets));
                    }
                }
            }
        }
    }

    Ok(frames)
}

fn imgs_equal(img1: &DynamicImage, img2: &DynamicImage, flip: bool) -> bool {
    if img1.width() != img2.width() || img1.height() != img2.height() {
        return false;
    }
    for xx in 0..img1.width() {
        for yy in 0..img1.height() {
            let x2 = if flip { img1.width() - 1 - xx } else { xx };
            if img1.get_pixel(xx, yy).0 != img2.get_pixel(x2, yy).0 {
                return false;
            }
        }
    }
    true
}

fn offsets_equal(
    offset1: &SpriteOffsets,
    offset2: &SpriteOffsets,
    img_width: i32,
    flip: bool,
) -> bool {
    let (center, head, lhand, rhand) = if flip {
        (
            (img_width - offset2.center_x - 1, offset2.center_y),
            (img_width - offset2.head_x - 1, offset2.head_y),
            (img_width - offset2.lhand_x - 1, offset2.lhand_y),
            (img_width - offset2.rhand_x - 1, offset2.rhand_y),
        )
    } else {
        (
            (offset2.center_x, offset2.center_y),
            (offset2.head_x, offset2.head_y),
            (offset2.lhand_x, offset2.lhand_y),
            (offset2.rhand_x, offset2.rhand_y),
        )
    };
    if (offset1.center_x, offset1.center_y) != center {
        return false;
    }
    if (offset1.head_x, offset1.head_y) != head {
        return false;
    }
    if (offset1.lhand_x, offset1.lhand_y) != lhand {
        return false;
    }
    if (offset1.rhand_x, offset1.rhand_y) != rhand {
        return false;
    }
    true
}

fn get_covered_bounds(
    in_img: &DynamicImage,
    (max_box_x, max_box_y, max_box_xm, max_box_ym): (i32, i32, i32, i32),
) -> (i32, i32, i32, i32) {
    let mut min_x = in_img.width() as i32;
    let mut min_y = in_img.height() as i32;
    let mut max_x = -1;
    let mut max_y = -1;
    for i in max_box_x..max_box_xm {
        for j in max_box_y..max_box_ym {
            if in_img.get_pixel(i as u32, j as u32).0[3] != 0 {
                if i < min_x {
                    min_x = i;
                }
                if i > max_x {
                    max_x = i;
                }
                if j < min_y {
                    min_y = j;
                }
                if j > max_y {
                    max_y = j;
                }
            }
        }
    }
    let abs_bounds = (min_x, min_y, max_x + 1, max_y + 1);
    sub_from_bounds(abs_bounds, (max_box_x, max_box_y))
}

fn add_to_bounds(
    (x, y, xm, ym): (i32, i32, i32, i32),
    (cx, cy): (i32, i32),
) -> (i32, i32, i32, i32) {
    (x + cx, y + cy, xm + cx, ym + cy)
}

fn sub_from_bounds(
    (x, y, xm, ym): (i32, i32, i32, i32),
    (cx, cy): (i32, i32),
) -> (i32, i32, i32, i32) {
    (x - cx, y - cy, xm - cx, ym - cy)
}

fn get_offset_from_rgb(
    img: &DynamicImage,
    (bounds_x, bounds_y, bounds_xm, bounds_ym): (i32, i32, i32, i32),
    black: bool,
    r: bool,
    g: bool,
    b: bool,
    white: bool,
) -> Result<[Option<(i32, i32)>; 5], anyhow::Error> {
    let mut results = [None; 5];
    for i in bounds_x..bounds_xm {
        for j in bounds_y..bounds_ym {
            let color = &img.get_pixel(i as u32, j as u32).0;
            if color[3] == 255 {
                if black && color[0] == 0 && color[1] == 0 && color[2] == 0 {
                    if results[0].is_none() {
                        results[0] = Some((i - bounds_x, j - bounds_y));
                    } else {
                        return Err(anyhow!(
                            "Multiple black pixels found when searching for offsets!"
                        ));
                    }
                }
                if r && color[0] == 255 {
                    if results[1].is_none() {
                        results[1] = Some((i - bounds_x, j - bounds_y));
                    } else {
                        return Err(anyhow!(
                            "Multiple red pixels found when searching for offsets!"
                        ));
                    }
                }
                if g && color[1] == 255 {
                    if results[2].is_none() {
                        results[2] = Some((i - bounds_x, j - bounds_y));
                    } else {
                        return Err(anyhow!(
                            "Multiple green pixels found when searching for offsets!"
                        ));
                    }
                }
                if b && color[2] == 255 {
                    if results[3].is_none() {
                        results[3] = Some((i - bounds_x, j - bounds_y));
                    } else {
                        return Err(anyhow!(
                            "Multiple blue pixels found when searching for offsets!"
                        ));
                    }
                }
                if white && color[0] == 255 && color[1] == 255 && color[2] == 255 {
                    if results[4].is_none() {
                        results[4] = Some((i - bounds_x, j - bounds_y));
                    } else {
                        return Err(anyhow!(
                            "Multiple white pixels found when searching for offsets!"
                        ));
                    }
                }
            }
        }
    }
    Ok(results)
}

fn get_sprite_frame_size_from_frames(frames: &[(DynamicImage, SpriteOffsets)]) -> (u32, u32) {
    let mut max_width = 0;
    let mut max_height = 0;

    for (frame_tex, frame_offset) in frames {
        max_width = max(max_width, frame_tex.width());
        max_height = max(max_height, frame_tex.height());
        let (obx, oby, obxm, obym) = frame_offset.get_centered_bounds((
            (frame_tex.width() / 2) as i32,
            (frame_tex.height() / 2) as i32,
        ));
        max_width = max(max_width, (obxm - obx) as u32);
        max_height = max(max_height, (obym - oby) as u32);
    }

    max_width = round_up_to_mult(max_width, 2);
    max_height = round_up_to_mult(max_height, 2);

    (max_width, max_height)
}

#[inline(always)]
fn round_up_to_mult(num: u32, mult: u32) -> u32 {
    (((num - 1) / mult) + 1) * mult
}
