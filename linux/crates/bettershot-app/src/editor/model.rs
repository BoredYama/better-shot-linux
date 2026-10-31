//! The editor's state, a port of the editing parts of
//! `AnnotationEditorModel.swift`. GTK-free so it can be unit tested.

use bettershot_core::annotation::background::BackgroundSettings;
use bettershot_core::annotation::document::AnnotationDocument;
use bettershot_core::annotation::shape::*;
use bettershot_core::annotation::store::EditPaths;
use bettershot_core::annotation::swatch::Swatch;
use bettershot_core::annotation::transform::{self, ImageTransform};
use bettershot_core::annotation::undo::UndoStack;
use bettershot_core::imaging::{self, EncodeOptions};
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Everything undo restores, including the base image after a crop or turn.
#[derive(Clone)]
pub struct Snapshot {
    pub shapes: Vec<AnnoShape>,
    pub bindings: Vec<ArrowBinding>,
    pub background: BackgroundSettings,
    pub base: Rc<RgbaImage>,
    pub base_path: PathBuf,
    pub generation: u64,
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.shapes == other.shapes
            && self.bindings == other.bindings
            && self.background == other.background
            && self.base_path == other.base_path
            && self.generation == other.generation
    }
}

/// The style new shapes get, and that the inspector edits on a selection.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolStyle {
    pub swatch: Swatch,
    pub stroke_width: f64,
    pub dash: DashStyle,
    pub corner_radius: f64,
    pub arrowhead_start: Arrowhead,
    pub arrowhead_end: Arrowhead,
    pub font_size: f64,
    pub font_family: FontFamily,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub align: TextAlign,
    pub density: f64,
    pub diameter: f64,
}

impl Default for ToolStyle {
    fn default() -> Self {
        Self {
            swatch: Swatch::red(),
            stroke_width: 8.0,
            dash: DashStyle::Solid,
            corner_radius: 0.0,
            arrowhead_start: Arrowhead::None,
            arrowhead_end: Arrowhead::Arrow,
            font_size: 48.0,
            font_family: FontFamily::Pro,
            bold: true,
            italic: false,
            underline: false,
            align: TextAlign::Start,
            density: 0.55,
            diameter: 64.0,
        }
    }
}

impl ToolStyle {
    /// Writes this style's fields onto a shape, keeping its geometry.
    pub fn apply(&self, shape: &mut AnnoShape) {
        match &mut shape.kind {
            ShapeKind::Geo(p) => {
                p.swatch = self.swatch.clone();
                p.stroke_width = self.stroke_width;
                p.dash = self.dash;
                p.corner_radius = self.corner_radius;
            }
            ShapeKind::Draw(p) => {
                p.swatch = self.swatch.clone();
                p.stroke_width = self.stroke_width;
            }
            ShapeKind::Arrow(p) => {
                p.swatch = self.swatch.clone();
                p.stroke_width = self.stroke_width;
                p.dash = self.dash;
                // A line stays a line; an arrow keeps the chosen heads.
                if p.arrowhead_start != Arrowhead::None || p.arrowhead_end != Arrowhead::None {
                    p.arrowhead_start = self.arrowhead_start;
                    p.arrowhead_end = self.arrowhead_end;
                }
            }
            ShapeKind::Text(p) => {
                p.swatch = self.swatch.clone();
                p.font_size = self.font_size;
                p.font_family = self.font_family;
                p.is_bold = self.bold;
                p.is_italic = self.italic;
                p.is_underline = self.underline;
                p.align = self.align;
            }
            ShapeKind::Redaction(p) => p.density = self.density,
            ShapeKind::Numbered(p) => {
                p.swatch = self.swatch.clone();
                p.diameter = self.diameter;
            }
            ShapeKind::Highlight(_) => {}
        }
    }

