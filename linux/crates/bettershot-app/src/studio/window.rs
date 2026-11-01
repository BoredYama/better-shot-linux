//! The studio window: header (Undo, Redo, Save, Export As…), the inspector
//! on the left, the preview in the middle with its transport, and the
//! timeline below. Interactions follow `RecordingStudioWindow.swift` and
//! `RecordingClipTimelineView.swift`: Space plays, S toggles Scissors (which
//! stays active across cuts), Delete removes the selected clip, and Save
//! re-renders the saved file while Export As always writes a new one.

use super::model::StudioModel;
use crate::app::App;
use crate::editor::inspector::{background_tile, dropdown, heading, slider, Slider};
use adw::prelude::*;
use bettershot_core::annotation::background::{BgColor, BgGradient, BgStyle};
use bettershot_core::app_prefs::AppPreferences;
use bettershot_core::geometry::{Point, Rect, Size};
use bettershot_core::recording::edit::{
    CameraAspectRatio, CompressionQuality, CompressionResolution, CursorAppearance, ExportAspectContentMode,
    ExportAspectPreset, ExportFrameRate, KeystrokePlacement, LayoutPreset, MaskEffect, MaskSegment, StudioStyle,
    SubtitleBarStyle, UnitRect,
};
use bettershot_core::recording::shots3d::{BlurMode, Easing3D};
use bettershot_core::recording::viewport::{UnitPoint, ZoomAnchorMode};
use bettershot_core::recording::{
    self, deliverable, Blur3D, ClipSegment, Preset3D, RecordingSession, Shot3D, StylePresetStore, VideoExportContainer,
    ZoomCue,
};
use bettershot_core::region::{self, RegionHandle};
use bettershot_platform::media;
use bettershot_platform::video::{self, AudioPlayer, ExportError};
use gtk::{cairo, gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TIMELINE_HEIGHT: i32 = 96;
const HANDLE_SIZE: f64 = 8.0;
const SELECTION: (f64, f64, f64) = (0.0, 0.48, 1.0);

/// A drag on the preview: the camera bubble, a mask's rect, or the crop.
#[derive(Debug, Clone, Copy)]
enum PreviewDrag {
    Camera { start: UnitPoint },
    Mask { id: uuid::Uuid, handle: RegionHandle, start: Rect },
    Crop { handle: RegionHandle, start: Rect },
}

/// A slider and the mask field it edits.
type MaskField<'a> = (&'a Slider, fn(&mut MaskSegment, f64));
/// A slider and the zoom cue field it edits.
type CueField<'a> = (&'a Slider, fn(&mut ZoomCue, f64));
/// A slider and the 3D shot field it edits.
type ShotField<'a> = (&'a Slider, fn(&mut Shot3D, f64));
const FRAME_STEP: f64 = 1.0 / 30.0;

pub struct Studio {
    pub model: RefCell<StudioModel>,
    widgets: RefCell<Option<Widgets>>,
    playing: Cell<bool>,
    play_timer: RefCell<Option<glib::SourceId>>,
    last_tick: Cell<Option<Instant>>,
    draft_timer: RefCell<Option<glib::SourceId>>,
    render_cancel: RefCell<Option<Arc<AtomicBool>>>,
    updating: Cell<bool>,
    coalescing: Cell<Option<&'static str>>,
    /// Where the canvas was last drawn in the preview: (x, y, scale).
    preview_transform: Cell<(f64, f64, f64)>,
    drag: RefCell<Option<PreviewDrag>>,
    /// The recording's soundtrack for the preview, seeked per clip.
    audio: RefCell<Option<AudioPlayer>>,
    audio_segment: Cell<Option<usize>>,
    presets: RefCell<StylePresetStore>,
    applied_preset: Cell<Option<uuid::Uuid>>,
}

struct Inspector {
    root: gtk::Box,
    tiles: Vec<(BgStyle, gtk::Button)>,
    padding: Slider,
    corner: Slider,
    shadow: Slider,
    preset_picker: gtk::DropDown,
    delete_preset: gtk::Button,
    cursor_note: gtk::Label,
    cursor_group: gtk::Box,
    cursor_visible: gtk::Switch,
    cursor_style: gtk::DropDown,
    cursor_size: Slider,
    cursor_smooth: gtk::Switch,
    click_effects: gtk::Switch,
    cursor_press: gtk::Switch,
    cursor_ripple: gtk::Switch,
    cursor_idle: gtk::Switch,
    camera_group: gtk::Box,
    camera_note: gtk::Label,
    camera_visible: gtk::Switch,
    layout: gtk::DropDown,
    camera_left: gtk::Switch,
    camera_size: Slider,
    camera_roundness: gtk::Scale,
    camera_aspect: gtk::DropDown,
    adjust_crop: gtk::ToggleButton,
    apply_crop: gtk::Button,
    crop_x: Slider,
    crop_y: Slider,
    crop_w: Slider,
    crop_h: Slider,
    mask_picker: gtk::DropDown,
    mask_box: gtk::Box,
    mask_effect: gtk::DropDown,
    mask_amount: Slider,
    mask_x: Slider,
    mask_y: Slider,
    mask_w: Slider,
    mask_h: Slider,
    mask_start: Slider,
    mask_end: Slider,
    shot_preset: gtk::DropDown,
    scene_count: gtk::DropDown,
    shot_picker: gtk::DropDown,
    shot_box: gtk::Box,
    shot_apply_preset: gtk::DropDown,
    shot_start: Slider,
    shot_end: Slider,
    shot_transition: Slider,
    shot_easing: gtk::DropDown,
    shot_enabled: gtk::Switch,
    blur_mode: gtk::DropDown,
    blur_box: gtk::Box,
    blur_strength: Slider,
    blur_falloff: Slider,
    blur_focus_x: Slider,
    blur_focus_y: Slider,
    blur_focus_size: Slider,
    blur_bokeh: gtk::Switch,
    keystrokes_box: gtk::Box,
    show_keystrokes: gtk::Switch,
    keystroke_placement: gtk::DropDown,
    subtitles_box: gtk::Box,
    subtitle_controls: gtk::Box,
    transcribe: gtk::Button,
    transcribe_note: gtk::Label,
    remove_subtitles: gtk::Button,
    show_subtitles: gtk::Switch,
    subtitle_position: Slider,
    subtitle_size: Slider,
    subtitle_highlight: gtk::Switch,
    zoom_enabled: gtk::Switch,
    cue_picker: gtk::DropDown,
    cue_box: gtk::Box,
    cue_start: Slider,
    cue_end: Slider,
    cue_zoom: Slider,
    cue_mode: gtk::DropDown,
    cue_x: Slider,
    cue_y: Slider,
    scissors: gtk::ToggleButton,
    delete_clip: gtk::Button,
    clip_box: gtk::Box,
    clip_speed: Slider,
    aspect: gtk::DropDown,
    aspect_mode_row: gtk::Box,
    aspect_mode: gtk::DropDown,
    aspect_note: gtk::Label,
    format: gtk::DropDown,
    frame_rate: gtk::DropDown,
    quality: gtk::DropDown,
    resolution: gtk::DropDown,
    include_audio: gtk::Switch,
}

struct Widgets {
    window: adw::ApplicationWindow,
    title: adw::WindowTitle,
    preview: gtk::DrawingArea,
    timeline: gtk::DrawingArea,
    time_label: gtk::Label,
    play_icon: gtk::Image,
    undo: gtk::Button,
    redo: gtk::Button,
    save: gtk::Button,
    export: gtk::Button,
    progress: gtk::ProgressBar,
    cancel: gtk::Button,
    status: gtk::Label,
    inspector: Inspector,
}

thread_local! {
    static OPEN: RefCell<Vec<(PathBuf, Rc<Studio>)>> = const { RefCell::new(Vec::new()) };
}

/// Opens the studio for a recording package, or the package around a path
/// (its screen master or deliverable). Returns false for other files.
pub fn open_path(app: &App, path: &Path) -> bool {
    match RecordingSession::containing(path) {
        Some(session) => {
            open(app, &session);
            true
        }
        None => false,
    }
}

pub fn open(app: &App, session: &RecordingSession) {
    let existing =
        OPEN.with(|open| open.borrow().iter().find(|(p, _)| *p == session.directory).map(|(_, s)| s.clone()));
    if let Some(studio) = existing {
        if let Some(widgets) = studio.widgets.borrow().as_ref() {
            widgets.window.present();
        }
        return;
    }
    let default_style = app.with_prefs(|p| StudioStyle::from_defaults(&p.default_beautifier_config()));
    let model = match StudioModel::load(session.clone(), default_style) {
        Ok(model) => model,
        Err(error) => return app.notify_error("Couldn’t open the recording", &error),
    };
    let studio = Rc::new(Studio {
        model: RefCell::new(model),
        widgets: RefCell::new(None),
        playing: Cell::new(false),
        play_timer: RefCell::new(None),
        last_tick: Cell::new(None),
        draft_timer: RefCell::new(None),
        render_cancel: RefCell::new(None),
        updating: Cell::new(false),
        coalescing: Cell::new(None),
        preview_transform: Cell::new((0.0, 0.0, 1.0)),
        drag: RefCell::new(None),
        audio: RefCell::new(AudioPlayer::open(&session.screen_path())),
        audio_segment: Cell::new(None),
        presets: RefCell::new(app.with_prefs(|p| StylePresetStore::load(p.prefs))),
        applied_preset: Cell::new(None),
    });
    build(app, &studio);
    OPEN.with(|open| open.borrow_mut().push((session.directory.clone(), studio.clone())));
}

// MARK: - Building

fn icon_button(icon: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(label));
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

fn switch_row(label: &str) -> (gtk::Box, gtk::Switch) {
    let title = gtk::Label::builder().label(label).xalign(0.0).hexpand(true).css_classes(["caption-heading"]).build();
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    switch.update_property(&[gtk::accessible::Property::Label(label)]);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&title);
    row.append(&switch);
    (row, switch)
}

fn connect_slider(
    studio: &Rc<Studio>,
    slider: &Slider,
    key: &'static str,
    apply: impl Fn(&mut StudioModel, f64) + 'static,
) {
    let weak = Rc::downgrade(studio);
    slider.adjustment.connect_value_changed(move |adjustment| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        studio.begin_coalesced(key);
        apply(&mut studio.model.borrow_mut(), adjustment.value());
        studio.after_edit(false);
    });
}

