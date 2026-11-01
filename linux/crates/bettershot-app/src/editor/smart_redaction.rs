//! The Smart Redaction popover, a port of `AnnotationSmartRedactionControls`
//! in `AnnotationInspector.swift` and `AnnotationEditorModel.smartRedact`.
//! Tesseract finds sensitive text in the current base image, and each match
//! gets a Pixelate or Blur shape in one undoable edit.

use super::{window, Editor};
use crate::app::App;
use adw::prelude::*;
use bettershot_core::annotation::shape::Tool;
use bettershot_core::text_recognition;
use gtk::gio;
use std::cell::Cell;
use std::rc::Rc;

pub struct Controls {
    pub button: gtk::MenuButton,
    actions: Vec<gtk::Button>,
    spinner: gtk::Spinner,
    status: gtk::Label,
    running: Cell<bool>,
}

impl Controls {
    pub fn is_running(&self) -> bool {
        self.running.get()
    }
}

pub fn build(editor: &Rc<Editor>) -> Controls {
    let button = gtk::MenuButton::builder().icon_name("view-conceal-symbolic").css_classes(["flat"]).build();
    button.set_tooltip_text(Some("Smart Redaction"));
    button.update_property(&[gtk::accessible::Property::Label("Smart Redaction")]);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(6);
    content.set_margin_end(6);
    let intro = gtk::Label::builder()
        .label("Automatically hide sensitive information")
        .css_classes(["dim-label"])
        .xalign(0.0)
        .build();
    content.append(&intro);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_homogeneous(true);
    let mut actions = Vec::new();
    for (tool, title, icon) in
        [(Tool::Pixelate, "Pixelate", "bettershot-pixelate-symbolic"), (Tool::Blur, "Blur", "bettershot-blur-symbolic")]
    {
        let label = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        label.set_halign(gtk::Align::Center);
        label.append(&gtk::Image::from_icon_name(icon));
        label.append(&gtk::Label::new(Some(title)));
        let action = gtk::Button::builder().child(&label).build();
        action.update_property(&[gtk::accessible::Property::Label(&format!("{title} sensitive text"))]);
        let weak = Rc::downgrade(editor);
        action.connect_clicked(move |_| {
            if let Some(editor) = weak.upgrade() {
                run(&editor, tool);
            }
        });
        row.append(&action);
        actions.push(action);
    }
    content.append(&row);
    let progress = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let spinner = gtk::Spinner::new();
    let status =
        gtk::Label::builder().xalign(0.0).css_classes(["dim-label", "caption"]).wrap(true).focusable(true).build();
    status.set_accessible_role(gtk::AccessibleRole::Status);
    status.set_visible(false);
    spinner.set_visible(false);
    progress.append(&spinner);
    progress.append(&status);
    content.append(&progress);
    let popover = gtk::Popover::builder().child(&content).build();
    button.set_popover(Some(&popover));

    Controls { button, actions, spinner, status, running: Cell::new(false) }
}

fn set_running(editor: &Editor, running: bool, message: Option<&str>) {
    let widgets = editor.widgets.borrow();
    let Some(controls) = widgets.as_ref().map(|w| &w.smart_redaction) else { return };
    controls.running.set(running);
    controls.spinner.set_visible(running);
    controls.spinner.set_spinning(running);
    let text = if running { Some("Scanning screenshot…") } else { message };
    controls.status.set_visible(text.is_some());
    controls.status.set_label(text.unwrap_or_default());
    // Disabling the focused button would send focus out of the popover to the
    // toolbar; keep it on the status, which reads the progress and result.
    if running && controls.actions.iter().any(|a| a.has_focus()) {
        controls.status.grab_focus();
    }
    for action in &controls.actions {
        action.set_sensitive(!running);
    }
}

fn run(editor: &Rc<Editor>, tool: Tool) {
    let running = editor.widgets.borrow().as_ref().is_some_and(|w| w.smart_redaction.is_running());
    if running {
        return;
    }
    super::canvas::finish_text(editor);
    let (image, generation) = {
        let model = editor.model.borrow();
        ((*model.current.base).clone(), model.current.generation)
    };
    set_running(editor, true, None);
    window::refresh(editor);
    let scratch = App::get().state().dirs.scratch_folder("OCR");
    let weak = Rc::downgrade(editor);
    gtk::glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(move || {
            let lines = text_recognition::recognize_lines(&image, &scratch)?;
            Ok::<_, text_recognition::RecognitionError>(text_recognition::sensitive_regions(
                &lines,
                image.width(),
                image.height(),
            ))
        })
        .await;
        let Some(editor) = weak.upgrade() else { return };
        // The base changed (crop, turn, or undo of one) while scanning.
        if editor.model.borrow().current.generation != generation {
            set_running(&editor, false, None);
            window::refresh(&editor);
            return;
        }
        let message = match found {
            Ok(Ok(regions)) => {
                let bounds: Vec<_> = regions.iter().map(|r| r.bounds).collect();
                let added = editor.model.borrow_mut().add_smart_redactions(&bounds, tool);
                match added {
                    0 => "No sensitive text found.".to_string(),
                    1 => "Added 1 redaction.".to_string(),
                    n => format!("Added {n} redactions."),
                }
            }
            Ok(Err(error)) => error.to_string(),
            Err(_) => "Scanning stopped unexpectedly. Try again.".to_string(),
        };
        set_running(&editor, false, Some(&message));
        window::refresh(&editor);
    });
}
