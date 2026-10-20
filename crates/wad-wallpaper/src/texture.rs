//! The Builder's backgrounds: textures, each its own material or pattern,
//! drawn here (SVG noise and patterns) so there's nothing to upload. Each
//! comes in black (the dark theme) and white (the light theme), with greys
//! between and no other colour. A port of the design spike's textures
//! (../ui-mockup-pieces/src/data/textures.ts): the same seeds, fields and
//! tiles, so they look as the spike drew them.
//!
//! [`svg`] writes a texture as an SVG of any size; with the raster feature,
//! [`png`] draws it (resvg: its filters make the noise).

use std::f64::consts::PI;
use std::fmt::Write;

/// Each texture's id and name, in the Builder's order.
pub const TEXTURES: &[(&str, &str)] = &[
    ("carbon", "Carbon"),
    ("marble", "Marble"),
    ("wood", "Woodgrain"),
    ("contour", "Contour"),
    ("grid", "Grid"),
    ("terrazzo", "Terrazzo"),
    ("honeycomb", "Honeycomb"),
    ("seigaiha", "Seigaiha"),
    ("brushed", "Brushed metal"),
    ("camo", "Camo"),
    ("stars", "Stars"),
    ("halftone", "Halftone"),
];

/// Black for the dark theme, white for the light one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Dark,
    Light,
}

impl Tone {
    fn dark(self) -> bool {
        self == Tone::Dark
    }
    fn bg(self) -> &'static str {
        if self.dark() { "#0c0c0c" } else { "#f7f7f7" }
    }
    fn ink(self) -> &'static str {
        if self.dark() { "#f2f2f2" } else { "#161616" }
    }
}

