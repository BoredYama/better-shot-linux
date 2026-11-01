//! The image editor's document model: engine shapes, arrow bindings, the
//! background ("beautifier") settings, and the `.bettershot` sidecar.
//!
//! Ports `Shape.swift`, `AnnotationSwatch.swift`, `AnnotationDocument.swift`,
//! `AnnotationBackground*.swift`, and `AnnotationImageTransform.swift`. The
//! JSON matches the macOS app, so a sidecar written on one platform reopens on
//! the other.

pub mod background;
pub mod document;
pub mod geometry;
pub mod shape;
pub mod store;
pub mod swatch;
pub mod transform;
pub mod undo;

pub use background::BackgroundSettings;
pub use document::AnnotationDocument;
pub use geometry::{Mat, Vec2};
pub use shape::{AnnoShape, ArrowBinding, ShapeId, ShapeKind};
pub use swatch::Swatch;
