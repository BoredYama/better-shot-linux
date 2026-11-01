//! The editor window, a port of `AnnotationEditorWindow.swift`: toolbar,
//! inspector on the left, canvas, and Save, Copy, and Export.
//!
//! Save commits the editable `.bettershot` sidecar and creates or atomically
//! replaces the capture's export in the save folder, even for an untouched
//! screenshot. Copy and Export never change that export.

use super::model::EditorModel;
use super::{canvas, inspector, smart_redaction, Editor};
use crate::app::App;
use crate::transfer_card::TransferCard;
use crate::{clipboard, deck_ui, sharing};
use adw::prelude::*;
use bettershot_core::annotation::background::settings_from_default_look;
use bettershot_core::annotation::document::AnnotationDocument;
use bettershot_core::annotation::shape::Tool;
use bettershot_core::annotation::store::EditPaths;
use bettershot_core::annotation::transform::ImageTransform;
use bettershot_core::capture_record::CaptureKind;
use bettershot_core::deck::{self, SaveTarget};
use bettershot_core::geometry::Rect;
use bettershot_core::imaging::{self, EncodeOptions};
use bettershot_core::shortcuts::{self, Action, Modifiers, Scope};
use bettershot_core::transfer::{Stage, TransferStatus};
use bettershot_platform::r2_client::UploadError;
use bettershot_render::{render, Scene};
use gtk::{gdk, gio, glib};
use image::RgbaImage;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct Widgets {
    pub window: adw::ApplicationWindow,
    pub area: gtk::DrawingArea,
    title: adw::WindowTitle,
    undo: gtk::Button,
    redo: gtk::Button,
    save: gtk::Button,
    tools: Vec<(Tool, gtk::ToggleButton)>,
    crop: gtk::ToggleButton,
    /// Rotate and flip; disabled while smart redaction scans.
    transforms: Vec<gtk::Button>,
    pub smart_redaction: smart_redaction::Controls,
    crop_bar: gtk::Revealer,
    zoom: gtk::Button,
    toasts: adw::ToastOverlay,
    transfer: TransferCard,
    transfer_timer: RefCell<Option<glib::SourceId>>,
    pub inspector: inspector::Inspector,
    canvas_keys: gtk::EventControllerKey,
    im: gtk::IMMulticontext,
    /// The path the editor was opened with, so the deck card that showed it updates.
    opened_from: PathBuf,
    record_id: Option<uuid::Uuid>,
}

thread_local! {
    /// Open editors, owned here until their window is destroyed; every widget
    /// callback holds only a weak reference.
    static OPEN: RefCell<Vec<(PathBuf, Rc<Editor>)>> = const { RefCell::new(Vec::new()) };
}

fn tool_icon(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "bettershot-select-symbolic",
        Tool::Rectangle => "bettershot-rectangle-symbolic",
        Tool::FilledRectangle => "bettershot-rectangle-filled-symbolic",
        Tool::Ellipse => "bettershot-ellipse-symbolic",
        Tool::Line => "bettershot-line-symbolic",
        Tool::Arrow => "bettershot-arrow-symbolic",
        Tool::Freehand => "bettershot-freehand-symbolic",
        Tool::NumberedCircle => "bettershot-numbered-symbolic",
        Tool::Text => "bettershot-text-symbolic",
        Tool::Highlight => "bettershot-highlight-symbolic",
        Tool::Pixelate => "bettershot-pixelate-symbolic",
        Tool::Blur => "bettershot-blur-symbolic",
    }
}

fn with_shortcut(app: &App, title: &str, action: Action) -> String {
    match shortcuts::effective(&app.state().prefs, action) {
        Some(shortcut) => format!("{title} ({})", shortcut.label()),
        None => title.to_owned(),
    }
}

// MARK: - Opening

/// Resolves what to edit: the capture's untouched library file. Files from
/// outside the library are imported first, so the original is never modified.
fn resolve_capture(app: &App, path: &Path) -> Result<(PathBuf, Option<uuid::Uuid>), String> {
    let mut state = app.state_mut();
    let state = &mut *state;
    let path =
        if state.deck.is_staged(path) { state.deck.retain(path, &mut state.history) } else { path.to_path_buf() };
    if let Some(record) = state.history.record_matching(&path) {
        return Ok((state.history.path_for_record(record), Some(record.id)));
    }
    let record = state
        .history
        .import_capture(&path, None, false, CaptureKind::Screenshot, None)
        .map_err(|e| format!("The image could not be added to the library: {e}"))?;
    Ok((state.history.path_for_record(&record), Some(record.id)))
}