fn build(app: &App, studio: &Rc<Studio>) {
    let name = studio.model.borrow().session.display_name();
    let window = adw::ApplicationWindow::builder()
        .application(&app.application)
        .title(format!("{name} — BetterShot Studio"))
        .default_width(1240)
        .default_height(820)
        .build();
    window.add_css_class("video-studio");

    let title = adw::WindowTitle::new(&name, "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    let undo = icon_button("edit-undo-symbolic", "Undo (Ctrl+Z)");
    let redo = icon_button("edit-redo-symbolic", "Redo (Ctrl+Shift+Z)");
    header.pack_start(&undo);
    header.pack_start(&redo);
    let save = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).build();
    save.set_tooltip_text(Some("Save the project and update the saved video (Ctrl+S)"));
    let export = icon_button("document-save-as-symbolic", "Export As…");
    let cancel = gtk::Button::builder().label("Cancel").visible(false).build();
    let progress = gtk::ProgressBar::builder().visible(false).valign(gtk::Align::Center).width_request(140).build();
    progress.update_property(&[gtk::accessible::Property::Label("Rendering progress")]);
    header.pack_end(&save);
    header.pack_end(&export);
    header.pack_end(&cancel);
    header.pack_end(&progress);

    let inspector = build_inspector(studio);
    let scroller = gtk::ScrolledWindow::builder()
        .child(&inspector.root)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .min_content_width(280)
        .hexpand(false)
        .build();
    scroller.add_css_class("editor-inspector");

    let preview = gtk::DrawingArea::new();
    preview.set_hexpand(true);
    preview.set_vexpand(true);
    preview.set_focusable(true);
    preview.add_css_class("studio-preview");
    preview.update_property(&[gtk::accessible::Property::Label("Video preview")]);
    let weak = Rc::downgrade(studio);
    preview.set_draw_func(move |_, cr, w, h| {
        if let Some(studio) = weak.upgrade() {
            draw_preview(&studio, cr, f64::from(w), f64::from(h));
        }
    });

    // Dragging the camera bubble, a mask's rect, or the crop on the preview.
    let drag = gtk::GestureDrag::new();
    let weak = Rc::downgrade(studio);
    drag.connect_drag_begin(move |_, x, y| {
        if let Some(studio) = weak.upgrade() {
            studio.preview_drag_begin(x, y);
        }
    });
    let weak = Rc::downgrade(studio);
    drag.connect_drag_update(move |_, dx, dy| {
        if let Some(studio) = weak.upgrade() {
            studio.preview_drag_update(dx, dy);
        }
    });
    let weak = Rc::downgrade(studio);
    drag.connect_drag_end(move |_, _, _| {
        if let Some(studio) = weak.upgrade() {
            studio.preview_drag_end();
        }
    });
    preview.add_controller(drag);
    let motion = gtk::EventControllerMotion::new();
    let weak = Rc::downgrade(studio);
    let hover = preview.clone();
    motion.connect_motion(move |_, x, y| {
        if let Some(studio) = weak.upgrade() {
            if studio.drag.borrow().is_none() {
                hover.set_cursor_from_name(studio.preview_cursor(x, y));
            }
        }
    });
    preview.add_controller(motion);

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    transport.set_margin_start(12);
    transport.set_margin_end(12);
    let play_icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
    let play = gtk::Button::builder().child(&play_icon).css_classes(["flat", "circular"]).build();
    play.set_tooltip_text(Some("Play or pause (Space)"));
    play.update_property(&[gtk::accessible::Property::Label("Play")]);
    let weak = Rc::downgrade(studio);
    play.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.toggle_playback();
        }
    });
    let time_label = gtk::Label::builder().label("0:00 / 0:00").css_classes(["numeric", "dim-label"]).build();
    let status = gtk::Label::builder()
        .xalign(1.0)
        .hexpand(true)
        .css_classes(["dim-label"])
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    status.set_accessible_role(gtk::AccessibleRole::Status);
    transport.append(&play);
    transport.append(&time_label);
    transport.append(&status);

    let timeline = gtk::DrawingArea::new();
    timeline.set_content_height(TIMELINE_HEIGHT);
    timeline.set_hexpand(true);
    timeline.add_css_class("studio-timeline");
    timeline.update_property(&[gtk::accessible::Property::Label("Timeline")]);
    let weak = Rc::downgrade(studio);
    timeline.set_draw_func(move |_, cr, w, h| {
        if let Some(studio) = weak.upgrade() {
            draw_timeline(&studio, cr, f64::from(w), f64::from(h));
        }
    });
    let click = gtk::GestureClick::new();
    let weak = Rc::downgrade(studio);
    click.connect_pressed(move |_, _, x, y| {
        if let Some(studio) = weak.upgrade() {
            let width = studio.widgets.borrow().as_ref().map(|w| f64::from(w.timeline.width())).unwrap_or(1.0);
            studio.timeline_pressed(x, y, width);
        }
    });
    timeline.add_controller(click);
    let drag = gtk::GestureDrag::new();
    let weak = Rc::downgrade(studio);
    drag.connect_drag_update(move |gesture, dx, _| {
        if let Some(studio) = weak.upgrade() {
            if let Some((start_x, _)) = gesture.start_point() {
                let width = studio.widgets.borrow().as_ref().map(|w| f64::from(w.timeline.width())).unwrap_or(1.0);
                studio.scrub(start_x + dx, width);
            }
        }
    });
    timeline.add_controller(drag);

    let right = gtk::Box::new(gtk::Orientation::Vertical, 6);
    right.append(&preview);
    right.append(&transport);
    right.append(&timeline);
    right.set_margin_bottom(8);
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content.append(&scroller);
    content.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    content.append(&right);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&content);
    window.set_content(Some(&root));

    for (button, undo_step) in [(&undo, true), (&redo, false)] {
        let weak = Rc::downgrade(studio);
        button.connect_clicked(move |_| {
            if let Some(studio) = weak.upgrade() {
                studio.undo_redo(undo_step);
            }
        });
    }
    let weak = Rc::downgrade(studio);
    save.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            save_project(&studio, false);
        }
    });
    let weak = Rc::downgrade(studio);
    export.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            export_as(&studio);
        }
    });
    let weak = Rc::downgrade(studio);
    cancel.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            if let Some(flag) = studio.render_cancel.borrow().as_ref() {
                flag.store(true, Ordering::Relaxed);
            }
        }
    });

    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(studio);
    keys.connect_key_pressed(move |_, key, _, state| {
        let Some(studio) = weak.upgrade() else { return glib::Propagation::Proceed };
        if studio.widgets.borrow().as_ref().is_some_and(|w| focus_is_editable(&w.window)) {
            return glib::Propagation::Proceed;
        }
        if studio.on_key(key, state) {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);

    let weak = Rc::downgrade(studio);
    window.connect_close_request(move |window| {
        let Some(studio) = weak.upgrade() else { return glib::Propagation::Proceed };
        if studio.render_cancel.borrow().is_some() {
            toast(&studio, "Wait for the render to finish, or cancel it.");
            return glib::Propagation::Stop;
        }
        if studio.model.borrow().is_dirty() {
            confirm_close(&studio, window);
            return glib::Propagation::Stop;
        }
        studio.teardown();
        glib::Propagation::Proceed
    });

    *studio.widgets.borrow_mut() = Some(Widgets {
        window: window.clone(),
        title,
        preview,
        timeline,
        time_label,
        play_icon,
        undo,
        redo,
        save,
        export,
        progress,
        cancel,
        status,
        inspector,
    });
    studio.sync();
    window.present();
    if let Some(widgets) = studio.widgets.borrow().as_ref() {
        widgets.preview.grab_focus();
    }
}

fn focus_is_editable(window: &adw::ApplicationWindow) -> bool {
    GtkWindowExt::focus(window)
        .is_some_and(|widget| widget.is::<gtk::Text>() || widget.is::<gtk::SpinButton>() || widget.is::<gtk::Entry>())
}

#[allow(clippy::too_many_lines)]
fn build_inspector(studio: &Rc<Studio>) -> Inspector {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(8);
    root.set_margin_bottom(12);

    // Background
    root.append(&heading("Background"));
    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(5)
        .min_children_per_line(4)
        .homogeneous(true)
        .row_spacing(4)
        .column_spacing(4)
        .build();
    let mut styles: Vec<(String, BgStyle)> = vec![("No background".into(), BgStyle::None)];
    styles.extend(BgGradient::presets().into_iter().map(|g| (g.title.clone(), BgStyle::Gradient(g))));
    styles.extend(BgColor::presets().into_iter().take(6).map(|c| (c.title.clone(), BgStyle::Solid(c))));
    let mut tiles = Vec::new();
    for (title, style) in styles {
        let button = background_tile(&title, style.clone());
        let weak = Rc::downgrade(studio);
        let chosen = style.clone();
        button.connect_clicked(move |_| {
            let Some(studio) = weak.upgrade() else { return };
            let chosen = chosen.clone();
            studio.model.borrow_mut().edit(|s| s.style.background = chosen);
            studio.after_edit(true);
        });
        grid.insert(&button, -1);
        tiles.push((style, button));
    }
    root.append(&grid);
    let padding = slider("Padding", 0.0, 0.25, 0.005, 3);
    let corner = slider("Corner radius", 0.0, 0.1, 0.002, 3);
    let shadow = slider("Shadow", 0.0, 1.0, 0.01, 2);
    connect_slider(studio, &padding, "padding", |m, v| m.edit_coalesced(|s| s.style.padding = v));
    connect_slider(studio, &corner, "corner", |m, v| m.edit_coalesced(|s| s.style.corner_radius = v));
    connect_slider(studio, &shadow, "shadow", |m, v| m.edit_coalesced(|s| s.style.shadow = v));
    root.append(&padding.root);
    root.append(&corner.root);
    root.append(&shadow.root);
    // Style presets (port of `RecordingStudioStylePresetBar`): apply, save
    // the current look under a name, delete the applied one.
    let preset_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let preset_picker = gtk::DropDown::from_strings(&["Presets…"]);
    preset_picker.set_hexpand(true);
    preset_picker.update_property(&[gtk::accessible::Property::Label("Style presets")]);
    let weak = Rc::downgrade(studio);
    preset_picker.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        studio.apply_preset(list.selected());
    });
    let delete_preset = icon_button("user-trash-symbolic", "Delete the applied preset");
    delete_preset.add_css_class("flat");
    let weak = Rc::downgrade(studio);
    delete_preset.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.delete_applied_preset();
        }
    });
    let add_preset = icon_button("list-add-symbolic", "Save current settings as a preset");
    add_preset.add_css_class("flat");
    let name_popover = gtk::Popover::new();
    let name_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let name_entry = gtk::Entry::builder().placeholder_text("Preset name").width_chars(18).build();
    name_entry.update_property(&[gtk::accessible::Property::Label("Preset name")]);
    let save_preset = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).build();
    name_box.append(&name_entry);
    name_box.append(&save_preset);
    name_popover.set_child(Some(&name_box));
    name_popover.set_parent(&add_preset);
    let popover = name_popover.clone();
    let entry = name_entry.clone();
    add_preset.connect_clicked(move |_| {
        entry.set_text("");
        popover.popup();
        entry.grab_focus();
    });
    let weak = Rc::downgrade(studio);
    let entry = name_entry.clone();
    let popover = name_popover.clone();
    let commit = move || {
        let Some(studio) = weak.upgrade() else { return };
        if studio.save_preset(&entry.text()) {
            popover.popdown();
        } else {
            entry.add_css_class("error");
        }
    };
    let commit_click = commit.clone();
    save_preset.connect_clicked(move |_| commit_click());
    name_entry.connect_activate(move |_| commit());
    name_entry.connect_changed(|entry| entry.remove_css_class("error"));
    preset_bar.append(&preset_picker);
    preset_bar.append(&delete_preset);
    preset_bar.append(&add_preset);
    root.append(&preset_bar);

    // Cursor (port of the Cursor tab: styles, size, motion, click effects)
    root.append(&heading("Cursor"));
    let cursor_note = gtk::Label::builder()
        .label("The cursor was recorded into the picture, so it cannot be restyled or hidden here. Record on an X11 session to get a restylable pointer.")
        .wrap(true)
        .max_width_chars(34)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    root.append(&cursor_note);
    let cursor_group = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (cursor_visible_row, cursor_visible) = switch_row("Show cursor");
    let styles: Vec<&str> = CursorAppearance::SELECTABLE.iter().map(|a| a.title()).collect();
    let (cursor_style_row, cursor_style) = dropdown("Style", &styles);
    let weak = Rc::downgrade(studio);
    cursor_style.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let appearance = CursorAppearance::SELECTABLE[(list.selected() as usize).min(4)];
        studio.model.borrow_mut().edit(|s| s.style.cursor.appearance = appearance);
        studio.after_edit(true);
    });
    let cursor_size = slider("Size", 1.0, 4.0, 0.1, 1);
    connect_slider(studio, &cursor_size, "cursor-size", |m, v| m.edit_coalesced(|s| s.style.cursor_scale = v));
    let (cursor_smooth_row, cursor_smooth) = switch_row("Smooth motion");
    let (click_effects_row, click_effects) = switch_row("Click effects");
    let (cursor_press_row, cursor_press) = switch_row("Press");
    let (cursor_ripple_row, cursor_ripple) = switch_row("Ripple");
    let (cursor_idle_row, cursor_idle) = switch_row("Hide when idle");
    type CursorToggle = fn(&mut super::model::Snapshot, bool);
    let toggles: [(&gtk::Switch, CursorToggle); 6] = [
        (&cursor_visible, |s, on| s.style.cursor.is_visible = on),
        (&cursor_smooth, |s, on| s.style.cursor.smooth_motion = on),
        (&click_effects, |s, on| s.shows_click_effects = on),
        (&cursor_press, |s, on| s.style.cursor.press_effect = on),
        (&cursor_ripple, |s, on| s.style.cursor.ripple_effect = on),
        (&cursor_idle, |s, on| s.style.cursor.hide_when_idle = on),
    ];
    for (switch, apply) in toggles {
        let weak = Rc::downgrade(studio);
        switch.connect_active_notify(move |switch| {
            let Some(studio) = weak.upgrade() else { return };
            if studio.updating.get() {
                return;
            }
            let on = switch.is_active();
            studio.model.borrow_mut().edit(|s| apply(s, on));
            studio.after_edit(true);
        });
    }
    for widget in [&cursor_visible_row, &cursor_style_row, &cursor_size.root, &cursor_smooth_row, &click_effects_row] {
        cursor_group.append(widget);
    }
    for row in [&cursor_press_row, &cursor_ripple_row] {
        row.set_margin_start(16);
        cursor_group.append(row);
    }
    cursor_group.append(&cursor_idle_row);
    root.append(&cursor_group);

    // Camera
    root.append(&heading("Camera"));
    let camera_note = gtk::Label::builder()
        .label("This recording has no camera movie.")
        .wrap(true)
        .max_width_chars(34)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    root.append(&camera_note);
    let camera_group = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (visible_row, camera_visible) = switch_row("Show camera");
    let weak = Rc::downgrade(studio);
    camera_visible.connect_active_notify(move |switch| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let on = switch.is_active();
        studio.model.borrow_mut().edit(|s| s.style.camera.is_visible = on);
        studio.after_edit(true);
    });
    let titles: Vec<&str> = LayoutPreset::ALL.iter().map(|p| p.title()).collect();
    let (layout_row, layout) = dropdown("Layout", &titles);
    let weak = Rc::downgrade(studio);
    layout.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let preset = LayoutPreset::ALL[(list.selected() as usize).min(LayoutPreset::ALL.len() - 1)];
        studio.model.borrow_mut().edit(|s| s.style.layout_preset = preset);
        studio.after_edit(true);
    });
    let (left_row, camera_left) = switch_row("Camera on the left");
    let weak = Rc::downgrade(studio);
    camera_left.connect_active_notify(move |switch| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let on = switch.is_active();
        studio.model.borrow_mut().edit(|s| s.style.camera_on_left = on);
        studio.after_edit(true);
    });
    let camera_size = slider("Size", 0.1, 0.6, 0.01, 2);
    connect_slider(studio, &camera_size, "camera-size", |m, v| m.edit_coalesced(|s| s.style.camera.size = v));
    let roundness_label = gtk::Label::builder().label("Roundness").xalign(0.0).css_classes(["caption-heading"]).build();
    let camera_roundness = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 0.5, 0.01);
    camera_roundness.set_draw_value(false);
    camera_roundness.update_property(&[gtk::accessible::Property::Label("Camera roundness")]);
    let weak = Rc::downgrade(studio);
    camera_roundness.connect_value_changed(move |scale| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        studio.begin_coalesced("roundness");
        let value = scale.value();
        studio.model.borrow_mut().edit_coalesced(|s| s.style.camera.roundness = value);
        studio.after_edit(false);
    });
    let ratios: Vec<&str> = CameraAspectRatio::ALL.iter().map(|r| r.raw_value()).collect();
    let (aspect_row, camera_aspect) = dropdown("Camera frame", &ratios);
    let weak = Rc::downgrade(studio);
    camera_aspect.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let ratio = CameraAspectRatio::ALL[(list.selected() as usize).min(CameraAspectRatio::ALL.len() - 1)];
        studio.model.borrow_mut().edit(|s| s.style.camera.aspect_ratio = ratio);
        studio.after_edit(true);
    });
    for widget in [&visible_row, &layout_row, &left_row, &camera_size.root] {
        camera_group.append(widget);
    }
    camera_group.append(&roundness_label);
    camera_group.append(&camera_roundness);
    camera_group.append(&aspect_row);
    root.append(&camera_group);

    // Effects: crop and masks
    root.append(&heading("Effects"));
    let crop_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let adjust_crop = gtk::ToggleButton::builder().label("Adjust Crop").css_classes(["flat"]).build();
    adjust_crop.set_tooltip_text(Some("Drag the crop's handles on the preview; Return applies, Escape cancels"));
    let weak = Rc::downgrade(studio);
    adjust_crop.connect_toggled(move |button| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        if button.is_active() {
            studio.model.borrow_mut().begin_crop_edit();
            studio.after_edit(true);
        } else {
            studio.cancel_crop_edit();
        }
    });
    let apply_crop = gtk::Button::builder().label("Apply").css_classes(["suggested-action"]).visible(false).build();
    let weak = Rc::downgrade(studio);
    apply_crop.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.apply_crop_edit();
        }
    });
    crop_buttons.append(&adjust_crop);
    crop_buttons.append(&apply_crop);
    root.append(&crop_buttons);
    let crop_label = gtk::Label::builder()
        .label("Crop (fractions of the frame)")
        .xalign(0.0)
        .css_classes(["caption-heading"])
        .build();
    root.append(&crop_label);
    let crop_x = slider("Left", 0.0, 0.99, 0.01, 2);
    let crop_y = slider("Top", 0.0, 0.99, 0.01, 2);
    let crop_w = slider("Width", 0.01, 1.0, 0.01, 2);
    let crop_h = slider("Height", 0.01, 1.0, 0.01, 2);
    for (control, index) in [(&crop_x, 0usize), (&crop_y, 1), (&crop_w, 2), (&crop_h, 3)] {
        connect_slider(studio, control, "crop", move |m, v| {
            let mut crop = m.state.crop;
            match index {
                0 => crop.x = v,
                1 => crop.y = v,
                2 => crop.width = v,
                _ => crop.height = v,
            }
            m.set_crop(crop, true);
        });
        root.append(&control.root);
    }
    let reset_crop = gtk::Button::builder().label("Reset Crop").css_classes(["flat"]).halign(gtk::Align::Start).build();
    let weak = Rc::downgrade(studio);
    reset_crop.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.model.borrow_mut().set_crop(UnitRect::UNIT, false);
            studio.after_edit(true);
        }
    });
    root.append(&reset_crop);

    let mask_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for (label, effect) in [("Add Blur", MaskEffect::Blur), ("Add Pixelate", MaskEffect::Pixelate)] {
        let button = gtk::Button::builder().label(label).css_classes(["flat"]).build();
        let weak = Rc::downgrade(studio);
        button.connect_clicked(move |_| {
            if let Some(studio) = weak.upgrade() {
                studio.model.borrow_mut().add_mask(effect);
                studio.after_edit(true);
            }
        });
        mask_buttons.append(&button);
    }
    root.append(&mask_buttons);
    let (mask_row, mask_picker) = dropdown("Mask", &["None"]);
    let weak = Rc::downgrade(studio);
    mask_picker.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let index = list.selected() as usize;
        let id = studio.model.borrow().state.masks.get(index).map(|m| m.id);
        studio.model.borrow_mut().selected_mask = id;
        studio.sync();
    });
    root.append(&mask_row);
    let mask_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (effect_row, mask_effect) = dropdown("Effect", &["Blur", "Pixelate"]);
    let weak = Rc::downgrade(studio);
    mask_effect.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let effect = if list.selected() == 0 { MaskEffect::Blur } else { MaskEffect::Pixelate };
        studio.update_selected_mask(false, |mask| mask.effect = effect);
    });
    let mask_amount = slider("Strength", MaskSegment::MINIMUM_AMOUNT, MaskSegment::MAXIMUM_AMOUNT, 1.0, 0);
    let mask_x = slider("Left", 0.0, 0.99, 0.01, 2);
    let mask_y = slider("Top", 0.0, 0.99, 0.01, 2);
    let mask_w = slider("Width", 0.01, 1.0, 0.01, 2);
    let mask_h = slider("Height", 0.01, 1.0, 0.01, 2);
    let mask_start = slider("From (s)", 0.0, 1.0, 0.1, 1);
    let mask_end = slider("Until (s)", 0.0, 1.0, 0.1, 1);
    let fields: [MaskField; 7] = [
        (&mask_amount, |m, v| m.amount = v),
        (&mask_x, |m, v| m.set_rect(UnitRect::new(v, m.y, m.width, m.height))),
        (&mask_y, |m, v| m.set_rect(UnitRect::new(m.x, v, m.width, m.height))),
        (&mask_w, |m, v| m.set_rect(UnitRect::new(m.x, m.y, v, m.height))),
        (&mask_h, |m, v| m.set_rect(UnitRect::new(m.x, m.y, m.width, v))),
        (&mask_start, |m, v| m.start = Some(v)),
        (&mask_end, |m, v| m.end = Some(v)),
    ];
    for (control, apply) in fields {
        let weak = Rc::downgrade(studio);
        control.adjustment.connect_value_changed(move |adjustment| {
            let Some(studio) = weak.upgrade() else { return };
            if studio.updating.get() {
                return;
            }
            studio.begin_coalesced("mask");
            let value = adjustment.value();
            studio.update_selected_mask(true, move |mask| apply(mask, value));
        });
    }
    let remove_mask = gtk::Button::builder()
        .label("Remove Mask")
        .css_classes(["flat", "destructive-action"])
        .halign(gtk::Align::Start)
        .build();
    let weak = Rc::downgrade(studio);
    remove_mask.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            let id = studio.model.borrow().selected_mask;
            if let Some(id) = id {
                studio.model.borrow_mut().remove_mask(id);
                studio.after_edit(true);
            }
        }
    });
    mask_box.append(&effect_row);
    for control in [&mask_amount, &mask_x, &mask_y, &mask_w, &mask_h, &mask_start, &mask_end] {
        mask_box.append(&control.root);
    }
    mask_box.append(&remove_mask);
    root.append(&mask_box);

    // 3D shots (port of `Recording3DInspector`: presets, scenes, timing,
    // and depth blur; keyframes and the orbit pad stay with sliders).
    let shots_label = gtk::Label::builder().label("3D Shots").xalign(0.0).css_classes(["caption-heading"]).build();
    root.append(&shots_label);
    let presets: Vec<&str> = Preset3D::ALL.iter().map(|p| p.title()).collect();
    let (preset_row, shot_preset) = dropdown("Preset", &presets);
    shot_preset.set_selected(Preset3D::ALL.iter().position(|p| *p == Preset3D::Glide).unwrap_or(0) as u32);
    root.append(&preset_row);
    let shot_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let add_shot = gtk::Button::builder().label("Add Shot").css_classes(["flat"]).build();
    add_shot.set_tooltip_text(Some("A three-second shot with the chosen preset at the playhead (D)"));
    let weak = Rc::downgrade(studio);
    add_shot.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.add_shot();
        }
    });
    let (scene_row, scene_count) = dropdown("Scene shots", &["1", "2", "3", "4", "5", "6"]);
    scene_count.set_selected(2);
    let auto_scene = gtk::Button::builder().label("Auto Scene").css_classes(["flat"]).build();
    auto_scene.set_tooltip_text(Some("Replace the shots with Cap's automatic sequence"));
    let weak = Rc::downgrade(studio);
    let count = scene_count.clone();
    auto_scene.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.model.borrow_mut().apply_auto_scene(count.selected() as usize + 1);
            studio.after_edit(true);
        }
    });
    shot_buttons.append(&add_shot);
    shot_buttons.append(&auto_scene);
    root.append(&shot_buttons);
    root.append(&scene_row);
    let (shot_row, shot_picker) = dropdown("Shot", &["None"]);
    let weak = Rc::downgrade(studio);
    shot_picker.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let index = list.selected() as usize;
        let id = studio.model.borrow().timeline_3d().shots.get(index).map(|s| s.id.clone());
        studio.model.borrow_mut().selected_shot = id;
        studio.sync();
    });
    root.append(&shot_row);
    let shot_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (apply_row, shot_apply_preset) = dropdown("Move", &presets);
    let weak = Rc::downgrade(studio);
    shot_apply_preset.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let preset = Preset3D::ALL[(list.selected() as usize).min(Preset3D::ALL.len() - 1)];
        studio.update_selected_shot(false, move |shot| shot.apply(preset));
    });
    let shot_start = slider("Start (s)", 0.0, 1.0, 0.1, 1);
    let shot_end = slider("End (s)", 0.0, 1.0, 0.1, 1);
    let shot_transition = slider("Transition (s)", 0.0, 2.0, 0.05, 2);
    let shot_fields: [ShotField; 3] = [
        (&shot_start, |shot, v| shot.start = v),
        (&shot_end, |shot, v| shot.end = v),
        (&shot_transition, |shot, v| shot.transition = v),
    ];
    for (control, apply) in shot_fields {
        let weak = Rc::downgrade(studio);
        control.adjustment.connect_value_changed(move |adjustment| {
            let Some(studio) = weak.upgrade() else { return };
            if studio.updating.get() {
                return;
            }
            studio.begin_coalesced("shot");
            let value = adjustment.value();
            studio.update_selected_shot(true, move |shot| apply(shot, value));
        });
    }
    let easings: Vec<&str> = Easing3D::ALL.iter().map(|e| e.title()).collect();
    let (easing_row, shot_easing) = dropdown("Easing", &easings);
    let weak = Rc::downgrade(studio);
    shot_easing.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let easing = Easing3D::ALL[(list.selected() as usize).min(3)];
        studio.update_selected_shot(false, move |shot| shot.easing = easing);
    });
    let (enabled_row, shot_enabled) = switch_row("Enabled");
    let weak = Rc::downgrade(studio);
    shot_enabled.connect_active_notify(move |switch| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let on = switch.is_active();
        studio.update_selected_shot(false, move |shot| shot.is_enabled = on);
    });
    let shot_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let reverse = gtk::Button::builder().label("Reverse").css_classes(["flat"]).build();
    let weak = Rc::downgrade(studio);
    reverse.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.update_selected_shot(false, |shot| shot.reverse());
        }
    });
    let remove_shot = gtk::Button::builder().label("Remove Shot").css_classes(["flat", "destructive-action"]).build();
    let weak = Rc::downgrade(studio);
    remove_shot.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            let id = studio.model.borrow().selected_shot.clone();
            if let Some(id) = id {
                studio.model.borrow_mut().remove_shot(&id);
                studio.after_edit(true);
            }
        }
    });
    shot_actions.append(&reverse);
    shot_actions.append(&remove_shot);
    for widget in [&apply_row, &shot_start.root, &shot_end.root, &shot_transition.root, &easing_row, &enabled_row] {
        shot_box.append(widget);
    }
    shot_box.append(&shot_actions);
    // Depth blur
    let blur_label = gtk::Label::builder().label("Depth blur").xalign(0.0).css_classes(["caption-heading"]).build();
    shot_box.append(&blur_label);
    let modes: Vec<&str> = BlurMode::ALL.iter().map(|m| m.title()).collect();
    let (mode_row, blur_mode) = dropdown("Blur", &modes);
    let weak = Rc::downgrade(studio);
    blur_mode.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let mode = BlurMode::ALL[(list.selected() as usize).min(3)];
        studio.update_selected_shot(false, move |shot| {
            let mut blur = shot.blur.clone().unwrap_or_else(Blur3D::showcase);
            blur.mode = mode;
            shot.blur = Some(blur);
        });
    });
    shot_box.append(&mode_row);
    let blur_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let blur_strength = slider("Strength", 0.0, 60.0, 1.0, 0);
    let blur_falloff = slider("Falloff", 0.0, 1.0, 0.01, 2);
    let blur_focus_x = slider("Focus X", 0.0, 1.0, 0.01, 2);
    let blur_focus_y = slider("Focus Y", 0.0, 1.0, 0.01, 2);
    let blur_focus_size = slider("Focus size", 0.0, 1.0, 0.01, 2);
    let blur_fields: [ShotField; 5] = [
        (&blur_strength, |shot, v| edit_blur(shot, |b| b.strength = v)),
        (&blur_falloff, |shot, v| edit_blur(shot, |b| b.falloff = v)),
        (&blur_focus_x, |shot, v| edit_blur(shot, |b| b.focus_x = v)),
        (&blur_focus_y, |shot, v| edit_blur(shot, |b| b.focus_y = v)),
        (&blur_focus_size, |shot, v| edit_blur(shot, |b| b.focus_size = v)),
    ];
    for (control, apply) in blur_fields {
        let weak = Rc::downgrade(studio);
        control.adjustment.connect_value_changed(move |adjustment| {
            let Some(studio) = weak.upgrade() else { return };
            if studio.updating.get() {
                return;
            }
            studio.begin_coalesced("blur");
            let value = adjustment.value();
            studio.update_selected_shot(true, move |shot| apply(shot, value));
        });
    }
    let (bokeh_row, blur_bokeh) = switch_row("Bokeh");
    let weak = Rc::downgrade(studio);
    blur_bokeh.connect_active_notify(move |switch| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let on = switch.is_active();
        studio.update_selected_shot(false, move |shot| edit_blur(shot, |b| b.bokeh = on));
    });
    for control in [&blur_strength, &blur_falloff, &blur_focus_x, &blur_focus_y, &blur_focus_size] {
        blur_box.append(&control.root);
    }
    blur_box.append(&bokeh_row);
    shot_box.append(&blur_box);
    root.append(&shot_box);

    // Keystroke captions (shown when the recording has chords).
    let keystrokes_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (show_keystrokes_row, show_keystrokes) = switch_row("Show keystrokes");
    let weak = Rc::downgrade(studio);
    show_keystrokes.connect_active_notify(move |switch| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let on = switch.is_active();
        studio.model.borrow_mut().edit(|s| s.shows_keystrokes = on);
        studio.after_edit(true);
    });
    let placements: Vec<&str> = KeystrokePlacement::ALL.iter().map(|p| p.title()).collect();
    let (placement_row, keystroke_placement) = dropdown("Placement", &placements);
    let weak = Rc::downgrade(studio);
    keystroke_placement.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let placement = KeystrokePlacement::ALL[(list.selected() as usize).min(5)];
        studio.model.borrow_mut().edit(|s| s.keystroke_placement = placement);
        studio.after_edit(true);
    });
    keystrokes_box.append(&show_keystrokes_row);
    keystrokes_box.append(&placement_row);
    root.append(&keystrokes_box);

    // Subtitles (shown when the project carries cues; macOS transcribes them).
    let subtitles_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (show_subtitles_row, show_subtitles) = switch_row("Show subtitles");
    let (highlight_row, subtitle_highlight) = switch_row("Highlight spoken word");
    type SubtitleToggle = fn(&mut super::model::Snapshot, bool);
    let subtitle_toggles: [(&gtk::Switch, SubtitleToggle); 2] = [
        (&show_subtitles, |s, on| s.shows_subtitles = on),
        (&subtitle_highlight, |s, on| s.subtitle_style.highlights_spoken_word = on),
    ];
    for (switch, apply) in subtitle_toggles {
        let weak = Rc::downgrade(studio);
        switch.connect_active_notify(move |switch| {
            let Some(studio) = weak.upgrade() else { return };
            if studio.updating.get() {
                return;
            }
            let on = switch.is_active();
            studio.model.borrow_mut().edit(|s| apply(s, on));
            studio.after_edit(true);
        });
    }
    let subtitle_position =
        slider("Position", SubtitleBarStyle::VERTICAL_RANGE.0, SubtitleBarStyle::VERTICAL_RANGE.1, 0.01, 2);
    let subtitle_size =
        slider("Text size", SubtitleBarStyle::FONT_SCALE_RANGE.0, SubtitleBarStyle::FONT_SCALE_RANGE.1, 0.05, 2);
    connect_slider(studio, &subtitle_position, "subtitle-position", |m, v| {
        m.edit_coalesced(|s| s.subtitle_style.vertical_position = v)
    });
    connect_slider(studio, &subtitle_size, "subtitle-size", |m, v| {
        m.edit_coalesced(|s| s.subtitle_style.font_scale = v)
    });
    let subtitle_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let transcribe = gtk::Button::builder().label("Transcribe").css_classes(["flat"]).build();
    transcribe.set_tooltip_text(Some("Turn the narration into subtitles with whisper.cpp"));
    let weak = Rc::downgrade(studio);
    transcribe.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            transcribe_narration(&studio);
        }
    });
    let remove_subtitles =
        gtk::Button::builder().label("Remove Subtitles").css_classes(["flat", "destructive-action"]).build();
    let weak = Rc::downgrade(studio);
    remove_subtitles.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.model.borrow_mut().set_transcript(Vec::new(), Vec::new());
            studio.after_edit(true);
        }
    });
    subtitle_buttons.append(&transcribe);
    subtitle_buttons.append(&remove_subtitles);
    let transcribe_note =
        gtk::Label::builder().wrap(true).max_width_chars(34).xalign(0.0).css_classes(["dim-label", "caption"]).build();
    let subtitle_controls = gtk::Box::new(gtk::Orientation::Vertical, 6);
    subtitle_controls.append(&show_subtitles_row);
    subtitle_controls.append(&subtitle_position.root);
    subtitle_controls.append(&subtitle_size.root);
    subtitle_controls.append(&highlight_row);
    subtitles_box.append(&subtitle_buttons);
    subtitles_box.append(&transcribe_note);
    subtitles_box.append(&subtitle_controls);
    root.append(&subtitles_box);

    // Zoom & Clips
    root.append(&heading("Zoom & Clips"));
    let (zoom_row, zoom_enabled) = switch_row("Zoom");
    let weak = Rc::downgrade(studio);
    zoom_enabled.connect_active_notify(move |switch| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        studio.model.borrow_mut().set_zoom_enabled(switch.is_active());
        studio.after_edit(true);
    });
    root.append(&zoom_row);
    let add_cue =
        gtk::Button::builder().label("Add Zoom at Playhead").css_classes(["flat"]).halign(gtk::Align::Start).build();
    let weak = Rc::downgrade(studio);
    add_cue.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            let added = studio.model.borrow_mut().add_zoom_cue_at_playhead();
            if added {
                studio.after_edit(true);
            } else {
                toast(&studio, "No room for a zoom here. Move the playhead outside other zooms.");
            }
        }
    });
    root.append(&add_cue);
    let (cue_row, cue_picker) = dropdown("Zoom", &["None"]);
    let weak = Rc::downgrade(studio);
    cue_picker.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let index = list.selected() as usize;
        let id = studio.model.borrow().state.zoom_cues.get(index).map(|c| c.id);
        studio.model.borrow_mut().selected_cue = id;
        studio.sync();
    });
    root.append(&cue_row);
    let cue_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let cue_start = slider("Start (s)", 0.0, 1.0, 0.1, 1);
    let cue_end = slider("End (s)", 0.0, 1.0, 0.1, 1);
    let cue_zoom = slider("Magnification", 1.0, 4.0, 0.1, 1);
    let cue_x = slider("Focus X", 0.0, 1.0, 0.01, 2);
    let cue_y = slider("Focus Y", 0.0, 1.0, 0.01, 2);
    let cue_fields: [CueField; 5] = [
        (&cue_start, |c, v| c.start = v),
        (&cue_end, |c, v| c.end = v),
        (&cue_zoom, |c, v| c.zoom = v),
        (&cue_x, |c, v| c.pinned_point = UnitPoint::new(v, c.pinned_point.y)),
        (&cue_y, |c, v| c.pinned_point = UnitPoint::new(c.pinned_point.x, v)),
    ];
    for (control, apply) in cue_fields {
        let weak = Rc::downgrade(studio);
        control.adjustment.connect_value_changed(move |adjustment| {
            let Some(studio) = weak.upgrade() else { return };
            if studio.updating.get() {
                return;
            }
            studio.begin_coalesced("cue");
            let value = adjustment.value();
            studio.update_selected_cue(true, move |cue| apply(cue, value));
        });
    }
    let modes: Vec<&str> = ZoomAnchorMode::ALL.iter().map(|m| m.title()).collect();
    let (mode_row, cue_mode) = dropdown("Follow", &modes);
    let weak = Rc::downgrade(studio);
    cue_mode.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let mode = ZoomAnchorMode::ALL[(list.selected() as usize).min(2)];
        studio.update_selected_cue(false, move |cue| cue.anchor_mode = mode);
    });
    let remove_cue = gtk::Button::builder()
        .label("Remove Zoom")
        .css_classes(["flat", "destructive-action"])
        .halign(gtk::Align::Start)
        .build();
    let weak = Rc::downgrade(studio);
    remove_cue.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            let id = studio.model.borrow().selected_cue;
            if let Some(id) = id {
                studio.model.borrow_mut().remove_zoom_cue(id);
                studio.after_edit(true);
            }
        }
    });
    for control in [&cue_start, &cue_end, &cue_zoom] {
        cue_box.append(&control.root);
    }
    cue_box.append(&mode_row);
    cue_box.append(&cue_x.root);
    cue_box.append(&cue_y.root);
    cue_box.append(&remove_cue);
    root.append(&cue_box);

    let clip_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let scissors = gtk::ToggleButton::builder().icon_name("edit-cut-symbolic").css_classes(["flat"]).build();
    scissors.set_tooltip_text(Some("Scissors: click the timeline to cut (S)"));
    scissors.update_property(&[gtk::accessible::Property::Label("Scissors")]);
    let weak = Rc::downgrade(studio);
    scissors.connect_toggled(move |button| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        studio.model.borrow_mut().scissors = button.is_active();
        studio.redraw_timeline();
    });
    let delete_clip = gtk::Button::builder().label("Delete Clip").css_classes(["flat"]).build();
    let weak = Rc::downgrade(studio);
    delete_clip.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.delete_clip();
        }
    });
    let reset_clips = gtk::Button::builder().label("Reset Clips").css_classes(["flat"]).build();
    let weak = Rc::downgrade(studio);
    reset_clips.connect_clicked(move |_| {
        if let Some(studio) = weak.upgrade() {
            studio.model.borrow_mut().reset_clips();
            studio.after_edit(true);
        }
    });
    clip_buttons.append(&scissors);
    clip_buttons.append(&delete_clip);
    clip_buttons.append(&reset_clips);
    root.append(&clip_buttons);
    let clip_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let clip_speed = slider("Clip speed", ClipSegment::MINIMUM_SPEED, ClipSegment::MAXIMUM_SPEED, 0.25, 2);
    let weak = Rc::downgrade(studio);
    clip_speed.adjustment.connect_value_changed(move |adjustment| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        studio.begin_coalesced("speed");
        let id = studio.model.borrow().selected_clip;
        if let Some(id) = id {
            studio.model.borrow_mut().set_clip_speed(id, adjustment.value(), true);
            studio.after_edit(false);
        }
    });
    clip_box.append(&clip_speed.root);
    root.append(&clip_box);

    // Export settings: the aspect reframe, then the encoder
    root.append(&heading("Export"));
    let aspects: Vec<&str> = ExportAspectPreset::ALL.iter().map(|p| p.title()).collect();
    let (aspect_row, aspect) = dropdown("Aspect ratio", &aspects);
    let weak = Rc::downgrade(studio);
    aspect.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let preset = ExportAspectPreset::ALL[(list.selected() as usize).min(ExportAspectPreset::ALL.len() - 1)];
        studio.model.borrow_mut().edit(|s| s.export_aspect = preset);
        studio.after_edit(true);
    });
    let (aspect_mode_row, aspect_mode) = dropdown("Framing", &["Fill", "Fit"]);
    let weak = Rc::downgrade(studio);
    aspect_mode.connect_selected_notify(move |list| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let mode = if list.selected() == 1 { ExportAspectContentMode::Fit } else { ExportAspectContentMode::Fill };
        studio.model.borrow_mut().edit(|s| s.export_aspect_mode = mode);
        studio.after_edit(true);
    });
    let aspect_note =
        gtk::Label::builder().wrap(true).max_width_chars(34).xalign(0.0).css_classes(["dim-label", "caption"]).build();
    root.append(&aspect_row);
    root.append(&aspect_mode_row);
    root.append(&aspect_note);
    let (format_row, format) = dropdown("Format", &["MP4", "MOV"]);
    let (rate_row, frame_rate) = dropdown("Frame rate", &["30 fps", "60 fps"]);
    let qualities: Vec<&str> = CompressionQuality::ALL.iter().map(|q| q.title()).collect();
    let (quality_row, quality) = dropdown("Quality", &qualities);
    let resolutions: Vec<&str> = CompressionResolution::ALL.iter().map(|r| r.title()).collect();
    let (resolution_row, resolution) = dropdown("Resolution", &resolutions);
    let (audio_row, include_audio) = switch_row("Include audio");
    for (list, apply) in [
        (
            &format,
            (|s: &mut bettershot_core::recording::edit::VideoCompressionSettings, i: usize| {
                s.container = Some(if i == 1 { VideoExportContainer::Mov } else { VideoExportContainer::Mp4 })
            }) as fn(&mut bettershot_core::recording::edit::VideoCompressionSettings, usize),
        ),
        (&frame_rate, |s, i| s.frame_rate = Some(if i == 1 { ExportFrameRate::Fps60 } else { ExportFrameRate::Fps30 })),
        (&quality, |s, i| s.quality = CompressionQuality::ALL[i.min(2)]),
        (&resolution, |s, i| s.resolution = CompressionResolution::ALL[i.min(3)]),
    ] {
        let weak = Rc::downgrade(studio);
        list.connect_selected_notify(move |list| {
            let Some(studio) = weak.upgrade() else { return };
            if studio.updating.get() {
                return;
            }
            let index = list.selected() as usize;
            studio.model.borrow_mut().edit(|s| apply(&mut s.export_settings, index));
            studio.after_edit(true);
        });
    }
    let weak = Rc::downgrade(studio);
    include_audio.connect_active_notify(move |switch| {
        let Some(studio) = weak.upgrade() else { return };
        if studio.updating.get() {
            return;
        }
        let on = switch.is_active();
        studio.model.borrow_mut().edit(|s| s.export_settings.remove_audio = !on);
        studio.after_edit(true);
    });
    for row in [&format_row, &rate_row, &quality_row, &resolution_row, &audio_row] {
        root.append(row);
    }

    Inspector {
        root,
        tiles,
        padding,
        corner,
        shadow,
        preset_picker,
        delete_preset,
        cursor_note,
        cursor_group,
        cursor_visible,
        cursor_style,
        cursor_size,
        cursor_smooth,
        click_effects,
        cursor_press,
        cursor_ripple,
        cursor_idle,
        camera_group,
        camera_note,
        camera_visible,
        layout,
        camera_left,
        camera_size,
        camera_roundness,
        camera_aspect,
        adjust_crop,
        apply_crop,
        crop_x,
        crop_y,
        crop_w,
        crop_h,
        mask_picker,
        mask_box,
        mask_effect,
        mask_amount,
        mask_x,
        mask_y,
        mask_w,
        mask_h,
        mask_start,
        mask_end,
        shot_preset,
        scene_count,
        shot_picker,
        shot_box,
        shot_apply_preset,
        shot_start,
        shot_end,
        shot_transition,
        shot_easing,
        shot_enabled,
        blur_mode,
        blur_box,
        blur_strength,
        blur_falloff,
        blur_focus_x,
        blur_focus_y,
        blur_focus_size,
        blur_bokeh,
        keystrokes_box,
        show_keystrokes,
        keystroke_placement,
        subtitles_box,
        subtitle_controls,
        transcribe,
        transcribe_note,
        remove_subtitles,
        show_subtitles,
        subtitle_position,
        subtitle_size,
        subtitle_highlight,
        zoom_enabled,
        cue_picker,
        cue_box,
        cue_start,
        cue_end,
        cue_zoom,
        cue_mode,
        cue_x,
        cue_y,
        scissors,
        delete_clip,
        clip_box,
        clip_speed,
        aspect,
        aspect_mode_row,
        aspect_mode,
        aspect_note,
        format,
        frame_rate,
        quality,
        resolution,
        include_audio,
    }
}

