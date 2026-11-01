//! The Media Gallery, a port of `Sources/Settings/MediaGallery.swift` and
//! `MediaGalleryActions.swift`: captures on this computer and cloud shares,
//! by kind, with search, sorting, icon and list views, and per-item actions.
//! Local and cloud are independent: a shared capture with a local file is in
//! both lists. Deleting asks first, and a failure stays on the item to retry.

use crate::app::App;
use crate::{clipboard, deck_ui, sharing};
use adw::prelude::*;
use bettershot_core::capture_record::CaptureKind;
use bettershot_core::gallery::{self, GalleryItem, SortOrder};
use bettershot_core::{imaging, share};
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Category {
    cloud: bool,
    kind: Option<CaptureKind>,
}

impl Category {
    const ALL: [Category; 6] = [
        Category { cloud: false, kind: None },
        Category { cloud: false, kind: Some(CaptureKind::Screenshot) },
        Category { cloud: false, kind: Some(CaptureKind::Recording) },
        Category { cloud: true, kind: None },
        Category { cloud: true, kind: Some(CaptureKind::Screenshot) },
        Category { cloud: true, kind: Some(CaptureKind::Recording) },
    ];

    fn title(self) -> &'static str {
        match self.kind {
            None => "All Media",
            Some(CaptureKind::Screenshot) => "Screenshots",
            Some(_) => "Videos",
        }
    }

    fn icon(self) -> &'static str {
        match (self.cloud, self.kind) {
            (false, None) => "folder-symbolic",
            (true, None) => "bettershot-share-symbolic",
            (_, Some(CaptureKind::Screenshot)) => "image-x-generic-symbolic",
            (_, Some(_)) => "video-x-generic-symbolic",
        }
    }

    fn location(self) -> &'static str {
        if self.cloud {
            "Cloud Shares"
        } else {
            "On this Computer"
        }
    }
}

struct Gallery {
    window: adw::ApplicationWindow,
    category: Cell<Category>,
    sort: Cell<SortOrder>,
    list_view: Cell<bool>,
    search: gtk::SearchEntry,
    title: adw::WindowTitle,
    count: gtk::Label,
    content: gtk::Stack,
    grid: gtk::FlowBox,
    list: gtk::ListBox,
    empty: adw::StatusPage,
    message: adw::Banner,
    items: RefCell<Vec<GalleryItem>>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Gallery>>> = const { RefCell::new(None) };
}

/// Shows the gallery, reusing the open window.
pub fn present(app: &App) {
    if let Some(gallery) = OPEN.with(|open| open.borrow().clone()) {
        refresh(&gallery);
        gallery.window.present();
        return;
    }
    let gallery = build(app);
    OPEN.with(|open| *open.borrow_mut() = Some(gallery.clone()));
    refresh(&gallery);
    gallery.window.present();
}

/// Reloads an open gallery, for example after a share or a save.
pub fn reload() {
    if let Some(gallery) = OPEN.with(|open| open.borrow().clone()) {
        refresh(&gallery);
    }
}

