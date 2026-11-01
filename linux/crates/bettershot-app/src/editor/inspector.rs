//! The editor's left-hand inspector: the style of the current tool or
//! selection, and the background around the screenshot. A port of
//! `AnnotationInspector.swift` and `AnnotationBackgroundInspector.swift`.

use super::{window, Editor};
use adw::prelude::*;
use bettershot_core::annotation::background::{
    AspectRatio, BackgroundSettings, BgColor, BgGradient, BgStyle, ShadowStyle,
};
use bettershot_core::annotation::shape::{Arrowhead, DashStyle, FontFamily, TextAlign, Tool};
use bettershot_core::annotation::swatch::Swatch;
use bettershot_core::background::{GradientPreset, ImageAlignment};
use gtk::{gdk, gio, glib};
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

/// A labelled slider sharing one adjustment with an editable value field.
pub(crate) struct Slider {
    pub(crate) root: gtk::Box,
    pub(crate) adjustment: gtk::Adjustment,
}

pub(crate) fn slider(label: &str, min: f64, max: f64, step: f64, digits: u32) -> Slider {
    let adjustment = gtk::Adjustment::new(min, min, max, step, step * 10.0, 0.0);
    let title = gtk::Label::builder().label(label).xalign(0.0).hexpand(true).css_classes(["caption-heading"]).build();
    let value = gtk::SpinButton::new(Some(&adjustment), step, digits);
    value.set_width_chars(5);
    value.add_css_class("flat");
    value.update_property(&[gtk::accessible::Property::Label(label)]);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.append(&title);
    header.append(&value);
    let scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&adjustment));
    scale.set_draw_value(false);
    scale.update_property(&[gtk::accessible::Property::Label(label)]);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&scale);
    Slider { root, adjustment }
}

pub(crate) fn dropdown(label: &str, items: &[&str]) -> (gtk::Box, gtk::DropDown) {
    let title = gtk::Label::builder().label(label).xalign(0.0).hexpand(true).css_classes(["caption-heading"]).build();
    let list = gtk::DropDown::from_strings(items);
    list.update_property(&[gtk::accessible::Property::Label(label)]);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&title);
    row.append(&list);
    (row, list)
}

pub(crate) fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["heading"]).margin_top(8).build()
}

/// A round color button labelled for screen readers.
pub(crate) fn color_button(title: &str, rgba: (f64, f64, f64, f64)) -> gtk::Button {
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_draw_func(move |_, cr, w, h| {
        let r = f64::from(w.min(h)) / 2.0;
        cr.arc(f64::from(w) / 2.0, f64::from(h) / 2.0, r - 1.0, 0.0, std::f64::consts::TAU);
        cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.45);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    let button =
        gtk::Button::builder().child(&area).css_classes(["flat", "circular", "swatch"]).tooltip_text(title).build();
    button.update_property(&[gtk::accessible::Property::Label(title)]);
    button
}

/// A background tile drawn with the same code the canvas and exports use.
pub(crate) fn background_tile(title: &str, style: BgStyle) -> gtk::Button {
    let area = gtk::DrawingArea::new();
    area.set_content_width(34);
    area.set_content_height(26);
    let drawn = style.clone();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        bettershot_render::rounded_rect(cr, (0.5, 0.5, w - 1.0, h - 1.0), [5.0; 4]);
        cr.clip();
        match &drawn {
            BgStyle::None => {
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.paint();
                cr.set_source_rgb(0.85, 0.2, 0.2);
                cr.set_line_width(2.0);
                cr.move_to(w, 0.0);
                cr.line_to(0.0, h);
                let _ = cr.stroke();
            }
            BgStyle::Solid(color) => {
                cr.set_source_rgba(color.red, color.green, color.blue, color.alpha);
                let _ = cr.paint();
            }
            BgStyle::Gradient(gradient) => {
                if let Some(preset) = &gradient.preset {
                    bettershot_render::draw_gradient_preset(cr, preset, (0.0, 0.0, w, h));
                }
            }
            BgStyle::CustomWallpaper { .. } => {}
        }
        cr.reset_clip();
        bettershot_render::rounded_rect(cr, (0.5, 0.5, w - 1.0, h - 1.0), [5.0; 4]);
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.4);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    let button =
        gtk::Button::builder().child(&area).css_classes(["flat", "background-tile"]).tooltip_text(title).build();
    button.update_property(&[gtk::accessible::Property::Label(title)]);
    button
}