/// Opens the editor for a capture or image file, reusing an open window.
pub fn open(app: &App, path: &Path) {
    let (capture, record_id) = match resolve_capture(app, path) {
        Ok(resolved) => resolved,
        Err(error) => return app.notify_error("Couldn’t open the editor", &error),
    };
    let existing =
        OPEN.with(|open| open.borrow().iter().find(|(p, _)| *p == capture).map(|(_, editor)| editor.clone()));
    if let Some(editor) = existing {
        if let Some(widgets) = editor.widgets.borrow().as_ref() {
            widgets.window.present();
        }
        return;
    }
    let paths = EditPaths::for_capture(&capture);
    let (document, base_path) = paths.load().unwrap_or_else(|| {
        let background = app.with_prefs(|p| settings_from_default_look(&p.default_beautifier_config()));
        (AnnotationDocument::new(Vec::new(), Vec::new(), background), capture.clone())
    });
    let base = match imaging::load_rgba(&base_path) {
        Ok(base) => base,
        Err(error) => return app.notify_error("Couldn’t open the image", &error.to_string()),
    };
    let scratch = app.state().dirs.scratch_folder("Editor");
    let model = EditorModel::new(capture.clone(), document, base_path, base, scratch);
    let editor = Rc::new(Editor::new(model));
    build(app, &editor, path.to_path_buf(), record_id);
    OPEN.with(|open| open.borrow_mut().push((capture, editor.clone())));
}

