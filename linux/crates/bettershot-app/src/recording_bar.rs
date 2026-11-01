//! The shared capture bar, a port of `RecordingPickerBar.swift`,
//! `RecordingBarChrome.swift`, and `RecordingControlPresenter.swift`: one
//! floating window whose picker row (64 px) offers the screenshot modes, the
//! recording sources, and the input toggles, and which morphs into the
//! recording row (38 px) with Stop/timer, Pause, Restart, and Discard while a
//! recording runs. Wayland cannot keep the bar out of a display recording,
//! so move it off the recorded area or record a window.

use crate::app::App;
use crate::{capture, recording};
use adw::prelude::*;
use bettershot_core::app_prefs::{keys, SELF_TIMER_CHOICES};
use bettershot_core::recording::{self as core_recording, RecordingState, SourceMode};
use bettershot_core::shortcuts::Action;
use bettershot_platform::devices;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;

struct Bar {
    window: gtk::Window,
    stack: gtk::Stack,
    stop_title: gtk::Label,
    timer: gtk::Label,
    paused: gtk::Label,
    stop: gtk::Button,
    pause: gtk::Button,
    pause_icon: gtk::Image,
    restart: gtk::Button,
    discard: gtk::Button,
    camera: gtk::ToggleButton,
    microphone: gtk::ToggleButton,
    audio: gtk::ToggleButton,
    script: gtk::ToggleButton,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Opens the bar, or brings it forward. `Action::RecordingOptions` and
/// `Action::Recording` both reach the same bar on Linux.
pub fn present(app: &App, _action: Action) {
    BAR.with(|bar| {
        let mut bar = bar.borrow_mut();
        let bar = bar.get_or_insert_with(|| build(app));
        bar.window.present();
    });
    refresh();
}

pub fn window() -> Option<gtk::Window> {
    BAR.with(|bar| bar.borrow().as_ref().map(|b| b.window.clone()))
}

/// Updates the bar from the recording state and clock.
pub fn refresh() {
    BAR.with(|bar| {
        let bar = bar.borrow();
        let Some(bar) = bar.as_ref() else { return };
        let state = recording::state();
        if state.is_active() {
            bar.stack.set_visible_child_name("recording");
            if !bar.window.is_visible() {
                bar.window.present();
            }
        } else {
            bar.stack.set_visible_child_name("picker");
        }
        let elapsed = core_recording::formatted_elapsed(recording::elapsed());
        bar.stop_title.set_label(state.stop_title());
        bar.timer.set_label(&elapsed);
        bar.paused.set_visible(state == RecordingState::Paused);
        let settling = state.is_settling();
        bar.stop.set_sensitive(!settling);
        bar.pause.set_sensitive(!settling);
        bar.restart.set_sensitive(!settling);
        bar.discard.set_sensitive(!settling);
        let resume = state == RecordingState::Paused;
        bar.pause_icon.set_icon_name(Some(if resume {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
        }));
        let pause_title = if resume { "Resume recording" } else { "Pause recording" };
        bar.pause.set_tooltip_text(Some(pause_title));
        bar.pause.update_property(&[gtk::accessible::Property::Label(pause_title)]);
        bar.stop.update_property(&[gtk::accessible::Property::Label(&format!(
            "Stop and save recording, {elapsed} elapsed"
        ))]);
        let app = App::get();
        let (camera, microphone, audio, script) = app.with_prefs(|p| {
            (
                p.recording_camera_device_id().is_some(),
                p.recording_microphone_device_id().is_some(),
                p.recording_system_audio(),
                p.teleprompter_enabled(),
            )
        });
        for (button, on, label) in [
            (&bar.camera, camera, if camera { "Camera on" } else { "Camera off" }),
            (&bar.microphone, microphone, if microphone { "Microphone on" } else { "Microphone off" }),
            (&bar.audio, audio, if audio { "System audio on" } else { "System audio off" }),
            (
                &bar.script,
                script,
                if script {
                    "Teleprompter on: click to edit the script"
                } else {
                    "Teleprompter off: click to write a script"
                },
            ),
        ] {
            if button.is_active() != on {
                button.set_active(on);
            }
            button.set_tooltip_text(Some(label));
            button.update_property(&[gtk::accessible::Property::Label(label)]);
        }
    });
}

/// Brings the bar above windows restored after a selection.
pub fn raise() {
    if let Some(bar) = window() {
        if recording::is_active() {
            bar.set_visible(false);
            bar.present();
        }
    }
}

/// After a recording finishes, the bar goes away as on macOS.
pub fn hide_recording() {
    BAR.with(|bar| {
        if let Some(bar) = bar.borrow().as_ref() {
            bar.stack.set_visible_child_name("picker");
            bar.window.set_visible(false);
        }
    });
}

fn item(icon: &str, caption: &str, title: &str) -> gtk::Button {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.set_valign(gtk::Align::Center);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(18);
    let label = gtk::Label::new(Some(caption));
    label.add_css_class("bar-caption");
    column.append(&image);
    column.append(&label);
    let button = gtk::Button::builder().child(&column).css_classes(["flat", "bar-item"]).tooltip_text(title).build();
    button.update_property(&[gtk::accessible::Property::Label(title)]);
    button
}

fn toggle(icon_on: &'static str, icon_off: &'static str, caption: &str) -> (gtk::ToggleButton, gtk::Image) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.set_valign(gtk::Align::Center);
    let image = gtk::Image::from_icon_name(icon_off);
    image.set_pixel_size(18);
    let label = gtk::Label::new(Some(caption));
    label.add_css_class("bar-caption");
    column.append(&image);
    column.append(&label);
    let button = gtk::ToggleButton::builder().child(&column).css_classes(["flat", "bar-item"]).build();
    let icon = image.clone();
    button.connect_toggled(move |button| {
        icon.set_icon_name(Some(if button.is_active() { icon_on } else { icon_off }));
    });
    (button, image)
}

