//! Image files for the capture pipeline: cropping a frozen frame, encoding in
//! the export format, thumbnails, and writes that never leave a partial file
//! or overwrite another capture.
//!
//! Ports `FrozenScreenFrame.swift`, `CaptureOrchestrator.saveImage`, and the
//! export helpers in `ScreenshotFileActions` (`BetterShotPreferences.swift`).
//! PNG stays lossless, and a PNG source saved as PNG is copied byte for byte.

use crate::file_naming;
use crate::geometry::Rect;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ImageEncoder, ImageFormat, RgbaImage};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug)]
pub enum ImagingError {
    Io(io::Error),
    Image(image::ImageError),
    /// The selection is not inside the captured frame.
    OutsideFrame,
    /// No free name was found for the file.
    NoFreeName,
}

impl std::fmt::Display for ImagingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Image(error) => write!(f, "{error}"),
            Self::OutsideFrame => write!(f, "The selected area is outside the captured display. Try capturing again."),
            Self::NoFreeName => write!(f, "No free file name was available. Try again."),
        }
    }
}

impl std::error::Error for ImagingError {}

impl From<io::Error> for ImagingError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<image::ImageError> for ImagingError {
    fn from(error: image::ImageError) -> Self {
        Self::Image(error)
    }
}

pub type Result<T> = std::result::Result<T, ImagingError>;

/// The deliverable formats. JPEG uses the `jpg` extension, as on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportFormat {
    #[default]
    Png,
    Jpeg,
}

impl ExportFormat {
    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        match raw {
            "png" => Some(Self::Png),
            "jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }

    pub fn file_extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }

    /// The format a path's extension names, if it is one we write.
    pub fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
        }
    }
}

/// Encoding choices for a deliverable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncodeOptions {
    pub format: ExportFormat,
    /// JPEG quality, 0.1...1.
    pub quality: f64,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self { format: ExportFormat::Png, quality: 0.9 }
    }
}

/// A full-display frame captured before selection. The displayed frame is also
/// the export source; selecting never takes a second shot.
#[derive(Debug, Clone)]
pub struct FrozenFrame {
    /// The frame's area in the logical (point) coordinate space the selector uses.
    pub points_rect: Rect,
    pub image: RgbaImage,
}

impl FrozenFrame {
    /// The pixels under `region`, a rect in the same space as `points_rect`.
    pub fn cropped(&self, region: Rect) -> Result<RgbaImage> {
        let frame = self.points_rect;
        let inside = region.min_x() >= frame.min_x()
            && region.min_y() >= frame.min_y()
            && region.max_x() <= frame.max_x()
            && region.max_y() <= frame.max_y();
        if frame.width <= 0.0 || frame.height <= 0.0 || region.width <= 0.0 || region.height <= 0.0 || !inside {
            return Err(ImagingError::OutsideFrame);
        }
        let sx = f64::from(self.image.width()) / frame.width;
        let sy = f64::from(self.image.height()) / frame.height;
        // `CGRect.integral`: the smallest whole-pixel rect containing the region.
        let x0 = ((region.min_x() - frame.min_x()) * sx).floor().max(0.0);
        let y0 = ((region.min_y() - frame.min_y()) * sy).floor().max(0.0);
        let x1 = ((region.max_x() - frame.min_x()) * sx).ceil().min(f64::from(self.image.width()));
        let y1 = ((region.max_y() - frame.min_y()) * sy).ceil().min(f64::from(self.image.height()));
        if x1 <= x0 || y1 <= y0 {
            return Err(ImagingError::OutsideFrame);
        }
        Ok(image::imageops::crop_imm(&self.image, x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32).to_image())
    }
}

/// Decodes an image file to RGBA.
pub fn load_rgba(path: &Path) -> Result<RgbaImage> {
    Ok(image::ImageReader::open(path)?.with_guessed_format()?.decode()?.to_rgba8())
}

/// Pixel dimensions without decoding the whole file, or (0, 0).
pub fn pixel_size(path: &Path) -> (u32, u32) {
    image::image_dimensions(path).unwrap_or((0, 0))
}

/// Whether the file's contents (not its name) are PNG.
pub fn is_png_file(path: &Path) -> bool {
    let mut header = [0u8; 8];
    File::open(path).and_then(|mut file| io::Read::read_exact(&mut file, &mut header)).is_ok()
        && header == *b"\x89PNG\r\n\x1a\n"
}

