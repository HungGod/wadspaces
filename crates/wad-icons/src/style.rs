//! The look of a web app's icon: the site's favicon as a black silhouette on
//! a white rounded card (KaleBrowser's packager.py, ported), a text card for a
//! site with no icon, or a picture the user chose, as it is.
//!
//! The silhouette follows the packager step for step, Pillow's integer
//! arithmetic included, so icons look as they did; the tests hold it to the
//! packager's own output (fixtures/icons).

use image::{GrayImage, Luma, Rgba, RgbaImage};
use resvg::tiny_skia::{self, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

/// The icon: a 512 px card.
pub const SIZE: u32 = 512;
const CARD_RADIUS: f32 = 96.0;
const CARD_BORDER: f32 = 4.0;
const CARD_BORDER_COLOR: [u8; 4] = [225, 225, 225, 255];
/// The mark inside the card.
const INNER: u32 = 480;

/// `img`'s silhouette on the white card.
pub fn card(img: &RgbaImage) -> RgbaImage {
    let icon = letterbox(img, INNER);
    let ink = silhouette_alpha(&icon);
    let corners = rounded_mask(INNER, CARD_RADIUS * INNER as f32 / SIZE as f32);
    let mut out = card_canvas();
    let at = (SIZE - INNER) / 2;
    for (x, y, a) in ink.enumerate_pixels() {
        let a = chop_multiply(a[0], corners.get_pixel(x, y)[0]);
        if a > 0 {
            over(out.get_pixel_mut(x + at, y + at), [0, 0, 0, a]);
        }
    }
    out
}

/// The dark theme's card, in WadSpaces' colours: a white card (a site's
/// silhouette, or the text card) with its white made the dark violet and its
/// black the acid green, the greys between (the border, edges) mixed so;
/// what's see-through stays so. A picture the user chose isn't a card: it
/// stays as it is in both themes.
pub fn dark(card: &RgbaImage) -> RgbaImage {
    const CARD: [f64; 3] = [31.0, 21.0, 51.0]; // #1f1533
    const INK: [f64; 3] = [198.0, 255.0, 31.0]; // #c6ff1f
    RgbaImage::from_fn(card.width(), card.height(), |x, y| {
        let p = card.get_pixel(x, y);
        let luma = 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
        let t = 1.0 - luma / 255.0;
        let mix = |c: usize| (CARD[c] * (1.0 - t) + INK[c] * t).round() as u8;
        Rgba([mix(0), mix(1), mix(2), p[3]])
    })
}

/// A picture the user chose for the icon: as it is, fitted to the icon's size.
pub fn plain(img: &RgbaImage) -> RgbaImage {
    letterbox(img, SIZE)
}

/// A site's icon as it is (in colour), fitted to `size`: how WadSpaces Client
/// shows a web app. Dark ink on nothing (GitHub's mark) would vanish on a dark
/// desktop: it goes on a white rounded tile.
pub fn fit(img: &RgbaImage, size: u32) -> RgbaImage {
    if !dark_ink(img) {
        return letterbox(img, size);
    }
    let mut tile = Pixmap::new(size, size).expect("a tile pixmap");
    fill_rounded(&mut tile, 0.0, 0.0, size as f32, size as f32, size as f32 * 0.22, [255, 255, 255, 255]);
    let mut out = to_image(&tile);
    let inner = (size as f32 * 0.72).round() as u32;
    let at = (size - inner) / 2;
    for (x, y, p) in letterbox(img, inner).enumerate_pixels() {
        if p[3] > 0 {
            over(out.get_pixel_mut(x + at, y + at), p.0);
        }
    }
    out
}

/// Mostly dark where it's drawn, and see-through around it.
fn dark_ink(img: &RgbaImage) -> bool {
    let (mut drawn, mut light, mut clear) = (0u64, 0f64, 0u64);
    for p in img.pixels() {
        if p[3] < 16 {
            clear += 1;
        } else if p[3] >= 128 {
            drawn += 1;
            light += (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 255.0;
        }
    }
    let all = img.width() as u64 * img.height() as u64;
    drawn > 0 && clear * 10 >= all && light / (drawn as f64) < 0.25
}

/// A card for a site with no icon to use: `label` (its host) in white on a
/// black rounded square.
pub fn text_card(label: &str) -> RgbaImage {
    let mut pixmap = card_pixmap();
    let inset = (SIZE - INNER - 16) as f32;
    let side = SIZE as f32 - 2.0 * inset;
    fill_rounded(&mut pixmap, inset, inset, side, side, CARD_RADIUS * INNER as f32 / SIZE as f32, [0, 0, 0, 255]);
    let (max_w, max_h) = (side * 0.88, side * 0.30);
    let readable = |l: &str| crate::text::fit_px(l, max_w, max_h) >= crate::text::READABLE_PX;
    let label = shorten(label, readable);
    let mid = SIZE as f32 / 2.0;
    match label.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())) {
        // "localhost:8080" too long for one line: the port under the host.
        Some((host, port)) if !readable(&label) => {
            let line_h = side * 0.2;
            crate::text::draw_centered(&mut pixmap, host, mid, mid - line_h * 0.75, max_w, line_h);
            crate::text::draw_centered(&mut pixmap, &format!(":{port}"), mid, mid + line_h * 0.75, max_w, line_h);
        }
        _ => crate::text::draw_centered(&mut pixmap, &label, mid, mid, max_w, max_h),
    }
    to_image(&pixmap)
}