fn separator(height: i32) -> gtk::Separator {
    let separator = gtk::Separator::new(gtk::Orientation::Vertical);
    separator.set_size_request(1, height);
    separator.set_valign(gtk::Align::Center);
    separator
}

fn build(app: &App) -> Bar {
    let picker = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    picker.add_css_class("picker-row");
    let capture_items: [(&str, &str, &str, Action); 6] = [
        ("bettershot-region-symbolic", "Region", "Capture an area", Action::Region),
        ("video-display-symbolic", "Screen", "Capture every display", Action::Fullscreen),
        ("window-new-symbolic", "Window", "Capture a window", Action::Window),
        ("view-continuous-symbolic", "Scroll", "Scrolling capture", Action::ScrollCapture),
        ("insert-text-symbolic", "Text", "Capture text", Action::Ocr),
        ("color-select-symbolic", "Color", "Pick a color", Action::ColorPicker),
    ];
    for (icon, caption, title, action) in capture_items {
        let button = item(icon, caption, title);
        button.connect_clicked(move |_| capture::perform(action));
        picker.append(&button);
    }
    picker.append(&separator(40));

    let record_items: [(&str, &str, &str, SourceMode); 3] = [
        ("video-display-symbolic", "Display", "Record the whole screen", SourceMode::Fullscreen),
        ("window-new-symbolic", "Window", "Record an app window", SourceMode::Window),
        ("bettershot-region-symbolic", "Area", "Drag to select an area to record", SourceMode::Area),
    ];
    for (icon, caption, title, mode) in record_items {
        let button = item(icon, caption, title);
        button.add_css_class("record-item");
        button.connect_clicked(move |_| recording::start(mode));
        picker.append(&button);
    }
    picker.append(&separator(40));

    let (camera, _) = toggle("camera-web-symbolic", "camera-disabled-symbolic", "Camera");
    camera.connect_toggled(|button| {
        let app = App::get();
        let on = app.with_prefs(|p| p.recording_camera_device_id().is_some());
        if button.is_active() == on {
            return;
        }
        if button.is_active() {
            match devices::cameras().into_iter().next() {
                Some(device) => app.set_pref(keys::RECORDING_CAMERA_DEVICE_ID, device.id),
                None => {
                    app.notify("No camera found", "Connect a camera, then turn it on here or in Settings › Recording.");
                    button.set_active(false);
                }
            }
        } else {
            app.set_pref(keys::RECORDING_CAMERA_DEVICE_ID, "");
        }
    });
    let (microphone, _) = toggle("audio-input-microphone-symbolic", "microphone-disabled-symbolic", "Mic");
    microphone.connect_toggled(|button| {
        let app = App::get();
        let on = app.with_prefs(|p| p.recording_microphone_device_id().is_some());
        if button.is_active() == on {
            return;
        }
        let value = if button.is_active() { "default" } else { "" };
        app.set_pref(keys::RECORDING_MICROPHONE_DEVICE_ID, value);
    });
    let (audio, _) = toggle("audio-volume-high-symbolic", "audio-volume-muted-symbolic", "Audio");
    audio.connect_toggled(|button| {
        let app = App::get();
        if app.with_prefs(|p| p.recording_system_audio()) != button.is_active() {
            app.set_pref(keys::RECORDING_SYSTEM_AUDIO, button.is_active());
        }
    });
    // The teleprompter: the toggle shows whether it is on; a click opens
    // the script window (port of the bar's Script item).
    let (script, _) = toggle("accessories-text-editor-symbolic", "accessories-text-editor-symbolic", "Script");
    script.connect_clicked(|button| {
        let app = App::get();
        button.set_active(app.with_prefs(|p| p.teleprompter_enabled()));
        crate::teleprompter::toggle_composer(&app);
    });
    picker.append(&camera);
    picker.append(&microphone);
    picker.append(&audio);
    picker.append(&script);

    // The countdown menu, shared by screenshots and recordings.
    let timer_menu = gio::Menu::new();
    for seconds in SELF_TIMER_CHOICES {
        let label = if seconds == 0 { "Off".to_string() } else { format!("{seconds} seconds") };
        let entry = gio::MenuItem::new(Some(&label), None);
        entry.set_action_and_target_value(Some("bar.timer"), Some(&seconds.to_variant()));
        timer_menu.append_item(&entry);
    }
    let current = app.with_prefs(|p| p.self_timer_delay());
    let timer_action = gio::SimpleAction::new_stateful("timer", Some(glib::VariantTy::INT64), &current.to_variant());
    timer_action.connect_activate(|action, value| {
        let Some(seconds) = value.and_then(|v| v.get::<i64>()) else { return };
        action.set_state(&seconds.to_variant());
        let app = App::get();
        app.set_pref(keys::SELF_TIMER, seconds);
        app.set_pref(keys::RECORDING_START_DELAY, seconds);
    });
    let actions = gio::SimpleActionGroup::new();
    actions.add_action(&timer_action);
    let timer_column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    timer_column.set_valign(gtk::Align::Center);
    let timer_icon = gtk::Image::from_icon_name("alarm-symbolic");
    timer_icon.set_pixel_size(18);
    let timer_caption = gtk::Label::new(Some("Timer"));
    timer_caption.add_css_class("bar-caption");
    timer_column.append(&timer_icon);
    timer_column.append(&timer_caption);
    let timer_button = gtk::MenuButton::builder()
        .child(&timer_column)
        .menu_model(&timer_menu)
        .css_classes(["flat", "bar-item"])
        .tooltip_text("Countdown before capturing or recording")
        .build();
    timer_button.update_property(&[gtk::accessible::Property::Label("Timer")]);
    picker.append(&timer_button);
    picker.append(&separator(40));
    let close = item("window-close-symbolic", "Close", "Close the capture bar");
    close.connect_clicked(|_| {
        if let Some(bar) = window() {
            bar.set_visible(false);
        }
    });
    picker.append(&close);

    // The recording row.
    let recording_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    recording_row.add_css_class("recording-row");
    let stop_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let stop_icon = gtk::Image::from_icon_name("media-playback-stop-symbolic");
    let stop_title = gtk::Label::new(Some("Stop"));
    stop_title.add_css_class("heading");
    let timer = gtk::Label::new(Some("00:00"));
    timer.add_css_class("timer");
    timer.set_width_chars(5);
    timer.set_xalign(0.0);
    let paused = gtk::Label::new(Some("Paused"));
    paused.add_css_class("caption");
    paused.set_visible(false);
    stop_content.append(&stop_icon);
    stop_content.append(&stop_title);
    stop_content.append(&timer);
    stop_content.append(&paused);
    let stop = gtk::Button::builder()
        .child(&stop_content)
        .css_classes(["flat", "record-tint"])
        .tooltip_text("Stop and save the recording")
        .build();
    stop.connect_clicked(|_| recording::stop());
    recording_row.append(&stop);
    recording_row.append(&separator(38));
    let pause_icon = gtk::Image::from_icon_name("media-playback-pause-symbolic");
    let pause = gtk::Button::builder().child(&pause_icon).css_classes(["flat"]).width_request(42).build();
    pause.connect_clicked(|_| recording::toggle_pause());
    recording_row.append(&pause);
    recording_row.append(&separator(38));
    let restart = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .css_classes(["flat"])
        .width_request(42)
        .tooltip_text("Start over")
        .build();
    restart.update_property(&[gtk::accessible::Property::Label("Start over")]);
    restart.connect_clicked(|_| recording::confirm(Action::RestartRecording));
    recording_row.append(&restart);
    recording_row.append(&separator(38));
    let discard = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .css_classes(["flat"])
        .width_request(42)
        .tooltip_text("Discard recording")
        .build();
    discard.update_property(&[gtk::accessible::Property::Label("Discard recording")]);
    discard.connect_clicked(|_| recording::confirm(Action::DiscardRecording));
    recording_row.append(&discard);

    let stack = gtk::Stack::new();
    stack.set_hhomogeneous(false);
    stack.set_vhomogeneous(false);
    stack.add_named(&picker, Some("picker"));
    stack.add_named(&recording_row, Some("recording"));
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&stack));
    let window = gtk::Window::builder()
        .title("Capture Bar")
        .application(&app.application)
        .decorated(false)
        .resizable(false)
        .child(&handle)
        .build();
    window.add_css_class("capture-bar");
    window.insert_action_group("bar", Some(&actions));
    window.update_property(&[gtk::accessible::Property::Label("Capture and recording bar")]);
    let keys_controller = gtk::EventControllerKey::new();
    keys_controller.connect_key_pressed(|_, key, _, _| {
        if key == gdk::Key::Escape && !recording::is_active() {
            if let Some(bar) = self::window() {
                bar.set_visible(false);
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys_controller);
    window.connect_close_request(|window| {
        // Closing never stops a recording; the bar just hides.
        window.set_visible(false);
        glib::Propagation::Stop
    });
    Bar {
        window,
        stack,
        stop_title,
        timer,
        paused,
        stop,
        pause,
        pause_icon,
        restart,
        discard,
        camera,
        microphone,
        audio,
        script,
    }
}
