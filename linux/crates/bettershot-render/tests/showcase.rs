//! Renders every shape kind and background feature through the production
//! renderer. Set BETTERSHOT_RENDER_SNAPSHOTS=1 to write the PNG to
//! target/render-snapshots/ for review.

use bettershot_core::annotation::background::{BackgroundSettings, BgGradient, Border, Watermark};
use bettershot_core::annotation::geometry::Vec2;
use bettershot_core::annotation::shape::*;
use bettershot_core::annotation::swatch::Swatch;
use bettershot_core::background::{GradientPreset, ImageAlignment};
use bettershot_render::{render, Scene};
use image::{Rgba, RgbaImage};

fn screenshot() -> RgbaImage {
    RgbaImage::from_fn(900, 560, |x, y| {
        if y < 40 {
            Rgba([236, 236, 240, 255])
        } else if (x / 24 + y / 24) % 2 == 0 {
            Rgba([250, 250, 252, 255])
        } else {
            Rgba([60, 70, 90, 255])
        }
    })
}

fn shapes() -> Vec<AnnoShape> {
    let mut shapes = vec![
        AnnoShape::new(30.0, 60.0, ShapeKind::Geo(GeoProps { w: 160.0, h: 100.0, ..Default::default() })),
        AnnoShape::new(
            220.0,
            60.0,
            ShapeKind::Geo(GeoProps {
                w: 160.0,
                h: 100.0,
                dash: DashStyle::Dashed,
                corner_radius: 20.0,
                swatch: Swatch::blue(),
                ..Default::default()
            }),
        ),
        AnnoShape::new(
            410.0,
            60.0,
            ShapeKind::Geo(GeoProps {
                geo: GeoKind::Ellipse,
                w: 160.0,
                h: 100.0,
                dash: DashStyle::Dotted,
                swatch: Swatch::green(),
                ..Default::default()
            }),
        ),
        AnnoShape::new(
            600.0,
            60.0,
            ShapeKind::Geo(GeoProps {
                w: 120.0,
                h: 100.0,
                fill: FillStyle::Solid,
                swatch: Swatch::yellow(),
                ..Default::default()
            }),
        ),
        AnnoShape::new(
            40.0,
            230.0,
            ShapeKind::Arrow(ArrowProps {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(200.0, 0.0),
                ..Default::default()
            }),
        ),
        AnnoShape::new(
            40.0,
            300.0,
            ShapeKind::Arrow(ArrowProps {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(200.0, 0.0),
                bend: 40.0,
                arrowhead_start: Arrowhead::Dot,
                arrowhead_end: Arrowhead::Triangle,
                swatch: Swatch::purple(),
                ..Default::default()
            }),
        ),
        AnnoShape::new(
            300.0,
            240.0,
            ShapeKind::Draw(DrawProps {
                points: (0..60)
                    .map(|i| {
                        let t = f64::from(i) / 59.0;
                        Vec2::with_pressure(t * 200.0, (t * 12.0).sin() * 30.0, 0.5)
                    })
                    .collect(),
                is_complete: true,
                swatch: Swatch::orange(),
                ..Default::default()
            }),
        ),
        AnnoShape::new(560.0, 220.0, ShapeKind::Text(TextProps { text: "Annotated".into(), ..Default::default() })),
        AnnoShape::new(760.0, 230.0, ShapeKind::Numbered(NumberedProps { value: 3, ..Default::default() })),
        AnnoShape::new(
            40.0,
            380.0,
            ShapeKind::Redaction(RedactionProps { kind: RedactionKind::Blur, w: 180.0, h: 140.0, density: 0.55 }),
        ),
        AnnoShape::new(
            260.0,
            380.0,
            ShapeKind::Redaction(RedactionProps { kind: RedactionKind::Pixelate, w: 180.0, h: 140.0, density: 0.55 }),
        ),
        AnnoShape::new(600.0, 380.0, ShapeKind::Highlight(HighlightProps { w: 220.0, h: 140.0 })),
    ];
    let mut rotated = AnnoShape::new(
        480.0,
        380.0,
        ShapeKind::Geo(GeoProps { w: 80.0, h: 60.0, swatch: Swatch::pink(), ..Default::default() }),
    );
    rotated.rotation = 0.4;
    shapes.push(rotated);
    shapes
}

#[test]
fn showcase_renders() {
    let base = screenshot();
    let shapes = shapes();
    let background = BackgroundSettings {
        style: bettershot_core::annotation::background::BgStyle::Gradient(BgGradient::from_preset(
            &GradientPreset::presets()[0],
        )),
        padding: 0.12,
        corner_radius: 0.03,
        shadow: 0.4,
        alignment: ImageAlignment::Center,
        border: Border { is_enabled: true, ..Default::default() },
        watermark: Watermark { is_enabled: true, text: "BetterShot".into(), ..Default::default() },
        ..Default::default()
    };
    let scene = Scene { base: &base, base_generation: 1, shapes: &shapes, bindings: &[], background: &background };
    let image = render(&scene).unwrap();
    assert_eq!(image.dimensions(), scene.canvas_size());
    if std::env::var_os("BETTERSHOT_RENDER_SNAPSHOTS").is_some() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/render-snapshots");
        std::fs::create_dir_all(&dir).unwrap();
        image.save(dir.join("showcase.png")).unwrap();
        let plain = BackgroundSettings::default();
        render(&Scene { background: &plain, ..scene }).unwrap().save(dir.join("showcase-plain.png")).unwrap();
    }
}