fn build(app: &App) -> Rc<Gallery> {
    let window = adw::ApplicationWindow::builder()
        .application(&app.application)
        .title("Media Gallery")
        .default_width(1000)
        .default_height(660)
        .width_request(360)
        .height_request(320)
        .build();

    // Sidebar: two sections of three categories.
    let sidebar_list = gtk::ListBox::new();
    sidebar_list.add_css_class("navigation-sidebar");
    let mut first_cloud_row = None;
    for category in Category::ALL {
        let row = gtk::ListBoxRow::new();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.set_margin_start(6);
        line.append(&gtk::Image::from_icon_name(category.icon()));
        line.append(&gtk::Label::new(Some(category.title())));
        row.set_child(Some(&line));
        row.update_property(&[gtk::accessible::Property::Label(&format!(
            "{} {}",
            if category.cloud { "Cloud" } else { "Local" },
            category.title()
        ))]);
        if category.cloud && first_cloud_row.is_none() {
            first_cloud_row = Some(row.clone());
        }
        sidebar_list.append(&row);
    }
    let cloud_row = first_cloud_row.expect("a cloud category exists");
    sidebar_list.set_header_func(move |row, _| {
        let header = match row.index() {
            0 => Some("On this Computer"),
            i if i == cloud_row.index() => Some("Cloud Shares"),
            _ => None,
        };
        row.set_header(
            header
                .map(|text| {
                    gtk::Label::builder()
                        .label(text)
                        .xalign(0.0)
                        .css_classes(["heading", "dim-label"])
                        .margin_top(12)
                        .margin_bottom(6)
                        .margin_start(12)
                        .build()
                })
                .as_ref(),
        );
    });
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&adw::HeaderBar::new());
    let sidebar_scroll = gtk::ScrolledWindow::builder().child(&sidebar_list).vexpand(true).build();
    sidebar_view.set_content(Some(&sidebar_scroll));
    let sidebar_page = adw::NavigationPage::builder().title("Media Gallery").child(&sidebar_view).build();

    // Content: header with view, sort, refresh, and search.
    let title = adw::WindowTitle::new("All Media", "On this Computer");
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    let search = gtk::SearchEntry::builder().placeholder_text("Search media").max_width_chars(18).build();
    search.update_property(&[gtk::accessible::Property::Label("Search media")]);
    let refresh_button = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_button.set_tooltip_text(Some("Refresh media"));
    refresh_button.update_property(&[gtk::accessible::Property::Label("Refresh media")]);
    let view_toggle = gtk::ToggleButton::builder().icon_name("view-list-symbolic").build();
    view_toggle.set_tooltip_text(Some("Show as a list"));
    view_toggle.update_property(&[gtk::accessible::Property::Label("Show as a list")]);
    let sort_menu = gio::Menu::new();
    for (index, order) in SortOrder::ALL.iter().enumerate() {
        sort_menu.append(Some(order.title()), Some(&format!("gallery.sort({index})")));
    }
    let sort_button =
        gtk::MenuButton::builder().icon_name("view-sort-descending-symbolic").menu_model(&sort_menu).build();
    sort_button.set_tooltip_text(Some("Sort media"));
    sort_button.update_property(&[gtk::accessible::Property::Label("Sort media")]);
    header.pack_start(&view_toggle);
    header.pack_start(&sort_button);
    header.pack_end(&search);
    header.pack_end(&refresh_button);

    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .homogeneous(false)
        .min_children_per_line(1)
        .max_children_per_line(8)
        .column_spacing(12)
        .row_spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .valign(gtk::Align::Start)
        .build();
    grid.update_property(&[gtk::accessible::Property::Label("Media")]);
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["boxed-list"]).build();
    list.set_margin_top(12);
    list.set_margin_bottom(12);
    list.set_margin_start(12);
    list.set_margin_end(12);
    list.set_valign(gtk::Align::Start);
    list.update_property(&[gtk::accessible::Property::Label("Media")]);
    let empty = adw::StatusPage::builder().icon_name("image-x-generic-symbolic").build();
    let content = gtk::Stack::new();
    content.add_named(
        &gtk::ScrolledWindow::builder().child(&grid).hscrollbar_policy(gtk::PolicyType::Never).build(),
        Some("grid"),
    );
    content.add_named(
        &gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).build(),
        Some("list"),
    );
    content.add_named(&empty, Some("empty"));
    content.set_vexpand(true);

    let message = adw::Banner::builder().button_label("Dismiss").build();
    message.connect_button_clicked(|banner| banner.set_revealed(false));
    let count = gtk::Label::builder().css_classes(["dim-label", "numeric", "caption"]).halign(gtk::Align::End).build();
    count.set_margin_end(12);
    count.set_margin_bottom(6);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&message);
    body.append(&content);
    body.append(&count);
    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&header);
    content_view.set_content(Some(&body));
    let content_page = adw::NavigationPage::builder().title("All Media").child(&content_view).build();

    let split = adw::NavigationSplitView::builder().sidebar(&sidebar_page).content(&content_page).build();
    split.set_min_sidebar_width(180.0);
    // Narrow windows collapse the sidebar into a back button.
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 600sp").expect("valid"));
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(breakpoint);
    window.set_content(Some(&split));

    let gallery = Rc::new(Gallery {
        window: window.clone(),
        category: Cell::new(Category::ALL[0]),
        sort: Cell::new(SortOrder::NewestFirst),
        list_view: Cell::new(false),
        search,
        title,
        count,
        content,
        grid,
        list,
        empty,
        message,
        items: RefCell::new(Vec::new()),
    });

    let actions = gio::SimpleActionGroup::new();
    let sort = gio::SimpleAction::new("sort", Some(glib::VariantTy::INT32));
    let weak = Rc::downgrade(&gallery);
    sort.connect_activate(move |_, value| {
        let (Some(gallery), Some(index)) = (weak.upgrade(), value.and_then(|v| v.get::<i32>())) else { return };
        gallery.sort.set(SortOrder::ALL[index.clamp(0, 3) as usize]);
        refresh(&gallery);
    });
    actions.add_action(&sort);
    window.insert_action_group("gallery", Some(&actions));

    sidebar_list.select_row(sidebar_list.row_at_index(0).as_ref());
    let weak = Rc::downgrade(&gallery);
    let split_ref = split.downgrade();
    sidebar_list.connect_row_selected(move |_, row| {
        let (Some(gallery), Some(row)) = (weak.upgrade(), row) else { return };
        gallery.category.set(Category::ALL[row.index().clamp(0, 5) as usize]);
        if let Some(split) = split_ref.upgrade() {
            split.set_show_content(true);
        }
        refresh(&gallery);
    });
    let weak = Rc::downgrade(&gallery);
    view_toggle.connect_toggled(move |toggle| {
        if let Some(gallery) = weak.upgrade() {
            gallery.list_view.set(toggle.is_active());
            refresh(&gallery);
        }
    });
    let weak = Rc::downgrade(&gallery);
    refresh_button.connect_clicked(move |_| {
        if let Some(gallery) = weak.upgrade() {
            refresh(&gallery);
        }
    });
    let weak = Rc::downgrade(&gallery);
    gallery.search.connect_search_changed(move |_| {
        if let Some(gallery) = weak.upgrade() {
            refresh(&gallery);
        }
    });
    // Double-click or Enter opens.
    let weak = Rc::downgrade(&gallery);
    gallery.grid.connect_child_activated(move |_, child| {
        if let Some(gallery) = weak.upgrade() {
            if let Some(item) = gallery.items.borrow().get(child.index() as usize).cloned() {
                open_item(&gallery, &item);
            }
        }
    });
    let weak = Rc::downgrade(&gallery);
    gallery.list.connect_row_activated(move |_, row| {
        if let Some(gallery) = weak.upgrade() {
            if let Some(item) = gallery.items.borrow().get(row.index() as usize).cloned() {
                open_item(&gallery, &item);
            }
        }
    });
    gallery.grid.set_activate_on_single_click(false);
    gallery.list.set_activate_on_single_click(false);

    window.connect_close_request(|_| {
        OPEN.with(|open| open.borrow_mut().take());
        glib::Propagation::Proceed
    });
    gallery
}

