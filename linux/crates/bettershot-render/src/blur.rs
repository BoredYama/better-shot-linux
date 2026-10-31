//! Gaussian blur approximated by three box blurs (sigma-matched), with edge
//! pixels extended past the borders like Core Image's `clampedToExtent`.

use image::RgbaImage;

/// Box sizes whose three passes approximate a Gaussian with `sigma`.
fn box_radii(sigma: f64) -> [usize; 3] {
    let n = 3.0;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut lower = ideal.floor() as i64;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let ideal_m = (12.0 * sigma * sigma - n * (lower * lower) as f64 - 4.0 * n * lower as f64 - 3.0 * n)
        / (-4.0 * lower as f64 - 4.0);
    let m = ideal_m.round() as i64;
    let size = |i: i64| -> usize {
        let width = if i < m { lower } else { upper };
        ((width.max(1) - 1) / 2) as usize
    };
    [size(0), size(1), size(2)]
}

/// One horizontal box pass over `channels` interleaved channels.
fn box_pass(
    src: &[f32],
    dst: &mut [f32],
    width: usize,
    height: usize,
    channels: usize,
    radius: usize,
    horizontal: bool,
) {
    if radius == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let (outer, inner) = if horizontal { (height, width) } else { (width, height) };
    let at = |o: usize, i: usize| if horizontal { (o * width + i) * channels } else { (i * width + o) * channels };
    let scale = 1.0 / (2 * radius + 1) as f32;
    let r = radius as isize;
    for o in 0..outer {
        for c in 0..channels {
            let sample = |i: isize| src[at(o, i.clamp(0, inner as isize - 1) as usize) + c];
            let mut sum: f32 = (-r..=r).map(sample).sum();
            for i in 0..inner {
                dst[at(o, i) + c] = sum * scale;
                sum += sample(i as isize + r + 1) - sample(i as isize - r);
            }
        }
    }
}

/// Blurs `data` (`channels` interleaved f32 channels) in place.
pub fn gaussian(data: &mut [f32], width: usize, height: usize, channels: usize, sigma: f64) {
    if sigma < 0.3 || width == 0 || height == 0 {
        return;
    }
    let mut scratch = vec![0.0; data.len()];
    for radius in box_radii(sigma) {
        box_pass(data, &mut scratch, width, height, channels, radius, true);
        box_pass(&scratch, data, width, height, channels, radius, false);
    }
}

/// A blurred copy of an RGBA image, blurring premultiplied color so
/// transparent pixels do not darken their neighbors.
pub fn blur_rgba(image: &RgbaImage, sigma: f64) -> RgbaImage {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let mut data: Vec<f32> = Vec::with_capacity(width * height * 4);
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0.map(f32::from);
        let alpha = a / 255.0;
        data.extend_from_slice(&[r * alpha, g * alpha, b * alpha, a]);
    }
    gaussian(&mut data, width, height, 4, sigma);
    let mut out = RgbaImage::new(width as u32, height as u32);
    for (pixel, chunk) in out.pixels_mut().zip(data.chunks_exact(4)) {
        let a = chunk[3];
        let alpha = (a / 255.0).max(1e-6);
        let channel = |v: f32| (v / alpha).round().clamp(0.0, 255.0) as u8;
        pixel.0 = [channel(chunk[0]), channel(chunk[1]), channel(chunk[2]), a.round().clamp(0.0, 255.0) as u8];
    }
    out
}

/// Pixelates by averaging `block`-sized cells.
pub fn pixelate(image: &RgbaImage, block: f64) -> RgbaImage {
    let (width, height) = image.dimensions();
    let low_w = ((f64::from(width) / block) as u32).max(1);
    let low_h = ((f64::from(height) / block) as u32).max(1);
    let low = image::imageops::resize(image, low_w, low_h, image::imageops::FilterType::Triangle);
    image::imageops::resize(&low, width, height, image::imageops::FilterType::Nearest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn blur_spreads_and_preserves_flat_areas() {
        let mut image = RgbaImage::from_pixel(41, 41, Rgba([0, 0, 0, 255]));
        image.put_pixel(20, 20, Rgba([255, 255, 255, 255]));
        let blurred = blur_rgba(&image, 3.0);
        let center = blurred.get_pixel(20, 20).0[0];
        let near = blurred.get_pixel(23, 20).0[0];
        assert!(center > near && near > 0 && center < 255, "{center} {near}");
        assert_eq!(blurred.get_pixel(0, 0).0, [0, 0, 0, 255], "flat areas stay flat, with clamped edges");
        let flat = blur_rgba(&RgbaImage::from_pixel(8, 8, Rgba([10, 200, 30, 255])), 5.0);
        assert!(flat.pixels().all(|p| p.0 == [10, 200, 30, 255]));
    }

    #[test]
    fn pixelate_makes_uniform_blocks() {
        let image = RgbaImage::from_fn(40, 40, |x, y| Rgba([(x * 6) as u8, (y * 6) as u8, 0, 255]));
        let out = pixelate(&image, 10.0);
        assert_eq!(out.get_pixel(0, 0), out.get_pixel(9, 9));
        assert_ne!(out.get_pixel(0, 0), out.get_pixel(10, 10));
    }
}