// MARK: - Drawing

fn draw_preview(studio: &Rc<Studio>, cr: &cairo::Context, width: f64, height: f64) {
    let mut model = studio.model.borrow_mut();
    let (canvas_w, canvas_h) = model.compositor.canvas;
    if canvas_w <= 0.0 || canvas_h <= 0.0 || width <= 0.0 || height <= 0.0 {
        return;
    }
    let scale = ((width - 24.0) / canvas_w).min((height - 24.0) / canvas_h).clamp(0.01, 2.0);
    let x = ((width - canvas_w * scale) / 2.0).floor();
    let y = ((height - canvas_h * scale) / 2.0).floor();
    studio.preview_transform.set((x, y, scale));
    let _ = cr.save();
    cr.translate(x, y);
    cr.scale(scale, scale);
    let playhead = model.playhead;
    let source_time = model.source_time(playhead);
    let camera_time = source_time - model.camera_lead_in;
    {
        let StudioModel { screen, camera, compositor, .. } = &mut *model;
        if let Some(screen_frame) = screen.frame_at(source_time) {
            let camera_frame = match camera.as_mut() {
                Some(camera) if camera_time >= 0.0 => camera.frame_at(camera_time),
                _ => None,
            };
            compositor.compose(cr, screen_frame, camera_frame, playhead);
        }
    }
    let _ = cr.restore();
    drop(model);
    draw_preview_overlays(studio, cr);
}

