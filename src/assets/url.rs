use crate::assets::util::{force_shiny_group, join_monster_and_form};
use crate::Config;
use route_recognizer::Router;
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub enum AssetType<'a> {
    PortraitCreditsTxt,
    SpriteCreditsTxt,
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

    match asset_type {
        AssetType::PortraitCreditsTxt => {
            let joined_f_dash = join_monster_and_form(monster_id, path_to_form, '-');
            format!(
                "{}/assets/portrait-credits-{}.txt",
                this_srv_url, joined_f_dash
            )
        }
        AssetType::SpriteCreditsTxt => {
            let joined_f_dash = join_monster_and_form(monster_id, path_to_form, '-');
            format!(
                "{}/assets/sprite-credits-{}.txt",
                this_srv_url, joined_f_dash
            )
        }
        AssetType::PortraitSheet => {
            let joined_f_dash = join_monster_and_form(monster_id, path_to_form, '-');
            format!("{}/assets/portrait-{}.png", this_srv_url, joined_f_dash)
        }
        AssetType::PortraitRecolorSheet => {
            let joined_f_dash =
                join_monster_and_form(monster_id, &force_shiny_group(path_to_form), '-');
            format!(
                "{}/assets/portrait_recolor-{}.png",
                this_srv_url, joined_f_dash
            )
        }
        AssetType::Portrait(emotion) => {
            let joined_f = join_monster_and_form(monster_id, path_to_form, '/');
            format!(
                "{}/portrait/{}/{}.png",
                assets_srv_url,
                joined_f,
                up(emotion)
            )
        }
        AssetType::PortraitFlipped(emotion) => {
            let joined_f = join_monster_and_form(monster_id, path_to_form, '/');
            format!(
                "{}/portrait/{}/{}.png",
                assets_srv_url,
                joined_f,
                up(emotion)
            )
        }
        AssetType::SpriteAnimDataXml => {
            let joined_f = join_monster_and_form(monster_id, path_to_form, '/');
            format!("{}/sprite/{}/AnimData.xml", assets_srv_url, joined_f)
        }
        AssetType::SpriteZip => {
            let joined_f = join_monster_and_form(monster_id, path_to_form, '/');
            format!("{}/assets/{}/sprites.zip", this_srv_url, joined_f)
        }
        AssetType::SpriteRecolorSheet => {
            let joined_f_dash =
                join_monster_and_form(monster_id, &force_shiny_group(path_to_form), '-');
            format!(
                "{}/assets/sprite_recolor-{}.png",
                this_srv_url, joined_f_dash
            )
        }
        AssetType::SpriteAnim(action) => {
            let joined_f = join_monster_and_form(monster_id, path_to_form, '/');
            format!(
                "{}/sprite/{}/{}-Anim.png",
                assets_srv_url,
                joined_f,
                up(action)
            )
        }
        AssetType::SpriteOffsets(action) => {
            let joined_f = join_monster_and_form(monster_id, path_to_form, '/');
            format!(
                "{}/sprite/{}/{}-Offsets.png",
                assets_srv_url,
                joined_f,
                up(action)
            )
        }
        AssetType::SpriteShadows(action) => {
            let joined_f = join_monster_and_form(monster_id, path_to_form, '/');
            format!(
                "{}/sprite/{}/{}-Shadow.png",
                assets_srv_url,
                joined_f,
                up(action)
            )
        }
    }
}

/// Matches a URL, if it matches returns a tuple of (monster id, form path, asset type)
pub fn match_url(path: &str) -> Option<(i32, VecDeque<i32>, AssetType)> {
    let mut router = Router::new();

    // This is a bit of a hack, but we treat - as / to easily support
    // SpriteBot-formatted file names.
    let path = path.replace('-', "/");

    router.add(
        "/assets/portrait/credits/*formpath.txt",
        AssetType::PortraitCreditsTxt,
    );
    router.add(
        "/assets/sprite/credits/*formpath.txt",
        AssetType::SpriteCreditsTxt,
    );
    router.add("/assets/portrait/*formpath.png", AssetType::PortraitSheet);
    router.add(
        "/assets/portrait_recolor/*formpath.png",
        AssetType::PortraitRecolorSheet,
    );
    router.add("/assets/*formpath/sprites.zip", AssetType::SpriteZip);
    router.add(
        "/assets/sprite_recolor/*formpath.png",
        AssetType::SpriteRecolorSheet,
    );
    router.add("/assets/portrait/*formpath.png", AssetType::PortraitSheet);
    router.add(
        "/assets/portrait_recolor/*formpath.png",
        AssetType::PortraitRecolorSheet,
    );
    router.add("/assets/sprites.zip", AssetType::SpriteZip);

    let m = router.recognize(&path).ok()?;

    let form_path = m.params().find("formpath").map(|s| {
        s.split('/')
            .map(|x| x.parse::<i32>())
            .collect::<Result<VecDeque<i32>, _>>()
    });

    let (monster_id, form_path) = match form_path {
        Some(Ok(mut x)) => (x.pop_front()?, x),
        Some(Err(_)) => return None,
        None => return None,
    };
    Some((monster_id, form_path, (*m.handler()).clone()))
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