/// A colour and its opacity.
#[derive(Debug, Clone, Copy)]
struct C(&'static str, f64);

const CLEAR: f64 = 0.0;

/// One layer, as CSS would have it (the first is on top).
enum Layer {
    Fill(C),
    /// A repeating tile (an SVG body) w×h, its grid starting at (x, y).
    Tile {
        w: f64,
        h: f64,
        x: f64,
        y: f64,
        body: String,
    },
    /// Noise over the whole picture (no tiles, so no seams).
    Whole(String),
    /// A linear gradient across everything (CSS degrees); stops 0..1.
    Linear {
        deg: f64,
        stops: Vec<(C, f64)>,
    },
    /// A radial gradient at (cx, cy) (0..1 of the size): a circle to the
    /// farthest corner, or an ellipse of rx×ry (0..1 of the size); stops 0..1.
    Radial {
        cx: f64,
        cy: f64,
        ellipse: Option<(f64, f64)>,
        stops: Vec<(C, f64)>,
    },
    /// A linear gradient in a repeating w×h tile from (x, y); stops in px.
    TileLinear {
        deg: f64,
        w: f64,
        h: f64,
        x: f64,
        y: f64,
        stops: Vec<(C, f64)>,
    },
    /// A dot in the middle of a repeating w×h tile from (x, y): solid to
    /// `solid` px, clear at `clear` px.
    TileDot {
        w: f64,
        h: f64,
        x: f64,
        y: f64,
        color: C,
        solid: f64,
        clear: f64,
    },
}

fn stops(out: &mut String, stops: &[(C, f64)], scale: f64) {
    for (C(c, a), at) in stops {
        let _ = write!(out, "<stop offset='{:.4}' stop-color='{c}' stop-opacity='{a}'/>", (at / scale).clamp(0.0, 1.0));
    }
}

/// CSS's gradient line for `deg` in a w×h box: its two ends, and its length.
fn line(deg: f64, w: f64, h: f64) -> ((f64, f64), (f64, f64), f64) {
    let r = deg.to_radians();
    let (dx, dy) = (r.sin(), -r.cos());
    let len = (w * r.sin()).abs() + (h * r.cos()).abs();
    let (cx, cy) = (w / 2.0, h / 2.0);
    ((cx - dx * len / 2.0, cy - dy * len / 2.0), (cx + dx * len / 2.0, cy + dy * len / 2.0), len)
}

fn linear_def(out: &mut String, id: &str, deg: f64, w: f64, h: f64, st: &[(C, f64)], scale: f64) {
    let ((x1, y1), (x2, y2), _) = line(deg, w, h);
    let _ = write!(
        out,
        "<linearGradient id='{id}' gradientUnits='userSpaceOnUse' x1='{x1:.3}' y1='{y1:.3}' x2='{x2:.3}' y2='{y2:.3}'>"
    );
    stops(out, st, scale);
    out.push_str("</linearGradient>");
}

fn rect(out: &mut String, fill: &str) {
    let _ = write!(out, "<rect width='100%' height='100%' fill='{fill}'/>");
}

/// The layers as an SVG of w×h.
fn document(w: u32, h: u32, layers: Vec<Layer>) -> String {
    let (fw, fh) = (w as f64, h as f64);
    let (mut defs, mut body) = (String::new(), String::new());
    for (i, l) in layers.into_iter().rev().enumerate() {
        let id = format!("l{i}");
        match l {
            Layer::Fill(C(c, a)) => {
                let _ = write!(body, "<rect width='100%' height='100%' fill='{c}' fill-opacity='{a}'/>");
            }
            Layer::Whole(noise) => {
                body.push_str(
                    &noise.replace("id='f'", &format!("id='{id}f'")).replace("url(#f)", &format!("url(#{id}f)")),
                );
            }
            Layer::Tile { w, h, x, y, body: tile } => {
                let tile = tile.replace("id='f'", &format!("id='{id}f'")).replace("url(#f)", &format!("url(#{id}f)"));
                let _ = write!(
                    defs,
                    "<pattern id='{id}' patternUnits='userSpaceOnUse' x='{x}' y='{y}' width='{w}' height='{h}'>{tile}</pattern>"
                );
                rect(&mut body, &format!("url(#{id})"));
            }
            Layer::Linear { deg, stops: st } => {
                linear_def(&mut defs, &id, deg, fw, fh, &st, 1.0);
                rect(&mut body, &format!("url(#{id})"));
            }
            Layer::Radial { cx, cy, ellipse, stops: st } => {
                let (px, py) = (cx * fw, cy * fh);
                let (rx, ry) = match ellipse {
                    Some((ex, ey)) => (ex * fw, ey * fh),
                    None => {
                        let r = [(0.0, 0.0), (fw, 0.0), (0.0, fh), (fw, fh)]
                            .iter()
                            .map(|(x, y)| ((x - px).powi(2) + (y - py).powi(2)).sqrt())
                            .fold(0.0, f64::max);
                        (r, r)
                    }
                };
                let _ = write!(
                    defs,
                    "<radialGradient id='{id}' gradientUnits='userSpaceOnUse' cx='0' cy='0' r='1' gradientTransform='translate({px:.2} {py:.2}) scale({rx:.2} {ry:.2})'>"
                );
                stops(&mut defs, &st, 1.0);
                defs.push_str("</radialGradient>");
                rect(&mut body, &format!("url(#{id})"));
            }
            Layer::TileLinear { deg, w, h, x, y, stops: st } => {
                let (_, _, len) = line(deg, w, h);
                linear_def(&mut defs, &format!("{id}g"), deg, w, h, &st, len);
                let _ = write!(
                    defs,
                    "<pattern id='{id}' patternUnits='userSpaceOnUse' x='{x}' y='{y}' width='{w}' height='{h}'><rect width='{w}' height='{h}' fill='url(#{id}g)'/></pattern>"
                );
                rect(&mut body, &format!("url(#{id})"));
            }
            Layer::TileDot { w, h, x, y, color: C(c, a), solid, clear } => {
                let _ = write!(
                    defs,
                    "<radialGradient id='{id}g' gradientUnits='userSpaceOnUse' cx='{}' cy='{}' r='{clear}'>\
                     <stop offset='0' stop-color='{c}' stop-opacity='{a}'/><stop offset='{:.4}' stop-color='{c}' stop-opacity='{a}'/>\
                     <stop offset='1' stop-color='{c}' stop-opacity='0'/></radialGradient>\
                     <pattern id='{id}' patternUnits='userSpaceOnUse' x='{x}' y='{y}' width='{w}' height='{h}'><rect width='{w}' height='{h}' fill='url(#{id}g)'/></pattern>",
                    w / 2.0,
                    h / 2.0,
                    solid / clear
                );
                rect(&mut body, &format!("url(#{id})"));
            }
        }
    }
    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}' viewBox='0 0 {w} {h}'><defs>{defs}</defs>{body}</svg>"
    )
}