/// Widget-space rect of a normalized source rect, through the content rect.
fn source_rect_in_widget(studio: &Studio, model: &StudioModel, rect: UnitRect) -> Rect {
    let (ox, oy, scale) = studio.preview_transform.get();
    let content = model.compositor.content_rect(model.playhead);
    Rect::new(
        ox + (content.x + rect.x * content.width) * scale,
        oy + (content.y + rect.y * content.height) * scale,
        rect.width * content.width * scale,
        rect.height * content.height * scale,
    )
}

/// The content rect in widget space, the bounds for crop and mask drags.
fn content_rect_in_widget(studio: &Studio, model: &StudioModel) -> Rect {
    source_rect_in_widget(studio, model, UnitRect::UNIT)
}

fn widget_rect_to_source(studio: &Studio, model: &StudioModel, rect: Rect) -> UnitRect {
    let content = content_rect_in_widget(studio, model);
    UnitRect::new(
        (rect.x - content.x) / content.width.max(1.0),
        (rect.y - content.y) / content.height.max(1.0),
        rect.width / content.width.max(1.0),
        rect.height / content.height.max(1.0),
    )
}

fn handle_square(cr: &cairo::Context, p: Point) {
    let half = HANDLE_SIZE / 2.0;
    cr.rectangle(p.x - half, p.y - half, HANDLE_SIZE, HANDLE_SIZE);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(SELECTION.0, SELECTION.1, SELECTION.2);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
}

