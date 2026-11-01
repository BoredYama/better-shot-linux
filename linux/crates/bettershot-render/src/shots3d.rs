// SPDX-License-Identifier: AGPL-3.0-only
// Adapts Cap 3D rendering, Copyright (c) 2023-present Cap Software, Inc.
// Swift/Metal adaptation Copyright (c) 2026 Kartik Labhshetwar.
// Rust port for the Linux app follows that adaptation.
// See Resources/Licenses/Cap.txt for the full license.
//
//! Port of `Recording3DBlurRenderer.swift` for the CPU: the inverse
//! perspective warp that keeps the camera fixed even when a steeply tilted
//! plane crosses its horizon, and the focus blur (a separable Gaussian or a
//! three-ring bokeh disc) whose strength is specified at 1080p and scales
//! with the canvas. The Metal kernels' math is kept line for line; pixels
//! are premultiplied RGBA floats in a top-left-origin canvas.

use bettershot_core::recording::shots3d::BlurMode;
use bettershot_core::recording::{Blur3D, Projection};
use image::RgbaImage;

/// A canvas of premultiplied RGBA floats, the working format of the warp
/// and blur passes.
#[derive(Debug, Clone, PartialEq)]
pub struct FloatImage {
    pub width: usize,
    pub height: usize,
    /// Row-major, four floats per pixel, premultiplied, 0...1.
    pub data: Vec<f32>,
}

impl FloatImage {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0.0; width * height * 4] }
    }

    pub fn from_rgba(image: &RgbaImage) -> Self {
        let (width, height) = (image.width() as usize, image.height() as usize);
        let mut data = Vec::with_capacity(width * height * 4);
        for pixel in image.pixels() {
            let a = f32::from(pixel[3]) / 255.0;
            data.push(f32::from(pixel[0]) / 255.0 * a);
            data.push(f32::from(pixel[1]) / 255.0 * a);
            data.push(f32::from(pixel[2]) / 255.0 * a);
            data.push(a);
        }
        Self { width, height, data }
    }

    pub fn to_rgba(&self) -> RgbaImage {
        let mut image = RgbaImage::new(self.width as u32, self.height as u32);
        for (pixel, chunk) in image.pixels_mut().zip(self.data.chunks_exact(4)) {
            let a = chunk[3].clamp(0.0, 1.0);
            let un = |c: f32| if a <= 0.0 { 0u8 } else { ((c / a).clamp(0.0, 1.0) * 255.0).round() as u8 };
            pixel.0 = [un(chunk[0]), un(chunk[1]), un(chunk[2]), (a * 255.0).round() as u8];
        }
        image
    }

    /// A 2×2 box-filtered half-size copy.
    fn downsampled(&self) -> Self {
        let (width, height) = (self.width / 2, self.height / 2);
        let mut out = Self::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let (a, b, c, d) = (
                    self.get(2 * x, 2 * y),
                    self.get(2 * x + 1, 2 * y),
                    self.get(2 * x, 2 * y + 1),
                    self.get(2 * x + 1, 2 * y + 1),
                );
                let i = (y * width + x) * 4;
                for ch in 0..4 {
                    out.data[i + ch] = (a[ch] + b[ch] + c[ch] + d[ch]) * 0.25;
                }
            }
        }
        out
    }

    /// Bilinear upsample to `width` × `height`.
    fn upsampled(&self, width: usize, height: usize) -> Self {
        let mut out = Self::new(width, height);
        let (sx, sy) = (self.width as f32 / width as f32, self.height as f32 / height as f32);
        parallel_rows(width, height, &mut out.data, |y, row| {
            for x in 0..width {
                let sample = self.sample((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
                row[x * 4..x * 4 + 4].copy_from_slice(&sample);
            }
        });
        out
    }

    #[inline]
    fn get(&self, x: usize, y: usize) -> [f32; 4] {
        let i = (y * self.width + x) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    /// Bilinear sample at a pixel coordinate, clamped to the edges like a
    /// clamp-to-extent sampler.
    fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let fx = (x - 0.5).clamp(0.0, (self.width - 1) as f32);
        let fy = (y - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let (a, b, c, d) = (self.get(x0, y0), self.get(x1, y0), self.get(x0, y1), self.get(x1, y1));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            out[i] = top + (bottom - top) * ty;
        }
        out
    }
}

/// Runs `fill(y, row)` for every row on all cores, writing each row's
/// pixels (four floats each) in place.
fn parallel_rows(width: usize, height: usize, out: &mut [f32], fill: impl Fn(usize, &mut [f32]) + Sync) {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 16);
    let rows_per_chunk = height.div_ceil(threads).max(1);
    let row_len = width * 4;
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in out.chunks_mut(rows_per_chunk * row_len).enumerate() {
            let fill = &fill;
            scope.spawn(move || {
                for (offset, row) in chunk.chunks_mut(row_len).enumerate() {
                    fill(chunk_index * rows_per_chunk + offset, row);
                }
            });
        }
    });
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `bettershotPerspective`: each destination pixel finds its source through
/// the inverse rows; rays behind the camera are clipped, and the plane's
/// edges are anti-aliased over one source pixel. Returns a transparent
/// canvas for a degenerate projection.
pub fn warp(image: &FloatImage, projection: &Projection) -> FloatImage {
    let (width, height) = (image.width, image.height);
    let mut out = FloatImage::new(width, height);
    let Some(rows) = projection.inverse_rows() else { return out };
    let (w, h) = (width as f32, height as f32);
    let r: Vec<[f32; 3]> = rows.iter().map(|row| [row[0] as f32, row[1] as f32, row[2] as f32]).collect();
    let r = &r;
    parallel_rows(width, height, &mut out.data, |y, row| {
        for x in 0..width {
            // Pixel centres, as the Metal destination coordinates are.
            let p = [x as f32 + 0.5, y as f32 + 0.5, 1.0];
            let q = [
                r[0][0] * p[0] + r[0][1] * p[1] + r[0][2],
                r[1][0] * p[0] + r[1][1] * p[1] + r[1][2],
                r[2][0] * p[0] + r[2][1] * p[1] + r[2][2],
            ];
            if q[2] <= 1e-6 {
                continue;
            }
            let source = [q[0] / q[2], q[1] / q[2]];
            let uv = [source[0] / w, source[1] / h];
            let qz2 = q[2] * q[2];
            let dx = [(r[0][0] * q[2] - q[0] * r[2][0]) / qz2 / w, (r[1][0] * q[2] - q[1] * r[2][0]) / qz2 / h];
            let dy = [(r[0][1] * q[2] - q[0] * r[2][1]) / qz2 / w, (r[1][1] * q[2] - q[1] * r[2][1]) / qz2 / h];
            let width_x = (dx[0].abs() + dy[0].abs()).max(1e-6);
            let width_y = (dx[1].abs() + dy[1].abs()).max(1e-6);
            let edge_x = smoothstep(0.0, width_x, uv[0]) * smoothstep(0.0, width_x, 1.0 - uv[0]);
            let edge_y = smoothstep(0.0, width_y, uv[1]) * smoothstep(0.0, width_y, 1.0 - uv[1]);
            let coverage = edge_x * edge_y;
            if coverage <= 0.0 {
                continue;
            }
            let sample = image.sample(source[0].clamp(0.0, w), source[1].clamp(0.0, h));
            for c in 0..4 {
                row[x * 4 + c] = sample[c] * coverage;
            }
        }
    });
    out
}

