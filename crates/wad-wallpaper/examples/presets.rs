//! The Builder's wallpapers for another app (the WadSpaces Android app):
//! `presets.json` (`[{name, wallpaper: {type, value}, thumb}]`, the blank
//! one first) and a PNG of each, drawn as wadspaces draw them; and
//! `textures.json` (`[{id, name, dark, light}]`) with each texture's two
//! PNGs.
//!
//!   cargo run -p wad-wallpaper --example presets -- OUT_DIR

use wad_wallpaper::{BLANK, Kind, PRESETS, TEXTURES, Tone, Wallpaper, layers, png, texture};

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
    let mut textures = vec![];
    for (id, name) in TEXTURES {
        let mut entry = serde_json::json!({ "id": id, "name": name });
        for (tone, key) in [(Tone::Dark, "dark"), (Tone::Light, "light")] {
            let file = format!("texture-{id}-{key}.png");
            let bytes = texture::png(id, tone, 480, 270).expect("a texture draws");
            std::fs::write(out.join(&file), bytes).expect("a texture");
            entry[key] = file.into();
        }
        textures.push(entry);
    }
    let text = serde_json::to_string_pretty(&textures).expect("json") + "\n";
    std::fs::write(out.join("textures.json"), text).expect("textures.json");
    println!("{} wallpapers and {} textures in {}", list.len(), textures.len(), out.display());
}