fn stroke_handles(cr: &cairo::Context, rect: Rect) {
    for handle in RegionHandle::ALL.into_iter().filter(|h| *h != RegionHandle::Move) {
        handle_square(cr, handle.point(rect));
    }
}

/// The crop being adjusted (dimmed outside, with its pixel size), or the
/// selected mask's rect, each with the same handles the image editor uses.
fn draw_preview_overlays(studio: &Rc<Studio>, cr: &cairo::Context) {
    let model = studio.model.borrow();
    if model.is_editing_crop() {
        let content = content_rect_in_widget(studio, &model);
        let rect = source_rect_in_widget(studio, &model, model.state.crop);
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.rectangle(content.x, content.y, content.width, content.height);
        cr.rectangle(rect.x, rect.y, rect.width, rect.height);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
        let _ = cr.fill();
        cr.set_fill_rule(cairo::FillRule::Winding);
        cr.rectangle(rect.x.round() + 0.5, rect.y.round() + 0.5, rect.width.round() - 1.0, rect.height.round() - 1.0);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        stroke_handles(cr, rect);
        let (w, h) = model.state.crop.cropped_size((f64::from(model.source_size.0), f64::from(model.source_size.1)));
        let label = format!("{} × {}", w.round(), h.round());
        cr.set_font_size(12.0);
        let extents = cr.text_extents(&label).ok();
        let lw = extents.map(|e| e.width()).unwrap_or(60.0);
        let (lx, ly) = (rect.x + 6.0, (rect.y - 26.0).max(content.y + 4.0));
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.72);
        cr.rectangle(lx, ly, lw + 12.0, 20.0);
        let _ = cr.fill();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.move_to(lx + 6.0, ly + 14.0);
        let _ = cr.show_text(&label);
        return;
    }
    let selected = model.selected_mask.and_then(|id| model.state.masks.iter().find(|m| m.id == id));
    if let Some(mask) = selected {
        let rect = source_rect_in_widget(studio, &model, mask.rect());
        cr.rectangle(rect.x.round() + 0.5, rect.y.round() + 0.5, rect.width.round() - 1.0, rect.height.round() - 1.0);
        cr.set_source_rgba(SELECTION.0, SELECTION.1, SELECTION.2, 0.9);
        cr.set_line_width(1.0);
        cr.set_dash(&[4.0, 3.0], 0.0);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
        stroke_handles(cr, rect);
    }
}

fn timeline_metrics(width: f64) -> (f64, f64) {
    let left = 12.0;
    (left, (width - 2.0 * left).max(1.0))
}

fn draw_timeline(studio: &Rc<Studio>, cr: &cairo::Context, width: f64, height: f64) {
    let model = studio.model.borrow();
    let duration = model.duration().max(0.001);
    let (left, span) = timeline_metrics(width);
    let px = |t: f64| left + (t / duration).clamp(0.0, 1.0) * span;
    let rgba =
        studio.widgets.borrow().as_ref().map(|w| w.timeline.color()).unwrap_or(gdk::RGBA::new(0.5, 0.5, 0.5, 1.0));
    let fg = (f64::from(rgba.red()), f64::from(rgba.green()), f64::from(rgba.blue()));
    let dark = fg.0 > 0.5;
    let base = if dark { 0.22 } else { 0.86 };
    cr.set_source_rgb(base, base, base);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();

    // 3D shot lane, above the zoom lane.
    let shots = model.timeline_3d();
    for shot in &shots.shots {
        let selected = model.selected_shot.as_ref() == Some(&shot.id);
        let (r, g, b) = if selected { (0.62, 0.42, 0.95) } else { (0.5, 0.35, 0.8) };
        cr.set_source_rgba(r, g, b, if shot.is_enabled { 0.9 } else { 0.4 });
        bettershot_render::rounded_rect(
            cr,
            (px(shot.start), 2.0, (px(shot.end) - px(shot.start)).max(3.0), 9.0),
            [3.0; 4],
        );
        let _ = cr.fill();
    }
    // Zoom lane.
    let lane_top = 13.0;
    let lane_h = 15.0;
    if model.state.zoom_enabled {
        for cue in &model.state.zoom_cues {
            let Some((start, end)) = model.cue_editor_range(cue) else { continue };
            let selected = model.selected_cue == Some(cue.id);
            let (r, g, b) = if selected { (0.98, 0.65, 0.2) } else { (0.85, 0.55, 0.15) };
            cr.set_source_rgba(r, g, b, if cue.is_enabled { 0.9 } else { 0.4 });
            bettershot_render::rounded_rect(
                cr,
                (px(start), lane_top, (px(end) - px(start)).max(3.0), lane_h),
                [4.0; 4],
            );
            let _ = cr.fill();
        }
    }

    // Clips.
    let clip_top = 32.0;
    let clip_h = 38.0;
    let mut editor_start = 0.0;
    for segment in &model.state.clips.segments {
        let editor_end = editor_start + segment.editor_duration();
        let selected = model.selected_clip == Some(segment.id);
        let (r, g, b) = if selected {
            (0.2, 0.5, 0.95)
        } else if dark {
            (0.36, 0.42, 0.52)
        } else {
            (0.62, 0.7, 0.82)
        };
        cr.set_source_rgb(r, g, b);
        bettershot_render::rounded_rect(
            cr,
            (px(editor_start) + 1.0, clip_top, (px(editor_end) - px(editor_start) - 2.0).max(2.0), clip_h),
            [6.0; 4],
        );
        let _ = cr.fill();
        if (segment.speed - 1.0).abs() > 0.001 {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
            cr.set_font_size(11.0);
            cr.move_to(px(editor_start) + 8.0, clip_top + 24.0);
            let _ = cr.show_text(&format!("{}×", trim_number(segment.speed)));
        }
        editor_start = editor_end;
    }
    // Cut badges with the removed duration.
    for marker in model.state.clips.cut_markers(model.source_duration) {
        let x = px(marker.editor_time);
        cr.set_source_rgb(0.9, 0.25, 0.25);
        cr.move_to(x - 5.0, clip_top + clip_h + 4.0);
        cr.line_to(x + 5.0, clip_top + clip_h + 4.0);
        cr.line_to(x, clip_top + clip_h + 10.0);
        cr.close_path();
        let _ = cr.fill();
        if marker.removed_duration() < 0.05 {
            continue;
        }
        cr.set_font_size(10.0);
        cr.set_source_rgba(fg.0, fg.1, fg.2, 0.8);
        let label = format!("-{}s", trim_number((marker.removed_duration() * 10.0).round() / 10.0));
        let extents = cr.text_extents(&label).map(|e| e.width()).unwrap_or(20.0);
        cr.move_to((x - extents / 2.0).clamp(left, width - extents - 4.0), clip_top + clip_h + 22.0);
        let _ = cr.show_text(&label);
    }
    // Playhead.
    let x = px(model.playhead);
    cr.set_source_rgb(0.95, 0.3, 0.3);
    cr.rectangle(x - 1.0, 2.0, 2.0, height - 4.0);
    let _ = cr.fill();
    if model.scissors {
        cr.set_font_size(11.0);
        cr.set_source_rgba(fg.0, fg.1, fg.2, 0.9);
        cr.move_to(left, height - 4.0);
        let _ = cr.show_text("Scissors: click to cut");
    }
}

/// Edits a shot's blur, starting from the showcase blur when it has none.
fn edit_blur(shot: &mut Shot3D, apply: impl FnOnce(&mut Blur3D)) {
    let mut blur = shot.blur.clone().unwrap_or_else(Blur3D::showcase);
    apply(&mut blur);
    shot.blur = Some(blur);
}