/// Open Image from File.
pub fn open_file_dialog(app: &App) {
    let app = app.clone();
    glib::spawn_future_local(async move {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        for mime in ["image/png", "image/jpeg", "image/webp", "image/bmp", "image/gif", "image/tiff"] {
            filter.add_mime_type(mime);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("Open Image").filters(&filters).modal(true).build();
        if let Ok(file) = dialog.open_future(None::<&gtk::Window>).await {
            if let Some(path) = file.path() {
                open(&app, &path);
            }
        }
    });
}

// MARK: - Building

fn build(app: &App, editor: &Rc<Editor>, opened_from: PathBuf, record_id: Option<uuid::Uuid>) {
    let name = capture_display_name(app, &editor.model.borrow().capture);
    let window = adw::ApplicationWindow::builder()
        .application(&app.application)
        .title(format!("{name} — BetterShot"))
        .default_width(1180)
        .default_height(800)
        .build();
    window.add_css_class("image-editor");

    let title = adw::WindowTitle::new(&name, "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    let icon_button = |icon: &str, label: String| {
        let button = gtk::Button::from_icon_name(icon);
        button.set_tooltip_text(Some(&label));
        button.update_property(&[gtk::accessible::Property::Label(&label)]);
        button
    };
    let undo = icon_button("edit-undo-symbolic", with_shortcut(app, "Undo", Action::ImageUndo));
    let redo = icon_button("edit-redo-symbolic", with_shortcut(app, "Redo", Action::ImageRedo));
    header.pack_start(&undo);
    header.pack_start(&redo);
    let save = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).build();
    save.set_tooltip_text(Some(&with_shortcut(app, "Save edits and update the saved file", Action::ImageSave)));
    let export = icon_button("document-save-as-symbolic", "Export As…".into());
    let copy = icon_button("edit-copy-symbolic", with_shortcut(app, "Copy", Action::ImageCopy));
    let share = icon_button("bettershot-share-symbolic", with_shortcut(app, "Share to Cloud", Action::ImageShare));
    header.pack_end(&save);
    header.pack_end(&export);
    header.pack_end(&share);
    header.pack_end(&copy);

    // Tools, then image transforms and crop.
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar.add_css_class("toolbar");
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    let mut tools = Vec::new();
    for tool in Tool::ALL {
        let button = gtk::ToggleButton::builder().icon_name(tool_icon(tool)).css_classes(["flat"]).build();
        let label = with_shortcut(app, tool.help_text(), tool.shortcut_action());
        button.set_tooltip_text(Some(&label));
        button.update_property(&[gtk::accessible::Property::Label(tool.title())]);
        let weak = Rc::downgrade(editor);
        button.connect_clicked(move |_| {
            if let Some(editor) = weak.upgrade() {
                choose_tool(&editor, tool);
            }
        });
        toolbar.append(&button);
        tools.push((tool, button));
    }
    let smart = smart_redaction::build(editor);
    toolbar.append(&smart.button);
    toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    let crop = gtk::ToggleButton::builder().icon_name("bettershot-crop-symbolic").css_classes(["flat"]).build();
    crop.set_tooltip_text(Some(&with_shortcut(app, "Crop", Action::ImageCrop)));
    crop.update_property(&[gtk::accessible::Property::Label("Crop")]);
    toolbar.append(&crop);
    let mut transforms = Vec::new();
    for (transform, icon) in [
        (ImageTransform::RotateLeft, "object-rotate-left-symbolic"),
        (ImageTransform::RotateRight, "object-rotate-right-symbolic"),
        (ImageTransform::FlipHorizontal, "object-flip-horizontal-symbolic"),
        (ImageTransform::FlipVertical, "object-flip-vertical-symbolic"),
    ] {
        let button = gtk::Button::from_icon_name(icon);
        transforms.push(button.clone());
        button.add_css_class("flat");
        button.set_tooltip_text(Some(transform.title()));
        button.update_property(&[gtk::accessible::Property::Label(transform.title())]);
        let weak = Rc::downgrade(editor);
        button.connect_clicked(move |_| {
            if let Some(editor) = weak.upgrade() {
                apply_transform(&editor, transform);
            }
        });
        toolbar.append(&button);
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);
    let zoom_out = icon_button("zoom-out-symbolic", with_shortcut(app, "Zoom Out", Action::ImageZoomOut));
    let zoom = gtk::Button::builder().label("100%").css_classes(["flat", "numeric"]).build();
    zoom.set_tooltip_text(Some(&with_shortcut(app, "Fit Canvas", Action::ImageFit)));
    let zoom_in = icon_button("zoom-in-symbolic", with_shortcut(app, "Zoom In", Action::ImageZoomIn));
    for button in [&zoom_out, &zoom, &zoom_in] {
        button.add_css_class("flat");
        toolbar.append(button);
    }

    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_focusable(true);
    area.set_can_focus(true);
    area.update_property(&[gtk::accessible::Property::Label("Image canvas")]);
    let im = gtk::IMMulticontext::new();
    let canvas_keys = canvas::install(editor, &area, &im);

    // The crop bar: Apply or Cancel, also Return and Escape.
    let crop_bar_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    crop_bar_box.add_css_class("toolbar");
    crop_bar_box.add_css_class("osd");
    crop_bar_box.set_halign(gtk::Align::Center);
    crop_bar_box.set_valign(gtk::Align::End);
    crop_bar_box.set_margin_bottom(18);
    let crop_hint = gtk::Label::new(Some("Drag to choose the area to keep"));
    let crop_cancel = gtk::Button::with_label("Cancel");
    let crop_apply = gtk::Button::builder().label("Crop").css_classes(["suggested-action"]).build();
    crop_bar_box.append(&crop_hint);
    crop_bar_box.append(&crop_cancel);
    crop_bar_box.append(&crop_apply);
    let crop_bar = gtk::Revealer::builder()
        .child(&crop_bar_box)
        .transition_type(gtk::RevealerTransitionType::Crossfade)
        .transition_duration(150)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .build();
    let canvas_overlay = gtk::Overlay::new();
    canvas_overlay.set_child(Some(&area));
    canvas_overlay.add_overlay(&crop_bar);
    // Share progress sits at the top, over the canvas edge, like macOS's toast.
    let transfer = TransferCard::new(false);
    transfer.root.set_halign(gtk::Align::Center);
    transfer.root.set_valign(gtk::Align::Start);
    transfer.root.set_margin_top(12);
    transfer.root.set_width_request(360);
    canvas_overlay.add_overlay(&transfer.root);
    let connect_transfer = |f: fn(&Rc<Editor>)| {
        let weak = Rc::downgrade(editor);
        move || {
            if let Some(editor) = weak.upgrade() {
                f(&editor);
            }
        }
    };
    transfer.connect_cancel(connect_transfer(cancel_share));
    transfer.connect_dismiss(connect_transfer(|e| set_transfer(e, None)));
    transfer.connect_retry(connect_transfer(share_image));
    transfer.connect_settings(|| crate::settings::present_sharing(&App::get()));

    let inspector = inspector::build(editor, &window);
    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&inspector.root)
        .end_child(&canvas_overlay)
        .resize_start_child(false)
        .shrink_start_child(false)
        .position(300)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&toolbar);
    body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    body.append(&paned);
    paned.set_vexpand(true);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&body));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&toasts));
    window.set_content(Some(&view));

    *editor.widgets.borrow_mut() = Some(Widgets {
        window: window.clone(),
        area: area.clone(),
        title,
        undo: undo.clone(),
        redo: redo.clone(),
        save: save.clone(),
        tools,
        crop: crop.clone(),
        transforms,
        smart_redaction: smart,
        crop_bar,
        zoom: zoom.clone(),
        toasts,
        transfer,
        transfer_timer: RefCell::new(None),
        inspector,
        canvas_keys,
        im,
        opened_from,
        record_id,
    });

    let connect = |button: &gtk::Button, f: fn(&Rc<Editor>)| {
        let weak = Rc::downgrade(editor);
        button.connect_clicked(move |_| {
            if let Some(editor) = weak.upgrade() {
                f(&editor);
            }
        });
    };
    connect(&undo, |e| undo_redo(e, true));
    connect(&redo, |e| undo_redo(e, false));
    connect(&save, |e| save_edits(e, false));
    connect(&copy, copy_image);
    connect(&share, share_image);
    connect(&export, export_as);
    connect(&zoom, |e| zoom_action(e, Action::ImageFit));
    connect(&zoom_in, |e| zoom_action(e, Action::ImageZoomIn));
    connect(&zoom_out, |e| zoom_action(e, Action::ImageZoomOut));
    connect(&crop_cancel, |e| set_crop(e, false));
    connect(&crop_apply, apply_crop);
    let weak = Rc::downgrade(editor);
    crop.connect_clicked(move |button| {
        if let Some(editor) = weak.upgrade() {
            set_crop(&editor, button.is_active());
        }
    });

    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(editor);
    keys.connect_key_pressed(move |_, key, _, state| match weak.upgrade() {
        Some(editor) => on_key(&editor, key, state),
        None => glib::Propagation::Proceed,
    });
    window.add_controller(keys);

    let weak = Rc::downgrade(editor);
    window.connect_close_request(move |window| {
        let Some(editor) = weak.upgrade() else { return glib::Propagation::Proceed };
        canvas::finish_text(&editor);
        if !editor.model.borrow().is_dirty() {
            cleanup(&editor);
            return glib::Propagation::Proceed;
        }
        confirm_close(&editor, window);
        glib::Propagation::Stop
    });

    let weak = Rc::downgrade(editor);
    window.connect_destroy(move |_| {
        if let Some(upload) = weak.upgrade().and_then(|e| e.share_upload.borrow_mut().take()) {
            upload.cancel();
        }
        OPEN.with(|open| open.borrow_mut().retain(|(_, e)| !weak.upgrade().is_some_and(|w| Rc::ptr_eq(&w, e))));
    });

    refresh(editor);
    window.present();
    area.grab_focus();
}