/// A noise filter's output over a whole tile.
fn noise(filter: &str) -> String {
    format!(
        "<filter id='f' x='0' y='0' width='100%' height='100%' color-interpolation-filters='sRGB'>{filter}</filter><rect width='100%' height='100%' filter='url(#f)'/>"
    )
}

/// A soft light (or shade) from one corner, over the ground.
fn ground(t: Tone) -> Layer {
    let (a, b) = if t.dark() { ("#202020", "#0b0b0b") } else { ("#ffffff", "#e9e9e9") };
    Layer::Radial { cx: 0.3, cy: 0.2, ellipse: None, stops: vec![(C(a, 1.0), 0.0), (C(b, 1.0), 0.75)] }
}

/// The same pseudo-random numbers every time (the spike's mulberry32).
fn rng(seed: u32) -> impl FnMut() -> f64 {
    let mut s = seed;
    move || {
        s = s.wrapping_add(0x6d2b_79f5);
        let mut t = (s ^ (s >> 15)).wrapping_mul(1 | s);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }
}

/// A smooth field that repeats every `size` px both ways: a few waves.
fn waves(seed: u32, size: f64, count: usize, most: i32) -> impl Fn(f64, f64) -> f64 {
    let mut r = rng(seed);
    let ws: Vec<(f64, f64, f64, f64)> = (0..count)
        .map(|_| {
            let mut i = (r() * (2 * most + 1) as f64).floor() as i32 - most;
            let j = (r() * (most + 1) as f64).floor() as i32;
            if i == 0 && j == 0 {
                i = 1;
            }
            let (i, j) = (i as f64, j as f64);
            (i, j, 1.0 / i.hypot(j), r() * PI * 2.0)
        })
        .collect();
    move |x, y| ws.iter().map(|(i, j, a, ph)| a * ((2.0 * PI * (i * x + j * y)) / size + ph).sin()).sum()
}

/// `f` pushed around by two other fields: wandering, organic lines.
fn warped(
    f: impl Fn(f64, f64) -> f64,
    g: impl Fn(f64, f64) -> f64,
    h: impl Fn(f64, f64) -> f64,
    k: f64,
) -> impl Fn(f64, f64) -> f64 {
    move |x, y| f(x + k * g(x, y), y + k * h(x, y))
}

/// Where `f` crosses each level, as an SVG path (marching squares).
fn isolines(f: impl Fn(f64, f64) -> f64, size: f64, n: usize, levels: &[f64]) -> String {
    let c = size / n as f64;
    let v: Vec<Vec<f64>> = (0..=n).map(|j| (0..=n).map(|i| f(i as f64 * c, j as f64 * c)).collect()).collect();
    let mut d = String::new();
    for &level in levels {
        for j in 0..n {
            for i in 0..n {
                let (x0, y0) = (i as f64 * c, j as f64 * c);
                let (x1, y1) = (x0 + c, y0 + c);
                let corners =
                    [(v[j][i], x0, y0), (v[j][i + 1], x1, y0), (v[j + 1][i + 1], x1, y1), (v[j + 1][i], x0, y1)];
                let mut hits = vec![];
                for e in 0..4 {
                    let (a, ax, ay) = corners[e];
                    let (b, bx, by) = corners[(e + 1) % 4];
                    if (a < level) != (b < level) {
                        let t = (level - a) / (b - a);
                        hits.push((ax + (bx - ax) * t, ay + (by - ay) * t));
                    }
                }
                for k in (0..hits.len().saturating_sub(1)).step_by(2) {
                    let ((ax, ay), (bx, by)) = (hits[k], hits[k + 1]);
                    let _ = write!(d, "M{ax:.1} {ay:.1}L{bx:.1} {by:.1}");
                }
            }
        }
    }
    d
}