fn trim_number(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn format_time(seconds: f64) -> String {
    recording::formatted_duration(seconds)
}

// MARK: - Behavior

impl Studio {
    fn begin_coalesced(&self, key: &'static str) {
        if self.coalescing.get() != Some(key) {
            self.model.borrow_mut().begin_coalesced();
            self.coalescing.set(Some(key));
        }
    }

    /// After a model change: redraw, refresh the inspector when its
    /// structure changed, and schedule the draft.
    fn after_edit(&self, sync_inspector: bool) {
        if sync_inspector {
            self.coalescing.set(None);
            self.sync();
        } else {
            self.refresh_header();
            self.redraw();
        }
        self.schedule_draft();
    }

    fn update_selected_mask(&self, coalesced: bool, apply: impl FnOnce(&mut MaskSegment)) {
        let mask = {
            let model = self.model.borrow();
            model.selected_mask.and_then(|id| model.state.masks.iter().find(|m| m.id == id).cloned())
        };
        let Some(mut mask) = mask else { return };
        apply(&mut mask);
        self.model.borrow_mut().update_mask(mask, coalesced);
        self.after_edit(!coalesced);
    }

    fn update_selected_shot(&self, coalesced: bool, apply: impl FnOnce(&mut Shot3D)) {
        let shot = {
            let model = self.model.borrow();
            model.selected_shot.as_ref().and_then(|id| model.state.shots_3d.iter().find(|s| s.id == *id).cloned())
        };
        let Some(mut shot) = shot else { return };
        apply(&mut shot);
        // Neighbours bound the shot; the minimum duration keeps it on the lane.
        let (lower, upper) = self.shot_bounds(&shot.id);
        shot.start = shot.start.clamp(lower, upper);
        shot.end = shot.end.clamp(lower, upper);
        if shot.end - shot.start < Shot3D::MINIMUM_DURATION {
            shot.end = (shot.start + Shot3D::MINIMUM_DURATION).min(upper);
            shot.start = (shot.end - Shot3D::MINIMUM_DURATION).max(lower);
        }
        self.model.borrow_mut().update_shot(shot, coalesced);
        self.after_edit(!coalesced);
    }

    /// The span a shot may occupy without touching its neighbours.
    fn shot_bounds(&self, id: &str) -> (f64, f64) {
        let model = self.model.borrow();
        let mut lower: f64 = 0.0;
        let mut upper: f64 = model.duration();
        if let Some(shot) = model.state.shots_3d.iter().find(|s| s.id == id) {
            for other in model.state.shots_3d.iter().filter(|s| s.id != id) {
                if other.end <= shot.start + 1e-9 {
                    lower = lower.max(other.end);
                } else if other.start >= shot.end - 1e-9 {
                    upper = upper.min(other.start);
                }
            }
        }
        (lower, upper)
    }

    fn add_shot(&self) {
        let preset = self
            .widgets
            .borrow()
            .as_ref()
            .map(|w| Preset3D::ALL[(w.inspector.shot_preset.selected() as usize).min(Preset3D::ALL.len() - 1)])
            .unwrap_or(Preset3D::Glide);
        let added = self.model.borrow_mut().add_shot_at_playhead(preset);
        if added {
            self.after_edit(true);
        } else {
            toast(self, "No room for a shot here. Move the playhead outside other shots.");
        }
    }

    fn update_selected_cue(&self, coalesced: bool, apply: impl FnOnce(&mut ZoomCue)) {
        let cue = {
            let model = self.model.borrow();
            model.selected_cue.and_then(|id| model.state.zoom_cues.iter().find(|c| c.id == id).cloned())
        };
        let Some(mut cue) = cue else { return };
        apply(&mut cue);
        self.model.borrow_mut().update_zoom_cue(cue, coalesced);
        self.after_edit(!coalesced);
    }

    fn redraw(&self) {
        if let Some(widgets) = self.widgets.borrow().as_ref() {
            widgets.preview.queue_draw();
            widgets.timeline.queue_draw();
            let model = self.model.borrow();
            widgets.time_label.set_label(&format!(
                "{} / {}",
                format_time(model.playhead),
                format_time(model.duration())
            ));
        }
    }

    /// Applies the preset at `index` of the picker (0 is the placeholder).
    fn apply_preset(&self, index: u32) {
        let preset = {
            let presets = self.presets.borrow();
            index.checked_sub(1).and_then(|i| presets.presets.get(i as usize).cloned())
        };
        let Some(preset) = preset else { return };
        let style = preset.value();
        self.applied_preset.set(Some(preset.id));
        {
            let mut presets = self.presets.borrow_mut();
            presets.set_active_preset(Some(preset.id));
            let _ = presets.save(&mut App::get().state_mut().prefs);
        }
        self.model.borrow_mut().edit(|s| s.style = style);
        self.after_edit(true);
    }

    /// Saves the current look under `name`; false when the name is empty or taken.
    fn save_preset(&self, name: &str) -> bool {
        let style = self.model.borrow().state.style.clone();
        let saved = {
            let mut presets = self.presets.borrow_mut();
            let id = presets.save_preset(name, &style).map(|p| p.id);
            if id.is_some() {
                presets.set_active_preset(id);
                let _ = presets.save(&mut App::get().state_mut().prefs);
            }
            id
        };
        match saved {
            Some(id) => {
                self.applied_preset.set(Some(id));
                self.sync();
                // Keyboard control returns to the preview, not the + button.
                if let Some(widgets) = self.widgets.borrow().as_ref() {
                    widgets.preview.grab_focus();
                }
                true
            }
            None => false,
        }
    }

    fn delete_applied_preset(&self) {
        let Some(id) = self.applied_preset.get() else { return };
        let name = self.presets.borrow().preset(Some(id)).map(|p| p.name.clone()).unwrap_or_default();
        let dialog = adw::AlertDialog::new(
            Some("Delete Preset?"),
            Some(&format!("“{name}” will be removed. Your current settings stay as they are.")),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let window = self.widgets.borrow().as_ref().map(|w| w.window.clone());
        let weak = self.weak_self();
        dialog.choose(window.as_ref(), None::<&gio::Cancellable>, move |response| {
            if response != "delete" {
                return;
            }
            let Some(studio) = weak.upgrade() else { return };
            {
                let mut presets = studio.presets.borrow_mut();
                presets.delete_preset(id);
                let _ = presets.save(&mut App::get().state_mut().prefs);
            }
            studio.applied_preset.set(None);
            studio.sync();
        });
    }

    /// Keeps the soundtrack on the playhead's clip at that clip's speed.
    fn sync_audio(&self, playing: bool) {
        let mut audio = self.audio.borrow_mut();
        let Some(player) = audio.as_mut() else { return };
        if !playing {
            player.pause();
            self.audio_segment.set(None);
            return;
        }
        let model = self.model.borrow();
        let Some(location) = model.state.clips.location(model.playhead) else { return };
        let segment = model.state.clips.segments.get(location.segment_index);
        if self.audio_segment.get() != Some(location.segment_index) {
            player.play_from(location.source_time, segment.map(|s| s.speed).unwrap_or(1.0));
            self.audio_segment.set(Some(location.segment_index));
        }
    }

    /// The camera bubble in widget space when it can be dragged (the Bubble
    /// layout places it by its center).
    fn draggable_bubble(&self, model: &StudioModel) -> Option<Rect> {
        let style = &model.state.style;
        if !model.has_camera || !style.camera.is_visible || style.layout_preset != LayoutPreset::Bubble {
            return None;
        }
        let bubble = model.compositor.layout.bubble_rect;
        if bubble.width <= 0.0 {
            return None;
        }
        let (ox, oy, scale) = self.preview_transform.get();
        Some(Rect::new(ox + bubble.x * scale, oy + bubble.y * scale, bubble.width * scale, bubble.height * scale))
    }

    /// What a press at (x, y) on the preview would drag.
    fn preview_hit(&self, x: f64, y: f64) -> Option<PreviewDrag> {
        let model = self.model.borrow();
        let point = Point::new(x, y);
        if model.is_editing_crop() {
            let rect = source_rect_in_widget(self, &model, model.state.crop);
            return region::handle_at(point, rect).map(|handle| PreviewDrag::Crop { handle, start: rect });
        }
        if let Some(bubble) = self.draggable_bubble(&model) {
            if bubble.contains(point) {
                return Some(PreviewDrag::Camera { start: model.state.style.camera.center });
            }
        }
        if let Some(mask) = model.selected_mask.and_then(|id| model.state.masks.iter().find(|m| m.id == id)) {
            let rect = source_rect_in_widget(self, &model, mask.rect());
            if let Some(handle) = region::handle_at(point, rect) {
                return Some(PreviewDrag::Mask { id: mask.id, handle, start: rect });
            }
        }
        // Pressing inside another mask selects it and moves it.
        model.state.masks.iter().rev().find_map(|mask| {
            let rect = source_rect_in_widget(self, &model, mask.rect());
            rect.contains(point).then_some(PreviewDrag::Mask { id: mask.id, handle: RegionHandle::Move, start: rect })
        })
    }

    fn preview_cursor(&self, x: f64, y: f64) -> Option<&'static str> {
        match self.preview_hit(x, y)? {
            PreviewDrag::Camera { .. } => Some("move"),
            PreviewDrag::Mask { handle, .. } | PreviewDrag::Crop { handle, .. } => Some(handle.cursor_name()),
        }
    }

    fn preview_drag_begin(&self, x: f64, y: f64) {
        if let Some(widgets) = self.widgets.borrow().as_ref() {
            widgets.preview.grab_focus();
        }
        let Some(hit) = self.preview_hit(x, y) else { return };
        if let PreviewDrag::Mask { id, .. } = hit {
            if self.model.borrow().selected_mask != Some(id) {
                self.model.borrow_mut().selected_mask = Some(id);
                self.sync();
            }
        }
        self.coalescing.set(None);
        self.begin_coalesced(match hit {
            PreviewDrag::Camera { .. } => "camera-drag",
            PreviewDrag::Mask { .. } => "mask-drag",
            PreviewDrag::Crop { .. } => "crop-drag",
        });
        *self.drag.borrow_mut() = Some(hit);
    }

    fn preview_drag_update(&self, dx: f64, dy: f64) {
        let Some(drag) = *self.drag.borrow() else { return };
        let delta = Size::new(dx, dy);
        match drag {
            PreviewDrag::Camera { start } => {
                let (_, _, scale) = self.preview_transform.get();
                let mut model = self.model.borrow_mut();
                let (w, h) = model.compositor.canvas;
                let center = UnitPoint::new(
                    (start.x + dx / scale / w.max(1.0)).clamp(0.0, 1.0),
                    (start.y + dy / scale / h.max(1.0)).clamp(0.0, 1.0),
                );
                model.edit_coalesced(|s| s.style.camera.center = center);
            }
            PreviewDrag::Mask { id, handle, start } => {
                let rect = {
                    let model = self.model.borrow();
                    let bounds = content_rect_in_widget(self, &model);
                    widget_rect_to_source(self, &model, region::apply(handle, delta, start, bounds))
                };
                let mask = {
                    let model = self.model.borrow();
                    model.state.masks.iter().find(|m| m.id == id).cloned()
                };
                if let Some(mut mask) = mask {
                    mask.set_rect(UnitRect::new(
                        rect.x,
                        rect.y,
                        rect.width.max(UnitRect::MINIMUM_SPAN),
                        rect.height.max(UnitRect::MINIMUM_SPAN),
                    ));
                    self.model.borrow_mut().update_mask(mask, true);
                }
            }
            PreviewDrag::Crop { handle, start } => {
                let rect = {
                    let model = self.model.borrow();
                    let bounds = content_rect_in_widget(self, &model);
                    widget_rect_to_source(self, &model, region::apply(handle, delta, start, bounds))
                };
                self.model.borrow_mut().set_crop(rect, true);
            }
        }
        self.after_edit(false);
    }

    fn preview_drag_end(&self) {
        if self.drag.borrow_mut().take().is_some() {
            self.coalescing.set(None);
            self.sync();
            self.schedule_draft();
        }
    }

    fn apply_crop_edit(&self) {
        self.model.borrow_mut().apply_crop_edit();
        self.coalescing.set(None);
        self.after_edit(true);
    }

    fn cancel_crop_edit(&self) {
        self.model.borrow_mut().cancel_crop_edit();
        self.coalescing.set(None);
        self.after_edit(true);
    }

    fn redraw_timeline(&self) {
        if let Some(widgets) = self.widgets.borrow().as_ref() {
            widgets.timeline.queue_draw();
        }
    }

    fn refresh_header(&self) {
        let widgets_ref = self.widgets.borrow();
        let Some(widgets) = widgets_ref.as_ref() else { return };
        let model = self.model.borrow();
        widgets.undo.set_sensitive(model.can_undo());
        widgets.redo.set_sensitive(model.can_redo());
        let busy = self.render_cancel.borrow().is_some();
        widgets.save.set_sensitive(!busy);
        widgets.export.set_sensitive(!busy);
        widgets.title.set_subtitle(if model.is_dirty() { "Edited" } else { "" });
    }

    /// Fills the inspector from the model.
    fn sync(&self) {
        self.updating.set(true);
        {
            let widgets_ref = self.widgets.borrow();
            let Some(widgets) = widgets_ref.as_ref() else {
                drop(widgets_ref);
                self.updating.set(false);
                return;
            };
            let model = self.model.borrow();
            let i = &widgets.inspector;
            let style = &model.state.style;
            for (tile_style, button) in &i.tiles {
                if *tile_style == style.background {
                    button.add_css_class("selected");
                } else {
                    button.remove_css_class("selected");
                }
            }
            i.padding.adjustment.set_value(style.padding);
            i.corner.adjustment.set_value(style.corner_radius);
            i.shadow.adjustment.set_value(style.shadow);
            {
                let presets = self.presets.borrow();
                let applied = self.applied_preset.get().and_then(|id| presets.preset(Some(id)));
                let mut names: Vec<String> = vec![match applied {
                    Some(preset) if !preset.matches(style) => format!("{} (edited)", preset.name),
                    Some(preset) => preset.name.clone(),
                    None => "Presets…".to_string(),
                }];
                names.extend(presets.presets.iter().map(|p| p.name.clone()));
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                i.preset_picker.set_model(Some(&gtk::StringList::new(&names)));
                i.preset_picker.set_selected(0);
                i.delete_preset.set_visible(applied.is_some());
            }
            let styles_pointer = model.can_style_pointer();
            i.cursor_note.set_visible(!styles_pointer);
            i.cursor_group.set_visible(styles_pointer);
            i.cursor_visible.set_active(style.cursor.is_visible);
            i.cursor_style.set_selected(
                CursorAppearance::SELECTABLE.iter().position(|a| *a == style.cursor.appearance).unwrap_or(0) as u32,
            );
            i.cursor_size.adjustment.set_value(style.cursor_scale);
            i.cursor_smooth.set_active(style.cursor.smooth_motion);
            i.click_effects.set_active(model.state.shows_click_effects);
            i.cursor_press.set_active(style.cursor.press_effect);
            i.cursor_ripple.set_active(style.cursor.ripple_effect);
            i.cursor_idle.set_active(style.cursor.hide_when_idle);
            for control in [&i.cursor_press, &i.cursor_ripple] {
                control.set_sensitive(model.state.shows_click_effects);
            }
            i.camera_note.set_visible(!model.has_camera);
            i.camera_group.set_visible(model.has_camera);
            i.camera_visible.set_active(style.camera.is_visible);
            i.layout.set_selected(LayoutPreset::ALL.iter().position(|p| *p == style.layout_preset).unwrap_or(0) as u32);
            i.camera_left.set_active(style.camera_on_left);
            i.camera_size.adjustment.set_value(style.camera.size);
            i.camera_roundness.set_value(style.camera.roundness);
            i.camera_aspect.set_selected(
                CameraAspectRatio::ALL.iter().position(|r| *r == style.camera.aspect_ratio).unwrap_or(0) as u32,
            );
            let editing_crop = model.is_editing_crop();
            i.adjust_crop.set_active(editing_crop);
            i.apply_crop.set_visible(editing_crop);
            let crop = model.state.crop;
            i.crop_x.adjustment.set_value(crop.x);
            i.crop_y.adjustment.set_value(crop.y);
            i.crop_w.adjustment.set_value(crop.width);
            i.crop_h.adjustment.set_value(crop.height);
            let duration = model.duration().max(0.1);
            let mask_names: Vec<String> =
                model.state.masks.iter().enumerate().map(|(n, m)| format!("{} {}", m.effect.title(), n + 1)).collect();
            let names: Vec<&str> =
                if mask_names.is_empty() { vec!["None"] } else { mask_names.iter().map(String::as_str).collect() };
            i.mask_picker.set_model(Some(&gtk::StringList::new(&names)));
            let mask_index = model.selected_mask.and_then(|id| model.state.masks.iter().position(|m| m.id == id));
            i.mask_picker.set_selected(mask_index.unwrap_or(0) as u32);
            i.mask_picker.set_sensitive(!model.state.masks.is_empty());
            let mask = mask_index.and_then(|index| model.state.masks.get(index));
            i.mask_box.set_visible(mask.is_some());
            if let Some(mask) = mask {
                i.mask_effect.set_selected(if mask.effect == MaskEffect::Blur { 0 } else { 1 });
                i.mask_amount.adjustment.set_value(mask.amount);
                i.mask_x.adjustment.set_value(mask.x);
                i.mask_y.adjustment.set_value(mask.y);
                i.mask_w.adjustment.set_value(mask.width);
                i.mask_h.adjustment.set_value(mask.height);
                for control in [&i.mask_start, &i.mask_end] {
                    control.adjustment.set_upper(duration);
                }
                let (start, end) = mask.editor_range(duration);
                i.mask_start.adjustment.set_value(start);
                i.mask_end.adjustment.set_value(end);
            }
            let timeline = model.timeline_3d();
            let shot_names: Vec<String> = timeline
                .shots
                .iter()
                .map(|s| {
                    format!(
                        "{} · {}–{} s",
                        s.title(),
                        trim_number((s.start * 10.0).round() / 10.0),
                        trim_number((s.end * 10.0).round() / 10.0)
                    )
                })
                .collect();
            let names: Vec<&str> =
                if shot_names.is_empty() { vec!["None"] } else { shot_names.iter().map(String::as_str).collect() };
            i.shot_picker.set_model(Some(&gtk::StringList::new(&names)));
            let shot_index =
                model.selected_shot.as_ref().and_then(|id| timeline.shots.iter().position(|s| s.id == *id));
            i.shot_picker.set_selected(shot_index.unwrap_or(0) as u32);
            i.shot_picker.set_sensitive(!timeline.shots.is_empty());
            let maximum = bettershot_core::recording::Timeline3D::maximum_auto_shots(model.duration()).clamp(1, 6);
            if i.scene_count.selected() as usize + 1 > maximum {
                i.scene_count.set_selected((maximum - 1) as u32);
            }
            let shot = shot_index.and_then(|index| timeline.shots.get(index));
            i.shot_box.set_visible(shot.is_some());
            if let Some(shot) = shot {
                drop(model);
                let (lower, upper) = self.shot_bounds(&shot.id);
                let model = self.model.borrow();
                for control in [&i.shot_start, &i.shot_end] {
                    control.adjustment.set_lower(lower);
                    control.adjustment.set_upper(upper);
                }
                i.shot_start.adjustment.set_value(shot.start);
                i.shot_end.adjustment.set_value(shot.end);
                i.shot_transition.adjustment.set_value(shot.transition);
                i.shot_apply_preset.set_selected(
                    Preset3D::ALL
                        .iter()
                        .position(|p| {
                            let (a, b) = p.poses();
                            a == shot.start_pose && b == shot.end_pose
                        })
                        .unwrap_or(0) as u32,
                );
                i.shot_easing.set_selected(Easing3D::ALL.iter().position(|e| *e == shot.easing).unwrap_or(0) as u32);
                i.shot_enabled.set_active(shot.is_enabled);
                let blur = shot.blur.clone().unwrap_or_default();
                i.blur_mode.set_selected(BlurMode::ALL.iter().position(|m| *m == blur.mode).unwrap_or(0) as u32);
                i.blur_box.set_visible(blur.mode != BlurMode::None);
                i.blur_strength.adjustment.set_upper(if blur.bokeh { 20.0 } else { 60.0 });
                i.blur_strength.adjustment.set_value(blur.strength);
                i.blur_falloff.adjustment.set_value(blur.falloff);
                i.blur_focus_x.adjustment.set_value(blur.focus_x);
                i.blur_focus_y.adjustment.set_value(blur.focus_y);
                i.blur_focus_size.adjustment.set_value(blur.focus_size);
                i.blur_bokeh.set_active(blur.bokeh);
                let _ = model;
            }
            let model = self.model.borrow();
            i.keystrokes_box.set_visible(model.has_keystrokes());
            i.show_keystrokes.set_active(model.state.shows_keystrokes);
            i.keystroke_placement.set_selected(
                KeystrokePlacement::ALL.iter().position(|p| *p == model.state.keystroke_placement).unwrap_or(4) as u32,
            );
            i.keystroke_placement.set_sensitive(model.state.shows_keystrokes);
            let transcription = bettershot_platform::transcription::availability(
                App::get().with_prefs(|p| p.whisper_model_path()).as_deref(),
            );
            i.subtitles_box.set_visible(model.has_subtitles() || model.has_audio);
            i.subtitle_controls.set_visible(model.has_subtitles());
            i.remove_subtitles.set_visible(model.has_subtitles());
            i.transcribe.set_visible(model.has_audio);
            i.transcribe.set_sensitive(transcription.is_ok() && self.render_cancel.borrow().is_none());
            i.transcribe_note.set_visible(model.has_audio && !model.has_subtitles());
            i.transcribe_note.set_label(match &transcription {
                Ok(_) => "Subtitles come from the narration through whisper.cpp; edit the result in the macOS app.",
                Err(reason) => reason,
            });
            i.show_subtitles.set_active(model.state.shows_subtitles);
            i.subtitle_position.adjustment.set_value(model.state.subtitle_style.vertical_position);
            i.subtitle_size.adjustment.set_value(model.state.subtitle_style.font_scale);
            i.subtitle_highlight.set_active(model.state.subtitle_style.highlights_spoken_word);
            i.subtitle_highlight.set_sensitive(model.has_word_timings());
            i.zoom_enabled.set_active(model.state.zoom_enabled);
            let cue_names: Vec<String> = model
                .state
                .zoom_cues
                .iter()
                .map(|c| {
                    format!(
                        "{}–{} s · {}×",
                        trim_number((c.start * 10.0).round() / 10.0),
                        trim_number((c.end * 10.0).round() / 10.0),
                        trim_number(c.zoom)
                    )
                })
                .collect();
            let names: Vec<&str> =
                if cue_names.is_empty() { vec!["None"] } else { cue_names.iter().map(String::as_str).collect() };
            i.cue_picker.set_model(Some(&gtk::StringList::new(&names)));
            let cue_index = model.selected_cue.and_then(|id| model.state.zoom_cues.iter().position(|c| c.id == id));
            i.cue_picker.set_selected(cue_index.unwrap_or(0) as u32);
            i.cue_picker.set_sensitive(!model.state.zoom_cues.is_empty());
            let cue = cue_index.and_then(|index| model.state.zoom_cues.get(index));
            i.cue_box.set_visible(cue.is_some());
            if let Some(cue) = cue {
                let (lower, upper) = model.cue_bounds(cue.id);
                for control in [&i.cue_start, &i.cue_end] {
                    control.adjustment.set_lower(lower);
                    control.adjustment.set_upper(upper);
                }
                i.cue_start.adjustment.set_value(cue.start);
                i.cue_end.adjustment.set_value(cue.end);
                i.cue_zoom.adjustment.set_value(cue.zoom);
                i.cue_mode
                    .set_selected(ZoomAnchorMode::ALL.iter().position(|m| *m == cue.anchor_mode).unwrap_or(0) as u32);
                i.cue_x.adjustment.set_value(cue.pinned_point.x);
                i.cue_y.adjustment.set_value(cue.pinned_point.y);
            }
            i.scissors.set_active(model.scissors);
            let clip = model.selected_clip.and_then(|id| model.state.clips.segments.iter().find(|s| s.id == id));
            i.delete_clip.set_sensitive(clip.is_some() && model.state.clips.segments.len() > 1);
            i.clip_box.set_visible(clip.is_some());
            if let Some(clip) = clip {
                i.clip_speed.adjustment.set_value(clip.speed);
            }
            i.aspect.set_selected(
                ExportAspectPreset::ALL.iter().position(|p| *p == model.state.export_aspect).unwrap_or(0) as u32,
            );
            let reframed = model.state.export_aspect.ratio().is_some();
            i.aspect_mode_row.set_visible(reframed);
            i.aspect_note.set_visible(reframed);
            i.aspect_mode.set_selected(if model.state.export_aspect_mode == ExportAspectContentMode::Fit {
                1
            } else {
                0
            });
            i.aspect_note.set_label(if model.state.export_aspect_mode == ExportAspectContentMode::Fit {
                "Shows the whole recording framed on the background."
            } else {
                "Crops into the recording; the camera follows your cursor and zooms."
            });
            let settings = &model.state.export_settings;
            i.format.set_selected(if settings.effective_container() == VideoExportContainer::Mov { 1 } else { 0 });
            i.frame_rate.set_selected(if settings.effective_frame_rate() == ExportFrameRate::Fps60 { 1 } else { 0 });
            i.quality
                .set_selected(CompressionQuality::ALL.iter().position(|q| *q == settings.quality).unwrap_or(1) as u32);
            i.resolution.set_selected(
                CompressionResolution::ALL.iter().position(|r| *r == settings.resolution).unwrap_or(0) as u32,
            );
            i.include_audio.set_active(!settings.remove_audio);
        }
        self.updating.set(false);
        self.refresh_header();
        self.redraw();
    }

    fn schedule_draft(&self) {
        if let Some(id) = self.draft_timer.borrow_mut().take() {
            id.remove();
        }
        let weak = self.weak_self();
        let id = glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if let Some(studio) = weak.upgrade() {
                studio.draft_timer.borrow_mut().take();
                if let Err(error) = studio.model.borrow().write_draft() {
                    eprintln!("bettershot: the draft could not be written: {error}");
                }
            }
        });
        *self.draft_timer.borrow_mut() = Some(id);
    }

    fn weak_self(&self) -> std::rc::Weak<Studio> {
        OPEN.with(|open| {
            open.borrow()
                .iter()
                .find(|(_, s)| std::ptr::eq(&**s, self))
                .map(|(_, s)| Rc::downgrade(s))
                .unwrap_or_default()
        })
    }

    fn seek(&self, time: f64) {
        {
            let mut model = self.model.borrow_mut();
            let duration = model.duration();
            model.playhead = time.clamp(0.0, duration);
        }
        if self.playing.get() {
            // A seek restarts the soundtrack from the new playhead.
            self.audio_segment.set(None);
            self.sync_audio(true);
        }
        self.redraw();
    }

    fn timeline_pressed(&self, x: f64, y: f64, width: f64) {
        let (left, span) = timeline_metrics(width);
        let duration = self.model.borrow().duration();
        let time = ((x - left) / span).clamp(0.0, 1.0) * duration;
        let scissors = self.model.borrow().scissors;
        // The 3D lane selects its shot.
        if !scissors && y < 12.0 {
            self.seek(time);
            let shot = self.model.borrow().shot_at_playhead();
            if shot.is_some() {
                self.model.borrow_mut().selected_shot = shot;
                self.sync();
                return;
            }
        }
        if scissors {
            self.seek(time);
            let split = self.model.borrow_mut().split_at_playhead();
            if split {
                self.after_edit(true);
            } else {
                toast(self, "Move the playhead a little further from the clip's ends to cut there.");
            }
            return;
        }
        self.seek(time);
        let clip = self.model.borrow().clip_at_playhead();
        self.model.borrow_mut().selected_clip = clip;
        self.sync();
    }

    fn scrub(&self, x: f64, width: f64) {
        if self.model.borrow().scissors {
            return;
        }
        let (left, span) = timeline_metrics(width);
        let duration = self.model.borrow().duration();
        self.seek(((x - left) / span).clamp(0.0, 1.0) * duration);
    }

    fn delete_clip(&self) {
        let deleted = self.model.borrow_mut().delete_selected_clip();
        if deleted {
            self.after_edit(true);
        } else {
            toast(self, "Select a clip to delete; the last clip cannot be removed.");
        }
    }

    fn undo_redo(&self, undo: bool) {
        self.coalescing.set(None);
        let changed = if undo { self.model.borrow_mut().undo() } else { self.model.borrow_mut().redo() };
        if changed {
            self.after_edit(true);
        }
    }

    fn toggle_playback(&self) {
        if self.playing.get() {
            self.stop_playback();
        } else {
            self.start_playback();
        }
    }

    fn start_playback(&self) {
        {
            let mut model = self.model.borrow_mut();
            if model.playhead >= model.duration() - 0.001 {
                model.playhead = 0.0;
            }
        }
        self.playing.set(true);
        self.last_tick.set(Some(Instant::now()));
        self.audio_segment.set(None);
        self.sync_audio(true);
        if let Some(widgets) = self.widgets.borrow().as_ref() {
            widgets.play_icon.set_icon_name(Some("media-playback-pause-symbolic"));
        }
        let weak = self.weak_self();
        let id = glib::timeout_add_local(Duration::from_millis(33), move || {
            let Some(studio) = weak.upgrade() else { return glib::ControlFlow::Break };
            let now = Instant::now();
            let elapsed = studio.last_tick.get().map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
            studio.last_tick.set(Some(now));
            let finished = {
                let mut model = studio.model.borrow_mut();
                let duration = model.duration();
                model.playhead = (model.playhead + elapsed).min(duration);
                model.playhead >= duration - 0.0005
            };
            studio.sync_audio(true);
            studio.redraw();
            if finished {
                studio.stop_playback();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        *self.play_timer.borrow_mut() = Some(id);
    }

    fn stop_playback(&self) {
        self.playing.set(false);
        self.sync_audio(false);
        if let Some(id) = self.play_timer.borrow_mut().take() {
            id.remove();
        }
        if let Some(widgets) = self.widgets.borrow().as_ref() {
            widgets.play_icon.set_icon_name(Some("media-playback-start-symbolic"));
        }
    }

    fn on_key(&self, key: gdk::Key, state: gdk::ModifierType) -> bool {
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        match key {
            gdk::Key::space => self.toggle_playback(),
            gdk::Key::s | gdk::Key::S if !ctrl => {
                let scissors = !self.model.borrow().scissors;
                self.model.borrow_mut().scissors = scissors;
                self.sync();
            }
            gdk::Key::s | gdk::Key::S if ctrl => save_project(&self.weak_self().upgrade().expect("studio"), false),
            gdk::Key::Delete | gdk::Key::BackSpace => self.delete_clip(),
            // Keyboard access to the Effects tools: C adjusts the crop, B and
            // P add a blur or pixelate mask (Linux additions).
            gdk::Key::c | gdk::Key::C if !ctrl => {
                if self.model.borrow().is_editing_crop() {
                    self.cancel_crop_edit();
                } else {
                    self.model.borrow_mut().begin_crop_edit();
                    self.after_edit(true);
                }
            }
            gdk::Key::b | gdk::Key::B if !ctrl => {
                self.model.borrow_mut().add_mask(MaskEffect::Blur);
                self.after_edit(true);
            }
            gdk::Key::p | gdk::Key::P if !ctrl => {
                self.model.borrow_mut().add_mask(MaskEffect::Pixelate);
                self.after_edit(true);
            }
            gdk::Key::d | gdk::Key::D if !ctrl => self.add_shot(),
            gdk::Key::z | gdk::Key::Z if ctrl => self.undo_redo(!shift),
            gdk::Key::Left => {
                let t = self.model.borrow().playhead - if shift { 1.0 } else { FRAME_STEP };
                self.seek(t);
            }
            gdk::Key::Right => {
                let t = self.model.borrow().playhead + if shift { 1.0 } else { FRAME_STEP };
                self.seek(t);
            }
            gdk::Key::Home => self.seek(0.0),
            gdk::Key::End => {
                let d = self.model.borrow().duration();
                self.seek(d);
            }
            gdk::Key::w | gdk::Key::W if ctrl => {
                if let Some(widgets) = self.widgets.borrow().as_ref() {
                    widgets.window.close();
                }
            }
            gdk::Key::Return | gdk::Key::KP_Enter if self.model.borrow().is_editing_crop() => self.apply_crop_edit(),
            gdk::Key::Escape if self.model.borrow().is_editing_crop() => self.cancel_crop_edit(),
            gdk::Key::Escape if self.model.borrow().scissors => {
                self.model.borrow_mut().scissors = false;
                self.sync();
            }
            _ => return false,
        }
        true
    }

    fn teardown(&self) {
        self.stop_playback();
        self.audio.borrow_mut().take();
        if let Some(id) = self.draft_timer.borrow_mut().take() {
            id.remove();
        }
        let directory = self.model.borrow().session.directory.clone();
        OPEN.with(|open| open.borrow_mut().retain(|(p, _)| *p != directory));
    }

    fn set_rendering(&self, rendering: bool, message: &str) {
        if let Some(widgets) = self.widgets.borrow().as_ref() {
            widgets.progress.set_visible(rendering);
            widgets.cancel.set_visible(rendering);
            widgets.progress.set_fraction(0.0);
            widgets.status.set_label(message);
        }
        self.refresh_header();
    }
}

fn toast(studio: &Studio, message: &str) {
    if let Some(widgets) = studio.widgets.borrow().as_ref() {
        widgets.status.set_label(message);
    }
}

fn confirm_close(studio: &Rc<Studio>, window: &adw::ApplicationWindow) {
    let name = studio.model.borrow().session.display_name();
    let dialog = adw::AlertDialog::new(
        Some(&format!("Save changes to “{name}”?")),
        Some("Your edits stay in a draft until you save; Discard throws the draft away."),
    );
    dialog.add_responses(&[("cancel", "Cancel"), ("discard", "Discard"), ("save", "Save")]);
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    let studio = studio.clone();
    let window = window.clone();
    dialog.choose(Some(&window.clone()), None::<&gio::Cancellable>, move |response| match response.as_str() {
        "save" => save_project(&studio, true),
        "discard" => {
            studio.model.borrow().discard_draft();
            studio.teardown();
            window.destroy();
        }
        _ => {}
    });
}

// MARK: - Save and export

/// Where a render runs, and what to do with its file afterwards.
enum RenderTarget {
    /// Save: the package's flattened deliverable, then the saved file.
    Save { then_close: bool },
    /// Export As: a new file the user chose.
    ExportAs(PathBuf),
}

/// Port of `saveProject` + `RecordingDeliverable.saveToDefaultLocation`:
/// commits `edit.json`, renders the flattened deliverable unless the
/// recording is untouched, and writes or replaces the saved file.
fn save_project(studio: &Rc<Studio>, then_close: bool) {
    if studio.render_cancel.borrow().is_some() {
        return;
    }
    let document = match studio.model.borrow_mut().save_document() {
        Ok(document) => document,
        Err(error) => return toast(studio, &format!("Couldn’t save the project. {error}")),
    };
    studio.refresh_header();
    let (session, unedited, container) = {
        let model = studio.model.borrow();
        (model.session.clone(), model.is_unedited(), model.state.export_settings.effective_container())
    };
    if unedited {
        session.remove_final_videos();
        finish_save(studio, session.screen_path(), then_close);
        return;
    }
    if let Some(fresh) = session.fresh_final_path(Some(&document.to_value())) {
        finish_save(studio, fresh, then_close);
        return;
    }
    let temporary = session.directory.join(format!("render-{}.{}", uuid::Uuid::new_v4(), container.file_extension()));
    render(studio, temporary, RenderTarget::Save { then_close }, Some(document.to_value()));
}

fn export_as(studio: &Rc<Studio>) {
    if studio.render_cancel.borrow().is_some() {
        return;
    }
    let app = App::get();
    let (file_name, save_dir) = {
        let mut state = app.state_mut();
        let container = studio.model.borrow().state.export_settings.effective_container();
        let name = studio
            .model
            .borrow()
            .file_name(container.file_extension(), &mut state.prefs)
            .unwrap_or_else(|_| format!("Recording.{}", container.file_extension()));
        (name, AppPreferences { prefs: &state.prefs, dirs: &state.dirs }.save_directory())
    };
    let dialog = gtk::FileDialog::builder().title("Export Video").initial_name(&file_name).modal(true).build();
    dialog.set_initial_folder(Some(&gio::File::for_path(&save_dir)));
    let window = studio.widgets.borrow().as_ref().map(|w| w.window.clone());
    let studio = studio.clone();
    dialog.save(window.as_ref(), None::<&gio::Cancellable>, move |result| {
        let Ok(file) = result else { return };
        let Some(path) = file.path() else { return };
        let path = match path.extension().and_then(|e| e.to_str()).and_then(VideoExportContainer::from_extension) {
            Some(_) => path,
            None => {
                path.with_extension(studio.model.borrow().state.export_settings.effective_container().file_extension())
            }
        };
        let temporary = path.with_file_name(format!(
            ".{}.part.{}",
            uuid::Uuid::new_v4(),
            path.extension().and_then(|e| e.to_str()).unwrap_or("mp4")
        ));
        render(&studio, temporary, RenderTarget::ExportAs(path), None);
    });
}

fn render(studio: &Rc<Studio>, temporary: PathBuf, target: RenderTarget, stamp: Option<serde_json::Value>) {
    let request = studio.model.borrow().export_request(temporary.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    *studio.render_cancel.borrow_mut() = Some(cancel.clone());
    studio.set_rendering(true, "Rendering…");
    let (sender, receiver) = async_channel::unbounded::<f64>();
    let weak = Rc::downgrade(studio);
    glib::spawn_future_local(async move {
        while let Ok(fraction) = receiver.recv().await {
            if let Some(studio) = weak.upgrade() {
                if let Some(widgets) = studio.widgets.borrow().as_ref() {
                    widgets.progress.set_fraction(fraction);
                }
            }
        }
    });
    let flag = cancel.clone();
    let studio = studio.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            video::export(
                &request,
                &|fraction| {
                    let _ = sender.send_blocking(fraction);
                },
                &|| flag.load(Ordering::Relaxed),
            )
        })
        .await
        .unwrap_or_else(|_| Err(ExportError::Failed("The render stopped unexpectedly.".into())));
        studio.render_cancel.borrow_mut().take();
        studio.set_rendering(false, "");
        match result {
            Ok(()) => match target {
                RenderTarget::Save { then_close } => {
                    let session = studio.model.borrow().session.clone();
                    match session.install_final_video(&temporary, stamp.as_ref()) {
                        Ok(installed) => finish_save(&studio, installed, then_close),
                        Err(error) => {
                            let _ = std::fs::remove_file(&temporary);
                            toast(
                                &studio,
                                &format!("The project was saved, but its video could not be written. {error}"),
                            );
                        }
                    }
                }
                RenderTarget::ExportAs(path) => match std::fs::rename(&temporary, &path) {
                    Ok(()) => {
                        let session = studio.model.borrow().session.clone();
                        let _ = session
                            .update_project_metadata(|m| m.export_path = Some(path.to_string_lossy().into_owned()));
                        toast(
                            &studio,
                            &format!(
                                "Exported to {}",
                                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                            ),
                        );
                    }
                    Err(error) => {
                        let _ = std::fs::remove_file(&temporary);
                        toast(&studio, &format!("Couldn’t export. {error}"));
                    }
                },
            },
            Err(ExportError::Cancelled) => toast(&studio, "Render cancelled."),
            Err(error) => toast(&studio, &format!("Couldn’t render the video. {error}")),
        }
    });
}