/// A host too long to read drops its leading names, down to the last two
/// ("console.cloud.google.com" → "google.com"); the port stays.
fn shorten(label: &str, fits: impl Fn(&str) -> bool) -> String {
    let (host, port) = match label.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (label, None),
    };
    let with_port = |h: &str| port.map_or_else(|| h.to_owned(), |p| format!("{h}:{p}"));
    if host.parse::<std::net::IpAddr>().is_ok() {
        return label.to_owned();
    }
    let mut parts: Vec<&str> = host.split('.').collect();
    while parts.len() > 2 && !fits(&with_port(&parts.join("."))) {
        parts.remove(0);
    }
    with_port(&parts.join("."))
}

// ---- the card ----

fn card_pixmap() -> Pixmap {
    let mut pixmap = Pixmap::new(SIZE, SIZE).expect("a 512 px pixmap");
    // Pillow's outline runs inside the box [2, 2, 509, 509]: border colour
    // there, then white inside it.
    let edge = CARD_BORDER / 2.0;
    let side = SIZE as f32 - CARD_BORDER;
    fill_rounded(&mut pixmap, edge, edge, side, side, CARD_RADIUS, CARD_BORDER_COLOR);
    fill_rounded(
        &mut pixmap,
        edge + CARD_BORDER,
        edge + CARD_BORDER,
        side - 2.0 * CARD_BORDER,
        side - 2.0 * CARD_BORDER,
        CARD_RADIUS - CARD_BORDER,
        [255, 255, 255, 255],
    );
    pixmap
}

fn card_canvas() -> RgbaImage {
    to_image(&card_pixmap())
}

fn fill_rounded(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, rgba: [u8; 4]) {
    let Some(path) = rounded_rect(x, y, w, h, r) else { return };
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    // The circle's control-point distance for cubic Béziers.
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    Rect::from_xywh(x, y, w, h)?;
    pb.finish()
}

/// Coverage of a `size`-square rounded rectangle (255 inside).
fn rounded_mask(size: u32, radius: f32) -> GrayImage {
    let mut pixmap = Pixmap::new(size, size).expect("a mask pixmap");
    fill_rounded(&mut pixmap, 0.0, 0.0, size as f32, size as f32, radius, [255, 255, 255, 255]);
    GrayImage::from_fn(size, size, |x, y| Luma([pixmap.pixel(x, y).map_or(0, |p| p.alpha())]))
}

