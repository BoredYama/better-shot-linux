//! The image editor: a port of `AnnotationEditorWindow.swift` and its canvas,
//! inspector, and toolbar, drawn through the shared `bettershot-render`.

mod canvas;
mod inspector;
pub mod model;
mod smart_redaction;
mod window;

pub use window::{open, open_file_dialog};

use bettershot_core::annotation::geometry::Vec2;
use bettershot_render::{RenderCache, Scene};
use model::EditorModel;
use std::cell::{Cell, RefCell};
use std::time::Instant;

/// Zoom and pan: widget = canvas × zoom + offset.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub zoom: f64,
    pub offset: (f64, f64),
    /// Refit when the window resizes, until the user zooms.
    pub fit: bool,
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, offset: (0.0, 0.0), fit: true }
    }
}

pub struct Editor {
    pub model: RefCell<EditorModel>,
    pub cache: RefCell<RenderCache>,
    pub view: Cell<View>,
    pub interaction: RefCell<canvas::Interaction>,
    pub widgets: RefCell<Option<window::Widgets>>,
    /// The last coalesced edit (inspector sliders), so one drag is one undo.
    last_edit: RefCell<Option<(String, Instant)>>,
    /// The running Share upload, if any.
    pub share_upload: RefCell<Option<std::rc::Rc<crate::sharing::Upload>>>,
}

impl Editor {
    pub fn new(model: EditorModel) -> Self {
        Self {
            model: RefCell::new(model),
            cache: RefCell::new(RenderCache::default()),
            view: Cell::new(View::default()),
            interaction: RefCell::new(canvas::Interaction::default()),
            widgets: RefCell::new(None),
            last_edit: RefCell::new(None),
            share_upload: RefCell::new(None),
        }
    }

    /// Records an undo checkpoint unless `key` was edited a moment ago, so a
    /// slider drag or typing in a field undoes as one step.
    pub fn checkpoint_coalesced(&self, key: &str) {
        let now = Instant::now();
        let mut last = self.last_edit.borrow_mut();
        let recent = last.as_ref().is_some_and(|(k, at)| k == key && now.duration_since(*at).as_millis() < 800);
        if !recent {
            self.model.borrow_mut().checkpoint();
        }
        *last = Some((key.to_owned(), now));
    }

    pub fn end_coalescing(&self) {
        self.last_edit.borrow_mut().take();
    }

    /// The canvas's size in canvas pixels and where the page starts on it.
    pub fn canvas_metrics(&self) -> ((f64, f64), (f64, f64)) {
        let model = self.model.borrow();
        let snapshot = &model.current;
        let scene = Scene {
            base: &snapshot.base,
            base_generation: snapshot.generation,
            shapes: &snapshot.shapes,
            bindings: &snapshot.bindings,
            background: &snapshot.background,
        };
        let (w, h) = scene.canvas_size();
        ((f64::from(w), f64::from(h)), scene.image_origin())
    }

    pub fn widget_to_page(&self, x: f64, y: f64) -> Vec2 {
        let view = self.view.get();
        let (_, origin) = self.canvas_metrics();
        Vec2::new((x - view.offset.0) / view.zoom - origin.0, (y - view.offset.1) / view.zoom - origin.1)
    }

    pub fn page_to_widget(&self, p: Vec2) -> (f64, f64) {
        let view = self.view.get();
        let (_, origin) = self.canvas_metrics();
        ((p.x + origin.0) * view.zoom + view.offset.0, (p.y + origin.1) * view.zoom + view.offset.1)
    }

    /// Fits the canvas into a widget of `width` × `height`, never above 100%.
    pub fn fit(&self, width: f64, height: f64) {
        let ((cw, ch), _) = self.canvas_metrics();
        let margin = 32.0;
        let zoom = ((width - margin * 2.0) / cw).min((height - margin * 2.0) / ch).clamp(0.02, 1.0);
        self.view.set(View { zoom, offset: ((width - cw * zoom) / 2.0, (height - ch * zoom) / 2.0), fit: true });
    }

    /// Zooms by `factor` keeping the widget point (x, y) fixed.
    pub fn zoom_at(&self, factor: f64, x: f64, y: f64) {
        let view = self.view.get();
        let zoom = (view.zoom * factor).clamp(0.05, 16.0);
        let ratio = zoom / view.zoom;
        self.view.set(View {
            zoom,
            offset: (x - (x - view.offset.0) * ratio, y - (y - view.offset.1) * ratio),
            fit: false,
        });
    }

    pub fn set_zoom(&self, zoom: f64, width: f64, height: f64) {
        let ((cw, ch), _) = self.canvas_metrics();
        self.view.set(View { zoom, offset: ((width - cw * zoom) / 2.0, (height - ch * zoom) / 2.0), fit: false });
    }
}
