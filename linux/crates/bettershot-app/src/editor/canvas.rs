//! The editor canvas: draws through the shared renderer, then the selection,
//! crop, and caret chrome, and handles every pointer and text interaction.
//! A port of `AnnotationCanvas.swift` and `AnnoEditorInteraction.swift`.

use super::{window, Editor};
use adw::prelude::*;
use bettershot_core::annotation::geometry::{Bounds, Vec2};
use bettershot_core::annotation::shape::*;
use bettershot_core::geometry::{Point, Rect, Size};
use bettershot_core::region::{self, RegionHandle};
use bettershot_render::geometry::{self as shape_geometry};
use bettershot_render::{arrows, draw_canvas, text, DrawOptions, Scene};
use gtk::{gdk, glib};
use std::rc::Rc;

/// What a pointer drag is doing.
#[derive(Debug, Clone, Default)]
pub enum Drag {
    #[default]
    None,
    Create {
        id: ShapeId,
        tool: Tool,
        origin: Vec2,
    },
    Move {
        start: Vec2,
        originals: Vec<(ShapeId, f64, f64)>,
        moved: bool,
    },
    Resize {
        id: ShapeId,
        handle: RegionHandle,
        original: AnnoShape,
        start: Vec2,
    },
    ArrowEnd {
        id: ShapeId,
        terminal: ArrowTerminal,
        original: AnnoShape,
    },
    Marquee {
        start: Vec2,
        current: Vec2,
        additive: Vec<ShapeId>,
    },
    Crop {
        handle: Option<RegionHandle>,
        start_rect: Rect,
        origin: (f64, f64),
    },
}

/// Text being typed into a text shape.
#[derive(Debug, Clone)]
pub struct TextEdit {
    pub id: ShapeId,
    /// Byte offset of the caret.
    pub caret: usize,
}

#[derive(Debug, Default)]
pub struct Interaction {
    pub drag: Drag,
    pub text: Option<TextEdit>,
    /// The pending crop in page pixels, while cropping.
    pub crop: Option<Rect>,
    pointer: (f64, f64),
}

const HANDLE_SIZE: f64 = 8.0;
const HIT_TOLERANCE: f64 = 6.0;
const SELECTION: (f64, f64, f64) = (0.192, 0.510, 0.929);