pub struct Inspector {
    pub root: gtk::ScrolledWindow,
    updating: Cell<bool>,
    style_hint: gtk::Label,
    color_box: gtk::Box,
    stroke: Slider,
    dash_row: gtk::Box,
    dash: gtk::DropDown,
    corner: Slider,
    heads_box: gtk::Box,
    head_start: gtk::DropDown,
    head_end: gtk::DropDown,
    text_box: gtk::Box,
    font: gtk::DropDown,
    font_size: Slider,
    bold: gtk::ToggleButton,
    italic: gtk::ToggleButton,
    underline: gtk::ToggleButton,
    align: [gtk::ToggleButton; 3],
    density: Slider,
    diameter: Slider,
    padding: Slider,
    corners: Slider,
    shadow: Slider,
    shadow_style: gtk::DropDown,
    ratio: gtk::DropDown,
    alignment: Vec<(ImageAlignment, gtk::ToggleButton)>,
    border: gtk::Switch,
    border_thickness: Slider,
    watermark_text: gtk::Entry,
    watermark_opacity: Slider,
    watermark_size: Slider,
    unrendered: gtk::Label,
}

const DASHES: [DashStyle; 3] = [DashStyle::Solid, DashStyle::Dashed, DashStyle::Dotted];

fn edit_style(editor: &Rc<Editor>, inspector_updating: bool, key: &str, f: impl FnOnce(&mut super::model::ToolStyle)) {
    if inspector_updating {
        return;
    }
    let has_selection = !editor.model.borrow().selection.is_empty();
    if has_selection {
        editor.checkpoint_coalesced(key);
    }
    {
        let mut model = editor.model.borrow_mut();
        f(&mut model.style);
        if has_selection {
            model.apply_style_to_selection_unrecorded();
        }
    }
    window::refresh(editor);
}

fn edit_background(editor: &Rc<Editor>, inspector_updating: bool, key: &str, f: impl FnOnce(&mut BackgroundSettings)) {
    if inspector_updating {
        return;
    }
    editor.checkpoint_coalesced(key);
    f(&mut editor.model.borrow_mut().current.background);
    window::refresh(editor);
}

/// Connects `f` to run when the inspector is not merely syncing its values.
fn guard(editor: &Rc<Editor>) -> impl Fn() -> Option<(Rc<Editor>, bool)> + Clone {
    let weak = Rc::downgrade(editor);
    move || {
        let editor = weak.upgrade()?;
        let updating = window::inspector_updating(&editor);
        Some((editor, updating))
    }
}