fn capture_display_name(app: &App, capture: &Path) -> String {
    let state = app.state();
    state
        .history
        .record_matching(capture)
        .map(|r| r.display_name().to_owned())
        .unwrap_or_else(|| capture.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

fn cleanup(editor: &Rc<Editor>) {
    let scratch = editor.model.borrow().base_path().parent().map(Path::to_path_buf);
    if let Some(scratch) = scratch.filter(|dir| dir.to_string_lossy().contains("/Editor/")) {
        let _ = std::fs::remove_dir_all(scratch);
    }
}

fn confirm_close(editor: &Rc<Editor>, window: &adw::ApplicationWindow) {
    let name = editor.widgets.borrow().as_ref().map(|w| w.title.title().to_string()).unwrap_or_default();
    let dialog = adw::AlertDialog::new(
        Some(&format!("Save changes to “{name}”?")),
        Some("Your edits will be lost if you close without saving."),
    );
    dialog.add_responses(&[("cancel", "Cancel"), ("discard", "Discard"), ("save", "Save")]);
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    let editor = editor.clone();
    let window = window.clone();
    dialog.choose(Some(&window.clone()), None::<&gio::Cancellable>, move |response| match response.as_str() {
        "save" => save_edits(&editor, true),
        "discard" => {
            editor.model.borrow_mut().mark_saved();
            cleanup(&editor);
            window.close();
        }
        _ => {}
    });
}

// MARK: - Refresh

pub fn canvas(editor: &Editor) -> Option<gtk::DrawingArea> {
    editor.widgets.borrow().as_ref().map(|w| w.area.clone())
}

pub fn queue_draw(editor: &Editor) {
    if let Some(area) = canvas(editor) {
        area.queue_draw();
    }
}

/// Routes keys through the input method while typing into a text shape.
pub fn set_typing(editor: &Editor, typing: bool) {
    if let Some(widgets) = editor.widgets.borrow().as_ref() {
        if typing {
            widgets.canvas_keys.set_im_context(Some(&widgets.im));
            widgets.im.focus_in();
        } else {
            widgets.im.reset();
            widgets.canvas_keys.set_im_context(None::<&gtk::IMContext>);
        }
    }
}

pub fn inspector_updating(editor: &Editor) -> bool {
    editor.widgets.borrow().as_ref().is_some_and(|w| w.inspector.is_updating())
}

/// Refits the canvas after the image changes size (crop, rotation).
fn refit(editor: &Editor) {
    if let Some(area) = canvas(editor) {
        editor.fit(f64::from(area.width()), f64::from(area.height()));
    }
}

pub fn refresh_zoom(editor: &Editor) {
    if let Some(widgets) = editor.widgets.borrow().as_ref() {
        widgets.zoom.set_label(&format!("{:.0}%", editor.view.get().zoom * 100.0));
    }
}

/// Redraws and brings the toolbar and inspector in line with the model.
pub fn refresh(editor: &Rc<Editor>) {
    let widgets = editor.widgets.borrow();
    let Some(w) = widgets.as_ref() else { return };
    w.area.queue_draw();
    {
        let model = editor.model.borrow();
        w.undo.set_sensitive(model.can_undo());
        w.redo.set_sensitive(model.can_redo());
        for (tool, button) in &w.tools {
            button.set_active(*tool == model.tool);
        }
        let cropping = editor.interaction.borrow().crop.is_some();
        w.crop.set_active(cropping);
        // Like macOS, the image can't turn or crop while smart redaction scans it.
        let scanning = w.smart_redaction.is_running();
        w.crop.set_sensitive(!scanning);
        for button in &w.transforms {
            button.set_sensitive(!scanning);
        }
        w.crop_bar.set_reveal_child(cropping);
        let (width, height) = model.base_size();
        let mut subtitle = format!("{width:.0} × {height:.0}");
        if model.is_dirty() {
            subtitle.push_str(" · Edited");
        }
        w.title.set_subtitle(&subtitle);
    }
    w.inspector.sync(editor);
    drop(widgets);
    refresh_zoom(editor);
}

fn toast(editor: &Editor, message: &str) {
    if let Some(widgets) = editor.widgets.borrow().as_ref() {
        let toast = adw::Toast::new(message);
        toast.set_timeout(3);
        widgets.toasts.add_toast(toast);
    }
}

// MARK: - Actions

/// A second click on the active tool returns to Select.
fn choose_tool(editor: &Rc<Editor>, tool: Tool) {
    canvas::finish_text(editor);
    {
        let mut model = editor.model.borrow_mut();
        model.tool = if model.tool == tool && tool != Tool::Select { Tool::Select } else { tool };
    }
    editor.interaction.borrow_mut().crop = None;
    refresh(editor);
}

fn undo_redo(editor: &Rc<Editor>, undo: bool) {
    canvas::finish_text(editor);
    editor.end_coalescing();
    let changed = if undo { editor.model.borrow_mut().undo() } else { editor.model.borrow_mut().redo() };
    if changed {
        refresh(editor);
    }
}

fn is_scanning(editor: &Editor) -> bool {
    editor.widgets.borrow().as_ref().is_some_and(|w| w.smart_redaction.is_running())
}

fn apply_transform(editor: &Rc<Editor>, transform: ImageTransform) {
    if is_scanning(editor) {
        return;
    }
    canvas::finish_text(editor);
    editor.interaction.borrow_mut().crop = None;
    let result = editor.model.borrow_mut().transform(transform);
    if let Err(error) = result {
        toast(editor, &format!("Couldn’t {}: {error}", transform.title().to_lowercase()));
    }
    refit(editor);
    refresh(editor);
}

fn set_crop(editor: &Rc<Editor>, on: bool) {
    if on && is_scanning(editor) {
        refresh(editor);
        return;
    }
    canvas::finish_text(editor);
    let full = {
        let (w, h) = editor.model.borrow().base_size();
        Rect::new(0.0, 0.0, w, h)
    };
    editor.interaction.borrow_mut().crop = on.then_some(full);
    refresh(editor);
}

fn apply_crop(editor: &Rc<Editor>) {
    let Some(rect) = editor.interaction.borrow_mut().crop.take() else { return };
    let (w, h) = editor.model.borrow().base_size();
    let x0 = rect.min_x().round().clamp(0.0, w);
    let y0 = rect.min_y().round().clamp(0.0, h);
    let x1 = rect.max_x().round().clamp(0.0, w);
    let y1 = rect.max_y().round().clamp(0.0, h);
    if x1 - x0 >= 1.0 && y1 - y0 >= 1.0 && (x0, y0, x1, y1) != (0.0, 0.0, w, h) {
        let result = editor.model.borrow_mut().crop(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
        if let Err(error) = result {
            toast(editor, &format!("Couldn’t crop: {error}"));
        }
        refit(editor);
    }
    refresh(editor);
}

fn zoom_action(editor: &Rc<Editor>, action: Action) {
    let Some(area) = canvas(editor) else { return };
    let (w, h) = (f64::from(area.width()), f64::from(area.height()));
    match action {
        Action::ImageZoomIn => editor.zoom_at(1.25, w / 2.0, h / 2.0),
        Action::ImageZoomOut => editor.zoom_at(1.0 / 1.25, w / 2.0, h / 2.0),
        Action::ImageActualSize => editor.set_zoom(1.0, w, h),
        _ => editor.fit(w, h),
    }
    refresh(editor);
}

fn nudge(editor: &Rc<Editor>, dx: f64, dy: f64) {
    {
        let mut model = editor.model.borrow_mut();
        if model.selection.is_empty() {
            return;
        }
        drop(model);
        editor.checkpoint_coalesced("nudge");
        model = editor.model.borrow_mut();
        let selection = model.selection.clone();
        for id in selection {
            if let Some(shape) = model.shape_mut(&id) {
                shape.x += dx;
                shape.y += dy;
            }
        }
    }
    refresh(editor);
}

fn change_size(editor: &Rc<Editor>, grow: bool) {
    editor.checkpoint_coalesced("size");
    {
        let mut model = editor.model.borrow_mut();
        let step = if grow { 1.0 } else { -1.0 };
        let style = &mut model.style;
        style.stroke_width = (style.stroke_width + step * 2.0).clamp(1.0, 40.0);
        style.font_size = (style.font_size + step * 4.0).clamp(8.0, 240.0);
        style.diameter = (style.diameter + step * 8.0).clamp(20.0, 240.0);
        model.apply_style_to_selection_unrecorded();
    }
    refresh(editor);
}

fn focus_is_editable(window: &adw::ApplicationWindow) -> bool {
    gtk::prelude::GtkWindowExt::focus(window).is_some_and(|w| {
        w.is::<gtk::Text>()
            || w.is::<gtk::TextView>()
            || w.is::<gtk::SpinButton>()
            || w.ancestor(gtk::SpinButton::static_type()).is_some()
    })
}

fn on_key(editor: &Rc<Editor>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
    let Some(window) = editor.widgets.borrow().as_ref().map(|w| w.window.clone()) else {
        return glib::Propagation::Proceed;
    };
    let mut modifiers = Modifiers::NONE;
    for (mask, flag) in [
        (gdk::ModifierType::SHIFT_MASK, Modifiers::SHIFT),
        (gdk::ModifierType::CONTROL_MASK, Modifiers::CONTROL),
        (gdk::ModifierType::ALT_MASK, Modifiers::ALT),
        (gdk::ModifierType::SUPER_MASK, Modifiers::SUPER),
    ] {
        if state.contains(mask) {
            modifiers = modifiers | flag;
        }
    }
    if focus_is_editable(&window) && !modifiers.contains(Modifiers::CONTROL) {
        return glib::Propagation::Proceed;
    }
    if editor.interaction.borrow().text.is_some() && !modifiers.contains(Modifiers::CONTROL) {
        return glib::Propagation::Proceed;
    }
    let cropping = editor.interaction.borrow().crop.is_some();
    match key {
        gdk::Key::Return | gdk::Key::KP_Enter if cropping => {
            apply_crop(editor);
            return glib::Propagation::Stop;
        }
        gdk::Key::Escape => {
            if cropping {
                set_crop(editor, false);
            } else if !editor.model.borrow().selection.is_empty() {
                editor.model.borrow_mut().select(Vec::new());
                refresh(editor);
            } else if editor.model.borrow().tool != Tool::Select {
                choose_tool(editor, Tool::Select);
            } else {
                return glib::Propagation::Proceed;
            }
            return glib::Propagation::Stop;
        }
        gdk::Key::Left | gdk::Key::Right | gdk::Key::Up | gdk::Key::Down
            if modifiers.bits() & !Modifiers::SHIFT.bits() == 0 =>
        {
            if editor.model.borrow().selection.is_empty() {
                return glib::Propagation::Proceed;
            }
            let step = if modifiers.contains(Modifiers::SHIFT) { 10.0 } else { 1.0 };
            let (dx, dy) = match key {
                gdk::Key::Left => (-step, 0.0),
                gdk::Key::Right => (step, 0.0),
                gdk::Key::Up => (0.0, -step),
                _ => (0.0, step),
            };
            nudge(editor, dx, dy);
            return glib::Propagation::Stop;
        }
        _ => {}
    }
    let Some(name) = key.to_lower().name() else { return glib::Propagation::Proceed };
    let action = shortcuts::action_for(&App::get().state().prefs, &name, modifiers, Scope::Image);
    let Some(action) = action else { return glib::Propagation::Proceed };
    if let Some(tool) = Tool::for_action(action) {
        choose_tool(editor, tool);
        return glib::Propagation::Stop;
    }
    match action {
        Action::ImageCrop => {
            let on = !cropping;
            set_crop(editor, on);
        }
        Action::ImageSave => save_edits(editor, false),
        Action::ImageExport => export_as(editor),
        Action::ImageCopy => copy_image(editor),
        Action::ImageShare => share_image(editor),
        Action::ImageUndo => undo_redo(editor, true),
        Action::ImageRedo => undo_redo(editor, false),
        Action::ImageSelectAll => {
            canvas::finish_text(editor);
            editor.model.borrow_mut().select_all();
            refresh(editor);
        }
        Action::ImageDelete => {
            if editor.model.borrow_mut().delete_selection() {
                refresh(editor);
            }
        }
        Action::ImageZoomIn | Action::ImageZoomOut | Action::ImageFit | Action::ImageActualSize => {
            zoom_action(editor, action)
        }
        Action::ImageIncreaseSize => change_size(editor, true),
        Action::ImageDecreaseSize => change_size(editor, false),
        Action::ImageRotateLeft => apply_transform(editor, ImageTransform::RotateLeft),
        Action::ImageRotateRight => apply_transform(editor, ImageTransform::RotateRight),
        Action::ImageFlipHorizontal => apply_transform(editor, ImageTransform::FlipHorizontal),
        Action::ImageFlipVertical => apply_transform(editor, ImageTransform::FlipVertical),
        _ => return glib::Propagation::Proceed,
    }
    glib::Propagation::Stop
}

// MARK: - Rendering and saving

/// An owned copy of the scene, for rendering off the main thread.
struct RenderJob {
    base: RgbaImage,
    generation: u64,
    shapes: Vec<bettershot_core::annotation::shape::AnnoShape>,
    bindings: Vec<bettershot_core::annotation::shape::ArrowBinding>,
    background: bettershot_core::annotation::background::BackgroundSettings,
}

impl RenderJob {
    fn from(editor: &Editor) -> Self {
        let model = editor.model.borrow();
        let current = &model.current;
        Self {
            base: (*current.base).clone(),
            generation: current.generation,
            shapes: current.shapes.clone(),
            bindings: current.bindings.clone(),
            background: current.background.clone(),
        }
    }

    async fn render(self) -> Result<RgbaImage, String> {
        gio::spawn_blocking(move || {
            let scene = Scene {
                base: &self.base,
                base_generation: self.generation,
                shapes: &self.shapes,
                bindings: &self.bindings,
                background: &self.background,
            };
            render(&scene).map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "Rendering stopped unexpectedly.".to_owned())?
    }
}

fn set_transfer(editor: &Rc<Editor>, status: Option<TransferStatus>) {
    let widgets = editor.widgets.borrow();
    let Some(w) = widgets.as_ref() else { return };
    if let Some(id) = w.transfer_timer.borrow_mut().take() {
        id.remove();
    }
    w.transfer.set_status(status.as_ref());
    if let Some(delay) = status.as_ref().and_then(TransferStatus::auto_dismiss) {
        let weak = Rc::downgrade(editor);
        let id = glib::timeout_add_local_once(delay, move || {
            if let Some(editor) = weak.upgrade() {
                if let Some(w) = editor.widgets.borrow().as_ref() {
                    w.transfer_timer.borrow_mut().take();
                }
                set_transfer(&editor, None);
            }
        });
        *w.transfer_timer.borrow_mut() = Some(id);
    }
}

fn cancel_share(editor: &Rc<Editor>) {
    if let Some(upload) = editor.share_upload.borrow_mut().take() {
        upload.cancel();
    }
    set_transfer(editor, None);
}

/// Share to Cloud: uploads the edited image (not the saved export, which
/// Share never changes), records the link with the capture, and copies it.
fn share_image(editor: &Rc<Editor>) {
    if editor.share_upload.borrow().is_some() {
        return;
    }
    canvas::finish_text(editor);
    let app = App::get();
    if let Some(blocked) = sharing::blocker(&app) {
        set_transfer(editor, Some(blocked));
        return;
    }
    set_transfer(editor, Some(TransferStatus::Working { stage: Stage::Rendering, progress: None }));
    let editor = editor.clone();
    glib::spawn_future_local(async move {
        let app = App::get();
        let name = capture_name(&app, &editor);
        let rendered = RenderJob::from(&editor)
            .render()
            .await
            .and_then(|image| write_scratch_png(&app, &image, &format!("{name}.png"), "ShareRender"));
        let path = match rendered {
            Ok(path) => path,
            Err(error) => {
                set_transfer(&editor, Some(TransferStatus::failed("Couldn’t prepare capture", &error)));
                return;
            }
        };
        let upload = match sharing::start(&app, &path, &name, None) {
            Ok(upload) => Rc::new(upload),
            Err(status) => {
                set_transfer(&editor, Some(status));
                return;
            }
        };
        set_transfer(&editor, Some(TransferStatus::Working { stage: Stage::Processing, progress: None }));
        *editor.share_upload.borrow_mut() = Some(upload.clone());
        let record_id = {
            let capture = editor.model.borrow().capture.clone();
            app.state().history.record_matching(&capture).map(|r| r.id)
        };
        while let Ok(event) = upload.events.recv().await {
            if !editor.share_upload.borrow().as_ref().is_some_and(|u| Rc::ptr_eq(u, &upload)) {
                break;
            }
            match event {
                sharing::Event::Progress(fraction) => set_transfer(
                    &editor,
                    Some(TransferStatus::Working { stage: Stage::Uploading, progress: Some(fraction) }),
                ),
                sharing::Event::Done(result) => {
                    editor.share_upload.borrow_mut().take();
                    match result {
                        Ok(uploaded) => {
                            if let Some(id) = record_id {
                                if let Err(error) =
                                    app.state_mut().history.set_share_url(id, Some(uploaded.link.clone()))
                                {
                                    eprintln!("bettershot: the share link could not be saved to the library: {error}");
                                }
                            }
                            let _ = clipboard::copy_text(&uploaded.link);
                            set_transfer(&editor, Some(TransferStatus::LinkReady { url: uploaded.link }));
                            crate::gallery::reload();
                        }
                        Err(UploadError::Cancelled) => set_transfer(&editor, None),
                        Err(UploadError::Failed(message)) => {
                            set_transfer(&editor, Some(TransferStatus::failed("Upload failed", &message)))
                        }
                    }
                    break;
                }
            }
        }
        if let Some(folder) = path.parent() {
            let _ = std::fs::remove_dir_all(folder);
        }
    });
}

fn write_scratch_png(app: &App, image: &RgbaImage, name: &str, purpose: &str) -> Result<PathBuf, String> {
    let folder = app.state().dirs.scratch_folder(purpose);
    imaging::write_new_image(image, EncodeOptions::default(), name, &folder).map_err(|e| e.to_string())
}

fn capture_name(app: &App, editor: &Editor) -> String {
    let capture = editor.model.borrow().capture.clone();
    deck::capture_name(&capture, &app.state().history)
}

/// Commits the sidecar and creates or replaces the capture's export.
fn commit_and_export(app: &App, editor: &Editor, rendered: Option<PathBuf>) -> Result<PathBuf, String> {
    let (capture, paths, base_path, document) = {
        let model = editor.model.borrow();
        (model.capture.clone(), model.paths.clone(), model.base_path().to_path_buf(), model.document())
    };
    let annotated = rendered.is_some();
    let display = match rendered {
        Some(rendered) => {
            paths.commit(&base_path, &rendered, document).map_err(|e| format!("The edits could not be saved: {e}"))?;
            let _ = std::fs::remove_file(&rendered);
            paths.display.clone()
        }
        None => {
            // Everything was cleared: the capture is untouched again.
            paths.remove().map_err(|e| e.to_string())?;
            capture.clone()
        }
    };
    let mut state = app.state_mut();
    let state = &mut *state;
    let app_prefs = bettershot_core::app_prefs::AppPreferences { prefs: &state.prefs, dirs: &state.dirs };
    let options = app_prefs.encode_options();
    let target = SaveTarget { directory: app_prefs.save_directory(), quality: options.quality };
    let record_id = state.history.record_matching(&capture).map(|r| r.id);
    let destination = match state.history.annotation_export_path(&capture) {
        Some(existing) => {
            imaging::replace_export(&display, &existing, target.quality).map_err(|e| e.to_string())?;
            existing
        }
        None => {
            let name = format!("{}.{}", deck::capture_name(&capture, &state.history), options.format.file_extension());
            deck::save_to_default_location(&display, &name, &target).map_err(|e| e.to_string())?
        }
    };
    if let Some(id) = record_id {
        let _ = state.history.set_beautified_path(&destination, id);
        let _ = state.history.set_has_annotations(id, annotated);
    }
    Ok(destination)
}

/// Save: commit edits and update the saved file. Closing after saving when
/// `then_close`.
fn save_edits(editor: &Rc<Editor>, then_close: bool) {
    canvas::finish_text(editor);
    editor.end_coalescing();
    let Some((save_button, window, opened_from, record_id)) = editor
        .widgets
        .borrow()
        .as_ref()
        .map(|w| (w.save.clone(), w.window.clone(), w.opened_from.clone(), w.record_id))
    else {
        return;
    };
    save_button.set_sensitive(false);
    let editor = editor.clone();
    glib::spawn_future_local(async move {
        let app = App::get();
        let has_edits = editor.model.borrow().has_edits();
        let job = RenderJob::from(&editor);
        let saved_state = editor.model.borrow().current.clone();
        let rendered = if has_edits {
            match job.render().await.and_then(|image| write_scratch_png(&app, &image, "render.png", "Render")) {
                Ok(path) => Some(path),
                Err(error) => {
                    save_button.set_sensitive(true);
                    return save_failed(&editor, &error);
                }
            }
        } else {
            None
        };
        match commit_and_export(&app, &editor, rendered) {
            Ok(destination) => {
                {
                    let mut model = editor.model.borrow_mut();
                    // Edits made while rendering stay unsaved.
                    if model.current == saved_state {
                        model.mark_saved();
                    }
                }
                deck_ui::retarget(&app, record_id, &opened_from, &destination);
                app.state_mut().last_capture = Some(destination.clone());
                let folder = destination
                    .parent()
                    .and_then(Path::file_name)
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                toast(&editor, &format!("Saved to {folder}"));
                save_button.set_sensitive(true);
                refresh(&editor);
                if then_close {
                    cleanup(&editor);
                    window.close();
                }
            }
            Err(error) => {
                save_button.set_sensitive(true);
                save_failed(&editor, &error);
            }
        }
    });
}

/// A failed save stays visible with a way to retry.
fn save_failed(editor: &Rc<Editor>, error: &str) {
    let Some(widgets) = editor.widgets.borrow().as_ref().map(|w| w.toasts.clone()) else { return };
    let toast = adw::Toast::builder().title(format!("Couldn’t save. {error}")).button_label("Retry").timeout(0).build();
    let weak = Rc::downgrade(editor);
    toast.connect_button_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            save_edits(&editor, false);
        }
    });
    widgets.add_toast(toast);
}

/// Copy the edited image; never changes the saved file.
fn copy_image(editor: &Rc<Editor>) {
    canvas::finish_text(editor);
    let editor = editor.clone();
    glib::spawn_future_local(async move {
        let app = App::get();
        let name = format!("{}.png", capture_name(&app, &editor));
        let result = RenderJob::from(&editor)
            .render()
            .await
            .and_then(|image| write_scratch_png(&app, &image, &name, "Clipboard"))
            .and_then(|path| clipboard::copy_file(&path));
        match result {
            Ok(()) => toast(&editor, "Copied"),
            Err(error) => toast(&editor, &format!("Couldn’t copy. {error}")),
        }
    });
}

/// Export As: a new file anywhere, in PNG or JPEG by its extension.
fn export_as(editor: &Rc<Editor>) {
    canvas::finish_text(editor);
    let Some(window) = editor.widgets.borrow().as_ref().map(|w| w.window.clone()) else { return };
    let editor = editor.clone();
    glib::spawn_future_local(async move {
        let app = App::get();
        let (directory, format) = app.with_prefs(|p| (p.save_directory(), p.export_format()));
        let name = format!("{}.{}", capture_name(&app, &editor), format.file_extension());
        let dialog = gtk::FileDialog::builder().title("Export Image").initial_name(&name).modal(true).build();
        if directory.is_dir() {
            dialog.set_initial_folder(Some(&gio::File::for_path(&directory)));
        }
        let Ok(file) = dialog.save_future(Some(&window)).await else { return };
        let Some(destination) = file.path() else { return };
        let quality = app.with_prefs(|p| p.export_quality());
        let result = RenderJob::from(&editor)
            .render()
            .await
            .and_then(|image| write_scratch_png(&app, &image, "export.png", "Render"))
            .and_then(|rendered| {
                let result = imaging::replace_export(&rendered, &destination, quality).map_err(|e| e.to_string());
                let _ = std::fs::remove_file(&rendered);
                result
            });
        match result {
            Ok(()) => toast(
                &editor,
                &format!("Exported {}", destination.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()),
            ),
            Err(error) => toast(&editor, &format!("Couldn’t export. {error}")),
        }
    });
}
