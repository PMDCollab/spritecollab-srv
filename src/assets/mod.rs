use std::error::Error;
use std::fmt::Debug;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use http_body_util::{BodyExt, Full};
use http_body_util::combinators::BoxBody;
use hyper::{Method, Response, StatusCode};
use hyper::body::{Body, Bytes};
use hyper::http::HeaderValue;
use log::warn;
use tokio::fs;
use zip::ZipWriter;

use crate::{Config, SpriteCollab};
use crate::assets::portrait_sheets::{
    make_portrait_recolor_sheet, make_portrait_sheet, PortraitSheetEmotions,
};
use crate::assets::sprite_sheets::make_sprite_recolor_sheet;
use crate::assets::url::{AssetType, match_url};
use crate::assets::util::{force_non_shiny_group, join_monster_and_form};
use crate::cache::CacheBehaviour;
use crate::cache::ScCache;
use crate::datafiles::tracker::{FormMatch, MonsterFormCollector};

pub mod fs_check;
mod img_util;
mod portrait_sheets;
mod sprite_sheets;
pub mod url;
pub mod util;

pub type AssetBody = BoxBody<Bytes, Box<dyn Error + Send + Sync + 'static>>;

pub fn make_box_body<B, E>(body: B) -> AssetBody
where
    B: Body<Data = Bytes, Error = E> + Send + Sync + 'static,
    E: Error + Send + Sync + 'static,
{
    BoxBody::new(body.map_err(|e| {
        let b: Box<dyn Error + Send + Sync + 'static> = Box::new(e);
        b
    }))
}

pub async fn match_and_process_assets_path(
    method: &Method,
    path: &str,
    sprite_collab: Arc<SpriteCollab>,
) -> Option<Response<AssetBody>> {
    if method != Method::GET {
        return None;
    }
    if let Some((monster_idx, form_path, asset_type)) = match_url(path) {
        let portrait_tile_x;
        let portrait_size;
        let emotions_incl_flipped;
        let tracker;
        {
            let data = sprite_collab.data();
            portrait_tile_x = data.sprite_config.portrait_tile_x;
            portrait_size = data.sprite_config.portrait_size;
            emotions_incl_flipped = data
                .sprite_config
                .emotions
                .iter()
                .cloned()
                .chain(
                    data.sprite_config
                        .emotions
                        .iter()
                        .map(|e| format!("{}^", e)),
                )
                .collect::<Vec<_>>();
            tracker = data.tracker.clone();
        }
        let collector = MonsterFormCollector::collect(&tracker, monster_idx)?;
        let (form_path, _, group) = match asset_type {
            AssetType::PortraitRecolorSheet => collector.find_form(
                force_non_shiny_group(&form_path)
                    .into_iter()
                    .map(FormMatch::Exact),
            )?,
            AssetType::SpriteRecolorSheet => collector.find_form(
                force_non_shiny_group(&form_path)
                    .into_iter()
                    .map(FormMatch::Exact),
            )?,
            _ => collector.find_form(form_path.into_iter().map(FormMatch::Exact))?,
        };

        let joined_p = join_monster_and_form(monster_idx, &form_path, '/');
        let portrait_base_path = PathBuf::from(Config::Workdir.get())
            .join(format!("spritecollab/portrait/{}", joined_p));
        let sprite_base_path =
            PathBuf::from(Config::Workdir.get()).join(format!("spritecollab/sprite/{}", joined_p));

        match asset_type {
            AssetType::PortraitCreditsTxt => Some(process_nested_result(
                sprite_collab
                    .cached_may_fail(
                        format!("portrait_credits_txt|{}/{:?}", monster_idx, form_path),
                        || make_credits_txt(&portrait_base_path),
                    )
                    .await
                    .map(|r| r.map(make_box_body).map(Response::new)),
                path,
            )),
            AssetType::SpriteCreditsTxt => Some(process_nested_result(
                sprite_collab
                    .cached_may_fail(
                        format!("sprite_credits_txt|{}/{:?}", monster_idx, form_path),
                        || make_credits_txt(&sprite_base_path),
                    )
                    .await
                    .map(|r| r.map(make_box_body).map(Response::new)),
                path,
            )),
            AssetType::PortraitSheet => Some(process_nested_result(
                sprite_collab
                    .cached_may_fail(
                        format!("portrait_sheet|{}/{:?}", monster_idx, form_path),
                        || {
                            make_portrait_sheet(
                                group,
                                PortraitSheetEmotions::new(emotions_incl_flipped, portrait_tile_x),
                                &portrait_base_path,
                                portrait_size,
                            )
                        },
                    )
                    .await
                    .map(|r| {
                        r.map(Bytes::from)
                            .map(Full::new)
                            .map(make_box_body)
                            .map(PngResponse)
                    }),
                path,
            )),
            AssetType::PortraitRecolorSheet => Some(process_nested_result(
                sprite_collab
                    .cached_may_fail(
                        format!("portrait_recolor_sheet|{}/{:?}", monster_idx, form_path),
                        || {
                            make_portrait_recolor_sheet(
                                group,
                                PortraitSheetEmotions::new(emotions_incl_flipped, portrait_tile_x),
                                &portrait_base_path,
                                portrait_size,
                            )
                        },
                    )
                    .await
                    .map(|r| {
                        r.map(Bytes::from)
                            .map(Full::new)
                            .map(make_box_body)
                            .map(PngResponse)
                    }),
                path,
            )),
            AssetType::SpriteZip => Some(process_nested_result(
                sprite_collab
                    .cached_may_fail(
                        format!("sprite_zip|{}/{:?}", monster_idx, form_path),
                        || make_sprite_zip(&sprite_base_path),
                    )
                    .await
                    .map(|r| {
                        r.map(Bytes::from)
                            .map(Full::new)
                            .map(make_box_body)
                            .map(ZipResponse)
                    }),
                path,
            )),
            AssetType::SpriteRecolorSheet => Some(process_nested_result(
                sprite_collab
                    .cached_may_fail(
                        format!("sprite_recolor_sheet|{}/{:?}", monster_idx, form_path),
                        || make_sprite_recolor_sheet(&sprite_base_path),
                    )
                    .await
                    .map(|r| {
                        r.map(Bytes::from)
                            .map(Full::new)
                            .map(make_box_body)
                            .map(PngResponse)
                    }),
                path,
            )),
            _ => None,
        }
    } else {
        None
    }
}