/// Twill-woven carbon fibre.
fn carbon(t: Tone) -> Vec<Layer> {
    let [a, b, c, d, e, f, g] = if t.dark() {
        ["#151515", "#262626", "#1b1b1b", "#1d1d1d", "#1a1a1a", "#262626", "#131313"]
    } else {
        ["#cfcfcf", "#ececec", "#dadada", "#e0e0e0", "#d6d6d6", "#f1f1f1", "#d2d2d2"]
    };
    let hard = |col: &'static str, px: f64| vec![(C(col, 1.0), 0.0), (C(col, 1.0), px), (C(col, CLEAR), px)];
    let tl = |deg, x, y, stops| Layer::TileLinear { deg, w: 20.0, h: 20.0, x, y, stops };
    vec![
        tl(27.0, 0.0, 5.0, hard(a, 5.0)),
        tl(207.0, 10.0, 0.0, hard(a, 5.0)),
        tl(27.0, 0.0, 10.0, hard(b, 5.0)),
        tl(207.0, 10.0, 5.0, hard(b, 5.0)),
        tl(90.0, 0.0, 0.0, hard(c, 10.0)),
        tl(
            180.0,
            0.0,
            0.0,
            vec![
                (C(d, 1.0), 0.0),
                (C(d, 1.0), 5.0),
                (C(e, 1.0), 5.0),
                (C(e, 1.0), 10.0),
                (C(e, CLEAR), 10.0),
                (C(f, CLEAR), 15.0),
                (C(f, 1.0), 15.0),
                (C(f, 1.0), 20.0),
            ],
        ),
        Layer::Fill(C(g, 1.0)),
    ]
}

/// Polished stone: veins over soft clouding. The veins repeat every 1260
/// px (the spike's 420, three times over, with its waves' sizes), so a
/// desktop shows no pattern.
fn marble(t: Tone) -> Vec<Layer> {
    const S: f64 = 1260.0;
    let veins = isolines(warped(waves(14, S, 6, 6), waves(15, S, 5, 9), waves(16, S, 5, 9), 38.0), S, 270, &[0.0]);
    let hair = isolines(warped(waves(24, S, 7, 12), waves(25, S, 5, 9), waves(26, S, 5, 9), 26.0), S, 270, &[0.4]);
    let k = if t.dark() { 0.8 } else { 0.35 };
    let alpha = if t.dark() { "0.9 0 0 0 -0.38" } else { "0.7 0 0 0 -0.3" };
    let cloud = noise(&format!(
        "<feTurbulence type='fractalNoise' baseFrequency='0.006' numOctaves='4' seed='8' stitchTiles='stitch'/>\
         <feColorMatrix type='matrix' values='0 0 0 0 {k}  0 0 0 0 {k}  0 0 0 0 {k}  {alpha}'/>"
    ));
    let ink = t.ink();
    let lines = format!(
        "<g fill='none' stroke='{ink}' stroke-linecap='round'><path d='{veins}' stroke-width='5' stroke-opacity='0.08'/>\
         <path d='{veins}' stroke-width='1.3' stroke-opacity='{}'/><path d='{hair}' stroke-width='0.7' stroke-opacity='0.3'/></g>",
        if t.dark() { 0.8 } else { 0.6 }
    );
    vec![Layer::Tile { w: S, h: S, x: 0.0, y: 0.0, body: lines }, Layer::Whole(cloud), ground(t)]
}