    /// Reads a shape's style, so selecting it shows its settings.
    pub fn read(&mut self, shape: &AnnoShape) {
        match &shape.kind {
            ShapeKind::Geo(p) => {
                self.swatch = p.swatch.clone();
                self.stroke_width = p.stroke_width;
                self.dash = p.dash;
                self.corner_radius = p.corner_radius;
            }
            ShapeKind::Draw(p) => {
                self.swatch = p.swatch.clone();
                self.stroke_width = p.stroke_width;
            }
            ShapeKind::Arrow(p) => {
                self.swatch = p.swatch.clone();
                self.stroke_width = p.stroke_width;
                self.dash = p.dash;
                if p.arrowhead_start != Arrowhead::None || p.arrowhead_end != Arrowhead::None {
                    self.arrowhead_start = p.arrowhead_start;
                    self.arrowhead_end = p.arrowhead_end;
                }
            }
            ShapeKind::Text(p) => {
                self.swatch = p.swatch.clone();
                self.font_size = p.font_size;
                self.font_family = p.font_family;
                self.bold = p.is_bold;
                self.italic = p.is_italic;
                self.underline = p.is_underline;
                self.align = p.align;
            }
            ShapeKind::Redaction(p) => self.density = p.density,
            ShapeKind::Numbered(p) => {
                self.swatch = p.swatch.clone();
                self.diameter = p.diameter;
            }
            ShapeKind::Highlight(_) => {}
        }
    }

    /// A new shape for `tool` at page point (x, y).
    pub fn new_shape(&self, tool: Tool, x: f64, y: f64, number: i64) -> Option<AnnoShape> {
        let kind = match tool {
            Tool::Select => return None,
            Tool::Rectangle | Tool::FilledRectangle | Tool::Ellipse => ShapeKind::Geo(GeoProps {
                geo: if tool == Tool::Ellipse { GeoKind::Ellipse } else { GeoKind::Rectangle },
                w: 1.0,
                h: 1.0,
                swatch: self.swatch.clone(),
                stroke_width: self.stroke_width,
                fill: if tool == Tool::FilledRectangle { FillStyle::Solid } else { FillStyle::None },
                dash: self.dash,
                corner_radius: if tool == Tool::Ellipse { 0.0 } else { self.corner_radius },
            }),
            Tool::Line | Tool::Arrow => ShapeKind::Arrow(ArrowProps {
                start: Default::default(),
                end: Default::default(),
                bend: 0.0,
                arrowhead_start: if tool == Tool::Line { Arrowhead::None } else { self.arrowhead_start },
                arrowhead_end: if tool == Tool::Line { Arrowhead::None } else { self.arrowhead_end },
                swatch: self.swatch.clone(),
                stroke_width: self.stroke_width,
                dash: self.dash,
            }),
            Tool::Freehand => ShapeKind::Draw(DrawProps {
                points: vec![bettershot_core::annotation::geometry::Vec2::with_pressure(0.0, 0.0, 0.5)],
                swatch: self.swatch.clone(),
                stroke_width: self.stroke_width,
                ..Default::default()
            }),
            Tool::NumberedCircle => ShapeKind::Numbered(NumberedProps {
                value: number,
                diameter: self.diameter,
                swatch: self.swatch.clone(),
            }),
            Tool::Text => ShapeKind::Text(TextProps {
                swatch: self.swatch.clone(),
                font_size: self.font_size,
                font_family: self.font_family,
                is_bold: self.bold,
                is_italic: self.italic,
                is_underline: self.underline,
                align: self.align,
                ..Default::default()
            }),
            Tool::Highlight => ShapeKind::Highlight(HighlightProps { w: 1.0, h: 1.0 }),
            Tool::Pixelate | Tool::Blur => ShapeKind::Redaction(RedactionProps {
                kind: if tool == Tool::Blur { RedactionKind::Blur } else { RedactionKind::Pixelate },
                w: 1.0,
                h: 1.0,
                density: self.density,
            }),
        };
        let (x, y) =
            if tool == Tool::NumberedCircle { (x - self.diameter / 2.0, y - self.diameter / 2.0) } else { (x, y) };
        Some(AnnoShape::new(x, y, kind))
    }
}

pub struct EditorModel {
    /// The capture's untouched library file.
    pub capture: PathBuf,
    pub paths: EditPaths,
    pub current: Snapshot,
    saved: Snapshot,
    pub selection: Vec<ShapeId>,
    pub tool: Tool,
    pub style: ToolStyle,
    undo: UndoStack<Snapshot>,
    next_generation: u64,
    scratch_dir: PathBuf,
}

