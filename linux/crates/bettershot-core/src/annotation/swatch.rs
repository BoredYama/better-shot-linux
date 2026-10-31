//! Port of `AnnotationSwatch.swift`: the annotation palette.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Swatch {
    pub id: String,
    pub title: String,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    #[serde(default = "one")]
    pub alpha: f64,
}

fn one() -> f64 {
    1.0
}

impl Default for Swatch {
    fn default() -> Self {
        Swatch::red()
    }
}

macro_rules! swatches {
    ($($name:ident, $id:literal, $title:literal, $r:literal, $g:literal, $b:literal;)*) => {
        impl Swatch {
            $(pub fn $name() -> Swatch {
                Swatch { id: $id.into(), title: $title.into(), red: $r, green: $g, blue: $b, alpha: 1.0 }
            })*

            /// The ten palette colors, in toolbar order.
            pub fn all() -> Vec<Swatch> {
                vec![$(Swatch::$name(),)*]
            }
        }
    };
}

swatches! {
    black, "black", "Black", 0.02, 0.02, 0.024;
    red, "red", "Red", 0.97, 0.22, 0.2;
    orange, "orange", "Orange", 1.0, 0.53, 0.08;
    yellow, "yellow", "Yellow", 1.0, 0.82, 0.18;
    green, "green", "Green", 0.18, 0.72, 0.36;
    turquoise, "turquoise", "Turquoise", 0.20, 0.77, 0.72;
    blue, "blue", "Blue", 0.18, 0.48, 1.0;
    purple, "purple", "Purple", 0.55, 0.30, 0.95;
    pink, "pink", "Pink", 1.0, 0.18, 0.43;
    white, "white", "White", 0.96, 0.96, 0.96;
}

impl Swatch {
    /// A color picked outside the palette, named like the macOS custom swatch.
    pub fn custom(red: f64, green: f64, blue: f64, alpha: f64) -> Swatch {
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0) as i64;
        Swatch {
            id: format!("custom-{}-{}-{}-{}", byte(red), byte(green), byte(blue), byte(alpha)),
            title: "Custom".into(),
            red,
            green,
            blue,
            alpha,
        }
    }

    fn is_light(&self) -> bool {
        0.299 * self.red + 0.587 * self.green + 0.114 * self.blue > 0.68
    }

    pub fn rgba(&self) -> (f64, f64, f64, f64) {
        (self.red, self.green, self.blue, self.alpha)
    }

    /// The digit color on a numbered callout of this color.
    pub fn numbered_text_rgba(&self) -> (f64, f64, f64, f64) {
        if self.is_light() {
            (0.0, 0.0, 0.0, 1.0)
        } else {
            (1.0, 1.0, 1.0, 1.0)
        }
    }

    /// The thin ring around a numbered callout.
    pub fn numbered_outline_rgba(&self) -> (f64, f64, f64, f64) {
        if self.is_light() {
            (0.0, 0.0, 0.0, 0.22)
        } else {
            (1.0, 1.0, 1.0, 0.42)
        }
    }
}