/// Returns the canvas key controller; the input method is attached to it only
/// while typing into a text shape, so tool shortcuts reach the window otherwise.
pub fn install(editor: &Rc<Editor>, area: &gtk::DrawingArea, im: &gtk::IMMulticontext) -> gtk::EventControllerKey {
    let draw_editor = Rc::downgrade(editor);
    area.set_draw_func(move |_, cr, width, height| {
        if let Some(editor) = draw_editor.upgrade() {
            draw(&editor, cr, f64::from(width), f64::from(height));
        }
    });

    let resize_editor = Rc::downgrade(editor);
    area.connect_resize(move |area, width, height| {
        if let Some(editor) = resize_editor.upgrade() {
            if editor.view.get().fit {
                editor.fit(f64::from(width), f64::from(height));
                window::refresh_zoom(&editor);
            }
            area.queue_draw();
        }
    });

    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let begin = Rc::downgrade(editor);
    drag.connect_drag_begin(move |gesture, x, y| {
        if let Some(editor) = begin.upgrade() {
            let shift = gesture.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
            drag_begin(&editor, x, y, shift);
        }
    });
    let update = Rc::downgrade(editor);
    drag.connect_drag_update(move |gesture, dx, dy| {
        if let Some(editor) = update.upgrade() {
            let shift = gesture.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
            drag_update(&editor, dx, dy, shift);
        }
    });
    let end = Rc::downgrade(editor);
    drag.connect_drag_end(move |_, _, _| {
        if let Some(editor) = end.upgrade() {
            drag_end(&editor);
        }
    });
    area.add_controller(drag);

    let click = gtk::GestureClick::new();
    let double = Rc::downgrade(editor);
    click.connect_pressed(move |_, presses, x, y| {
        let Some(editor) = double.upgrade() else { return };
        if presses == 2 {
            double_click(&editor, x, y);
        }
    });
    area.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    let hover = Rc::downgrade(editor);
    let hover_area = area.clone();
    motion.connect_motion(move |_, x, y| {
        if let Some(editor) = hover.upgrade() {
            editor.interaction.borrow_mut().pointer = (x, y);
            hover_area.set_cursor_from_name(Some(cursor_for(&editor, x, y)));
        }
    });
    area.add_controller(motion);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    let scroll_editor = Rc::downgrade(editor);
    let scroll_area = area.clone();
    scroll.connect_scroll(move |controller, dx, dy| {
        let Some(editor) = scroll_editor.upgrade() else { return glib::Propagation::Proceed };
        if controller.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
            let (x, y) = editor.interaction.borrow().pointer;
            editor.zoom_at(if dy < 0.0 { 1.1 } else { 1.0 / 1.1 }, x, y);
        } else {
            let mut view = editor.view.get();
            view.offset.0 -= dx * 32.0;
            view.offset.1 -= dy * 32.0;
            view.fit = false;
            editor.view.set(view);
        }
        window::refresh_zoom(&editor);
        scroll_area.queue_draw();
        glib::Propagation::Stop
    });
    area.add_controller(scroll);

    // Typing into a text shape goes through the input method.
    let keys = gtk::EventControllerKey::new();
    let key_editor = Rc::downgrade(editor);
    keys.connect_key_pressed(move |_, key, _, state| {
        let Some(editor) = key_editor.upgrade() else { return glib::Propagation::Proceed };
        if text_key(&editor, key, state) {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    area.add_controller(keys.clone());
    im.set_client_widget(Some(area));
    let commit_editor = Rc::downgrade(editor);
    im.connect_commit(move |_, text| {
        if let Some(editor) = commit_editor.upgrade() {
            insert_text(&editor, text);
        }
    });
    let focus = gtk::EventControllerFocus::new();
    let focus_im = im.clone();
    focus.connect_enter(move |_| focus_im.focus_in());
    let blur_im = im.clone();
    focus.connect_leave(move |_| blur_im.focus_out());
    area.add_controller(focus);
    keys
}

// MARK: - Drawing

fn draw(editor: &Rc<Editor>, cr: &gtk::cairo::Context, width: f64, height: f64) {
    if editor.view.get().fit {
        editor.fit(width, height);
    }
    let view = editor.view.get();
    let ((cw, ch), _) = editor.canvas_metrics();

    // A checkerboard shows where the export is transparent.
    let _ = cr.save();
    cr.rectangle(view.offset.0, view.offset.1, cw * view.zoom, ch * view.zoom);
    cr.clip();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.paint();
    cr.set_source_rgb(0.86, 0.86, 0.86);
    let cell = 8.0;
    let mut y = view.offset.1;
    let mut row = 0;
    while y < view.offset.1 + ch * view.zoom {
        let mut x = view.offset.0 + if row % 2 == 0 { 0.0 } else { cell };
        while x < view.offset.0 + cw * view.zoom {
            cr.rectangle(x, y, cell, cell);
            x += cell * 2.0;
        }
        y += cell;
        row += 1;
    }
    let _ = cr.fill();
    let _ = cr.restore();

    {
        let model = editor.model.borrow();
        let snapshot = &model.current;
        let scene = Scene {
            base: &snapshot.base,
            base_generation: snapshot.generation,
            shapes: &snapshot.shapes,
            bindings: &snapshot.bindings,
            background: &snapshot.background,
        };
        let _ = cr.save();
        cr.translate(view.offset.0, view.offset.1);
        cr.scale(view.zoom, view.zoom);
        draw_canvas(cr, &scene, &mut editor.cache.borrow_mut(), &DrawOptions::default());
        let _ = cr.restore();
    }

    draw_selection(editor, cr);
    draw_crop(editor, cr);
    draw_caret(editor, cr);
}

fn widget_points(editor: &Editor, points: &[Vec2]) -> Vec<(f64, f64)> {
    points.iter().map(|p| editor.page_to_widget(*p)).collect()
}

fn polygon(cr: &gtk::cairo::Context, points: &[(f64, f64)]) {
    cr.new_path();
    for (i, (x, y)) in points.iter().enumerate() {
        if i == 0 {
            cr.move_to(*x, *y);
        } else {
            cr.line_to(*x, *y);
        }
    }
    cr.close_path();
}

fn handle_square(cr: &gtk::cairo::Context, (x, y): (f64, f64)) {
    let half = HANDLE_SIZE / 2.0;
    cr.rectangle(x - half, y - half, HANDLE_SIZE, HANDLE_SIZE);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(SELECTION.0, SELECTION.1, SELECTION.2);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
}

/// Which handles a shape offers.
fn handles_for(shape: &AnnoShape) -> &'static [RegionHandle] {
    use RegionHandle::*;
    match shape.kind {
        ShapeKind::Geo(_) | ShapeKind::Redaction(_) | ShapeKind::Highlight(_) => {
            &[TopLeft, Top, TopRight, Right, BottomRight, Bottom, BottomLeft, Left]
        }
        ShapeKind::Numbered(_) => &[TopLeft, TopRight, BottomRight, BottomLeft],
        ShapeKind::Text(_) => &[Left, Right],
        ShapeKind::Draw(_) | ShapeKind::Arrow(_) => &[],
    }
}

fn local_handle_point(handle: RegionHandle, b: Bounds) -> Vec2 {
    let p = handle.point(Rect::new(b.x, b.y, b.w, b.h));
    Vec2::new(p.x, p.y)
}

fn arrow_ends(shape: &AnnoShape, model: &super::model::EditorModel) -> Option<(Vec2, Vec2)> {
    let set = shape_geometry::ShapeSet { shapes: &model.current.shapes, bindings: &model.current.bindings };
    let info = arrows::arrow_info(shape, &set)?;
    let transform = shape.page_transform();
    Some((transform.apply(info.start.handle), transform.apply(info.end.handle)))
}

fn draw_selection(editor: &Editor, cr: &gtk::cairo::Context) {
    let model = editor.model.borrow();
    let set = shape_geometry::ShapeSet { shapes: &model.current.shapes, bindings: &model.current.bindings };
    let selected = model.selected_shapes();
    cr.set_source_rgb(SELECTION.0, SELECTION.1, SELECTION.2);
    cr.set_line_width(1.5);
    for shape in &selected {
        if let ShapeKind::Arrow(_) = shape.kind {
            continue;
        }
        let corners = widget_points(editor, &shape_geometry::page_corners(shape, &set));
        cr.set_source_rgb(SELECTION.0, SELECTION.1, SELECTION.2);
        polygon(cr, &corners);
        let _ = cr.stroke();
    }
    if let [shape] = selected.as_slice() {
        if let ShapeKind::Arrow(_) = shape.kind {
            if let Some((start, end)) = arrow_ends(shape, &model) {
                for point in [start, end] {
                    let (x, y) = editor.page_to_widget(point);
                    cr.arc(x, y, HANDLE_SIZE / 2.0 + 1.0, 0.0, std::f64::consts::TAU);
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgb(SELECTION.0, SELECTION.1, SELECTION.2);
                    let _ = cr.stroke();
                }
            }
        } else if editor.interaction.borrow().text.is_none() {
            let bounds = shape_geometry::local_bounds(shape, &set);
            let transform = shape.page_transform();
            for handle in handles_for(shape) {
                handle_square(cr, editor.page_to_widget(transform.apply(local_handle_point(*handle, bounds))));
            }
        }
    }
    drop(model);
    if let Drag::Marquee { start, current, .. } = &editor.interaction.borrow().drag {
        let (x0, y0) = editor.page_to_widget(*start);
        let (x1, y1) = editor.page_to_widget(*current);
        cr.rectangle(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs());
        cr.set_source_rgba(0.12, 0.56, 1.0, 0.18);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(SELECTION.0, SELECTION.1, SELECTION.2);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }
}

fn crop_widget_rect(editor: &Editor, crop: Rect) -> Rect {
    let (x0, y0) = editor.page_to_widget(Vec2::new(crop.min_x(), crop.min_y()));
    let (x1, y1) = editor.page_to_widget(Vec2::new(crop.max_x(), crop.max_y()));
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn image_widget_rect(editor: &Editor) -> Rect {
    let (w, h) = editor.model.borrow().base_size();
    crop_widget_rect(editor, Rect::new(0.0, 0.0, w, h))
}

fn draw_crop(editor: &Editor, cr: &gtk::cairo::Context) {
    let Some(crop) = editor.interaction.borrow().crop else { return };
    let image = image_widget_rect(editor);
    let rect = crop_widget_rect(editor, crop);
    cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
    cr.rectangle(image.x, image.y, image.width, image.height);
    cr.rectangle(rect.x, rect.y, rect.width, rect.height);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
    let _ = cr.fill();
    cr.set_fill_rule(gtk::cairo::FillRule::Winding);
    cr.rectangle(rect.x.round() + 0.5, rect.y.round() + 0.5, rect.width.round() - 1.0, rect.height.round() - 1.0);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    for handle in RegionHandle::ALL.into_iter().filter(|h| *h != RegionHandle::Move) {
        let p = handle.point(rect);
        handle_square(cr, (p.x, p.y));
    }
    let label = format!("{} × {}", crop.width.round(), crop.height.round());
    let description = text::font(FontFamily::Pro, 12.0, true, false);
    let (lw, lh) = text::line_size(&label, &description);
    let (lx, ly) = (rect.x + 6.0, (rect.y - lh - 10.0).max(image.y + 4.0));
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.72);
    cr.rectangle(lx, ly, lw + 12.0, lh + 6.0);
    let _ = cr.fill();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    text::draw_line_centered(cr, &label, &description, lx + (lw + 12.0) / 2.0, ly + (lh + 6.0) / 2.0);
}

fn draw_caret(editor: &Editor, cr: &gtk::cairo::Context) {
    let interaction = editor.interaction.borrow();
    let Some(edit) = &interaction.text else { return };
    let model = editor.model.borrow();
    let Some(shape) = model.shape(&edit.id) else { return };
    let ShapeKind::Text(props) = &shape.kind else { return };
    let (x, y, h) = text::caret_at(props, edit.caret);
    let transform = shape.page_transform();
    let top = editor.page_to_widget(transform.apply(Vec2::new(x, y)));
    let bottom = editor.page_to_widget(transform.apply(Vec2::new(x, y + h)));
    let (w, hh) = text::measure(props);
    let frame: Vec<(f64, f64)> =
        [Vec2::new(-4.0, -4.0), Vec2::new(w + 4.0, -4.0), Vec2::new(w + 4.0, hh + 4.0), Vec2::new(-4.0, hh + 4.0)]
            .iter()
            .map(|p| editor.page_to_widget(transform.apply(*p)))
            .collect();
    cr.set_source_rgba(SELECTION.0, SELECTION.1, SELECTION.2, 0.8);
    cr.set_line_width(1.0);
    cr.set_dash(&[4.0, 3.0], 0.0);
    polygon(cr, &frame);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);
    cr.set_source_rgb(SELECTION.0, SELECTION.1, SELECTION.2);
    cr.set_line_width(2.0);
    cr.move_to(top.0, top.1);
    cr.line_to(bottom.0, bottom.1);
    let _ = cr.stroke();
}