pub(crate) fn to_image(pixmap: &Pixmap) -> RgbaImage {
    RgbaImage::from_fn(pixmap.width(), pixmap.height(), |x, y| {
        let c = pixmap.pixel(x, y).expect("in bounds").demultiply();
        Rgba([c.red(), c.green(), c.blue(), c.alpha()])
    })
}

/// `src` over `dst` (straight alpha), rounded as Pillow's alpha_composite.
fn over(dst: &mut Rgba<u8>, src: [u8; 4]) {
    let (sa, da) = (src[3] as f64 / 255.0, dst[3] as f64 / 255.0);
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        *dst = Rgba([0, 0, 0, 0]);
        return;
    }
    for c in 0..3 {
        let v = (src[c] as f64 * sa + dst[c] as f64 * da * (1.0 - sa)) / oa;
        dst[c] = v.round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (oa * 255.0).round() as u8;
}

// ---- fitting ----

/// `img` scaled to fit a `size` square, centred on transparency (Lanczos, on
/// premultiplied colour so transparent edges don't darken, as Pillow).
pub(crate) fn letterbox(img: &RgbaImage, size: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = RgbaImage::new(size, size);
    if w == 0 || h == 0 {
        return out;
    }
    let scale = (size as f64 / w as f64).min(size as f64 / h as f64);
    let nw = ((w as f64 * scale).round_ties_even() as u32).max(1);
    let nh = ((h as f64 * scale).round_ties_even() as u32).max(1);
    // 0..1 floats: the resize clamps to that range.
    let pre = image::Rgba32FImage::from_fn(w, h, |x, y| {
        let p = img.get_pixel(x, y);
        let a = p[3] as f32 / 255.0;
        Rgba([p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a, a])
    });
    let scaled = image::imageops::resize(&pre, nw, nh, image::imageops::FilterType::Lanczos3);
    let (ox, oy) = ((size - nw) / 2, (size - nh) / 2);
    for (x, y, p) in scaled.enumerate_pixels() {
        let a = p[3].clamp(0.0, 1.0);
        let a8 = (a * 255.0).round() as u8;
        if a8 == 0 {
            continue;
        }
        let un = |c: f32| (c / a * 255.0).round().clamp(0.0, 255.0) as u8;
        out.put_pixel(x + ox, y + oy, Rgba([un(p[0]), un(p[1]), un(p[2]), a8]));
    }
    out
}

// ---- the silhouette (packager.py _build_silhouette_alpha) ----

/// 255 where the icon's mark is (to paint black), 0 where it isn't. The first
/// that fits:
///
/// - a light plate (a mark on a light disc or rounded square with see-through
///   corners, like Google's "G"): the mark, keyed on colour distance from the
///   plate;
/// - a solid picture: Otsu's threshold, the side the background isn't on;
/// - a logo on transparency: its own alpha.
pub(crate) fn silhouette_alpha(icon: &RgbaImage) -> GrayImage {
    let (w, h) = icon.dimensions();
    let alpha = GrayImage::from_fn(w, h, |x, y| Luma([icon.get_pixel(x, y)[3]]));
    let Some((x0, y0, x1, y1)) = bbox(&alpha) else { return GrayImage::new(w, h) };
    let box_w = (x1 - x0).max(1);
    let box_h = (y1 - y0).max(1);
    let opaque_count =
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| alpha.get_pixel(x, y)[0] > 250).count();
    let opaque_fraction = opaque_count as f64 / (box_w * box_h) as f64;

    let ring = border_ring(w, h, (x0, y0, x1, y1), 0.14);
    let mean_border_alpha = masked_mean(&alpha, &ring);
    let gray = luma(icon);

    let min_dim = box_w.min(box_h);
    let band = plate_band(&alpha, 6.max((min_dim as f64 * 0.03) as u32));
    let band_hist = histogram(&gray, &band);
    let band_total = band_hist.iter().sum::<u64>().max(1) as f64;
    let band_mean = band_hist.iter().enumerate().map(|(i, &n)| i as f64 * n as f64).sum::<f64>() / band_total;
    let frac_light = band_hist[210..].iter().sum::<u64>() as f64 / band_total;
    let light_plate = frac_light > 0.90 && band_mean > 210.0 && opaque_fraction < 0.90;

    if light_plate {
        let bg = [0, 1, 2].map(|c| {
            let channel = GrayImage::from_fn(w, h, |x, y| Luma([icon.get_pixel(x, y)[c]]));
            masked_mean(&channel, &band)
        });
        return plate_mark_alpha(icon, &alpha, bg);
    }
    if opaque_fraction > 0.90 && mean_border_alpha > 200.0 {
        let bg_luma = masked_mean(&gray, &ring);
        return otsu_silhouette(&gray, &alpha, bg_luma);
    }
    alpha
}