pub fn build(editor: &Rc<Editor>, parent: &impl IsA<gtk::Window>) -> Inspector {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_margin_top(12);
    content.set_margin_bottom(18);
    content.set_margin_start(14);
    content.set_margin_end(14);
    content.add_css_class("editor-inspector");

    // MARK: Style
    content.append(&heading("Style"));
    let style_hint = gtk::Label::builder()
        .label("Choose a tool, or select a shape to change its style.")
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    content.append(&style_hint);

    let color_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    color_box.append(&gtk::Label::builder().label("Color").xalign(0.0).css_classes(["caption-heading"]).build());
    let colors = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(6)
        .min_children_per_line(6)
        .build();
    let g = guard(editor);
    for swatch in Swatch::all() {
        let button = color_button(&swatch.title, swatch.rgba());
        let g = g.clone();
        button.connect_clicked(move |_| {
            if let Some((editor, updating)) = g() {
                edit_style(&editor, updating, "color", |style| style.swatch = swatch.clone());
            }
        });
        colors.insert(&button, -1);
    }
    let custom = gtk::ColorDialogButton::new(Some(
        gtk::ColorDialog::builder().title("Annotation Color").with_alpha(true).build(),
    ));
    custom.set_tooltip_text(Some("Custom color"));
    custom.update_property(&[gtk::accessible::Property::Label("Custom color")]);
    let custom_guard = g.clone();
    custom.connect_rgba_notify(move |button| {
        if let Some((editor, updating)) = custom_guard() {
            let c = button.rgba();
            edit_style(&editor, updating, "color", |style| {
                style.swatch =
                    Swatch::custom(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), f64::from(c.alpha()))
            });
        }
    });
    colors.insert(&custom, -1);
    color_box.append(&colors);
    content.append(&color_box);

    let stroke = slider("Thickness", 1.0, 40.0, 1.0, 0);
    let sg = g.clone();
    stroke.adjustment.connect_value_changed(move |a| {
        if let Some((editor, updating)) = sg() {
            let v = a.value();
            edit_style(&editor, updating, "stroke", |style| style.stroke_width = v);
        }
    });
    content.append(&stroke.root);

    let (dash_row, dash) = dropdown("Line", &["Solid", "Dashed", "Dotted"]);
    let dg = g.clone();
    dash.connect_selected_notify(move |d| {
        if let Some((editor, updating)) = dg() {
            let value = DASHES[(d.selected() as usize).min(2)];
            edit_style(&editor, updating, "dash", |style| style.dash = value);
        }
    });
    content.append(&dash_row);

    let corner = slider("Corner radius", 0.0, 80.0, 1.0, 0);
    let cg = g.clone();
    corner.adjustment.connect_value_changed(move |a| {
        if let Some((editor, updating)) = cg() {
            let v = a.value();
            edit_style(&editor, updating, "corner", |style| style.corner_radius = v);
        }
    });
    content.append(&corner.root);

    let heads: Vec<&str> = Arrowhead::ALL.iter().map(|h| h.title()).collect();
    let heads_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (start_row, head_start) = dropdown("Start", &heads);
    let (end_row, head_end) = dropdown("End", &heads);
    heads_box.append(&start_row);
    heads_box.append(&end_row);
    for (list, is_start) in [(&head_start, true), (&head_end, false)] {
        let hg = g.clone();
        list.connect_selected_notify(move |d| {
            if let Some((editor, updating)) = hg() {
                let head = Arrowhead::ALL[(d.selected() as usize).min(Arrowhead::ALL.len() - 1)];
                edit_style(&editor, updating, "heads", |style| {
                    if is_start {
                        style.arrowhead_start = head;
                    } else {
                        style.arrowhead_end = head;
                    }
                });
            }
        });
    }
    content.append(&heads_box);

    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let families: Vec<&str> = FontFamily::ALL.iter().map(|f| f.title()).collect();
    let (font_row, font) = dropdown("Font", &families);
    let fg = g.clone();
    font.connect_selected_notify(move |d| {
        if let Some((editor, updating)) = fg() {
            let family = FontFamily::ALL[(d.selected() as usize).min(2)];
            edit_style(&editor, updating, "font", |style| style.font_family = family);
        }
    });
    text_box.append(&font_row);
    let font_size = slider("Size", 8.0, 240.0, 1.0, 0);
    let fsg = g.clone();
    font_size.adjustment.connect_value_changed(move |a| {
        if let Some((editor, updating)) = fsg() {
            let v = a.value();
            edit_style(&editor, updating, "fontSize", |style| style.font_size = v);
        }
    });
    text_box.append(&font_size.root);
    let traits = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    traits.add_css_class("linked");
    let toggle = |icon: &str, label: &str| {
        let button = gtk::ToggleButton::builder().icon_name(icon).tooltip_text(label).build();
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        button
    };
    let bold = toggle("format-text-bold-symbolic", "Bold");
    let italic = toggle("format-text-italic-symbolic", "Italic");
    let underline = toggle("format-text-underline-symbolic", "Underline");
    for (button, which) in [(&bold, 0), (&italic, 1), (&underline, 2)] {
        let tg = g.clone();
        button.connect_toggled(move |b| {
            if let Some((editor, updating)) = tg() {
                let on = b.is_active();
                edit_style(&editor, updating, "traits", |style| match which {
                    0 => style.bold = on,
                    1 => style.italic = on,
                    _ => style.underline = on,
                });
            }
        });
        traits.append(button);
    }
    let alignment_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    alignment_box.add_css_class("linked");
    alignment_box.set_margin_start(8);
    let align = [
        toggle("format-justify-left-symbolic", "Align left"),
        toggle("format-justify-center-symbolic", "Align center"),
        toggle("format-justify-right-symbolic", "Align right"),
    ];
    for (button, value) in align.iter().zip([TextAlign::Start, TextAlign::Middle, TextAlign::End]) {
        let ag = g.clone();
        button.connect_clicked(move |_| {
            if let Some((editor, updating)) = ag() {
                edit_style(&editor, updating, "align", |style| style.align = value);
            }
        });
        alignment_box.append(button);
    }
    let text_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    text_row.append(&traits);
    text_row.append(&alignment_box);
    text_box.append(&text_row);
    content.append(&text_box);

    let density = slider("Strength", 0.0, 1.0, 0.05, 2);
    let deng = g.clone();
    density.adjustment.connect_value_changed(move |a| {
        if let Some((editor, updating)) = deng() {
            let v = a.value();
            edit_style(&editor, updating, "density", |style| style.density = v);
        }
    });
    content.append(&density.root);

    let diameter = slider("Size", 20.0, 240.0, 2.0, 0);
    let dig = g.clone();
    diameter.adjustment.connect_value_changed(move |a| {
        if let Some((editor, updating)) = dig() {
            let v = a.value();
            edit_style(&editor, updating, "diameter", |style| style.diameter = v);
        }
    });
    content.append(&diameter.root);

    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // MARK: Background
    content.append(&heading("Background"));
    let tiles = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(5)
        .min_children_per_line(5)
        .build();
    let mut styles: Vec<(String, BgStyle)> = vec![("No background".into(), BgStyle::None)];
    styles.extend(
        GradientPreset::presets().iter().map(|p| (p.name.clone(), BgStyle::Gradient(BgGradient::from_preset(p)))),
    );
    styles.extend(BgColor::presets().into_iter().map(|c| (c.title.clone(), BgStyle::Solid(c))));
    for (title, style) in styles {
        let tile = background_tile(&title, style.clone());
        let bg = g.clone();
        tile.connect_clicked(move |_| {
            if let Some((editor, updating)) = bg() {
                let style = style.clone();
                edit_background(&editor, updating, "style", |settings| {
                    settings.custom_wallpaper_path = None;
                    settings.style = style;
                });
            }
        });
        tiles.insert(&tile, -1);
    }
    let custom_background = gtk::ColorDialogButton::new(Some(
        gtk::ColorDialog::builder().title("Background Color").with_alpha(false).build(),
    ));
    custom_background.set_tooltip_text(Some("Custom background color"));
    custom_background.update_property(&[gtk::accessible::Property::Label("Custom background color")]);
    let cbg = g.clone();
    custom_background.connect_rgba_notify(move |button| {
        if let Some((editor, updating)) = cbg() {
            let c = button.rgba();
            edit_background(&editor, updating, "style", |settings| {
                settings.style =
                    BgStyle::Solid(BgColor::custom(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue())));
            });
        }
    });
    tiles.insert(&custom_background, -1);
    let wallpaper = gtk::Button::builder()
        .icon_name("insert-image-symbolic")
        .tooltip_text("Use an image")
        .css_classes(["flat"])
        .build();
    wallpaper.update_property(&[gtk::accessible::Property::Label("Use an image as the background")]);
    let wg = g.clone();
    let wallpaper_parent = parent.clone().upcast::<gtk::Window>();
    wallpaper.connect_clicked(move |_| {
        let Some((editor, _)) = wg() else { return };
        let parent = wallpaper_parent.clone();
        glib::spawn_future_local(async move {
            let filter = gtk::FileFilter::new();
            filter.add_mime_type("image/png");
            filter.add_mime_type("image/jpeg");
            filter.set_name(Some("Images"));
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog =
                gtk::FileDialog::builder().title("Choose Background Image").filters(&filters).modal(true).build();
            if let Ok(file) = dialog.open_future(Some(&parent)).await {
                if let Some(path) = file.path() {
                    let path = path.to_string_lossy().into_owned();
                    edit_background(&editor, false, "wallpaper", |settings| {
                        settings.custom_wallpaper_path = Some(path.clone());
                        settings.style = BgStyle::CustomWallpaper { path };
                    });
                }
            }
        });
    });
    tiles.insert(&wallpaper, -1);
    content.append(&tiles);

    let padding = slider("Padding", 0.0, 0.4, 0.01, 2);
    let corners = slider("Corners", 0.0, 0.12, 0.002, 3);
    let shadow = slider("Shadow", 0.0, 1.0, 0.02, 2);
    for (s, key) in [(&padding, "padding"), (&corners, "corners"), (&shadow, "shadow")] {
        let sg = g.clone();
        s.adjustment.connect_value_changed(move |a| {
            if let Some((editor, updating)) = sg() {
                let v = a.value();
                edit_background(&editor, updating, key, |settings| match key {
                    "padding" => settings.padding = v,
                    "corners" => settings.corner_radius = v,
                    _ => settings.shadow = v,
                });
            }
        });
        content.append(&s.root);
    }
    let shadow_titles: Vec<&str> = ShadowStyle::ALL.iter().map(|s| s.title()).collect();
    let (shadow_row, shadow_style) = dropdown("Shadow style", &shadow_titles);
    let ssg = g.clone();
    shadow_style.connect_selected_notify(move |d| {
        if let Some((editor, updating)) = ssg() {
            let style = ShadowStyle::ALL[(d.selected() as usize).min(3)];
            edit_background(&editor, updating, "shadowStyle", |settings| settings.shadow_style = style);
        }
    });
    content.append(&shadow_row);
    let ratios: Vec<&str> = AspectRatio::ALL.iter().map(|r| r.title()).collect();
    let (ratio_row, ratio) = dropdown("Aspect ratio", &ratios);
    let rg = g.clone();
    ratio.connect_selected_notify(move |d| {
        if let Some((editor, updating)) = rg() {
            let value = AspectRatio::ALL[(d.selected() as usize).min(4)];
            edit_background(&editor, updating, "ratio", |settings| settings.aspect_ratio = value);
        }
    });
    content.append(&ratio_row);

    let grid = gtk::Grid::builder().row_spacing(2).column_spacing(2).halign(gtk::Align::Start).build();
    let mut alignment = Vec::new();
    for (index, value) in ImageAlignment::ALL.into_iter().enumerate() {
        let button =
            gtk::ToggleButton::builder().css_classes(["flat", "alignment-cell"]).tooltip_text(value.title()).build();
        button.update_property(&[gtk::accessible::Property::Label(&format!("Align {}", value.title()))]);
        button.set_child(Some(&gtk::Image::from_icon_name("media-record-symbolic")));
        let ag = g.clone();
        button.connect_clicked(move |_| {
            if let Some((editor, updating)) = ag() {
                edit_background(&editor, updating, "alignment", |settings| settings.alignment = value);
            }
        });
        grid.attach(&button, (index % 3) as i32, (index / 3) as i32, 1, 1);
        alignment.push((value, button));
    }
    let alignment_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    alignment_row.append(
        &gtk::Label::builder().label("Position").xalign(0.0).hexpand(true).css_classes(["caption-heading"]).build(),
    );
    alignment_row.append(&grid);
    content.append(&alignment_row);

    content.append(&heading("Border"));
    let border = gtk::Switch::builder().valign(gtk::Align::Center).build();
    border.update_property(&[gtk::accessible::Property::Label("Border")]);
    let border_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    border_row.append(&gtk::Label::builder().label("Show border").xalign(0.0).hexpand(true).build());
    let border_color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().title("Border Color").build()));
    border_color.set_rgba(&gdk::RGBA::new(0.96, 0.96, 0.96, 1.0));
    border_color.update_property(&[gtk::accessible::Property::Label("Border color")]);
    border_row.append(&border_color);
    border_row.append(&border);
    let bsg = g.clone();
    border.connect_active_notify(move |s| {
        if let Some((editor, updating)) = bsg() {
            let on = s.is_active();
            edit_background(&editor, updating, "border", |settings| settings.border.is_enabled = on);
        }
    });
    let bcg = g.clone();
    border_color.connect_rgba_notify(move |button| {
        if let Some((editor, updating)) = bcg() {
            let c = button.rgba();
            edit_background(&editor, updating, "borderColor", |settings| {
                settings.border.is_enabled = true;
                settings.border.color =
                    Swatch::custom(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), f64::from(c.alpha()));
            });
        }
    });
    content.append(&border_row);
    let border_thickness = slider("Thickness", 0.0, 0.06, 0.001, 3);
    let btg = g.clone();
    border_thickness.adjustment.connect_value_changed(move |a| {
        if let Some((editor, updating)) = btg() {
            let v = a.value();
            edit_background(&editor, updating, "borderThickness", |settings| settings.border.thickness = v);
        }
    });
    content.append(&border_thickness.root);

    content.append(&heading("Watermark"));
    let watermark_text = gtk::Entry::builder().placeholder_text("Text repeated across the image").build();
    watermark_text.update_property(&[gtk::accessible::Property::Label("Watermark text")]);
    let wtg = g.clone();
    watermark_text.connect_changed(move |entry| {
        if let Some((editor, updating)) = wtg() {
            let text = entry.text().to_string();
            edit_background(&editor, updating, "watermarkText", |settings| {
                settings.watermark.is_enabled = !text.trim().is_empty();
                settings.watermark.text = text;
            });
        }
    });
    content.append(&watermark_text);
    let watermark_opacity = slider("Opacity", 0.0, 0.75, 0.01, 2);
    let watermark_size = slider("Text size", 12.0, 240.0, 1.0, 0);
    for (s, key) in [(&watermark_opacity, "watermarkOpacity"), (&watermark_size, "watermarkSize")] {
        let sg = g.clone();
        s.adjustment.connect_value_changed(move |a| {
            if let Some((editor, updating)) = sg() {
                let v = a.value();
                edit_background(&editor, updating, key, |settings| {
                    if key == "watermarkOpacity" {
                        settings.watermark.opacity = v;
                    } else {
                        settings.watermark.font_size = v;
                    }
                });
            }
        });
        content.append(&s.root);
    }

    let unrendered = gtk::Label::builder()
        .label("This image has a 3D camera or progressive blur from BetterShot for macOS. Linux keeps those settings when saving but does not show them yet.")
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .visible(false)
        .build();
    content.append(&unrendered);

    let root = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(280)
        .child(&content)
        .build();
    root.set_overlay_scrolling(true);

    Inspector {
        root,
        updating: Cell::new(false),
        style_hint,
        color_box,
        stroke,
        dash_row,
        dash,
        corner,
        heads_box,
        head_start,
        head_end,
        text_box,
        font,
        font_size,
        bold,
        italic,
        underline,
        align,
        density,
        diameter,
        padding,
        corners,
        shadow,
        shadow_style,
        ratio,
        alignment,
        border,
        border_thickness,
        watermark_text,
        watermark_opacity,
        watermark_size,
        unrendered,
    }
}