fn refresh(gallery: &Rc<Gallery>) {
    let category = gallery.category.get();
    let all = gallery::collect(&App::get().state().history);
    let mut items = gallery::filtered(&all, category.kind, category.cloud, &gallery.search.text());
    gallery.sort.get().sort(&mut items);
    gallery.title.set_title(category.title());
    gallery.title.set_subtitle(category.location());
    gallery.count.set_label(&format!("{} {}", items.len(), if items.len() == 1 { "item" } else { "items" }));

    while let Some(child) = gallery.grid.first_child() {
        gallery.grid.remove(&child);
    }
    while let Some(child) = gallery.list.first_child() {
        gallery.list.remove(&child);
    }
    for item in &items {
        if gallery.list_view.get() {
            gallery.list.append(&list_row(gallery, item, category.cloud));
        } else {
            gallery.grid.append(&card(gallery, item, category.cloud));
        }
    }
    *gallery.items.borrow_mut() = items;
    if gallery.items.borrow().is_empty() {
        let searching = !gallery.search.text().trim().is_empty();
        let (title, description) = match (searching, category.cloud) {
            (true, _) => ("No matching media", "Try another name or media type."),
            (false, true) => ("No cloud shares yet", "Share a screenshot from its card or the editor to see it here."),
            (false, false) => ("No saved media yet", "Save or edit a screenshot, then refresh."),
        };
        gallery.empty.set_title(title);
        gallery.empty.set_description(Some(description));
        gallery.content.set_visible_child_name("empty");
    } else {
        gallery.content.set_visible_child_name(if gallery.list_view.get() { "list" } else { "grid" });
    }
}