/// The tight box (x0, y0, x1, y1; ends exclusive) of non-zero pixels.
fn bbox(img: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p[0] > 0 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    (x1 > 0).then_some((x0, y0, x1, y1))
}

/// Pillow's RGB → L: (19595 R + 38470 G + 7471 B + 0x8000) >> 16.
fn luma(img: &RgbaImage) -> GrayImage {
    GrayImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y);
        Luma([((p[0] as u32 * 19595 + p[1] as u32 * 38470 + p[2] as u32 * 7471 + 0x8000) >> 16) as u8])
    })
}

/// A ring just inside the content box (a fraction `frac` of it deep): the
/// background around a centred logo.
fn border_ring(w: u32, h: u32, (x0, y0, x1, y1): (u32, u32, u32, u32), frac: f64) -> GrayImage {
    let ix = 1.max(((x1 - x0) as f64 * frac) as u32);
    let iy = 1.max(((y1 - y0) as f64 * frac) as u32);
    GrayImage::from_fn(w, h, |x, y| {
        let in_box = x >= x0 && x < x1 && y >= y0 && y < y1;
        let in_hole = x >= x0 + ix && x + ix < x1 && y >= y0 + iy && y + iy < y1;
        Luma([if in_box && !in_hole { 255 } else { 0 }])
    })
}

/// A ring `inset` px wide along the opaque shape's own edge, so a round plate
/// is sampled on the plate, not in its see-through corners.
fn plate_band(alpha: &GrayImage, inset: u32) -> GrayImage {
    let opaque = GrayImage::from_fn(alpha.width(), alpha.height(), |x, y| {
        Luma([if alpha.get_pixel(x, y)[0] > 128 { 255 } else { 0 }])
    });
    let mut core = opaque.clone();
    for _ in 0..inset {
        core = min_filter3(&core);
    }
    GrayImage::from_fn(alpha.width(), alpha.height(), |x, y| {
        Luma([opaque.get_pixel(x, y)[0].saturating_sub(core.get_pixel(x, y)[0])])
    })
}

/// Pillow's MinFilter(3): each pixel the least of its 3×3 neighbours, the
/// image's edge pixels repeated beyond it.
fn min_filter3(img: &GrayImage) -> GrayImage {
    let (w, h) = img.dimensions();
    GrayImage::from_fn(w, h, |x, y| {
        let mut m = u8::MAX;
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let sx = (x as i64 + dx).clamp(0, w as i64 - 1) as u32;
                let sy = (y as i64 + dy).clamp(0, h as i64 - 1) as u32;
                m = m.min(img.get_pixel(sx, sy)[0]);
            }
        }
        Luma([m])
    })
}

fn histogram(img: &GrayImage, mask: &GrayImage) -> [u64; 256] {
    let mut hist = [0u64; 256];
    for (p, m) in img.pixels().zip(mask.pixels()) {
        if m[0] != 0 {
            hist[p[0] as usize] += 1;
        }
    }
    hist
}