pub async fn make_sprite_zip(
    sprite_base_path: &Path,
) -> Result<CacheBehaviour<Vec<u8>>, anyhow::Error> {
    let buf = Vec::with_capacity(50000000);
    let mut zip = ZipWriter::new(Cursor::new(buf));

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut paths = fs::read_dir(sprite_base_path).await?;

    while let Some(path) = paths.next_entry().await? {
        if path.file_type().await?.is_file() {
            let rfile_name = path.file_name();
            let file_name = rfile_name.to_string_lossy();
            if file_name != "credits.txt" {
                zip.start_file(file_name, options)?;
                zip.write_all(&fs::read(&path.path()).await?)?;
            }
        }
    }

    let buf = zip.finish()?.into_inner();
    Ok(CacheBehaviour::Cache(buf))
}

pub async fn make_credits_txt(base_path: &Path) -> Result<CacheBehaviour<String>, anyhow::Error> {
    let credits_path = base_path.join("credits.txt");
    Ok(CacheBehaviour::Cache(if credits_path.is_file() {
        fs::read_to_string(&credits_path).await?
    } else {
        "".to_owned()
    }))
}

pub fn process_nested_result<T, E1, E2>(
    result: Result<Result<T, E1>, E2>,
    request_path: &str,
) -> Response<AssetBody>
where
    T: TryInto<Response<AssetBody>>,
    T::Error: Debug,
    E1: Debug,
    E2: Debug,
{
    match result {
        Ok(Ok(t)) => match t.try_into() {
            Ok(success_reponse) => success_reponse,
            Err(e) => make_err_response(e, request_path).map(make_box_body),
        },
        Ok(Err(e)) => make_err_response(e, request_path).map(make_box_body),
        Err(e) => make_err_response(e, request_path).map(make_box_body),
    }
}

pub fn make_err_response<E: Debug>(err: E, request_path: &str) -> Response<String> {
    warn!("Error processing asset at '{}': {:?}", request_path, err);
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(
            format!(
                "<html><body><h1>Internal Server Error</h1><pre>{:?}</pre><br><img src=\"https://http.cat/500\"></body></html>",
                err
            )
        )
        .unwrap_or_else(|_| Response::new(String::from(
            "<html><body><h1>Internal Server Error</h1><img src=\"https://http.cat/500\"></body></html>"
        )))
}

struct ZipResponse(AssetBody);

impl TryInto<Response<AssetBody>> for ZipResponse {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Response<AssetBody>, Self::Error> {
        let mut resp = Response::new(self.0);
        let headers = resp.headers_mut();
        headers.insert("Content-Type", HeaderValue::from_str("application/zip")?);
        headers.insert(
            "Content-Disposition",
            HeaderValue::from_str("attachment; filename=sprite.zip")?,
        );
        Ok(resp)
    }
}

struct PngResponse(AssetBody);

impl TryInto<Response<AssetBody>> for PngResponse {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Response<AssetBody>, Self::Error> {
        let mut resp = Response::new(self.0);
        let headers = resp.headers_mut();
        headers.insert("Content-Type", HeaderValue::from_str("image/png")?);
        Ok(resp)
    }
}