/// Loads a small thumbnail off the main thread.
fn load_thumbnail(picture: &gtk::Picture, item: &GalleryItem) {
    let path = if item.preview.exists() { item.preview.clone() } else { item.capture.clone() };
    let picture = picture.downgrade();
    glib::spawn_future_local(async move {
        // Cropped to 16:10 around the center, so every card has one shape.
        let thumbnail = gio::spawn_blocking(move || {
            imaging::load_rgba(&path).ok().map(|image| {
                let (w, h) = image.dimensions();
                let (cw, ch) = if f64::from(w) / f64::from(h.max(1)) > 1.6 {
                    (((f64::from(h) * 1.6).round() as u32).clamp(1, w), h)
                } else {
                    (w, ((f64::from(w) / 1.6).round() as u32).clamp(1, h))
                };
                let cropped = image::imageops::crop_imm(&image, (w - cw) / 2, (h - ch) / 2, cw, ch).to_image();
                imaging::thumbnail(&cropped, 440)
            })
        })
        .await
        .ok()
        .flatten();
        let (Some(picture), Some(thumbnail)) = (picture.upgrade(), thumbnail) else { return };
        let (w, h) = thumbnail.dimensions();
        let bytes = glib::Bytes::from_owned(thumbnail.into_raw());
        let texture = gdk::MemoryTexture::new(w as i32, h as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, w as usize * 4);
        picture.set_paintable(Some(&texture));
    });
}

fn subtitle(item: &GalleryItem) -> String {
    let date = item.created_at.with_timezone(&chrono::Local).format("%b %-d, %Y");
    format!("{date} · {}", item.kind_label())
}

fn card(gallery: &Rc<Gallery>, item: &GalleryItem, cloud: bool) -> gtk::FlowBoxChild {
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_size_request(200, 125);
    picture.set_can_shrink(true);
    picture.add_css_class("gallery-thumbnail");
    picture.set_accessible_role(gtk::AccessibleRole::Presentation);
    if item.has_local_file() {
        load_thumbnail(&picture, item);
    }
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.set_overflow(gtk::Overflow::Hidden);
    overlay.add_css_class("card");
    if !item.has_local_file() {
        let missing =
            gtk::Image::from_icon_name(if cloud { "bettershot-share-symbolic" } else { "image-missing-symbolic" });
        missing.set_pixel_size(32);
        missing.add_css_class("dim-label");
        overlay.add_overlay(&missing);
    }
    let menu = actions_button(gallery, item, cloud);
    menu.set_halign(gtk::Align::End);
    menu.set_valign(gtk::Align::Start);
    menu.set_margin_top(6);
    menu.set_margin_end(6);
    menu.add_css_class("osd");
    menu.add_css_class("circular");
    overlay.add_overlay(&menu);

    let name =
        gtk::Label::builder().label(&item.title).ellipsize(gtk::pango::EllipsizeMode::Middle).xalign(0.0).build();
    name.set_tooltip_text(Some(&item.title));
    let detail = gtk::Label::builder().label(subtitle(item)).xalign(0.0).css_classes(["dim-label", "caption"]).build();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.append(&overlay);
    column.append(&name);
    column.append(&detail);
    // A thumbnail's natural size is its pixels; the clamp keeps cards small.
    let clamp = adw::Clamp::builder().maximum_size(220).tightening_threshold(220).child(&column).build();
    let child = gtk::FlowBoxChild::new();
    child.set_child(Some(&clamp));
    child.set_halign(gtk::Align::Start);
    child.update_property(&[
        gtk::accessible::Property::Label(&item.title),
        gtk::accessible::Property::Description(if item.has_local_file() {
            item.kind_label()
        } else {
            "Local source unavailable"
        }),
    ]);
    attach_context_menu(gallery, &child, item, cloud);
    child
}

fn list_row(gallery: &Rc<Gallery>, item: &GalleryItem, cloud: bool) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    PreferencesRowExt::set_use_markup(&row, false);
    PreferencesRowExt::set_title(&row, &item.title);
    row.set_subtitle(&subtitle(item));
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_size_request(64, 40);
    picture.set_can_shrink(true);
    picture.add_css_class("gallery-thumbnail");
    if item.has_local_file() {
        load_thumbnail(&picture, item);
    }
    row.add_prefix(&picture);
    let menu = actions_button(gallery, item, cloud);
    menu.add_css_class("flat");
    menu.set_valign(gtk::Align::Center);
    row.add_suffix(&menu);
    row.set_activatable(true);
    attach_context_menu(gallery, &row, item, cloud);
    row
}

