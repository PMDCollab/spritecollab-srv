use image::{Rgba, RgbaImage};
use std::io::Cursor;

pub fn to_png(img: RgbaImage) -> Result<Vec<u8>, anyhow::Error> {
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageOutputFormat::Png)?;
    Ok(png)
}

pub fn add_palette_to(img: &mut RgbaImage) {
    let mut palette: Vec<Rgba<u8>> = Vec::with_capacity(32);
    for px in img.pixels() {
        if px.0[3] == 0 {
            continue;
        }
        if !palette.contains(px) {
            palette.push(*px);
        }
    }
    for (x, px) in palette.into_iter().enumerate() {
        img.put_pixel(x as u32, 0, px);
    }
}
