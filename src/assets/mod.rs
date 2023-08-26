use crate::assets::portrait_sheets::{
    make_portrait_recolor_sheet, make_portrait_sheet, PortraitSheetEmotions,
};
use crate::assets::sprite_sheets::make_sprite_recolor_sheet;
use crate::assets::url::{match_url, AssetType};
use crate::assets::util::{force_non_shiny_group, join_monster_and_form};
use crate::cache::CacheBehaviour;
use crate::cache::ScCache;
use crate::datafiles::tracker::{FormMatch, MonsterFormCollector};
use crate::{Config, SpriteCollab};
use hyper::http::HeaderValue;
use hyper::{Body, Method, Response, StatusCode};
use log::warn;
use std::fmt::Debug;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use zip::ZipWriter;

pub mod fs_check;
mod img_util;
mod portrait_sheets;
mod sprite_sheets;
pub mod url;
pub mod util;

pub async fn match_and_process_assets_path(
    method: &Method,
    path: &str,
    sprite_collab: Arc<SpriteCollab>,
) -> Option<Response<Body>> {
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
                    .map(|r| r.map(PngResponse)),
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
                    .map(|r| r.map(PngResponse)),
                path,
            )),
            AssetType::SpriteZip => Some(process_nested_result(
                sprite_collab
                    .cached_may_fail(
                        format!("sprite_zip|{}/{:?}", monster_idx, form_path),
                        || make_sprite_zip(&sprite_base_path),
                    )
                    .await
                    .map(|r| r.map(ZipResponse)),
                path,
            )),
            AssetType::SpriteRecolorSheet => Some(process_nested_result(
                sprite_collab
                    .cached_may_fail(
                        format!("sprite_recolor_sheet|{}/{:?}", monster_idx, form_path),
                        || make_sprite_recolor_sheet(&sprite_base_path),
                    )
                    .await
                    .map(|r| r.map(PngResponse)),
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

    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

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

pub fn process_nested_result<T, E1, E2>(
    result: Result<Result<T, E1>, E2>,
    request_path: &str,
) -> Response<Body>
where
    T: TryInto<Response<Body>>,
    T::Error: Debug,
    E1: Debug,
    E2: Debug,
{
    match result {
        Ok(Ok(t)) => match t.try_into() {
            Ok(success_reponse) => success_reponse,
            Err(e) => make_err_response(e, request_path),
        },
        Ok(Err(e)) => make_err_response(e, request_path),
        Err(e) => make_err_response(e, request_path),
    }
}

pub fn make_err_response<E: Debug>(err: E, request_path: &str) -> Response<Body> {
    warn!("Error processing asset at '{}': {:?}", request_path, err);
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Body::from(
            format!(
                "<html><body><h1>Internal Server Error</h1><pre>{:?}</pre><br><img src=\"https://http.cat/500\"></body></html>",
                err
            )
        ))
        .unwrap_or_else(|_| Response::new(Body::from(
            "<html><body><h1>Internal Server Error</h1><img src=\"https://http.cat/500\"></body></html>"
        )))
}

struct ZipResponse(Vec<u8>);

impl TryInto<Response<Body>> for ZipResponse {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Response<Body>, Self::Error> {
        let mut resp = Response::new(Body::from(self.0));
        let headers = resp.headers_mut();
        headers.insert("Content-Type", HeaderValue::from_str("application/zip")?);
        headers.insert(
            "Content-Disposition",
            HeaderValue::from_str("attachment; filename=sprite.zip")?,
        );
        Ok(resp)
    }
}

struct PngResponse(Vec<u8>);

impl TryInto<Response<Body>> for PngResponse {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Response<Body>, Self::Error> {
        let mut resp = Response::new(Body::from(self.0));
        let headers = resp.headers_mut();
        headers.insert("Content-Type", HeaderValue::from_str("image/png")?);
        Ok(resp)
    }
}
