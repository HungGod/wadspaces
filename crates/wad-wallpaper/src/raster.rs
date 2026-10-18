//! Drawing a wallpaper's layers to a PNG (what the image's swaybg shows).

use tiny_skia::{
    Color, GradientStop, LinearGradient, Paint, Pixmap, Point, RadialGradient, Rect, SpreadMode, Transform,
};

use crate::{Layer, Paint as Drawn, Rgba, Stop, Wallpaper};

/// The size gradients are drawn at, as the client does.
pub const SIZE: (u32, u32) = (1920, 1080);

fn rgba(c: Rgba) -> Color {
    Color::from_rgba(c.r, c.g, c.b, c.a).unwrap_or(Color::TRANSPARENT)
}

fn gradient_stops(stops: &[Stop]) -> Vec<GradientStop> {
    // tiny-skia wants 0..1 (CSS allows stops past the ends): clamped, as the client does.
    stops.iter().map(|s| GradientStop::new(s.at.clamp(0.0, 1.0), rgba(s.color))).collect()
}

/// A gradient or a colour drawn at [`SIZE`], as a PNG; None for a picture
/// (it's a file already) or what can't be drawn.
pub fn draw_png(w: &Wallpaper) -> Option<Vec<u8>> {
    match w.paint() {
        Drawn::Layers(l) if !l.is_empty() => png(&l, SIZE.0, SIZE.1),
        _ => None,
    }
}

/// `layers` (bottom first) drawn on the dark base, as a PNG.
pub fn png(layers: &[Layer], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut pm = Pixmap::new(w, h)?;
    pm.fill(Color::from_rgba8(0x0a, 0x06, 0x14, 255));
    let (fw, fh) = (w as f32, h as f32);
    let all = Rect::from_xywh(0.0, 0.0, fw, fh)?;
    for layer in layers {
        let mut paint = Paint { anti_alias: true, ..Paint::default() };
        match layer {
            Layer::Color(c) => paint.set_color(rgba(*c)),
            Layer::Linear { deg, stops } => {
                let ((x0, y0), (x1, y1)) = Layer::line(*deg, fw, fh);
                paint.shader = LinearGradient::new(
                    Point::from_xy(x0, y0),
                    Point::from_xy(x1, y1),
                    gradient_stops(stops),
                    SpreadMode::Pad,
                    Transform::identity(),
                )?;
            }
            Layer::Radial { rx, ry, cx, cy, stops } => {
                // A unit circle, scaled to the ellipse and moved to its centre.
                let to = Transform::from_row(rx * fw, 0.0, 0.0, ry * fh, cx * fw, cy * fh);
                paint.shader = RadialGradient::new(
                    Point::zero(),
                    0.0,
                    Point::zero(),
                    1.0,
                    gradient_stops(stops),
                    SpreadMode::Pad,
                    to,
                )?;
            }
        }
        pm.fill_rect(all, &paint, Transform::identity(), None);
    }
    pm.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_draw() {
        for (name, css) in crate::PRESETS {
            let png = png(&crate::layers(css), 192, 108).unwrap_or_else(|| panic!("{name}"));
            assert!(png.starts_with(b"\x89PNG"), "{name}");
        }
    }

    #[test]
    fn a_fade_to_transparent_keeps_its_colour() {
        // Pink fading out over the dark base: halfway is still pinkish, not grey.
        let css = "linear-gradient(90deg, rgb(255 61 129 / 1), transparent)";
        let pm = Pixmap::decode_png(&png(&crate::layers(css), 101, 1).unwrap()).unwrap();
        let mid = pm.pixel(50, 0).unwrap();
        assert!(mid.red() > 100 && mid.red() > mid.green() * 3, "{mid:?}");
    }
}