/// `focusRadius`: the blur radius at a pixel for the settings' mode, with
/// the focus region sharp and the falloff shaping the fade.
fn focus_radius(x: f32, y: f32, frame: (f32, f32), blur: &Blur3D, radius: f32) -> f32 {
    let (uvx, uvy) = (x / frame.0, y / frame.1);
    let aspect = frame.0 / frame.1;
    let edge = blur.focus_size as f32 * 0.5;
    let falloff = blur.falloff as f32;
    let angle = (blur.angle * std::f64::consts::PI / 180.0) as f32;
    let mut spread = (edge + 0.35) * (1.0 + 3.0 * falloff);
    let distance = match blur.mode {
        BlurMode::Directional => {
            let delta = ((uvx - 0.5) * aspect, uvy - 0.5);
            let extent = (aspect + 1.0) * 0.5;
            spread = 0.7 * (1.0 + 3.0 * falloff);
            let position = blur.position as f32;
            delta.0 * angle.cos() + delta.1 * angle.sin() - (-extent + (extent - -extent) * position)
        }
        BlurMode::TiltShift => {
            let delta = ((uvx - blur.focus_x as f32) * aspect, uvy - blur.focus_y as f32);
            ((delta.0 * angle.sin() + delta.1 * angle.cos()).abs() - edge).max(0.0) - edge
        }
        _ => {
            let delta = ((uvx - blur.focus_x as f32) * aspect, uvy - blur.focus_y as f32);
            (delta.0 * delta.0 + delta.1 * delta.1).sqrt() - edge
        }
    };
    let fade = smoothstep(0.0, spread.max(0.0001), distance);
    radius * fade.powf(2.0 + (0.7 - 2.0) * falloff)
}

