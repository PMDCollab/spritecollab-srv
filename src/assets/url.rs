use crate::Config;
use itertools::Itertools;

pub enum AssetType<'a> {
    PortraitSheet,
    PortraitRecolorSheet,
    Portrait(&'a str),
    PortraitFlipped(&'a str),
    SpriteAnimDataXml,
    SpriteZip,
    SpriteRecolorSheet,
    SpriteAnim(&'a str),
    SpriteOffsets(&'a str),
    SpriteShadows(&'a str),
}

pub fn get_url(
    asset_type: AssetType,
    this_srv_url: &str,
    monster_id: i32,
    path_to_form: &[i32],
) -> String {
    let assets_srv_url = Config::GitAssetsUrl.get();
    let mut form_joined = path_to_form.iter().map(|v| format!("{:04}", v)).join("/");
    if !form_joined.is_empty() {
        form_joined = format!("/{}", form_joined);
    }

    match asset_type {
        AssetType::PortraitSheet => {
            format!(
                "{}/assets/{:04}{}/portrait_sheet.png",
                this_srv_url, monster_id, form_joined
            )
        }
        AssetType::PortraitRecolorSheet => {
            format!(
                "{}/assets/{:04}{}/portrait_recolor_sheet.png",
                this_srv_url, monster_id, form_joined
            )
        }
        AssetType::Portrait(emotion) => {
            format!(
                "{}/portrait/{:04}{}/{}.png",
                assets_srv_url,
                monster_id,
                form_joined,
                up(emotion)
            )
        }
        AssetType::PortraitFlipped(emotion) => {
            format!(
                "{}/portrait/{:04}{}/{}^.png",
                assets_srv_url,
                monster_id,
                form_joined,
                up(emotion)
            )
        }
        AssetType::SpriteAnimDataXml => {
            format!(
                "{}/sprite/{:04}{}/AnimData.xml",
                assets_srv_url, monster_id, form_joined
            )
        }
        AssetType::SpriteZip => {
            format!(
                "{}/assets/{:04}{}/sprites.zip",
                this_srv_url, monster_id, form_joined
            )
        }
        AssetType::SpriteRecolorSheet => {
            format!(
                "{}/assets/{:04}{}/sprite_recolor_sheet.png",
                this_srv_url, monster_id, form_joined
            )
        }
        AssetType::SpriteAnim(action) => {
            format!(
                "{}/sprite/{:04}{}/{}-Anim.png",
                assets_srv_url,
                monster_id,
                form_joined,
                up(action)
            )
        }
        AssetType::SpriteOffsets(action) => {
            format!(
                "{}/sprite/{:04}{}/{}-Offsets.png",
                assets_srv_url,
                monster_id,
                form_joined,
                up(action)
            )
        }
        AssetType::SpriteShadows(action) => {
            format!(
                "{}/sprite/{:04}{}/{}-Shadows.png",
                assets_srv_url,
                monster_id,
                form_joined,
                up(action)
            )
        }
    }
}

fn up(s: &str) -> String {
    // a bit ugly, but it works for now
    if s == "teary-eyed" {
        return "Teary-Eyed".to_string();
    }
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
