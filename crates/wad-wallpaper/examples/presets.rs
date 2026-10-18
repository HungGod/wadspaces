//! The Builder's wallpapers for another app (the WadSpaces Android app):
//! `presets.json` (`[{name, wallpaper: {type, value}, thumb}]`, the blank
//! one first) and a PNG of each, drawn as wadspaces draw them.
//!
//!   cargo run -p wad-wallpaper --example presets -- OUT_DIR

use wad_wallpaper::{BLANK, Kind, PRESETS, Wallpaper, layers, png};

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("an output folder"));
    std::fs::create_dir_all(&out).expect("the output folder");
    let all = std::iter::once(("Plain", BLANK)).chain(PRESETS.iter().copied());
    let mut list = vec![];
    for (name, css) in all {
        let file = format!("{}.png", name.to_lowercase().replace(' ', "-"));
        let bytes = png(&layers(css), 480, 270).expect("a preset draws");
        std::fs::write(out.join(&file), bytes).expect("a thumbnail");
        let wallpaper = Wallpaper { kind: Kind::Gradient, value: css.into() };
        list.push(serde_json::json!({ "name": name, "wallpaper": wallpaper, "thumb": file }));
    }
    let text = serde_json::to_string_pretty(&list).expect("json") + "\n";
    std::fs::write(out.join("presets.json"), text).expect("presets.json");
    println!("{} wallpapers in {}", list.len(), out.display());
}