/// Applies the depth blur to a canvas (`Recording3DBlurRenderer.apply`):
/// nothing while inactive, a two-pass Gaussian, or one bokeh pass.
pub fn apply_blur(image: &FloatImage, settings: &Blur3D) -> FloatImage {
    if !settings.is_active() {
        return image.clone();
    }
    let blur = settings.sanitized();
    if !blur.is_active() {
        return image.clone();
    }
    // Large canvases blur at half resolution (the blurred regions cannot
    // show the lost detail) and keep the sharp focus from the full picture.
    if image.width * image.height >= 200_000 && image.width >= 4 && image.height >= 4 {
        let half = image.downsampled();
        let blurred_half = blur_passes(&half, &blur);
        let upsampled = blurred_half.upsampled(image.width, image.height);
        let frame = (image.width as f32, image.height as f32);
        let radius = (blur.strength * f64::from(image.height as u32) / 1080.0) as f32;
        let mut out = image.clone();
        let width = image.width;
        parallel_rows(width, image.height, &mut out.data, |y, row| {
            for x in 0..width {
                let r = focus_radius(x as f32 + 0.5, y as f32 + 0.5, frame, &blur, radius);
                if r < 0.5 {
                    continue;
                }
                // Fade from the sharp picture into the blurred one over a pixel.
                let mix = ((r - 0.5) / 1.0).clamp(0.0, 1.0);
                let i = (y * width + x) * 4;
                for c in 0..4 {
                    row[x * 4 + c] = image.data[i + c] + (upsampled.data[i + c] - image.data[i + c]) * mix;
                }
            }
        });
        return out;
    }
    blur_passes(image, &blur)
}

fn blur_passes(image: &FloatImage, blur: &Blur3D) -> FloatImage {
    let frame = (image.width as f32, image.height as f32);
    let radius = (blur.strength * f64::from(image.height as u32) / 1080.0) as f32;
    if blur.bokeh {
        return bokeh_pass(image, blur, frame, radius);
    }
    let horizontal = gaussian_pass(image, blur, frame, radius, true);
    gaussian_pass(&horizontal, blur, frame, radius, false)
}

fn gaussian_pass(image: &FloatImage, blur: &Blur3D, frame: (f32, f32), radius: f32, horizontal: bool) -> FloatImage {
    let mut out = image.clone();
    let width = image.width;
    parallel_rows(width, image.height, &mut out.data, |y, row| {
        for x in 0..width {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = focus_radius(px, py, frame, blur, radius);
            if r < 0.5 {
                continue;
            }
            let extent = r.min(frame.1 * (40.0 / 1080.0)).min(160.0) as i32;
            let sigma = r * 0.5;
            let mut sum = image.sample(px, py);
            let mut total = 1.0;
            let mut i = 1;
            while i <= extent {
                let a = (-((i * i) as f32) / (2.0 * sigma * sigma)).exp();
                let b = if i < extent { (-(((i + 1) * (i + 1)) as f32) / (2.0 * sigma * sigma)).exp() } else { 0.0 };
                let weight = a + b;
                let offset = i as f32 + b / weight;
                let (ox, oy) = if horizontal { (offset, 0.0) } else { (0.0, offset) };
                let forward = image.sample(px + ox, py + oy);
                let backward = image.sample(px - ox, py - oy);
                for c in 0..4 {
                    sum[c] += (forward[c] + backward[c]) * weight;
                }
                total += 2.0 * weight;
                i += 2;
            }
            for c in 0..4 {
                row[x * 4 + c] = sum[c] / total;
            }
        }
    });
    out
}

