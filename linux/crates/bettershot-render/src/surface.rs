//! Converting between `image::RgbaImage` (straight alpha, RGBA bytes) and
//! cairo's ARGB32 surfaces (premultiplied, native-endian BGRA on little-endian).

use cairo::{Format, ImageSurface};
use image::RgbaImage;

pub fn to_surface(image: &RgbaImage) -> Result<ImageSurface, cairo::Error> {
    let (width, height) = image.dimensions();
    let mut surface = ImageSurface::create(Format::ARgb32, width as i32, height as i32)?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface.data().map_err(|_| cairo::Error::SurfaceFinished)?;
        for (y, row) in image.rows().enumerate() {
            let line = &mut data[y * stride..y * stride + width as usize * 4];
            for (pixel, out) in row.zip(line.chunks_exact_mut(4)) {
                let [r, g, b, a] = pixel.0;
                let premultiply = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                let argb: u32 = (u32::from(a) << 24)
                    | (u32::from(premultiply(r)) << 16)
                    | (u32::from(premultiply(g)) << 8)
                    | u32::from(premultiply(b));
                out.copy_from_slice(&argb.to_ne_bytes());
            }
        }
    }
    surface.mark_dirty();
    Ok(surface)
}

pub fn from_surface(surface: &mut ImageSurface) -> Result<RgbaImage, cairo::Error> {
    surface.flush();
    let width = surface.width() as u32;
    let height = surface.height() as u32;
    let stride = surface.stride() as usize;
    let data = surface.data().map_err(|_| cairo::Error::SurfaceFinished)?;
    let mut image = RgbaImage::new(width, height);
    for (y, row) in image.rows_mut().enumerate() {
        let line = &data[y * stride..y * stride + width as usize * 4];
        for (pixel, chunk) in row.zip(line.chunks_exact(4)) {
            let argb = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let a = (argb >> 24) as u8;
            let unpremultiply = |c: u32| -> u8 {
                if a == 0 {
                    0
                } else {
                    ((c * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
                }
            };
            pixel.0 =
                [unpremultiply((argb >> 16) & 0xff), unpremultiply((argb >> 8) & 0xff), unpremultiply(argb & 0xff), a];
        }
    }
    Ok(image)
}