fn masked_mean(img: &GrayImage, mask: &GrayImage) -> f64 {
    let (mut sum, mut n) = (0.0, 0u64);
    for (p, m) in img.pixels().zip(mask.pixels()) {
        if m[0] != 0 {
            sum += p[0] as f64;
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { sum / n as f64 }
}

/// The mark on a light plate, by each pixel's largest channel distance from
/// the plate's colour (a light yellow stroke is far from white in blue, though
/// close in brightness), on a soft ramp so edges stay smooth.
fn plate_mark_alpha(icon: &RgbaImage, alpha: &GrayImage, bg: [f64; 3]) -> GrayImage {
    let bg = bg.map(|c| c.round_ties_even() as i32);
    let lut = smoothstep_lut(|p| (p - 44.0) / 26.0 + 0.5);
    GrayImage::from_fn(icon.width(), icon.height(), |x, y| {
        let p = icon.get_pixel(x, y);
        let dist = (0..3).map(|c| (p[c] as i32 - bg[c]).unsigned_abs()).max().unwrap_or(0) as usize;
        Luma([chop_multiply(lut[dist.min(255)], alpha.get_pixel(x, y)[0])])
    })
}

/// Otsu's cutoff over the opaque pixels' brightness; the background's
/// brightness decides which side is the mark.
fn otsu_silhouette(gray: &GrayImage, alpha: &GrayImage, bg_luma: f64) -> GrayImage {
    let content = GrayImage::from_fn(alpha.width(), alpha.height(), |x, y| {
        Luma([if alpha.get_pixel(x, y)[0] > 128 { 255 } else { 0 }])
    });
    let (threshold, below, above) = otsu(&histogram(gray, &content));
    let dark_fg = bg_luma > threshold as f64;
    let width = ((above - below) * 0.20).clamp(8.0, 40.0);
    let t = threshold as f64;
    let lut = smoothstep_lut(|p| if dark_fg { (t - p) / width + 0.5 } else { (p - t) / width + 0.5 });
    GrayImage::from_fn(gray.width(), gray.height(), |x, y| {
        Luma([chop_multiply(lut[gray.get_pixel(x, y)[0] as usize], alpha.get_pixel(x, y)[0])])
    })
}

/// Otsu's method: the cutoff that best splits the histogram in two, with the
/// two halves' means.
fn otsu(hist: &[u64; 256]) -> (u32, f64, f64) {
    let total = hist.iter().sum::<u64>() as f64;
    if total == 0.0 {
        return (128, 0.0, 255.0);
    }
    let sum_all: f64 = hist.iter().enumerate().map(|(i, &n)| i as f64 * n as f64).sum();
    let (mut w_b, mut sum_b, mut max_var) = (0.0, 0.0, -1.0);
    let (mut threshold, mut below, mut above) = (128, 0.0, 255.0);
    for (t, &n) in hist.iter().enumerate() {
        w_b += n as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += t as f64 * n as f64;
        let mean_b = sum_b / w_b;
        let mean_f = (sum_all - sum_b) / w_f;
        let var = w_b * w_f * (mean_b - mean_f).powi(2);
        if var > max_var {
            max_var = var;
            threshold = t as u32;
            below = mean_b;
            above = mean_f;
        }
    }
    (threshold, below, above)
}

/// 256 entries: smoothstep of `t(p)` clamped to 0..1, as 0..255.
fn smoothstep_lut(t: impl Fn(f64) -> f64) -> [u8; 256] {
    std::array::from_fn(|p| {
        let t = t(p as f64).clamp(0.0, 1.0);
        (t * t * (3.0 - 2.0 * t) * 255.0).round_ties_even() as u8
    })
}

/// Pillow's ImageChops.multiply: a × b / 255, truncated.
fn chop_multiply(a: u8, b: u8) -> u8 {
    (a as u32 * b as u32 / 255) as u8
}

#[cfg(test)]
mod tests {

    #[test]
    fn dark_marks_get_a_white_tile_and_colour_stays_as_it_is() {
        // A black disc on nothing (GitHub's), and a coloured one.
        let disc = |c: [u8; 4]| {
            RgbaImage::from_fn(64, 64, |x, y| {
                let (dx, dy) = (x as i32 - 32, y as i32 - 32);
                if dx * dx + dy * dy < 400 { Rgba(c) } else { Rgba([0, 0, 0, 0]) }
            })
        };
        let dark = fit(&disc([20, 20, 20, 255]), 128);
        assert_eq!(dark.get_pixel(64, 6).0, [255, 255, 255, 255]); // the tile, above the mark
        assert!(dark.get_pixel(64, 64)[0] < 60); // the mark
        let colour = fit(&disc([234, 67, 53, 255]), 128);
        assert_eq!(colour.get_pixel(64, 6)[3], 0);
    }

    use super::*;

    #[test]
    fn long_hosts_lose_their_leading_names() {
        let fits = |l: &str| l.len() <= 14;
        assert_eq!(shorten("console.cloud.google.com", fits), "google.com");
        assert_eq!(shorten("app.zoom.us", fits), "app.zoom.us");
        assert_eq!(shorten("very.long.intranet.example:8443", fits), "intranet.example:8443", "two names at least");
        assert_eq!(shorten("192.168.100.200:8080", fits), "192.168.100.200:8080");
    }

    #[test]
    fn otsu_splits_two_peaks() {
        let mut hist = [0u64; 256];
        hist[20] = 100;
        hist[220] = 100;
        let (t, below, above) = otsu(&hist);
        assert!((20..220).contains(&t), "{t}");
        assert_eq!((below, above), (20.0, 220.0));
    }

    #[test]
    fn luma_matches_pillow() {
        let img = RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255]));
        assert_eq!(luma(&img).get_pixel(0, 0)[0], 76);
        let img = RgbaImage::from_pixel(1, 1, Rgba([255, 255, 255, 0]));
        assert_eq!(luma(&img).get_pixel(0, 0)[0], 255);
    }

    #[test]
    fn empty_icon_gives_an_empty_card() {
        let out = card(&RgbaImage::new(16, 16));
        assert_eq!(out.dimensions(), (SIZE, SIZE));
        // White card in the middle, see-through corners, no ink.
        assert_eq!(out.get_pixel(256, 256), &Rgba([255, 255, 255, 255]));
        assert_eq!(out.get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn transparent_logo_is_its_alpha() {
        // A plus: it fills too little of its box to be read as a solid picture.
        let mut img = RgbaImage::new(64, 64);
        for y in 16..48 {
            for x in 16..48 {
                if (26..38).contains(&x) || (26..38).contains(&y) {
                    img.put_pixel(x, y, Rgba([255, 120, 0, 255]));
                }
            }
        }
        let out = card(&img);
        assert_eq!(out.get_pixel(256, 256), &Rgba([0, 0, 0, 255]), "the plus is black");
        assert_eq!(out.get_pixel(40, 256), &Rgba([255, 255, 255, 255]), "around it, the card");
        // The dark theme's: acid on violet, corners still see-through.
        let d = dark(&out);
        assert_eq!(d.get_pixel(256, 256), &Rgba([198, 255, 31, 255]), "the plus is acid");
        assert_eq!(d.get_pixel(40, 256), &Rgba([31, 21, 51, 255]), "the card is violet");
        assert_eq!(d.get_pixel(0, 0)[3], 0);
        let text = dark(&text_card("localhost:8080"));
        assert!(text.pixels().any(|p| p[3] == 255 && p[1] > 200), "the text card's letters are acid");
    }
}
