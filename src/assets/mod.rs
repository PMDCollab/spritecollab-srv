use std::fmt::Debug;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use hyper::{Body, Method, Response, StatusCode};
use hyper::http::HeaderValue;
use itertools::Itertools;
use log::warn;
use tokio::fs;
use zip::ZipWriter;
use crate::assets::url::{AssetType, match_url};
use crate::cache::ScCache;
use crate::datafiles::tracker::{FormMatch, MonsterFormCollector};
use crate::sprite_collab::CacheBehaviour;
use crate::{Config, SpriteCollab};
use crate::assets::portrait_sheets::{make_portrait_recolor_sheet, make_portrait_sheet, PortraitSheetEmotions};
use crate::assets::sprite_sheets::make_sprite_recolor_sheet;

pub mod url;
mod sprite_sheets;
mod portrait_sheets;
mod img_util;

pub async fn match_and_process_assets_path(method: &Method, path: &str, sprite_collab: Arc<SpriteCollab>) -> Option<Response<Body>> {
    if method != Method::GET {
        return None;
    }
    if let Some((monster_idx, form_path, asset_type)) = match_url(path) {
        let portrait_tile_y;
        let portrait_size;
        let emotions;
        let tracker;
        {
            let data = sprite_collab.data();
            portrait_tile_y = data.sprite_config.portrait_tile_x;
            portrait_size = data.sprite_config.portrait_size;
            emotions = data.sprite_config.emotions.clone();
            tracker = data.tracker.clone();
        }
        let collector = MonsterFormCollector::collect(&tracker, monster_idx)?;
        let (form_path, _, group) = collector.find_form(form_path.into_iter().map(FormMatch::Exact))?;

        let mut form_joined = form_path.iter().map(|v| format!("{:04}", v)).join("/");
        if !form_joined.is_empty() {
            form_joined = format!("/{}", form_joined);
        }
        let portrait_base_path = PathBuf::from(Config::Workdir.get())
            .join(&format!("spritecollab/portrait/{:04}{}", monster_idx, form_joined));
        let sprite_base_path = PathBuf::from(Config::Workdir.get())
            .join(&format!("spritecollab/sprite/{:04}{}", monster_idx, form_joined));

        match asset_type {
            AssetType::PortraitSheet => {
                Some(process_nested_result(sprite_collab.cached_may_fail(
                    format!("portrait_sheet|{}/{:?}", monster_idx, form_path),
                    || make_portrait_sheet(group, PortraitSheetEmotions::new(emotions, portrait_tile_y), &portrait_base_path, portrait_size)
                ).await.map(|r| r.map(PngResponse)), path))
            }
            AssetType::PortraitRecolorSheet => {
                Some(process_nested_result(sprite_collab.cached_may_fail(
                    format!("portrait_recolor_sheet|{}/{:?}", monster_idx, form_path),
                    || make_portrait_recolor_sheet(group, PortraitSheetEmotions::new(emotions, portrait_tile_y), &portrait_base_path, portrait_size)
                ).await.map(|r| r.map(PngResponse)), path))
            }
            AssetType::SpriteZip => {
                Some(process_nested_result(sprite_collab.cached_may_fail(
                    format!("sprite_zip|{}/{:?}", monster_idx, form_path),
                    || make_sprite_zip(&sprite_base_path)
                ).await.map(|r| r.map(ZipResponse)), path))
            }
            AssetType::SpriteRecolorSheet => {
                Some(process_nested_result(sprite_collab.cached_may_fail(
                    format!("sprite_recolor_sheet|{}/{:?}", monster_idx, form_path),
                    || make_sprite_recolor_sheet(&sprite_base_path)
                ).await.map(|r| r.map(PngResponse)), path))
            }
            _ => None
        }
    } else {
        None
    }
}

pub async fn make_sprite_zip(sprite_base_path: &Path) -> Result<CacheBehaviour<Vec<u8>>, anyhow::Error> {
    let buf = Vec::with_capacity(50000000);
    let mut zip = ZipWriter::new(Cursor::new(buf));

    let options = zip::write::FileOptions::default()
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

pub fn process_nested_result<T, E1, E2>(result: Result<Result<T, E1>, E2>, request_path: &str) -> Response<Body>
where
    T: TryInto<Response<Body>>,
    T::Error: Debug,
    E1: Debug,
    E2: Debug
{
    match result {
        Ok(Ok(t)) => match t.try_into() {
            Ok(success_reponse) => success_reponse,
            Err(e) => make_err_response(e, request_path)
        },
        Ok(Err(e)) => make_err_response(e, request_path),
        Err(e) => make_err_response(e, request_path)
    }
}

pub fn make_err_response<E: Debug>(err: E, request_path: &str) -> Response<Body> {
    warn!("Error processing asset at '{}': {:?}", request_path, err);
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
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
        headers.insert("Content-Disposition", HeaderValue::from_str("attachment; filename=sprite.zip")?);
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
