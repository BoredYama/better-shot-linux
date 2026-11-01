//! "What's New in BetterShot", a port of `ReleaseNotesWindowController.swift`.
//! Notes come from the Linux port's own `linux/CHANGELOG.md`, compiled in. At
//! launch only unread notes for released versions show; closing marks them
//! read. Settings › About reopens the current release's notes, or before the
//! first release, the Unreleased changes in this build.

use crate::app::App;
use adw::prelude::*;
use bettershot_core::release_notes::{self, Block, ReleaseNotes};
use bettershot_core::version;
use gtk::glib;
use std::cell::RefCell;

thread_local! {
    static WINDOW: RefCell<Option<adw::Window>> = const { RefCell::new(None) };
}

/// Shows unread notes at launch; returns whether anything was shown.
pub fn present_pending(app: &App) -> bool {
    let current = &version::APP_VERSION.version;
    let releases = release_notes::parse(release_notes::LINUX_CHANGELOG);
    let notes = release_notes::pending(&releases, current, &app.state().prefs);
    if notes.is_empty() {
        return false;
    }
    show(app, notes, true, false);
    true
}

/// Settings › About › What's New: the current release's notes, or the
/// Unreleased changes when this version has no release entry yet.
pub fn present(app: &App) {
    let current = &version::APP_VERSION.version;
    let notes: Vec<ReleaseNotes> =
        release_notes::parse(release_notes::LINUX_CHANGELOG).into_iter().filter(|r| &r.version == current).collect();
    if notes.is_empty() {
        if let Some(body) = release_notes::unreleased(release_notes::LINUX_CHANGELOG) {
            show(app, vec![ReleaseNotes { version: "Unreleased".into(), body }], false, true);
            return;
        }
    }
    show(app, notes, false, false);
}

fn show(app: &App, notes: Vec<ReleaseNotes>, marks_seen: bool, unreleased: bool) {
    if let Some(window) = WINDOW.with(|w| w.borrow().clone()) {
        window.present();
        return;
    }
    let current = version::APP_VERSION.version.clone();
    let window = adw::Window::builder()
        .application(&app.application)
        .title("What’s New in BetterShot")
        .default_width(640)
        .default_height(660)
        .width_request(360)
        .height_request(360)
        .build();

    let icon = gtk::Image::from_icon_name(bettershot_core::paths::APP_ID);
    icon.set_pixel_size(48);
    icon.set_accessible_role(gtk::AccessibleRole::Presentation);
    let title = gtk::Label::builder().label("What’s new in BetterShot").xalign(0.0).css_classes(["title-2"]).build();
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    let subtitle =
        gtk::Label::builder().label(format!("Version {current}")).xalign(0.0).css_classes(["dim-label"]).build();
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 4);
    titles.append(&title);
    titles.append(&subtitle);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    top.set_margin_top(12);
    top.set_margin_bottom(18);
    top.set_margin_start(24);
    top.set_margin_end(24);
    top.append(&icon);
    top.append(&titles);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(18);
    body.set_margin_bottom(18);
    body.set_margin_start(24);
    body.set_margin_end(24);
    if unreleased {
        body.append(
            &gtk::Label::builder()
                .label("Changes in this build that aren’t in a release yet.")
                .xalign(0.0)
                .wrap(true)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
    }
    if notes.is_empty() {
        body.append(
            &gtk::Label::builder()
                .label("Release notes couldn’t be loaded. Read them on GitHub using the link below.")
                .xalign(0.0)
                .wrap(true)
                .css_classes(["dim-label"])
                .build(),
        );
    }
    let several = notes.len() > 1;
    for release in &notes {
        if several {
            let heading = gtk::Label::builder()
                .label(format!("Version {}", release.version))
                .xalign(0.0)
                .css_classes(["title-3"])
                .margin_top(6)
                .build();
            heading.set_accessible_role(gtk::AccessibleRole::Heading);
            body.append(&heading);
        }
        for block in release_notes::blocks(&release.body) {
            let label = match block {
                Block::Heading(text) => {
                    let label = gtk::Label::builder().label(text).xalign(0.0).css_classes(["heading"]).build();
                    label.set_accessible_role(gtk::AccessibleRole::Heading);
                    label
                }
                Block::Paragraph(markup) => gtk::Label::builder()
                    .label(markup)
                    .use_markup(true)
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .selectable(true)
                    // Selectable labels select everything when focused; the
                    // text stays selectable with the mouse.
                    .focusable(false)
                    .build(),
            };
            body.append(&label);
        }
    }
    let scroller =
        gtk::ScrolledWindow::builder().child(&body).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();

    let all = gtk::LinkButton::with_label(release_notes::ALL_NOTES_URL, "All Release Notes");
    let done = gtk::Button::builder().label("Done").css_classes(["suggested-action", "pill"]).build();
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bottom.set_margin_top(12);
    bottom.set_margin_bottom(12);
    bottom.set_margin_start(20);
    bottom.set_margin_end(20);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bottom.append(&all);
    bottom.append(&spacer);
    bottom.append(&done);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&top);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    content.append(&scroller);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    content.append(&bottom);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    view.set_content(Some(&content));
    window.set_content(Some(&view));
    window.set_default_widget(Some(&done));

    let window_ref = window.downgrade();
    done.connect_clicked(move |_| {
        if let Some(window) = window_ref.upgrade() {
            window.close();
        }
    });
    let keys = gtk::EventControllerKey::new();
    let window_ref = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            if let Some(window) = window_ref.upgrade() {
                window.close();
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    let presented = (!notes.is_empty() && marks_seen).then_some(current);
    window.connect_close_request(move |_| {
        if let Some(version) = &presented {
            let app = App::get();
            let result = release_notes::mark_seen(version, &mut app.state_mut().prefs);
            if let Err(error) = result {
                eprintln!("bettershot: release notes could not be marked read: {error}");
            }
        }
        WINDOW.with(|w| w.borrow_mut().take());
        glib::Propagation::Proceed
    });
    WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
    window.present();
    done.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_note_is_valid_pango_markup() {
        let unreleased = release_notes::unreleased(release_notes::LINUX_CHANGELOG)
            .map(|body| ReleaseNotes { version: "Unreleased".into(), body });
        let releases = release_notes::parse(release_notes::BUNDLED_CHANGELOG)
            .into_iter()
            .chain(release_notes::parse(release_notes::LINUX_CHANGELOG))
            .chain(unreleased);
        for release in releases {
            for block in release_notes::blocks(&release.body) {
                if let Block::Paragraph(markup) = block {
                    // `<a href>` is a GtkLabel extension; Pango checks the rest
                    // with links as spans.
                    let mut pango = markup.replace("</a>", "</span>");
                    while let Some(start) = pango.find("<a href=\"") {
                        let end = start + pango[start..].find("\">").expect("closed link tag") + 2;
                        pango.replace_range(start..end, "<span>");
                    }
                    gtk::pango::parse_markup(&pango, '\0')
                        .unwrap_or_else(|e| panic!("{}: {e}: {markup}", release.version));
                }
            }
        }
    }
}