fn cursor_for(editor: &Editor, x: f64, y: f64) -> &'static str {
    let interaction = editor.interaction.borrow();
    if let Some(crop) = interaction.crop {
        return region::handle_at(Point::new(x, y), crop_widget_rect(editor, crop))
            .map_or("crosshair", RegionHandle::cursor_name);
    }
    let tool = editor.model.borrow().tool;
    match tool {
        Tool::Select => {
            drop(interaction);
            if handle_hit(editor, x, y).is_some() {
                "grab"
            } else if shape_at(editor, editor.widget_to_page(x, y)).is_some() {
                "move"
            } else {
                "default"
            }
        }
        Tool::Text => "text",
        _ => "crosshair",
    }
}

// MARK: - Hit testing

fn shape_at(editor: &Editor, page: Vec2) -> Option<ShapeId> {
    let model = editor.model.borrow();
    let set = shape_geometry::ShapeSet { shapes: &model.current.shapes, bindings: &model.current.bindings };
    let tolerance = HIT_TOLERANCE / editor.view.get().zoom;
    model
        .current
        .shapes
        .iter()
        .rev()
        .find(|shape| shape_geometry::hit(shape, &set, page, tolerance))
        .map(|s| s.id.clone())
}

enum HandleHit {
    Box(ShapeId, RegionHandle),
    Arrow(ShapeId, ArrowTerminal),
}