/// Runs whisper.cpp on the recording's audio and installs the transcript.
fn transcribe_narration(studio: &Rc<Studio>) {
    if studio.render_cancel.borrow().is_some() {
        return;
    }
    let chosen = App::get().with_prefs(|p| p.whisper_model_path());
    let (command, model_path) = match bettershot_platform::transcription::availability(chosen.as_deref()) {
        Ok(found) => found,
        Err(reason) => return toast(studio, &reason),
    };
    let (movie, scratch) = {
        let model = studio.model.borrow();
        (model.session.screen_path(), model.session.directory.join(".transcription"))
    };
    toast(studio, "Transcribing…");
    if let Some(widgets) = studio.widgets.borrow().as_ref() {
        widgets.inspector.transcribe.set_sensitive(false);
    }
    let studio = studio.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            let result = bettershot_platform::transcription::transcribe(&movie, &command, &model_path, &scratch);
            let _ = std::fs::remove_dir_all(&scratch);
            result
        })
        .await
        .unwrap_or_else(|_| Err("Transcription stopped unexpectedly.".into()));
        match result {
            Ok(transcript) => {
                let count = transcript.cues.len();
                studio.model.borrow_mut().set_transcript(transcript.cues, transcript.words);
                studio.after_edit(true);
                toast(&studio, &format!("Transcribed {count} subtitles."));
            }
            Err(reason) => {
                studio.sync();
                toast(&studio, &format!("Couldn’t transcribe. {reason}"));
            }
        }
    });
}

