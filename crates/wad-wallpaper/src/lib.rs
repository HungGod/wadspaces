//! Builder wallpapers are CSS: gradients, a colour, or a picture (the
//! client's lib/types.ts `Wallpaper`). The desktop in a wadspace (swaybg)
//! needs a file, and the native client draws them itself; this reads the
//! CSS the way a browser draws it: layers bottom-up, linear and elliptical
//! radial gradients, fades to `transparent` in the neighbouring colour.
//! The client's lib/wallpaperRender.ts, in Rust (its tests are here too).

use serde::{Deserialize, Serialize};

#[cfg(feature = "raster")]
mod raster;
pub mod texture;
#[cfg(feature = "raster")]
pub use raster::{SIZE, draw_png, draw_png_toned, png};
pub use texture::{TEXTURES, Tone};

/// A wallpaper as the Builder keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wallpaper {
    #[serde(rename = "type")]
    pub kind: Kind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Gradient,
    Color,
    Image,
    /// One of [`TEXTURES`], by id (drawn dark or light).
    Texture,
}

/// What a wallpaper paints: its layers, bottom first, or a picture (its URL).
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Layers(Vec<Layer>),
    Image(String),
    /// A texture by id: drawn as a picture ([`texture::png`]).
    Texture(String),
}

impl Wallpaper {
    pub fn paint(&self) -> Paint {
        match self.kind {
            Kind::Image => Paint::Image(self.value.clone()),
            Kind::Texture => Paint::Texture(self.value.clone()),
            Kind::Color => Paint::Layers(layer(&self.value).into_iter().collect()),
            Kind::Gradient => Paint::Layers(layers(&self.value)),
        }
    }
}

/// Red, green, blue and alpha, 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    fn rgb8(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub color: Rgba,
    /// Where on the gradient, 0..1 (CSS allows past either end: 1.2).
    pub at: f32,
}

/// One layer of a wallpaper.
#[derive(Debug, Clone, PartialEq)]
pub enum Layer {
    Color(Rgba),
    /// CSS's angle (0deg up, 90deg right) and its stops.
    Linear {
        deg: f32,
        stops: Vec<Stop>,
    },
    /// An ellipse, all fractions of the picture: radii, then centre.
    Radial {
        rx: f32,
        ry: f32,
        cx: f32,
        cy: f32,
        stops: Vec<Stop>,
    },
}

impl Layer {
    /// Where a linear gradient's line starts and ends on a `w`×`h` picture
    /// (CSS: through the middle, long enough to reach the corners).
    pub fn line(deg: f32, w: f32, h: f32) -> ((f32, f32), (f32, f32)) {
        let a = deg.to_radians();
        let len = (w * a.sin()).abs() + (h * a.cos()).abs();
        let (dx, dy) = (a.sin() * len / 2.0, -a.cos() * len / 2.0);
        ((w / 2.0 - dx, h / 2.0 - dy), (w / 2.0 + dx, h / 2.0 + dy))
    }
}

/// A gradient wallpaper's layers, bottom first (CSS lists the top one first).
pub fn layers(css: &str) -> Vec<Layer> {
    split_top(css).iter().rev().filter_map(|l| layer(l)).collect()
}

/// One layer: a gradient or a colour; None if it isn't one this reads.
pub fn layer(css: &str) -> Option<Layer> {
    let css = css.trim();
    let gradient =
        |name: &str| css.strip_prefix(name).and_then(|r| r.strip_prefix('(')).and_then(|r| r.strip_suffix(')'));
    if let Some(inner) = gradient("linear-gradient") {
        let mut args = split_top(inner);
        let mut deg = 180.0;
        if let Some(first) = args.first() {
            if let Some(d) = first.strip_suffix("deg").and_then(|d| d.trim().parse::<f32>().ok()) {
                deg = d;
                args.remove(0);
            } else if first.starts_with("to ") {
                deg = match first.as_str() {
                    "to top" => 0.0,
                    "to right" => 90.0,
                    "to left" => 270.0,
                    _ => 180.0,
                };
                args.remove(0);
            }
        }
        return Some(Layer::Linear { deg, stops: stops(&args)? });
    }
    if let Some(inner) = gradient("radial-gradient") {
        let mut args = split_top(inner);
        let (mut rx, mut ry, mut cx, mut cy) = (0.5, 0.5, 0.5, 0.5);
        if let Some(shape) = args.first().and_then(|a| radial_shape(a)) {
            [rx, ry, cx, cy] = shape;
            args.remove(0);
        }
        return Some(Layer::Radial { rx, ry, cx, cy, stops: stops(&args)? });
    }
    color(css).map(Layer::Color)
}