/// The item's actions, as a menu model on an action group named `item`.
fn item_actions(gallery: &Rc<Gallery>, item: &GalleryItem, cloud: bool) -> (gio::Menu, gio::SimpleActionGroup) {
    let group = gio::SimpleActionGroup::new();
    let add = |name: &str, f: Box<dyn Fn()>| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| f());
        group.add_action(&action);
    };
    let menu = gio::Menu::new();
    let main = gio::Menu::new();
    let weak = Rc::downgrade(gallery);
    let it = item.clone();
    add(
        "open",
        Box::new(move || {
            if let Some(g) = weak.upgrade() {
                open_item(&g, &it)
            }
        }),
    );
    main.append(Some("Open"), Some("item.open"));
    if item.has_local_file() {
        let weak = Rc::downgrade(gallery);
        let it = item.clone();
        add(
            "edit",
            Box::new(move || {
                if let Some(g) = weak.upgrade() {
                    edit_item(&g, &it)
                }
            }),
        );
        main.append(Some("Edit"), Some("item.edit"));
        let it = item.clone();
        let window = gallery.window.downgrade();
        add(
            "reveal",
            Box::new(move || {
                let file = gio::File::for_path(if it.preview.exists() { &it.preview } else { &it.capture });
                gtk::FileLauncher::new(Some(&file)).open_containing_folder(
                    window.upgrade().as_ref(),
                    None::<&gio::Cancellable>,
                    |_| {},
                );
            }),
        );
        main.append(Some("Show in Files"), Some("item.reveal"));
    }
    if let Some(url) = &item.cloud_url {
        let link = url.clone();
        let weak = Rc::downgrade(gallery);
        add(
            "copy-link",
            Box::new(move || {
                let Some(g) = weak.upgrade() else { return };
                match clipboard::copy_text(&link) {
                    Ok(()) => show_message(&g, "Link copied."),
                    Err(_) => show_message(&g, "Couldn’t copy the link. Try again."),
                }
            }),
        );
        main.append(Some("Copy Link"), Some("item.copy-link"));
        let link = url.clone();
        let window = gallery.window.downgrade();
        add(
            "open-cloud",
            Box::new(move || {
                gtk::UriLauncher::new(&link).launch(window.upgrade().as_ref(), None::<&gio::Cancellable>, |_| {});
            }),
        );
        main.append(Some("Open Cloud Link"), Some("item.open-cloud"));
    }
    menu.append_section(None, &main);
    let destructive = gio::Menu::new();
    let weak = Rc::downgrade(gallery);
    let it = item.clone();
    add(
        "delete",
        Box::new(move || {
            if let Some(g) = weak.upgrade() {
                confirm_delete(&g, &it, cloud)
            }
        }),
    );
    destructive.append(Some(if cloud { "Delete Cloud Share…" } else { "Move to Trash…" }), Some("item.delete"));
    menu.append_section(None, &destructive);
    (menu, group)
}

fn actions_button(gallery: &Rc<Gallery>, item: &GalleryItem, cloud: bool) -> gtk::MenuButton {
    let (menu, group) = item_actions(gallery, item, cloud);
    let button = gtk::MenuButton::builder().icon_name("view-more-symbolic").menu_model(&menu).build();
    button.insert_action_group("item", Some(&group));
    let label = format!("Actions for {}", item.title);
    button.set_tooltip_text(Some("Media actions"));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);
    button
}

fn attach_context_menu(gallery: &Rc<Gallery>, widget: &impl IsA<gtk::Widget>, item: &GalleryItem, cloud: bool) {
    let (menu, group) = item_actions(gallery, item, cloud);
    widget.insert_action_group("item", Some(&group));
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(widget);
    popover.set_has_arrow(false);
    let owned = popover.clone();
    widget.connect_destroy(move |_| owned.unparent());
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gdk::BUTTON_SECONDARY);
    gesture.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.popup();
    });
    widget.add_controller(gesture);
}

fn show_message(gallery: &Gallery, text: &str) {
    gallery.message.set_title(text);
    gallery.message.set_revealed(true);
}