fn bokeh_pass(image: &FloatImage, blur: &Blur3D, frame: (f32, f32), radius: f32) -> FloatImage {
    let mut out = image.clone();
    let width = image.width;
    // The ring offsets are the same for every pixel; only the radius scales them.
    let taps: Vec<(f32, f32, f32)> = (1..=3)
        .flat_map(|ring| {
            let count = ring * 5;
            let ring_weight = 1.0 + (ring as f32 / 3.0 - 1.0) * 0.3;
            (0..count).map(move |j| {
                let angle = std::f32::consts::TAU * j as f32 / count as f32;
                (angle.cos() * ring as f32 / 3.0, angle.sin() * ring as f32 / 3.0, ring_weight)
            })
        })
        .collect();
    let taps = &taps;
    parallel_rows(width, image.height, &mut out.data, |y, row| {
        for x in 0..width {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = focus_radius(px, py, frame, blur, radius);
            if r < 0.5 {
                continue;
            }
            let mut sum = image.sample(px, py);
            let mut total = 1.0;
            for (dx, dy, ring_weight) in taps {
                let color = image.sample(px + dx * r, py + dy * r);
                let light = color[0] * 0.299 + color[1] * 0.587 + color[2] * 0.114;
                let weight = ring_weight * (1.0 + 1.5 * smoothstep(0.7, 1.0, light));
                for c in 0..4 {
                    sum[c] += color[c] * weight;
                }
                total += weight;
            }
            for c in 0..4 {
                row[x * 4 + c] = sum[c] / total;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::recording::{Pose3D, Preset3D};

    fn checker(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let on = (x / 8 + y / 8) % 2 == 0;
            pixel.0 = if on { [255, 255, 255, 255] } else { [0, 0, 0, 255] };
        }
        image
    }

    #[test]
    fn identity_warp_keeps_the_picture() {
        let source = FloatImage::from_rgba(&checker(64, 48));
        let warped = warp(&source, &Projection::IDENTITY).to_rgba();
        let original = checker(64, 48);
        let mut mismatches = 0;
        for (a, b) in warped.pixels().zip(original.pixels()) {
            if (i32::from(a[0]) - i32::from(b[0])).abs() > 2 || a[3] < 250 {
                mismatches += 1;
            }
        }
        // Only the one-pixel anti-aliased border may differ.
        assert!(mismatches <= 64 * 2 + 48 * 2, "{mismatches} mismatches");
    }

    #[test]
    fn a_camera_shot_leaves_transparent_padding_and_keeps_the_centre() {
        let source = FloatImage::from_rgba(&checker(192, 108));
        let projection = Preset3D::Glide.poses().0.projection((192.0, 108.0));
        let warped = warp(&source, &projection).to_rgba();
        let transparent = warped.pixels().filter(|p| p[3] == 0).count();
        assert!(transparent > 500, "a tilted plane leaves uncovered canvas: {transparent}");
        let (cx, cy) = projection.project(96.0, 54.0);
        let centre = warped.get_pixel(cx.round().clamp(0.0, 191.0) as u32, cy.round().clamp(0.0, 107.0) as u32);
        assert!(centre[3] > 200, "the plane's centre lands where the projection says: {centre:?}");
        let behind = Projection { m44: 0.0, m14: 0.0, m24: 0.0, ..Projection::IDENTITY };
        assert!(warp(&source, &behind).data.iter().all(|v| *v == 0.0), "a degenerate projection is empty");
        let flat = Pose3D::identity().projection((192.0, 108.0));
        assert!(flat.is_identity());
    }

    #[test]
    fn focus_blur_softens_only_outside_the_focus() {
        let source = FloatImage::from_rgba(&checker(480, 270));
        let blur = Blur3D {
            mode: BlurMode::Radial,
            strength: 40.0,
            focus_x: 0.5,
            focus_y: 0.5,
            focus_size: 0.3,
            falloff: 0.2,
            ..Blur3D::default()
        };
        let blurred = apply_blur(&source, &blur).to_rgba();
        let centre = blurred.get_pixel(240, 135);
        assert!(centre[0] == 255 || centre[0] == 0, "the focus stays sharp: {centre:?}");
        // Where four checker blocks meet near the corner, the blur averages them.
        let corner = blurred.get_pixel(16, 8);
        assert!(corner[0] > 40 && corner[0] < 215, "the corner is averaged: {corner:?}");
        let bokeh = apply_blur(&source, &Blur3D { bokeh: true, strength: 20.0, ..blur.clone() }).to_rgba();
        let edge = bokeh.get_pixel(16, 8);
        assert!(edge[0] > 40 && edge[0] < 215, "bokeh averages too: {edge:?}");
        let off = apply_blur(&source, &Blur3D::none());
        assert_eq!(off, source);
    }
}

#[cfg(test)]
mod timing {
    use super::*;
    use bettershot_core::recording::{Preset3D, Shot3D, Timeline3D};

    #[test]
    fn report_pass_timing() {
        let mut image = RgbaImage::new(1280, 800);
        for (x, y, p) in image.enumerate_pixels_mut() {
            p.0 = [(x % 256) as u8, (y % 256) as u8, 128, 255];
        }
        let float = FloatImage::from_rgba(&image);
        let mut shot = Shot3D::new(0.0, 4.0);
        shot.apply(Preset3D::Glide);
        let timeline = Timeline3D::new(&[shot], 4.0);
        let pose = timeline.pose(2.0);
        let projection = pose.projection((1280.0, 800.0));
        let started = std::time::Instant::now();
        let warped = warp(&float, &projection);
        let warp_ms = started.elapsed().as_millis();
        let started = std::time::Instant::now();
        let _ = apply_blur(&warped, &timeline.defocus(2.0));
        let blur_ms = started.elapsed().as_millis();
        let started = std::time::Instant::now();
        let _ = FloatImage::from_rgba(&warped.to_rgba());
        let convert_ms = started.elapsed().as_millis();
        eprintln!("TIMING warp {warp_ms} ms, blur {blur_ms} ms, convert {convert_ms} ms");
    }
}