/// Wood, its grain running across.
fn wood(t: Tone) -> Vec<Layer> {
    let (k, o) = if t.dark() { (0.3, 0.05) } else { (0.24, 0.7) };
    let grain = noise(&format!(
        "<feTurbulence type='fractalNoise' baseFrequency='0.0035 0.11' numOctaves='4' seed='3' stitchTiles='stitch'/>\
         <feColorMatrix type='matrix' values='{k} 0 0 0 {o}  {k} 0 0 0 {o}  {k} 0 0 0 {o}  0 0 0 0 1'/>"
    ));
    let shade = if t.dark() { 0.25 } else { 0.08 };
    vec![
        Layer::Linear { deg: 180.0, stops: vec![(C("#ffffff", 0.05), 0.0), (C("#000000", shade), 1.0)] },
        Layer::Whole(grain),
    ]
}

/// A topographic map's contour lines, every fourth one bolder; repeating
/// every 1200 px (the spike's 400, three times over).
fn contour(t: Tone) -> Vec<Layer> {
    const S: f64 = 1200.0;
    let f = warped(waves(9, S, 7, 6), waves(10, S, 4, 6), waves(11, S, 4, 6), 22.0);
    let levels: Vec<f64> = (0..17).map(|i| -3.2 + i as f64 * 0.4).collect();
    let thin: Vec<f64> = levels.iter().enumerate().filter(|(i, _)| i % 4 != 0).map(|(_, l)| *l).collect();
    let bold: Vec<f64> = levels.iter().enumerate().filter(|(i, _)| i % 4 == 0).map(|(_, l)| *l).collect();
    let (thin, bold) = (isolines(&f, S, 210, &thin), isolines(&f, S, 210, &bold));
    let ink = t.ink();
    let body = format!(
        "<g fill='none' stroke='{ink}' stroke-linecap='round'><path d='{thin}' stroke-width='0.9' stroke-opacity='0.35'/>\
         <path d='{bold}' stroke-width='1.7' stroke-opacity='0.7'/></g>"
    );
    vec![Layer::Tile { w: S, h: S, x: 0.0, y: 0.0, body }, ground(t)]
}

/// Graph paper: a fine grid inside a bold one.
fn grid(t: Tone) -> Vec<Layer> {
    let line = |a: f64| if t.dark() { C("#ffffff", a) } else { C("#000000", a * 0.8) };
    let lines = |deg, size, a| {
        let c = line(a);
        Layer::TileLinear {
            deg,
            w: size,
            h: size,
            x: -1.0,
            y: -1.0,
            stops: vec![(c, 0.0), (c, 1.0), (C(c.0, CLEAR), 1.0)],
        }
    };
    vec![lines(180.0, 96.0, 0.2), lines(90.0, 96.0, 0.2), lines(180.0, 16.0, 0.07), lines(90.0, 16.0, 0.07), ground(t)]
}

/// Stone chips set in resin.
fn terrazzo(t: Tone) -> Vec<Layer> {
    let mut r = rng(7);
    let shades = if t.dark() {
        ["#f2f2f2", "#c4c4c4", "#8c8c8c", "#5c5c5c", "#f2f2f2"]
    } else {
        ["#141414", "#3c3c3c", "#6e6e6e", "#a4a4a4", "#141414"]
    };
    let mut body = String::new();
    for _ in 0..340 {
        let (cx, cy) = (4.0 + r() * 272.0, 4.0 + r() * 272.0);
        let size = 1.4 + r().powf(2.4) * 8.0;
        let sides = 3 + (r() * 4.0).floor() as usize;
        let turn = r() * PI;
        let pts: Vec<String> = (0..sides)
            .map(|k| {
                let a = turn + (k as f64 / sides as f64) * PI * 2.0;
                let d = size * (0.6 + r() * 0.5);
                format!("{:.1},{:.1}", cx + a.cos() * d, cy + a.sin() * d)
            })
            .collect();
        let shade = shades[(r() * 5.0).floor() as usize];
        let alpha = 0.55 + r() * 0.35;
        let _ = write!(body, "<polygon points='{}' fill='{shade}' fill-opacity='{alpha:.2}'/>", pts.join(" "));
    }
    let (a, b) = if t.dark() { ("#2c2c2c", "#171717") } else { ("#fbfbfb", "#e2e2e2") };
    vec![
        Layer::Tile { w: 280.0, h: 280.0, x: 0.0, y: 0.0, body },
        Layer::Radial { cx: 0.4, cy: 0.3, ellipse: None, stops: vec![(C(a, 1.0), 0.0), (C(b, 1.0), 0.8)] },
    ]
}