/// Port of `MediaGalleryItem.open`: a cloud item opens its link; a local one
/// opens as a capture card.
fn open_item(gallery: &Rc<Gallery>, item: &GalleryItem) {
    if gallery.category.get().cloud {
        if let Some(url) = &item.cloud_url {
            gtk::UriLauncher::new(url).launch(Some(&gallery.window), None::<&gio::Cancellable>, |_| {});
            return;
        }
    }
    if !item.has_local_file() {
        show_message(gallery, "This file was moved or deleted. Refresh the gallery.");
        return;
    }
    let path = if item.preview.exists() { item.preview.clone() } else { item.capture.clone() };
    deck_ui::show_existing(&App::get(), &path);
}

fn edit_item(gallery: &Rc<Gallery>, item: &GalleryItem) {
    if !item.has_local_file() {
        show_message(gallery, "This file was moved or deleted. Refresh the gallery.");
        return;
    }
    crate::editor::open(&App::get(), &item.capture);
}

fn confirm_delete(gallery: &Rc<Gallery>, item: &GalleryItem, cloud: bool) {
    let (heading, body, label) = if cloud {
        (
            "Delete this cloud share?",
            format!(
                "The shared copy of “{}” will be permanently deleted and its link will stop working. Local files stay on this computer.",
                item.title
            ),
            "Delete Cloud Share",
        )
    } else {
        (
            "Move this capture to Trash?",
            format!(
                "“{}” and its local source and edit files will move to Trash. Close its editor first. Existing cloud shares stay online.",
                item.title
            ),
            "Move to Trash",
        )
    };
    let dialog = adw::AlertDialog::new(Some(heading), Some(&body));
    dialog.add_responses(&[("cancel", "Cancel"), ("delete", label)]);
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let weak = Rc::downgrade(gallery);
    let item = item.clone();
    dialog.choose(Some(&gallery.window), None::<&gio::Cancellable>, move |response| {
        if response != "delete" {
            return;
        }
        let Some(gallery) = weak.upgrade() else { return };
        if cloud {
            delete_cloud(&gallery, item);
        } else {
            delete_local(&gallery, &item);
        }
    });
}

fn delete_local(gallery: &Rc<Gallery>, item: &GalleryItem) {
    let app = App::get();
    let result = {
        let mut state = app.state_mut();
        // Tests use a private folder, never the user's Trash.
        let test_trash =
            (std::env::var("BETTERSHOT_TESTING").as_deref() == Ok("1")).then(|| state.dirs.data.join("Trash"));
        gallery::delete_local(item, &mut state.history, |path| match &test_trash {
            Some(folder) => {
                std::fs::create_dir_all(folder)?;
                std::fs::rename(path, folder.join(uuid::Uuid::new_v4().to_string()))
            }
            None => gio::File::for_path(path).trash(None::<&gio::Cancellable>).map_err(std::io::Error::other),
        })
    };
    match result {
        Ok(()) => {
            deck_ui::forget(&item.preview);
            deck_ui::forget(&item.capture);
            refresh(gallery);
        }
        Err(error) => show_message(gallery, &format!("Couldn’t move “{}” to Trash. {error} Try again.", item.title)),
    }
}

fn delete_cloud(gallery: &Rc<Gallery>, item: GalleryItem) {
    let Some(url) = item.cloud_url.clone() else { return };
    let credentials = sharing::credentials(&App::get());
    let Some(slug) = share::deletion_slug(&url, &credentials.public_base_url) else {
        show_message(
            gallery,
            "This link belongs to different cloud storage. Select its original R2 settings in Settings → Sharing, then retry.",
        );
        return;
    };
    show_message(gallery, "Deleting…");
    let gallery = gallery.clone();
    glib::spawn_future_local(async move {
        match sharing::delete_share(credentials, slug).await {
            Ok(()) => {
                let result = App::get().state_mut().history.forget_cloud_link(&url);
                gallery.message.set_revealed(false);
                if let Err(error) = result {
                    show_message(
                        &gallery,
                        &format!("The share was deleted, but the library couldn’t be updated. {error}"),
                    );
                }
                refresh(&gallery);
            }
            Err(error) => show_message(&gallery, &format!("Couldn’t delete “{}”. {error}", item.title)),
        }
    });
}
