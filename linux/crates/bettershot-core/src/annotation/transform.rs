//! Port of `AnnotationImageTransform.swift` and the crop step: exact
//! quarter-turns, reflections, and crops of the base image, with every shape
//! moved to stay on the pixels it annotates.

use super::geometry::{Mat, Vec2};
use super::shape::AnnoShape;
use image::RgbaImage;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageTransform {
    RotateLeft,
    RotateRight,
    FlipHorizontal,
    FlipVertical,
}

impl ImageTransform {
    pub const ALL: [ImageTransform; 4] = [
        ImageTransform::RotateLeft,
        ImageTransform::RotateRight,
        ImageTransform::FlipHorizontal,
        ImageTransform::FlipVertical,
    ];

    pub fn title(self) -> &'static str {
        match self {
            ImageTransform::RotateLeft => "Rotate Left",
            ImageTransform::RotateRight => "Rotate Right",
            ImageTransform::FlipHorizontal => "Flip Horizontal",
            ImageTransform::FlipVertical => "Flip Vertical",
        }
    }

    pub fn shortcut_action(self) -> crate::shortcuts::Action {
        use crate::shortcuts::Action;
        match self {
            ImageTransform::RotateLeft => Action::ImageRotateLeft,
            ImageTransform::RotateRight => Action::ImageRotateRight,
            ImageTransform::FlipHorizontal => Action::ImageFlipHorizontal,
            ImageTransform::FlipVertical => Action::ImageFlipVertical,
        }
    }

    pub fn pixel_size(self, width: f64, height: f64) -> (f64, f64) {
        match self {
            ImageTransform::RotateLeft | ImageTransform::RotateRight => (height, width),
            _ => (width, height),
        }
    }

    /// Maps old page coordinates to new ones for an image of `width` × `height`.
    pub fn matrix(self, width: f64, height: f64) -> Mat {
        match self {
            ImageTransform::RotateLeft => Mat::new(0.0, -1.0, 1.0, 0.0, 0.0, width),
            ImageTransform::RotateRight => Mat::new(0.0, 1.0, -1.0, 0.0, height, 0.0),
            ImageTransform::FlipHorizontal => Mat::new(-1.0, 0.0, 0.0, 1.0, width, 0.0),
            ImageTransform::FlipVertical => Mat::new(1.0, 0.0, 0.0, -1.0, 0.0, height),
        }
    }

    pub fn apply_to_shape(self, shape: &AnnoShape, width: f64, height: f64) -> AnnoShape {
        let mut result = shape.clone();
        let origin = self.matrix(width, height).apply(Vec2::new(shape.x, shape.y));
        result.x = origin.x;
        result.y = origin.y;
        let toggled = if shape.is_mirrored() { None } else { Some(true) };
        match self {
            ImageTransform::RotateLeft => result.rotation -= FRAC_PI_2,
            ImageTransform::RotateRight => result.rotation += FRAC_PI_2,
            ImageTransform::FlipHorizontal => {
                result.rotation = -shape.rotation;
                result.mirrored = toggled;
            }
            ImageTransform::FlipVertical => {
                result.rotation = PI - shape.rotation;
                result.mirrored = toggled;
            }
        }
        result.rotation %= TAU;
        result
    }

    /// The transformed pixels; exact, with no resampling.
    pub fn apply_to_image(self, image: &RgbaImage) -> RgbaImage {
        match self {
            ImageTransform::RotateLeft => image::imageops::rotate270(image),
            ImageTransform::RotateRight => image::imageops::rotate90(image),
            ImageTransform::FlipHorizontal => image::imageops::flip_horizontal(image),
            ImageTransform::FlipVertical => image::imageops::flip_vertical(image),
        }
    }
}

/// Crops the base image to whole pixels and moves shapes with it.
pub fn crop(
    image: &RgbaImage,
    shapes: &[AnnoShape],
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> (RgbaImage, Vec<AnnoShape>) {
    let x = x.min(image.width().saturating_sub(1));
    let y = y.min(image.height().saturating_sub(1));
    let width = width.clamp(1, image.width() - x);
    let height = height.clamp(1, image.height() - y);
    let cropped = image::imageops::crop_imm(image, x, y, width, height).to_image();
    let moved = shapes
        .iter()
        .map(|shape| {
            let mut shape = shape.clone();
            shape.x -= f64::from(x);
            shape.y -= f64::from(y);
            shape
        })
        .collect();
    (cropped, moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::shape::{NumberedProps, ShapeKind};
    use image::Rgba;

    fn marked(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 255]));
        image.put_pixel(1, 0, Rgba([255, 0, 0, 255]));
        image
    }

    /// Shapes must land on the same pixels as the image they annotate.
    #[test]
    fn transforms_move_pixels_and_shapes_together() {
        let (w, h) = (4.0, 3.0);
        let shape = AnnoShape::new(1.0, 0.0, ShapeKind::Numbered(NumberedProps::default()));
        for transform in ImageTransform::ALL {
            let image = transform.apply_to_image(&marked(4, 3));
            let moved = transform.apply_to_shape(&shape, w, h);
            let (nw, nh) = transform.pixel_size(w, h);
            assert_eq!((f64::from(image.width()), f64::from(image.height())), (nw, nh), "{transform:?}");
            // The red pixel's top-left corner maps to the shape's new origin, up to the pixel's own extent.
            let red =
                image.enumerate_pixels().find(|(_, _, p)| p.0 == [255, 0, 0, 255]).map(|(x, y, _)| (x, y)).unwrap();
            let (rx, ry) = (f64::from(red.0), f64::from(red.1));
            assert!((moved.x - rx).abs() <= 1.0 && (moved.y - ry).abs() <= 1.0, "{transform:?}: {moved:?} vs {red:?}");
        }
        let twice = ImageTransform::FlipHorizontal.apply_to_shape(
            &ImageTransform::FlipHorizontal.apply_to_shape(&shape, w, h),
            w,
            h,
        );
        assert_eq!((twice.x, twice.y, twice.mirrored), (shape.x, shape.y, None), "flipping twice restores the shape");
        let round =
            ImageTransform::RotateLeft.apply_to_shape(&ImageTransform::RotateRight.apply_to_shape(&shape, w, h), h, w);
        assert!((round.x - shape.x).abs() < 1e-9 && (round.y - shape.y).abs() < 1e-9 && round.rotation.abs() < 1e-9);
    }

    #[test]
    fn crops_move_shapes() {
        let shape = AnnoShape::new(10.0, 20.0, ShapeKind::Numbered(NumberedProps::default()));
        let (image, shapes) = crop(&RgbaImage::new(100, 100), &[shape], 5, 8, 50, 500);
        assert_eq!(image.dimensions(), (50, 92), "crops clamp to the image");
        assert_eq!((shapes[0].x, shapes[0].y), (5.0, 12.0));
    }
}