/// Honeycomb: pointy hexagons.
fn honeycomb(t: Tone) -> Vec<Layer> {
    const R: f64 = 15.0;
    let (w, h) = ((3f64.sqrt() * R * 1000.0).round() / 1000.0, 3.0 * R);
    let hex = |cx: f64, cy: f64| {
        let pts: Vec<String> = (0..6)
            .map(|k| {
                let a = ((60 * k - 90) as f64).to_radians();
                format!("{:.2},{:.2}", cx + a.cos() * (R - 1.2), cy + a.sin() * (R - 1.2))
            })
            .collect();
        format!("<polygon points='{}'/>", pts.join(" "))
    };
    let cells = [hex(0.0, 0.0), hex(w, 0.0), hex(w / 2.0, h / 2.0), hex(0.0, h), hex(w, h)].concat();
    let ink = t.ink();
    let line = if t.dark() { 0.4 } else { 0.45 };
    let body = format!(
        "<g fill='{ink}' fill-opacity='0.04' stroke='{ink}' stroke-opacity='{line}' stroke-width='1.3'>{cells}</g>"
    );
    vec![Layer::Tile { w, h, x: 0.0, y: 0.0, body }, ground(t)]
}

/// Seigaiha: overlapping waves of rings.
fn seigaiha(t: Tone) -> Vec<Layer> {
    const R: f64 = 22.0;
    let mut body = String::new();
    let fill = if t.dark() { "#121212" } else { "#f6f6f6" };
    let ink = t.ink();
    let mut y = -R;
    while y <= 2.0 * R {
        let xs: &[f64] = if (y / (R / 2.0)).round() as i64 % 2 == 0 { &[0.0, 2.0 * R] } else { &[R] };
        for x in xs {
            let _ = write!(body, "<circle cx='{x}' cy='{y}' r='{R}' fill='{fill}'/>");
            for k in [0.86, 0.66, 0.46, 0.26] {
                let _ = write!(
                    body,
                    "<circle cx='{x}' cy='{y}' r='{:.1}' fill='none' stroke='{ink}' stroke-opacity='{:.2}' stroke-width='1.6'/>",
                    R * k,
                    0.5 - k * 0.25
                );
            }
        }
        y += R / 2.0;
    }
    let shade = if t.dark() { 0.45 } else { 0.06 };
    vec![
        Layer::Linear { deg: 180.0, stops: vec![(C("#000000", CLEAR), 0.0), (C("#000000", shade), 1.0)] },
        Layer::Tile { w: 2.0 * R, h: R, x: 0.0, y: 0.0, body },
    ]
}

