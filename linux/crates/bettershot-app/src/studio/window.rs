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
use bettershot_core::recording::edit::{
    CameraAspectRatio, CompressionQuality, CompressionResolution, ExportFrameRate, LayoutPreset, MaskEffect,
    MaskSegment, StudioStyle, UnitRect,
};
use bettershot_core::recording::viewport::{UnitPoint, ZoomAnchorMode};
use bettershot_core::recording::{self, deliverable, ClipSegment, RecordingSession, VideoExportContainer, ZoomCue};
use bettershot_platform::media;
use bettershot_platform::video::{self, ExportError};
use gtk::{cairo, gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TIMELINE_HEIGHT: i32 = 96;

/// A slider and the mask field it edits.
type MaskField<'a> = (&'a Slider, fn(&mut MaskSegment, f64));
/// A slider and the zoom cue field it edits.
type CueField<'a> = (&'a Slider, fn(&mut ZoomCue, f64));
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
}

struct Inspector {
    root: gtk::Box,
    tiles: Vec<(BgStyle, gtk::Button)>,
    padding: Slider,
    corner: Slider,
    shadow: Slider,
    camera_group: gtk::Box,
    camera_note: gtk::Label,
    camera_visible: gtk::Switch,
    layout: gtk::DropDown,
    camera_left: gtk::Switch,
    camera_size: Slider,
    camera_roundness: gtk::Scale,
    camera_aspect: gtk::DropDown,
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
    click.connect_pressed(move |_, _, x, _| {
        if let Some(studio) = weak.upgrade() {
            let width = studio.widgets.borrow().as_ref().map(|w| f64::from(w.timeline.width())).unwrap_or(1.0);
            studio.timeline_pressed(x, width);
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

    // Cursor
    root.append(&heading("Cursor"));
    let cursor_note = gtk::Label::builder()
        .label("The cursor was recorded into the picture, so it cannot be restyled or hidden here. Cursor styles come with a separate pointer timeline.")
        .wrap(true)
        .max_width_chars(34)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    root.append(&cursor_note);

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

    // Export settings
    root.append(&heading("Export"));
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
        camera_group,
        camera_note,
        camera_visible,
        layout,
        camera_left,
        camera_size,
        camera_roundness,
        camera_aspect,
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
    cr.translate(x, y);
    cr.scale(scale, scale);
    let playhead = model.playhead;
    let source_time = model.source_time(playhead);
    let camera_time = source_time - model.camera_lead_in;
    let StudioModel { screen, camera, compositor, .. } = &mut *model;
    let Some(screen_frame) = screen.frame_at(source_time) else { return };
    let camera_frame = match camera.as_mut() {
        Some(camera) if camera_time >= 0.0 => camera.frame_at(camera_time),
        _ => None,
    };
    compositor.compose(cr, screen_frame, camera_frame, playhead);
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

    // Zoom lane.
    let lane_top = 8.0;
    let lane_h = 18.0;
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
        self.redraw();
    }

    fn timeline_pressed(&self, x: f64, width: f64) {
        let (left, span) = timeline_metrics(width);
        let duration = self.model.borrow().duration();
        let time = ((x - left) / span).clamp(0.0, 1.0) * duration;
        let scissors = self.model.borrow().scissors;
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