fn handle_hit(editor: &Editor, x: f64, y: f64) -> Option<HandleHit> {
    let model = editor.model.borrow();
    let [shape] = model.selected_shapes()[..] else { return None };
    let near = |p: Vec2| {
        let (px, py) = editor.page_to_widget(p);
        (px - x).hypot(py - y) <= HANDLE_SIZE + 2.0
    };
    if let ShapeKind::Arrow(_) = shape.kind {
        let (start, end) = arrow_ends(shape, &model)?;
        if near(end) {
            return Some(HandleHit::Arrow(shape.id.clone(), ArrowTerminal::End));
        }
        if near(start) {
            return Some(HandleHit::Arrow(shape.id.clone(), ArrowTerminal::Start));
        }
        return None;
    }
    let set = shape_geometry::ShapeSet { shapes: &model.current.shapes, bindings: &model.current.bindings };
    let bounds = shape_geometry::local_bounds(shape, &set);
    let transform = shape.page_transform();
    handles_for(shape)
        .iter()
        .find(|handle| near(transform.apply(local_handle_point(**handle, bounds))))
        .map(|handle| HandleHit::Box(shape.id.clone(), *handle))
}

// MARK: - Pointer

fn drag_begin(editor: &Rc<Editor>, x: f64, y: f64, shift: bool) {
    if let Some(area) = window::canvas(editor) {
        area.grab_focus();
    }
    editor.end_coalescing();
    let page = editor.widget_to_page(x, y);

    let crop = editor.interaction.borrow().crop;
    if let Some(crop) = crop {
        let rect = crop_widget_rect(editor, crop);
        let handle = region::handle_at(Point::new(x, y), rect);
        editor.interaction.borrow_mut().drag = Drag::Crop { handle, start_rect: rect, origin: (x, y) };
        return;
    }

    // A click inside the text being edited moves the caret; outside, it ends editing.
    let editing = editor.interaction.borrow().text.clone();
    if let Some(edit) = editing {
        let caret = {
            let model = editor.model.borrow();
            model.shape(&edit.id).and_then(|shape| match &shape.kind {
                ShapeKind::Text(props) => {
                    let local = shape.page_transform().inverse().apply(page);
                    let (w, h) = text::measure(props);
                    (local.x >= -4.0 && local.y >= -4.0 && local.x <= w + 4.0 && local.y <= h + 4.0)
                        .then(|| text::index_at(props, local.x, local.y))
                }
                _ => None,
            })
        };
        match caret {
            Some(caret) => {
                if let Some(edit) = editor.interaction.borrow_mut().text.as_mut() {
                    edit.caret = caret;
                }
                window::queue_draw(editor);
                return;
            }
            None => finish_text(editor),
        }
    }

    let tool = editor.model.borrow().tool;
    if tool == Tool::Select {
        if let Some(hit) = handle_hit(editor, x, y) {
            let drag = {
                let model = editor.model.borrow();
                match hit {
                    HandleHit::Box(id, handle) => {
                        model.shape(&id).cloned().map(|original| Drag::Resize { id, handle, original, start: page })
                    }
                    HandleHit::Arrow(id, terminal) => {
                        model.shape(&id).cloned().map(|original| Drag::ArrowEnd { id, terminal, original })
                    }
                }
            };
            if let Some(drag) = drag {
                editor.model.borrow_mut().checkpoint();
                editor.interaction.borrow_mut().drag = drag;
            }
            return;
        }
        match shape_at(editor, page) {
            Some(id) => {
                {
                    let mut model = editor.model.borrow_mut();
                    let selected = model.selection.contains(&id);
                    if shift {
                        let mut selection = model.selection.clone();
                        if selected {
                            selection.retain(|s| s != &id);
                        } else {
                            selection.push(id.clone());
                        }
                        model.select(selection);
                    } else if !selected {
                        model.select(vec![id]);
                    }
                }
                let originals = {
                    let model = editor.model.borrow();
                    model.selected_shapes().iter().map(|s| (s.id.clone(), s.x, s.y)).collect()
                };
                editor.interaction.borrow_mut().drag = Drag::Move { start: page, originals, moved: false };
            }
            None => {
                let additive = if shift { editor.model.borrow().selection.clone() } else { Vec::new() };
                if !shift {
                    editor.model.borrow_mut().select(Vec::new());
                }
                editor.interaction.borrow_mut().drag = Drag::Marquee { start: page, current: page, additive };
            }
        }
        window::refresh(editor);
        return;
    }

    if tool == Tool::Text {
        // Clicking existing text edits it.
        if let Some(id) = shape_at(editor, page) {
            let is_text = editor.model.borrow().shape(&id).is_some_and(|s| matches!(s.kind, ShapeKind::Text(_)));
            if is_text {
                begin_text(editor, &id, Some(page));
                return;
            }
        }
        let shape = {
            let model = editor.model.borrow_mut();
            let number = model.next_number();
            let mut shape = model.style.new_shape(Tool::Text, page.x, page.y, number);
            if let Some(shape) = shape.as_mut() {
                if let ShapeKind::Text(props) = &shape.kind {
                    shape.y -= text::line_height(props) / 2.0;
                }
            }
            shape
        };
        if let Some(shape) = shape {
            let id = shape.id.clone();
            {
                let mut model = editor.model.borrow_mut();
                model.checkpoint();
                model.add_shape(shape);
                model.select(vec![id.clone()]);
            }
            editor.interaction.borrow_mut().text = Some(TextEdit { id, caret: 0 });
            window::set_typing(editor, true);
            window::refresh(editor);
        }
        return;
    }

    let created = {
        let mut model = editor.model.borrow_mut();
        let number = model.next_number();
        let shape = model.style.new_shape(tool, page.x, page.y, number);
        shape.map(|shape| {
            let id = shape.id.clone();
            model.checkpoint();
            model.add_shape(shape);
            id
        })
    };
    if let Some(id) = created {
        if tool == Tool::NumberedCircle {
            editor.model.borrow_mut().select(vec![id]);
        } else {
            editor.interaction.borrow_mut().drag = Drag::Create { id, tool, origin: page };
        }
        window::refresh(editor);
    }
}