/// Brushed metal: fine streaks across.
fn brushed(t: Tone) -> Vec<Layer> {
    let k = if t.dark() { 1 } else { 0 };
    let alpha = if t.dark() { "0.5 0 0 0 -0.16" } else { "0.45 0 0 0 -0.17" };
    let streaks = noise(&format!(
        "<feTurbulence type='fractalNoise' baseFrequency='0.0025 0.75' numOctaves='2' seed='5' stitchTiles='stitch'/>\
         <feColorMatrix type='matrix' values='0 0 0 0 {k}  0 0 0 0 {k}  0 0 0 0 {k}  {alpha}'/>"
    ));
    let [a, b, c, d] = if t.dark() {
        ["#3c3c3c", "#1c1c1c", "#363636", "#1a1a1a"]
    } else {
        ["#f4f4f4", "#cfcfcf", "#ececec", "#c9c9c9"]
    };
    vec![
        Layer::Whole(streaks),
        Layer::Linear {
            deg: 125.0,
            stops: vec![(C(a, 1.0), 0.0), (C(b, 1.0), 0.45), (C(c, 1.0), 0.7), (C(d, 1.0), 1.0)],
        },
    ]
}

/// Camouflage in flat patches: urban greys, or snow.
fn camo(t: Tone) -> Vec<Layer> {
    let steps = if t.dark() { "0.07 0.07 0.13 0.22 0.17 0.1 0.1" } else { "0.96 0.96 0.86 0.7 0.79 0.91 0.91" };
    let body = noise(&format!(
        "<feTurbulence type='fractalNoise' baseFrequency='0.011' numOctaves='3' seed='21' stitchTiles='stitch'/>\
         <feColorMatrix type='matrix' values='1 0 0 0 0  1 0 0 0 0  1 0 0 0 0  0 0 0 0 1'/>\
         <feComponentTransfer><feFuncR type='discrete' tableValues='{steps}'/><feFuncG type='discrete' tableValues='{steps}'/>\
         <feFuncB type='discrete' tableValues='{steps}'/></feComponentTransfer>"
    ));
    vec![Layer::Whole(body)]
}

/// Stars, a few of them bright: or ink specks on paper.
fn stars(t: Tone) -> Vec<Layer> {
    let mut r = rng(42);
    let ink = t.ink();
    let mut body = String::new();
    for _ in 0..70 {
        let big = r() > 0.93;
        let (x, y) = (r() * 300.0, r() * 300.0);
        let (size, alpha) = if big { (1.6, 1.0) } else { (0.4 + r() * 0.7, 0.35 + r() * 0.5) };
        if big {
            let _ = write!(body, "<circle cx='{x:.1}' cy='{y:.1}' r='4.5' fill='{ink}' fill-opacity='0.12'/>");
        }
        let _ = write!(body, "<circle cx='{x:.1}' cy='{y:.1}' r='{size:.2}' fill='{ink}' fill-opacity='{alpha:.2}'/>");
    }
    let (glow, top, bottom) =
        if t.dark() { (C("#ffffff", 0.1), "#161616", "#050505") } else { (C("#000000", 0.05), "#ffffff", "#ececec") };
    vec![
        Layer::Tile { w: 300.0, h: 300.0, x: 0.0, y: 0.0, body },
        Layer::Radial {
            cx: 0.7,
            cy: 0.2,
            ellipse: Some((0.9, 0.6)),
            stops: vec![(glow, 0.0), (C(glow.0, CLEAR), 0.7)],
        },
        Layer::Linear { deg: 180.0, stops: vec![(C(top, 1.0), 0.0), (C(bottom, 1.0), 1.0)] },
    ]
}

/// Halftone dots, fading out from a corner.
fn halftone(t: Tone) -> Vec<Layer> {
    let (dot, bg) = if t.dark() { ("#f2f2f2", "#0c0c0c") } else { ("#161616", "#f7f7f7") };
    vec![
        Layer::Radial {
            cx: 0.85,
            cy: 0.12,
            ellipse: None,
            stops: vec![(C(bg, CLEAR), 0.0), (C(bg, 0.55), 0.4), (C(t.bg(), 1.0), 0.78)],
        },
        Layer::TileDot { w: 11.0, h: 11.0, x: 0.0, y: 0.0, color: C(dot, 0.9), solid: 2.0, clear: 2.6 },
        Layer::TileDot { w: 11.0, h: 11.0, x: 5.5, y: 5.5, color: C(dot, 0.5), solid: 1.1, clear: 1.7 },
        Layer::Fill(C(t.bg(), 1.0)),
    ]
}