impl EditorModel {
    pub fn new(
        capture: PathBuf,
        document: AnnotationDocument,
        base_path: PathBuf,
        base: RgbaImage,
        scratch_dir: PathBuf,
    ) -> Self {
        let current = Snapshot {
            shapes: document.shapes,
            bindings: document.bindings,
            background: document.background,
            base: Rc::new(base),
            base_path,
            generation: 1,
        };
        Self {
            paths: EditPaths::for_capture(&capture),
            capture,
            saved: current.clone(),
            current,
            selection: Vec::new(),
            tool: Tool::Select,
            style: ToolStyle::default(),
            undo: UndoStack::new(200),
            next_generation: 2,
            scratch_dir,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.current != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.current.clone();
    }

    /// Records the current state before an edit, for undo.
    pub fn checkpoint(&mut self) {
        self.undo.record(self.current.clone());
    }

    /// Reverts to the last checkpoint and forgets it, for an edit that
    /// changed nothing worth undoing.
    pub fn revert_to_checkpoint(&mut self) {
        if let Some(previous) = self.undo.discard_last() {
            self.current = previous;
            self.prune_selection();
        }
    }

    /// Whether anything changed since the last checkpoint.
    pub fn is_dirty_since_checkpoint(&self) -> bool {
        self.undo.peek().is_none_or(|last| *last != self.current)
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.undo(self.current.clone()) {
            Some(previous) => {
                self.current = previous;
                self.prune_selection();
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.undo.redo(self.current.clone()) {
            Some(next) => {
                self.current = next;
                self.prune_selection();
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    fn prune_selection(&mut self) {
        let shapes = &self.current.shapes;
        self.selection.retain(|id| shapes.iter().any(|s| &s.id == id));
    }

    pub fn shape(&self, id: &ShapeId) -> Option<&AnnoShape> {
        self.current.shapes.iter().find(|s| &s.id == id)
    }

    pub fn shape_mut(&mut self, id: &ShapeId) -> Option<&mut AnnoShape> {
        self.current.shapes.iter_mut().find(|s| &s.id == id)
    }

    pub fn selected_shapes(&self) -> Vec<&AnnoShape> {
        self.current.shapes.iter().filter(|s| self.selection.contains(&s.id)).collect()
    }

    pub fn select(&mut self, ids: Vec<ShapeId>) {
        self.selection = ids;
        if let Some(first) = self.selection.first().and_then(|id| self.shape(id)).cloned() {
            self.style.read(&first);
        }
    }

    pub fn select_all(&mut self) {
        self.selection = self.current.shapes.iter().map(|s| s.id.clone()).collect();
    }

    pub fn add_shape(&mut self, shape: AnnoShape) {
        self.current.shapes.push(shape);
    }

    pub fn remove_shape(&mut self, id: &ShapeId) {
        self.current.shapes.retain(|s| &s.id != id);
        self.current.bindings.retain(|b| &b.arrow_id != id && &b.to_id != id);
        self.selection.retain(|s| s != id);
    }

    pub fn delete_selection(&mut self) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.checkpoint();
        for id in std::mem::take(&mut self.selection) {
            self.remove_shape(&id);
        }
        true
    }

    /// Applies the current style to the selection; the caller records undo.
    pub fn apply_style_to_selection_unrecorded(&mut self) {
        let style = self.style.clone();
        let selection = self.selection.clone();
        for shape in self.current.shapes.iter_mut().filter(|s| selection.contains(&s.id)) {
            style.apply(shape);
        }
    }

    /// The next number for a numbered callout.
    pub fn next_number(&self) -> i64 {
        self.current
            .shapes
            .iter()
            .filter_map(|s| match &s.kind {
                ShapeKind::Numbered(p) => Some(p.value),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    pub fn base_size(&self) -> (f64, f64) {
        (f64::from(self.current.base.width()), f64::from(self.current.base.height()))
    }

    fn replace_base(&mut self, image: RgbaImage) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.scratch_dir)?;
        let name = format!("BetterShot-Base-{}.png", uuid::Uuid::new_v4());
        let path = imaging::write_new_image(&image, EncodeOptions::default(), &name, &self.scratch_dir)
            .map_err(std::io::Error::other)?;
        self.current.base = Rc::new(image);
        self.current.base_path = path;
        self.current.generation = self.next_generation;
        self.next_generation += 1;
        Ok(())
    }

    /// Rotates or flips the base and every shape together.
    pub fn transform(&mut self, transform: ImageTransform) -> std::io::Result<()> {
        self.checkpoint();
        let (w, h) = self.base_size();
        let image = transform.apply_to_image(&self.current.base);
        self.current.shapes = self.current.shapes.iter().map(|s| transform.apply_to_shape(s, w, h)).collect();
        self.replace_base(image)
    }

    /// Crops the base to whole pixels, moving shapes with it.
    pub fn crop(&mut self, x: u32, y: u32, width: u32, height: u32) -> std::io::Result<()> {
        self.checkpoint();
        let (image, shapes) = transform::crop(&self.current.base, &self.current.shapes, x, y, width, height);
        self.current.shapes = shapes;
        self.replace_base(image)
    }

    pub fn document(&self) -> AnnotationDocument {
        AnnotationDocument::new(
            self.current.shapes.clone(),
            self.current.bindings.clone(),
            self.current.background.clone(),
        )
    }

    /// Whether saving produces anything beyond the untouched capture.
    pub fn has_edits(&self) -> bool {
        !self.current.shapes.is_empty()
            || self.current.background.has_renderable_content()
            || self.current.base_path != self.capture
    }

    pub fn base_path(&self) -> &Path {
        &self.current.base_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> (tempfile::TempDir, EditorModel) {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("Shot.png");
        let base = RgbaImage::new(40, 20);
        base.save(&capture).unwrap();
        let document = AnnotationDocument::new(Vec::new(), Vec::new(), BackgroundSettings::default());
        let scratch = dir.path().join("scratch");
        let model = EditorModel::new(capture.clone(), document, capture, base, scratch);
        (dir, model)
    }

    #[test]
    fn undo_covers_shapes_style_and_crops() {
        let (_dir, mut model) = model();
        assert!(!model.is_dirty() && !model.has_edits());
        model.checkpoint();
        let shape = model.style.new_shape(Tool::Rectangle, 5.0, 5.0, 1).unwrap();
        let id = shape.id.clone();
        model.add_shape(shape);
        assert!(model.is_dirty());
        model.select(vec![id.clone()]);
        model.style.swatch = Swatch::blue();
        model.checkpoint();
        model.apply_style_to_selection_unrecorded();
        assert_eq!(model.shape(&id).unwrap().swatch().unwrap().id, "blue");

        model.crop(10, 0, 20, 20).unwrap();
        assert_eq!(model.base_size(), (20.0, 20.0));
        assert_eq!(model.shape(&id).unwrap().x, -5.0, "shapes move with the crop");
        assert!(model.base_path().exists() && model.has_edits());

        assert!(model.undo());
        assert_eq!(model.base_size(), (40.0, 20.0), "undo restores the uncropped base");
        assert!(model.undo());
        assert_eq!(model.shape(&id).unwrap().swatch().unwrap().id, "red");
        assert!(model.undo());
        assert!(model.current.shapes.is_empty() && !model.is_dirty());
        assert!(model.redo() && model.redo() && model.redo());
        assert_eq!(model.base_size(), (20.0, 20.0));
    }

    #[test]
    fn numbering_and_deletion() {
        let (_dir, mut model) = model();
        for _ in 0..3 {
            let shape = model.style.new_shape(Tool::NumberedCircle, 32.0, 32.0, model.next_number()).unwrap();
            model.add_shape(shape);
        }
        assert_eq!(model.next_number(), 4);
        let first = model.current.shapes[0].clone();
        assert_eq!((first.x, first.y), (0.0, 0.0), "a numbered circle is centered on the click");
        model.select_all();
        assert!(model.delete_selection());
        assert!(model.current.shapes.is_empty());
        assert!(model.undo());
        assert_eq!(model.current.shapes.len(), 3);
    }

    #[test]
    fn transforms_turn_the_base() {
        let (_dir, mut model) = model();
        model.transform(ImageTransform::RotateRight).unwrap();
        assert_eq!(model.base_size(), (20.0, 40.0));
        model.transform(ImageTransform::RotateLeft).unwrap();
        assert_eq!(model.base_size(), (40.0, 20.0));
        assert!(model.has_edits(), "a turned base is a new file even when it matches");
    }
}