impl Inspector {
    pub fn is_updating(&self) -> bool {
        self.updating.get()
    }

    /// Shows the controls the tool or selection uses, with their values.
    pub fn sync(&self, editor: &Editor) {
        self.updating.set(true);
        let model = editor.model.borrow();
        let tools: HashSet<Tool> = if model.selection.is_empty() {
            [model.tool].into_iter().collect()
        } else {
            model.selected_shapes().iter().map(|s| s.tool()).collect()
        };
        let any = |f: fn(Tool) -> bool| tools.iter().any(|t| f(*t));
        let shows_color = any(Tool::supports_color);
        self.color_box.set_visible(shows_color);
        self.stroke.root.set_visible(any(Tool::supports_stroke));
        self.dash_row.set_visible(any(|t| matches!(t, Tool::Rectangle | Tool::Ellipse | Tool::Line | Tool::Arrow)));
        self.corner.root.set_visible(any(|t| matches!(t, Tool::Rectangle | Tool::FilledRectangle)));
        self.heads_box.set_visible(any(|t| t == Tool::Arrow));
        self.text_box.set_visible(any(|t| t == Tool::Text));
        self.density.root.set_visible(any(Tool::is_redaction));
        self.diameter.root.set_visible(any(|t| t == Tool::NumberedCircle));
        let shows_any = shows_color || any(Tool::is_redaction);
        self.style_hint.set_visible(!shows_any);

        let style = &model.style;
        self.stroke.adjustment.set_value(style.stroke_width);
        self.dash.set_selected(DASHES.iter().position(|d| *d == style.dash).unwrap_or(0) as u32);
        self.corner.adjustment.set_value(style.corner_radius);
        self.head_start
            .set_selected(Arrowhead::ALL.iter().position(|h| *h == style.arrowhead_start).unwrap_or(0) as u32);
        self.head_end.set_selected(Arrowhead::ALL.iter().position(|h| *h == style.arrowhead_end).unwrap_or(1) as u32);
        self.font.set_selected(FontFamily::ALL.iter().position(|f| *f == style.font_family).unwrap_or(0) as u32);
        self.font_size.adjustment.set_value(style.font_size);
        self.bold.set_active(style.bold);
        self.italic.set_active(style.italic);
        self.underline.set_active(style.underline);
        for (button, value) in self.align.iter().zip([TextAlign::Start, TextAlign::Middle, TextAlign::End]) {
            button.set_active(style.align == value);
        }
        self.density.adjustment.set_value(style.density);
        self.diameter.adjustment.set_value(style.diameter);

        let background = &model.current.background;
        self.padding.adjustment.set_value(background.padding);
        self.corners.adjustment.set_value(background.corner_radius);
        self.shadow.adjustment.set_value(background.shadow);
        self.shadow_style
            .set_selected(ShadowStyle::ALL.iter().position(|s| *s == background.shadow_style).unwrap_or(0) as u32);
        self.ratio
            .set_selected(AspectRatio::ALL.iter().position(|r| *r == background.aspect_ratio).unwrap_or(0) as u32);
        let canvas = background.uses_canvas_layout();
        for widget in [&self.padding.root, &self.corners.root, &self.shadow.root] {
            widget.set_sensitive(canvas);
        }
        self.shadow_style.set_sensitive(canvas);
        self.ratio.set_sensitive(canvas);
        for (value, button) in &self.alignment {
            button.set_active(*value == background.alignment);
            button.set_sensitive(canvas);
        }
        self.border.set_active(background.border.is_enabled);
        self.border_thickness.adjustment.set_value(background.border.thickness);
        self.border_thickness.root.set_sensitive(background.border.is_enabled);
        if self.watermark_text.text() != background.watermark.text {
            self.watermark_text.set_text(&background.watermark.text);
        }
        self.watermark_opacity.adjustment.set_value(background.watermark.opacity);
        self.watermark_size.adjustment.set_value(background.watermark.font_size);
        self.unrendered.set_visible(background.has_unrendered_effects());
        drop(model);
        self.updating.set(false);
    }
}