/// A texture as an SVG of w×h; None if there's no such texture.
pub fn svg(id: &str, tone: Tone, w: u32, h: u32) -> Option<String> {
    let layers = match id {
        "carbon" => carbon(tone),
        "marble" => marble(tone),
        "wood" => wood(tone),
        "contour" => contour(tone),
        "grid" => grid(tone),
        "terrazzo" => terrazzo(tone),
        "honeycomb" => honeycomb(tone),
        "seigaiha" => seigaiha(tone),
        "brushed" => brushed(tone),
        "camo" => camo(tone),
        "stars" => stars(tone),
        "halftone" => halftone(tone),
        _ => return None,
    };
    Some(document(w, h, layers))
}

/// A texture drawn at w×h.
#[cfg(feature = "raster")]
pub fn pixmap(id: &str, tone: Tone, w: u32, h: u32) -> Option<resvg::tiny_skia::Pixmap> {
    let text = svg(id, tone, w, h)?;
    let tree = resvg::usvg::Tree::from_str(&text, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    Some(pixmap)
}

/// A texture drawn at w×h, as a PNG.
#[cfg(feature = "raster")]
pub fn png(id: &str, tone: Tone, w: u32, h: u32) -> Option<Vec<u8>> {
    pixmap(id, tone, w, h)?.encode_png().ok()
}

#[cfg(all(test, feature = "raster"))]
mod tests {
    use super::*;

    #[test]
    fn every_texture_draws_in_greys_dark_or_light() {
        for (id, _) in TEXTURES {
            for tone in [Tone::Dark, Tone::Light] {
                let pm = pixmap(id, tone, 320, 200).unwrap_or_else(|| panic!("{id} draws"));
                let px = pm.pixels();
                let mut sum = 0.0;
                let (mut lo, mut hi) = (255u8, 0u8);
                for p in px {
                    let c = p.demultiply();
                    let (r, g, b) = (c.red(), c.green(), c.blue());
                    assert!(r.abs_diff(g) <= 2 && g.abs_diff(b) <= 2, "{id} {tone:?}: a colour ({r}, {g}, {b})");
                    assert_eq!(c.alpha(), 255, "{id} {tone:?}: see-through");
                    sum += r as f64;
                    lo = lo.min(r);
                    hi = hi.max(r);
                }
                let mean = sum / px.len() as f64 / 255.0;
                match tone {
                    Tone::Dark => assert!(mean < 0.35, "{id} dark is too light: {mean:.2}"),
                    Tone::Light => assert!(mean > 0.6, "{id} light is too dark: {mean:.2}"),
                }
                assert!(hi - lo > 8, "{id} {tone:?}: nothing on it");
            }
        }
        assert!(svg("plaid", Tone::Dark, 10, 10).is_none());
    }

    /// `TEXTURES_OUT=<dir> cargo test -p wad-wallpaper full_size -- --ignored --nocapture`
    /// draws every texture at the desktop's size, and says how long each took.
    #[test]
    #[ignore = "writes PNGs; run on purpose"]
    fn full_size() {
        let Ok(out) = std::env::var("TEXTURES_OUT") else { return };
        for (id, _) in TEXTURES {
            let t = std::time::Instant::now();
            let png = png(id, Tone::Dark, 1920, 1080).unwrap();
            println!("{id}: {:?}, {} KB", t.elapsed(), png.len() / 1024);
            std::fs::write(format!("{out}/{id}-dark-full.png"), png).unwrap();
        }
    }

    #[test]
    fn the_same_texture_every_time() {
        assert_eq!(svg("terrazzo", Tone::Dark, 100, 100), svg("terrazzo", Tone::Dark, 100, 100));
        let mut r = rng(7);
        let first = r();
        assert!((0.0..1.0).contains(&first));
    }
}