/// Splits on commas that aren't inside parentheses.
pub fn split_top(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for ch in s.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if ch == ',' && depth == 0 {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// "#ff3d8155 0%" stops; ones without a position spread evenly, like CSS;
/// `transparent` fades in its neighbour's colour (not through black).
pub fn stops(parts: &[String]) -> Option<Vec<Stop>> {
    let mut read: Vec<(String, Option<f32>)> = parts
        .iter()
        .map(|p| match p.rsplit_once(char::is_whitespace) {
            Some((c, at)) if at.ends_with('%') && at[..at.len() - 1].parse::<f32>().is_ok() => {
                (c.trim().to_string(), Some(at[..at.len() - 1].parse::<f32>().unwrap() / 100.0))
            }
            _ => (p.trim().to_string(), None),
        })
        .collect();
    if read.is_empty() {
        return None;
    }
    if read[0].1.is_none() {
        read[0].1 = Some(0.0);
    }
    let last = read.len() - 1;
    if read[last].1.is_none() {
        read[last].1 = Some(1.0);
    }
    let mut i = 1;
    while i < last {
        if read[i].1.is_some() {
            i += 1;
            continue;
        }
        let mut j = i;
        while read[j].1.is_none() {
            j += 1;
        }
        let (a, b) = (read[i - 1].1.unwrap(), read[j].1.unwrap());
        for (n, stop) in read[i..j].iter_mut().enumerate() {
            stop.1 = Some(a + (b - a) * (n + 1) as f32 / (j - i + 1) as f32);
        }
        i = j;
    }
    // A transparent stop: the colour before it, else after, at no opacity.
    let colors: Vec<Option<Rgba>> = read.iter().map(|(c, _)| if is_transparent(c) { None } else { color(c) }).collect();
    let mut out = Vec::with_capacity(read.len());
    for (i, (c, at)) in read.iter().enumerate() {
        let color = if is_transparent(c) {
            let near = colors[..i].iter().rev().chain(colors[i + 1..].iter()).find_map(|c| *c);
            near.map(|n| Rgba { a: 0.0, ..n }).unwrap_or(Rgba::new(0.0, 0.0, 0.0, 0.0))
        } else {
            colors[i]?
        };
        out.push(Stop { color, at: at.unwrap() });
    }
    Some(out)
}

fn is_transparent(c: &str) -> bool {
    c.trim().eq_ignore_ascii_case("transparent")
}

/// A radial gradient's "RX% RY% at X% Y%", as fractions: [rx, ry, cx, cy].
pub fn radial_shape(arg: &str) -> Option<[f32; 4]> {
    let words: Vec<&str> = arg.split_whitespace().collect();
    let [rx, ry, "at", cx, cy] = words.as_slice() else { return None };
    let pct = |s: &str| s.strip_suffix('%')?.parse::<f32>().ok().map(|v| v / 100.0);
    Some([pct(rx)?, pct(ry)?, pct(cx)?, pct(cy)?])
}

/// A CSS colour: #rgb(a), #rrggbb(aa), rgb()/rgba() (commas or spaces,
/// "/ alpha"), `transparent`, or a few names.
pub fn color(c: &str) -> Option<Rgba> {
    let c = c.trim().to_ascii_lowercase();
    if let Some(hex) = c.strip_prefix('#') {
        if !hex.chars().all(|x| x.is_ascii_hexdigit()) {
            return None;
        }
        let full: String = match hex.len() {
            3 | 4 => hex.chars().flat_map(|x| [x, x]).collect(),
            6 | 8 => hex.to_string(),
            _ => return None,
        };
        let byte = |i: usize| u8::from_str_radix(&full[i..i + 2], 16).ok();
        let a = if full.len() == 8 { byte(6)? as f32 / 255.0 } else { 1.0 };
        return Some(Rgba::rgb8(byte(0)?, byte(2)?, byte(4)?, a));
    }
    if let Some(args) = c.strip_prefix("rgba(").or_else(|| c.strip_prefix("rgb(")).and_then(|r| r.strip_suffix(')')) {
        let (rgb, slash) = match args.split_once('/') {
            Some((rgb, a)) => (rgb, Some(a)),
            None => (args, None),
        };
        let nums: Vec<&str> = rgb.split(|ch: char| ch == ',' || ch.is_whitespace()).filter(|x| !x.is_empty()).collect();
        let channel = |s: &str| s.parse::<f32>().ok().map(|v| v.clamp(0.0, 255.0) / 255.0);
        let alpha = |s: &str| {
            let s = s.trim();
            match s.strip_suffix('%') {
                Some(p) => p.parse::<f32>().ok().map(|v| v / 100.0),
                None => s.parse::<f32>().ok(),
            }
        };
        let a = match (slash, nums.get(3)) {
            (Some(a), _) => alpha(a)?,
            (None, Some(a)) => alpha(a)?,
            _ => 1.0,
        };
        return Some(Rgba::new(
            channel(nums.first()?)?,
            channel(nums.get(1)?)?,
            channel(nums.get(2)?)?,
            a.clamp(0.0, 1.0),
        ));
    }
    match c.as_str() {
        "transparent" => Some(Rgba::new(0.0, 0.0, 0.0, 0.0)),
        "black" => Some(Rgba::new(0.0, 0.0, 0.0, 1.0)),
        "white" => Some(Rgba::new(1.0, 1.0, 1.0, 1.0)),
        "red" => Some(Rgba::rgb8(255, 0, 0, 1.0)),
        "green" => Some(Rgba::rgb8(0, 128, 0, 1.0)),
        "blue" => Some(Rgba::rgb8(0, 0, 255, 1.0)),
        _ => None,
    }
}

/// The Builder's choices (the client's components/desktop/wallpapers.ts).
pub const PRESETS: &[(&str, &str)] = &[
    (
        "Aurora",
        "radial-gradient(120% 90% at 12% 8%, rgb(198 255 31 / 0.38), transparent 55%), radial-gradient(110% 85% at 88% 92%, rgb(255 61 129 / 0.5), transparent 60%), linear-gradient(160deg, #0a0614, #1b132d)",
    ),
    (
        "Nebula",
        "radial-gradient(75% 70% at 72% 30%, rgb(255 61 129 / 0.55), transparent 62%), radial-gradient(90% 90% at 18% 82%, rgb(127 211 255 / 0.38), transparent 62%), linear-gradient(200deg, #140d22, #07040e)",
    ),
    (
        "Acid Dusk",
        "radial-gradient(120% 75% at 50% 108%, rgb(198 255 31 / 0.5), transparent 62%), radial-gradient(90% 60% at 50% 100%, rgb(59 29 94 / 0.8), transparent 75%), linear-gradient(175deg, #0a0614 0%, #1b132d 60%, #2a1747 100%)",
    ),
    (
        "Neon Tide",
        "radial-gradient(80% 65% at 50% 112%, rgb(127 211 255 / 0.6), transparent 70%), radial-gradient(60% 50% at 85% 10%, rgb(255 61 129 / 0.25), transparent 70%), linear-gradient(180deg, #0a0614, #142640)",
    ),
    (
        "Ember",
        "radial-gradient(80% 80% at 92% 8%, rgb(255 61 129 / 0.7), transparent 60%), radial-gradient(60% 60% at 8% 100%, rgb(255 179 107 / 0.32), transparent 65%), linear-gradient(135deg, #1d0a1c, #0a0614)",
    ),
    (
        "Glacier",
        "radial-gradient(90% 75% at 28% 18%, rgb(127 211 255 / 0.48), transparent 60%), radial-gradient(70% 70% at 90% 95%, rgb(198 255 31 / 0.14), transparent 65%), linear-gradient(160deg, #0d1a2c, #07040e)",
    ),
    (
        "Midnight",
        "radial-gradient(120% 80% at 50% -5%, rgb(120 64 210 / 0.6), transparent 65%), radial-gradient(50% 40% at 50% 100%, rgb(255 61 129 / 0.18), transparent 70%), linear-gradient(180deg, #140d22, #05030a)",
    ),
    (
        "Prism",
        "radial-gradient(60% 70% at 18% 28%, rgb(255 61 129 / 0.5), transparent 65%), radial-gradient(60% 70% at 82% 22%, rgb(127 211 255 / 0.45), transparent 65%), radial-gradient(75% 70% at 55% 100%, rgb(198 255 31 / 0.34), transparent 65%), #0e0919",
    ),
    (
        "Haze",
        "radial-gradient(100% 100% at 0% 0%, rgb(198 255 31 / 0.26), transparent 55%), radial-gradient(100% 100% at 100% 100%, rgb(127 211 255 / 0.3), transparent 55%), radial-gradient(70% 70% at 60% 40%, rgb(255 61 129 / 0.26), transparent 60%), #0e0919",
    ),
];

/// A new customization's background until one's picked.
pub const BLANK: &str = "linear-gradient(160deg, #17112a 0%, #0e0919 100%)";

#[cfg(test)]
mod tests;