/// Encodes `image` into `writer`. JPEG has no alpha, so transparency is
/// flattened onto white.
pub fn encode(image: &RgbaImage, options: EncodeOptions, writer: impl Write) -> Result<()> {
    match options.format {
        ExportFormat::Png => {
            PngEncoder::new_with_quality(writer, CompressionType::Default, FilterType::Adaptive).write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        ExportFormat::Jpeg => {
            let quality = (options.quality.clamp(0.1, 1.0) * 100.0).round() as u8;
            let flattened = flatten_on_white(image);
            JpegEncoder::new_with_quality(writer, quality).write_image(
                flattened.as_raw(),
                flattened.width(),
                flattened.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
    }
    Ok(())
}

fn flatten_on_white(image: &RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let [r, g, b, a] = image.get_pixel(x, y).0;
        let alpha = u16::from(a);
        let blend = |channel: u8| ((u16::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        image::Rgb([blend(r), blend(g), blend(b)])
    })
}

/// Encodes to a hidden file in `directory`, then gives it `file_name`, or the
/// first free "name-1.png" when that is taken. The final link fails atomically
/// when the name exists, so two captures taken in the same second never
/// overwrite each other, and nobody sees a half-written file.
pub fn write_new_image(
    image: &RgbaImage,
    options: EncodeOptions,
    file_name: &str,
    directory: &Path,
) -> Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let staging = hidden_path(directory, options.format.file_extension());
    let result = (|| {
        let mut writer = BufWriter::new(File::create(&staging)?);
        encode(image, options, &mut writer)?;
        writer.into_inner().map_err(|error| error.into_error())?.sync_all()?;
        link_unique(&staging, file_name, directory, "-")
    })();
    let _ = fs::remove_file(&staging);
    result
}

/// Moves `source` into `directory` as `file_name` (or the next free numbered
/// name). Works across file systems.
pub fn move_unique(source: &Path, file_name: &str, directory: &Path, separator: &str) -> Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let staging = hidden_path(directory, source.extension().and_then(|e| e.to_str()).unwrap_or("tmp"));
    if fs::rename(source, &staging).is_err() {
        fs::copy(source, &staging)?;
        fs::remove_file(source)?;
    }
    let result = link_unique(&staging, file_name, directory, separator);
    let _ = fs::remove_file(&staging);
    result
}

fn hidden_path(directory: &Path, extension: &str) -> PathBuf {
    directory.join(format!(".{}.{extension}", Uuid::new_v4()))
}

/// Hard-links `staging` to a free name. `link` refuses an existing name, which
/// makes "find a free name and take it" atomic.
fn link_unique(staging: &Path, file_name: &str, directory: &Path, separator: &str) -> Result<PathBuf> {
    for _ in 0..100 {
        let candidate = file_naming::unique_path_with(file_name, directory, separator, |p| p.exists());
        match fs::hard_link(staging, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ImagingError::NoFreeName)
}

/// Writes `source` to `destination` in the destination's format, replacing it
/// only once the new file is complete, so the last good export survives a
/// failure. PNG to PNG copies bytes; anything else is re-encoded.
pub fn replace_export(source: &Path, destination: &Path, quality: f64) -> Result<()> {
    let format = ExportFormat::for_path(destination).unwrap_or(ExportFormat::Png);
    let directory = destination.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(directory)?;
    let name = destination.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let staging = directory.join(format!(".BetterShot-{}-{name}", Uuid::new_v4()));
    let result = (|| {
        if format == ExportFormat::Png && is_png_file(source) {
            fs::copy(source, &staging)?;
        } else {
            let image = load_rgba(source)?;
            let mut writer = BufWriter::new(File::create(&staging)?);
            encode(&image, EncodeOptions { format, quality }, &mut writer)?;
            writer.into_inner().map_err(|error| error.into_error())?.sync_all()?;
        }
        fs::rename(&staging, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

/// A downscaled copy whose longer edge is at most `max_edge` pixels.
pub fn thumbnail(image: &RgbaImage, max_edge: u32) -> RgbaImage {
    let longest = image.width().max(image.height());
    if longest <= max_edge || longest == 0 {
        return image.clone();
    }
    let scale = f64::from(max_edge) / f64::from(longest);
    let width = ((f64::from(image.width()) * scale).round() as u32).max(1);
    let height = ((f64::from(image.height()) * scale).round() as u32).max(1);
    DynamicImage::ImageRgba8(image.clone())
        .resize_exact(width, height, image::imageops::FilterType::Triangle)
        .to_rgba8()
}

/// Guesses a format from a path for decoding helpers.
pub fn format_for_path(path: &Path) -> Option<ImageFormat> {
    ImageFormat::from_path(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn gradient(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| Rgba([x as u8, y as u8, 7, 255]))
    }

    /// Mirrors `Tests/FrozenScreenFrameCheck.swift`: crop origins, scales, and pixels.
    #[test]
    fn frozen_frame_crops_at_native_pixels() {
        let frame = FrozenFrame { points_rect: Rect::new(100.0, 50.0, 100.0, 60.0), image: gradient(200, 120) };
        let crop = frame.cropped(Rect::new(110.0, 60.0, 20.0, 10.0)).unwrap();
        assert_eq!(crop.dimensions(), (40, 20), "2x scale doubles the region");
        assert_eq!(crop.get_pixel(0, 0).0, [20, 20, 7, 255], "origin maps through the frame offset");

        let fractional = frame.cropped(Rect::new(100.25, 50.25, 10.0, 10.0)).unwrap();
        assert_eq!(fractional.dimensions(), (21, 21), "partial pixels round outward, like CGRect.integral");

        assert!(matches!(frame.cropped(Rect::new(90.0, 60.0, 20.0, 10.0)), Err(ImagingError::OutsideFrame)));
        assert!(matches!(frame.cropped(Rect::new(110.0, 60.0, 0.0, 10.0)), Err(ImagingError::OutsideFrame)));
    }

    #[test]
    fn png_round_trips_losslessly_and_names_never_collide() {
        let directory = tempfile::tempdir().unwrap();
        let image = gradient(33, 17);
        let options = EncodeOptions::default();
        let first = write_new_image(&image, options, "shot.png", directory.path()).unwrap();
        let second = write_new_image(&image, options, "shot.png", directory.path()).unwrap();
        assert_eq!(first.file_name().unwrap(), "shot.png");
        assert_eq!(second.file_name().unwrap(), "shot-1.png");
        assert_eq!(load_rgba(&first).unwrap(), image, "PNG is lossless");
        assert!(is_png_file(&first));
        let names: Vec<_> = fs::read_dir(directory.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 2, "no hidden staging files remain: {names:?}");
    }

    #[test]
    fn exports_replace_atomically_in_the_destination_format() {
        let directory = tempfile::tempdir().unwrap();
        let source = write_new_image(&gradient(8, 8), EncodeOptions::default(), "a.png", directory.path()).unwrap();
        let png = directory.path().join("out.png");
        replace_export(&source, &png, 0.9).unwrap();
        assert_eq!(fs::read(&png).unwrap(), fs::read(&source).unwrap(), "PNG to PNG copies bytes");

        let jpeg = directory.path().join("out.jpg");
        fs::write(&jpeg, b"old").unwrap();
        replace_export(&source, &jpeg, 0.8).unwrap();
        assert_eq!(
            image::ImageReader::open(&jpeg).unwrap().with_guessed_format().unwrap().format(),
            Some(ImageFormat::Jpeg)
        );

        let missing = directory.path().join("missing.png");
        assert!(replace_export(&missing, &png, 0.9).is_err());
        assert_eq!(fs::read(&png).unwrap(), fs::read(&source).unwrap(), "a failed replace keeps the last good export");
    }

    #[test]
    fn move_unique_and_thumbnails() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("portal.png");
        fs::write(&source, b"x").unwrap();
        let target = directory.path().join("deck");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("raw.png"), b"taken").unwrap();
        let moved = move_unique(&source, "raw.png", &target, "-").unwrap();
        assert_eq!(moved.file_name().unwrap(), "raw-1.png");
        assert!(!source.exists());

        let thumb = thumbnail(&gradient(400, 100), 200);
        assert_eq!(thumb.dimensions(), (200, 50));
        assert_eq!(thumbnail(&gradient(10, 10), 200).dimensions(), (10, 10));
    }

    #[test]
    fn jpeg_flattens_transparency_on_white() {
        let mut clear = RgbaImage::new(2, 2);
        clear.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        let mut bytes = Vec::new();
        encode(&clear, EncodeOptions { format: ExportFormat::Jpeg, quality: 1.0 }, &mut bytes).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert!(decoded.get_pixel(0, 0).0.iter().all(|c| *c > 240), "{:?}", decoded.get_pixel(0, 0));
    }
}