/// Writes the deliverable to the save folder, replacing the saved file.
fn finish_save(studio: &Rc<Studio>, deliverable_path: PathBuf, then_close: bool) {
    let app = App::get();
    let session = studio.model.borrow().session.clone();
    let (file_name, save_dir) = {
        let mut state = app.state_mut();
        let name = studio.model.borrow().file_name(deliverable::default_extension(), &mut state.prefs);
        (name, AppPreferences { prefs: &state.prefs, dirs: &state.dirs }.save_directory())
    };
    let file_name = match file_name {
        Ok(name) => name,
        Err(error) => return toast(studio, &format!("Couldn’t name the video. {error}")),
    };
    let studio = studio.clone();
    glib::spawn_future_local(async move {
        let for_save = session.clone();
        let result = gio::spawn_blocking(move || {
            deliverable::save(&deliverable_path, &file_name, Some(&for_save), true, &save_dir, media::write_video)
        })
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("Saving stopped unexpectedly.")));
        match result {
            Ok(path) => {
                toast(
                    &studio,
                    &format!(
                        "Saved to {}",
                        path.parent()
                            .and_then(Path::file_name)
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                );
                crate::recording_card::refresh_saved(&session);
                if then_close {
                    studio.teardown();
                    if let Some(widgets) = studio.widgets.borrow().as_ref() {
                        widgets.window.destroy();
                    }
                }
            }
            Err(error) => {
                toast(&studio, &format!("The project was saved, but the video could not be written. {error}"))
            }
        }
    });
}