fn snap_angle(delta: Vec2) -> Vec2 {
    let angle = (delta.y.atan2(delta.x) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
    Vec2::from_angle(angle).mul(delta.len())
}

fn drag_update(editor: &Rc<Editor>, dx: f64, dy: f64, shift: bool) {
    let zoom = editor.view.get().zoom;
    let delta = Vec2::new(dx / zoom, dy / zoom);
    let drag = editor.interaction.borrow().drag.clone();
    match drag {
        Drag::None => return,
        Drag::Create { id, tool, origin } => {
            let current = origin.add(delta);
            let mut model = editor.model.borrow_mut();
            let Some(shape) = model.shape_mut(&id) else { return };
            match &mut shape.kind {
                ShapeKind::Arrow(props) => {
                    props.end = if shift { snap_angle(delta) } else { delta };
                }
                ShapeKind::Draw(props) => {
                    props.points.push(Vec2::with_pressure(delta.x, delta.y, 0.5));
                }
                _ => {
                    let (mut w, mut h) = (delta.x.abs(), delta.y.abs());
                    if shift && tool.supports_aspect_lock() {
                        let side = w.max(h);
                        w = side;
                        h = side;
                    }
                    let x = if current.x < origin.x { origin.x - w } else { origin.x };
                    let y = if current.y < origin.y { origin.y - h } else { origin.y };
                    shape.x = x;
                    shape.y = y;
                    match &mut shape.kind {
                        ShapeKind::Geo(p) => {
                            p.w = w.max(1.0);
                            p.h = h.max(1.0);
                        }
                        ShapeKind::Redaction(p) => {
                            p.w = w.max(1.0);
                            p.h = h.max(1.0);
                        }
                        ShapeKind::Highlight(p) => {
                            p.w = w.max(1.0);
                            p.h = h.max(1.0);
                        }
                        _ => {}
                    }
                }
            }
        }
        Drag::Move { start, originals, moved } => {
            if !moved && (dx.hypot(dy)) < 2.0 {
                return;
            }
            let mut model = editor.model.borrow_mut();
            if !moved {
                model.checkpoint();
            }
            let snapped = if shift {
                if delta.x.abs() > delta.y.abs() {
                    Vec2::new(delta.x, 0.0)
                } else {
                    Vec2::new(0.0, delta.y)
                }
            } else {
                delta
            };
            for (id, x, y) in &originals {
                if let Some(shape) = model.shape_mut(id) {
                    shape.x = x + snapped.x;
                    shape.y = y + snapped.y;
                }
            }
            drop(model);
            editor.interaction.borrow_mut().drag = Drag::Move { start, originals, moved: true };
        }
        Drag::Resize { id, handle, original, start } => {
            let transform = original.page_transform();
            let inverse = transform.inverse();
            let local_start = inverse.apply(start);
            let local_now = inverse.apply(start.add(delta));
            let local_delta = local_now.sub(local_start);
            let (w, h) = match &original.kind {
                ShapeKind::Geo(p) => (p.w, p.h),
                ShapeKind::Redaction(p) => (p.w, p.h),
                ShapeKind::Highlight(p) => (p.w, p.h),
                ShapeKind::Numbered(p) => (p.diameter, p.diameter),
                ShapeKind::Text(p) => (text::measure(p).0, text::measure(p).1),
                _ => return,
            };
            let bounds = Rect::new(-1e7, -1e7, 2e7, 2e7);
            let mut rect =
                region::apply(handle, Size::new(local_delta.x, local_delta.y), Rect::new(0.0, 0.0, w, h), bounds);
            if let ShapeKind::Numbered(_) = original.kind {
                let side = rect.width.max(rect.height).max(12.0);
                if handle.moves_left() {
                    rect.x = rect.max_x() - side;
                }
                if handle.moves_top() {
                    rect.y = rect.max_y() - side;
                }
                rect.width = side;
                rect.height = side;
            }
            let origin = transform.apply(Vec2::new(rect.x, rect.y));
            let mut model = editor.model.borrow_mut();
            let Some(shape) = model.shape_mut(&id) else { return };
            shape.x = origin.x;
            shape.y = origin.y;
            match &mut shape.kind {
                ShapeKind::Geo(p) => {
                    p.w = rect.width.max(1.0);
                    p.h = rect.height.max(1.0);
                }
                ShapeKind::Redaction(p) => {
                    p.w = rect.width.max(1.0);
                    p.h = rect.height.max(1.0);
                }
                ShapeKind::Highlight(p) => {
                    p.w = rect.width.max(1.0);
                    p.h = rect.height.max(1.0);
                }
                ShapeKind::Numbered(p) => p.diameter = rect.width,
                ShapeKind::Text(p) => {
                    p.auto_size = false;
                    p.w = rect.width.max(TEXT_MIN_WIDTH);
                    // Text grows downward only; keep its top edge.
                    let top = original.page_transform().apply(Vec2::new(rect.x, 0.0));
                    shape.x = top.x;
                    shape.y = top.y;
                }
                _ => {}
            }
        }
        Drag::ArrowEnd { id, terminal, original } => {
            let ShapeKind::Arrow(props) = &original.kind else { return };
            let transform = original.page_transform();
            let anchor_local = if terminal == ArrowTerminal::End { props.end } else { props.start };
            let page = transform.apply(anchor_local).add(delta);
            let mut local = transform.inverse().apply(page);
            if shift {
                let other = if terminal == ArrowTerminal::End { props.start } else { props.end };
                local = other.add(snap_angle(local.sub(other)));
            }
            let mut model = editor.model.borrow_mut();
            model.current.bindings.retain(|b| !(b.arrow_id == id && b.terminal == terminal));
            if let Some(ShapeKind::Arrow(p)) = model.shape_mut(&id).map(|s| &mut s.kind) {
                if terminal == ArrowTerminal::End {
                    p.end = local;
                } else {
                    p.start = local;
                }
            }
        }
        Drag::Marquee { start, additive, .. } => {
            let current = start.add(delta);
            let rect = Bounds {
                x: start.x.min(current.x),
                y: start.y.min(current.y),
                w: (current.x - start.x).abs(),
                h: (current.y - start.y).abs(),
            };
            let mut model = editor.model.borrow_mut();
            let hits: Vec<ShapeId> = {
                let set = shape_geometry::ShapeSet { shapes: &model.current.shapes, bindings: &model.current.bindings };
                model
                    .current
                    .shapes
                    .iter()
                    .filter(|s| shape_geometry::intersects_rect(s, &set, rect))
                    .map(|s| s.id.clone())
                    .collect()
            };
            let mut selection = additive.clone();
            selection.extend(hits.into_iter().filter(|id| !additive.contains(id)));
            model.select(selection);
            drop(model);
            editor.interaction.borrow_mut().drag = Drag::Marquee { start, current, additive };
        }
        Drag::Crop { handle, start_rect, origin } => {
            let image = image_widget_rect(editor);
            let rect = match handle {
                Some(handle) => region::apply(handle, Size::new(dx, dy), start_rect, image),
                None => {
                    let end = Point::new(
                        (origin.0 + dx).clamp(image.min_x(), image.max_x()),
                        (origin.1 + dy).clamp(image.min_y(), image.max_y()),
                    );
                    let start = Point::new(
                        origin.0.clamp(image.min_x(), image.max_x()),
                        origin.1.clamp(image.min_y(), image.max_y()),
                    );
                    Rect::from_corners(start, end)
                }
            };
            let a = editor.widget_to_page(rect.min_x(), rect.min_y());
            let b = editor.widget_to_page(rect.max_x(), rect.max_y());
            editor.interaction.borrow_mut().crop = Some(Rect::new(a.x, a.y, b.x - a.x, b.y - a.y));
        }
    }
    window::refresh(editor);
}

fn drag_end(editor: &Rc<Editor>) {
    let drag = std::mem::take(&mut editor.interaction.borrow_mut().drag);
    match drag {
        Drag::Create { id, tool, .. } => {
            let mut model = editor.model.borrow_mut();
            let keep = model.shape(&id).is_some_and(|shape| match &shape.kind {
                ShapeKind::Arrow(p) => p.start.dist(p.end) >= 2.0,
                ShapeKind::Draw(_) => true,
                ShapeKind::Geo(p) => p.w >= 2.0 && p.h >= 2.0,
                ShapeKind::Redaction(p) => p.w >= 2.0 && p.h >= 2.0,
                ShapeKind::Highlight(p) => p.w >= 2.0 && p.h >= 2.0,
                _ => true,
            });
            if !keep {
                // A click with a drawing tool draws nothing.
                model.revert_to_checkpoint();
            } else {
                if let Some(ShapeKind::Draw(props)) = model.shape_mut(&id).map(|s| &mut s.kind) {
                    props.is_complete = true;
                }
                let _ = tool;
                model.select(vec![id]);
            }
        }
        Drag::Crop { .. } => {}
        Drag::Resize { .. } | Drag::ArrowEnd { .. } => {
            let mut model = editor.model.borrow_mut();
            if !model.is_dirty_since_checkpoint() {
                model.revert_to_checkpoint();
            }
        }
        _ => {}
    }
    window::refresh(editor);
}

fn double_click(editor: &Rc<Editor>, x: f64, y: f64) {
    if editor.model.borrow().tool != Tool::Select {
        return;
    }
    let page = editor.widget_to_page(x, y);
    if let Some(id) = shape_at(editor, page) {
        let is_text = editor.model.borrow().shape(&id).is_some_and(|s| matches!(s.kind, ShapeKind::Text(_)));
        if is_text {
            begin_text(editor, &id, Some(page));
        }
    }
}

// MARK: - Text

pub fn begin_text(editor: &Rc<Editor>, id: &ShapeId, at: Option<Vec2>) {
    let caret = {
        let model = editor.model.borrow();
        let Some(shape) = model.shape(id) else { return };
        let ShapeKind::Text(props) = &shape.kind else { return };
        match at {
            Some(page) => {
                let local = shape.page_transform().inverse().apply(page);
                text::index_at(props, local.x, local.y)
            }
            None => props.text.len(),
        }
    };
    {
        let mut model = editor.model.borrow_mut();
        model.checkpoint();
        model.select(vec![id.clone()]);
    }
    editor.interaction.borrow_mut().text = Some(TextEdit { id: id.clone(), caret });
    window::set_typing(editor, true);
    if let Some(area) = window::canvas(editor) {
        area.grab_focus();
    }
    window::refresh(editor);
}

/// Ends text editing; an empty text shape is removed.
pub fn finish_text(editor: &Rc<Editor>) {
    let Some(edit) = editor.interaction.borrow_mut().text.take() else { return };
    window::set_typing(editor, false);
    let mut model = editor.model.borrow_mut();
    let empty =
        model.shape(&edit.id).is_some_and(|s| matches!(&s.kind, ShapeKind::Text(p) if p.text.trim().is_empty()));
    if empty {
        model.remove_shape(&edit.id);
    }
    if !model.is_dirty_since_checkpoint() {
        model.revert_to_checkpoint();
    }
    drop(model);
    window::refresh(editor);
}

fn with_text(editor: &Rc<Editor>, f: impl FnOnce(&mut String, &mut usize)) {
    let mut interaction = editor.interaction.borrow_mut();
    let Some(edit) = interaction.text.as_mut() else { return };
    let mut model = editor.model.borrow_mut();
    if let Some(ShapeKind::Text(props)) = model.shape_mut(&edit.id).map(|s| &mut s.kind) {
        f(&mut props.text, &mut edit.caret);
        edit.caret = edit.caret.min(props.text.len());
    }
}

fn insert_text(editor: &Rc<Editor>, inserted: &str) {
    if editor.interaction.borrow().text.is_none() {
        return;
    }
    with_text(editor, |text, caret| {
        text.insert_str(*caret, inserted);
        *caret += inserted.len();
    });
    window::refresh(editor);
}

fn previous_boundary(text: &str, caret: usize) -> usize {
    text[..caret].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn next_boundary(text: &str, caret: usize) -> usize {
    text[caret..].chars().next().map_or(caret, |c| caret + c.len_utf8())
}

/// Editing keys while typing into a text shape. Returns whether it was handled.
fn text_key(editor: &Rc<Editor>, key: gdk::Key, state: gdk::ModifierType) -> bool {
    if editor.interaction.borrow().text.is_none() {
        return false;
    }
    let control = state.contains(gdk::ModifierType::CONTROL_MASK);
    match key {
        gdk::Key::Escape => finish_text(editor),
        gdk::Key::Return | gdk::Key::KP_Enter => {
            if control {
                finish_text(editor);
            } else {
                insert_text(editor, "\n");
            }
        }
        gdk::Key::BackSpace => with_text(editor, |text, caret| {
            if *caret > 0 {
                let start = previous_boundary(text, *caret);
                text.replace_range(start..*caret, "");
                *caret = start;
            }
        }),
        gdk::Key::Delete | gdk::Key::KP_Delete => with_text(editor, |text, caret| {
            if *caret < text.len() {
                let end = next_boundary(text, *caret);
                text.replace_range(*caret..end, "");
            }
        }),
        gdk::Key::Left => with_text(editor, |text, caret| *caret = previous_boundary(text, *caret)),
        gdk::Key::Right => with_text(editor, |text, caret| *caret = next_boundary(text, *caret)),
        gdk::Key::Home => with_text(editor, |text, caret| *caret = text[..*caret].rfind('\n').map_or(0, |i| i + 1)),
        gdk::Key::End => {
            with_text(editor, |text, caret| *caret = text[*caret..].find('\n').map_or(text.len(), |i| *caret + i))
        }
        gdk::Key::v if control => {
            let editor = editor.clone();
            if let Some(display) = gdk::Display::default() {
                display.clipboard().read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
                    if let Ok(Some(text)) = result {
                        insert_text(&editor, &text);
                    }
                });
            }
        }
        // Everything else, such as Ctrl+S, reaches the window's shortcuts.
        _ => return false,
    }
    window::refresh(editor);
    true
}
