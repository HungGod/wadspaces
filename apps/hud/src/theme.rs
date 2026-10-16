//! The HUD in the site's colours (wad-gtk's theme), dark or light as
//! WadSpaces Client is: the client writes its choice to
//! ~/.config/wadspaces/theme (wad_gtk::theme), and the HUD follows that
//! file.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn parse(s: &str) -> Theme {
        if s.trim() == "light" { Theme::Light } else { Theme::Dark }
    }
}

/// Where WadSpaces Client keeps its theme for the HUD.
pub fn file() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    config.join("wadspaces/theme")
}

/// The theme now (dark until the app says otherwise). WADSPACES_HUD_THEME wins (snapshots).
pub fn current() -> Theme {
    if let Ok(t) = std::env::var("WADSPACES_HUD_THEME") {
        return Theme::parse(&t);
    }
    std::fs::read_to_string(file()).map(|s| Theme::parse(&s)).unwrap_or(Theme::Dark)
}

/// The site's tokens, as GTK named colours.
fn palette(t: Theme) -> &'static str {
    match t {
        Theme::Dark => {
            "@define-color bg #0a0614;
@define-color surface #140d22;
@define-color surface2 #1b132d;
@define-color surface3 #241a39;
@define-color line rgba(240, 234, 250, 0.09);
@define-color line_strong rgba(240, 234, 250, 0.18);
@define-color fg #f5f2fa;
@define-color muted rgba(245, 242, 250, 0.62);
@define-color faint rgba(245, 242, 250, 0.4);
@define-color accent #c6ff1f;
@define-color accent_fg #0a0614;
@define-color accent_soft rgba(198, 255, 31, 0.2);
@define-color accent2 #ff3d81;
@define-color danger #ff5d7a;
@define-color warn #e8c36a;
@define-color dim rgba(3, 2, 8, 0.62);
"
        }
        // The site's light theme is strictly greyscale.
        Theme::Light => {
            "@define-color bg #f4f4f4;
@define-color surface #ffffff;
@define-color surface2 #f6f6f6;
@define-color surface3 #ebebeb;
@define-color line rgba(10, 6, 20, 0.09);
@define-color line_strong rgba(10, 6, 20, 0.18);
@define-color fg #0a0614;
@define-color muted #5f5f5f;
@define-color faint #9a9a9a;
@define-color accent #0a0614;
@define-color accent_fg #ffffff;
@define-color accent_soft rgba(10, 6, 20, 0.07);
@define-color accent2 #d9d9d9;
@define-color danger #3a3a3a;
@define-color warn #5f5f5f;
@define-color dim rgba(10, 6, 20, 0.35);
"
        }
    }
}

const RULES: &str = r#"
* { font-family: "Inter Variable", "Inter", "Noto Sans", sans-serif; }
window { background: transparent; }
.pill {
  background: alpha(@surface, 0.94); color: @fg; border: 1px solid @line_strong;
  border-radius: 999px; padding: 3px 10px; min-height: 0;
  font-size: 12px; font-weight: 500; box-shadow: none;
}
.pill:hover { background: @surface2; }
.pill.offline { color: @warn; }
.pill.done { color: @accent; border-color: @accent; }
.pill.focus { color: @accent; border-color: alpha(@accent, 0.45); }
.pill.logo-button { padding: 2px 8px; }
.pill.icon-pill { padding: 4px 9px; }
.pill image { color: @fg; }
.pill.battery { padding: 3px 10px 3px 8px; color: @muted; font-feature-settings: "tnum"; }
.pill.battery.low, .pill.battery.low image { color: @warn; border-color: alpha(@warn, 0.5); }
.pill.clock { font-feature-settings: "tnum"; color: @muted; }
.pill.arrow { padding: 3px 9px; font-size: 14px; font-weight: 600; color: @muted; }
.pill.arrow:hover { color: @fg; }
.pill.arrow.attention { color: @accent; border-color: @accent; }

button.dim, button.dim:hover, button.dim:active {
  background: @dim; background-image: none; border: none; border-radius: 0; box-shadow: none; outline: none;
}
button.catcher, button.catcher:hover, button.catcher:active {
  background: transparent; background-image: none; border: none; border-radius: 0; box-shadow: none; outline: none;
}
.card {
  background: @surface; color: @fg; border: 1px solid @line;
  border-radius: 24px; padding: 22px; box-shadow: 0 20px 60px -20px rgba(3, 2, 8, 0.6);
}
.title { font-size: 20px; font-weight: 700; color: @fg; }
.sub { color: @muted; font-size: 13px; }
.section { color: @faint; font-size: 11px; font-weight: 600; letter-spacing: 1px; }
.error { color: @danger; font-size: 13px; }
.menu-button {
  background: @surface2; color: @fg; border: 1px solid @line; border-radius: 14px;
  padding: 12px 18px; font-size: 15px; font-weight: 500; min-width: 220px; box-shadow: none;
}
.menu-button:hover { background: @surface3; border-color: @line_strong; }
.menu-button.danger { color: @danger; border-color: alpha(@danger, 0.35); }
.menu-button.quiet { background: transparent; border-color: transparent; color: @muted; }
.menu-button.quiet:hover { color: @fg; }
.small-button {
  background: @surface2; color: @fg; border: 1px solid @line; border-radius: 10px;
  padding: 4px 12px; font-size: 13px; font-weight: 500; box-shadow: none;
}
.small-button:hover { background: @surface3; }
.small-button.primary { background: @accent; color: @accent_fg; border-color: @accent; }
.net-row { background: transparent; border: none; border-radius: 12px; padding: 8px 10px; color: @fg; font-size: 14px; box-shadow: none; }
.net-row:hover { background: @surface2; }
.net-row.active { background: @accent_soft; }
.net-row .tag { color: @faint; font-size: 12px; }
.net-row .bars { color: @muted; font-size: 11px; min-width: 34px; }
.device-row { padding: 4px 2px; }
.device-row image { color: @muted; }
.dim-row { opacity: 0.55; }
.chip { background: @surface2; color: @muted; border: 1px solid @line; border-radius: 999px; padding: 2px 10px; font-size: 12px; min-height: 0; box-shadow: none; }
.chip:hover { background: @surface3; }
.chip:checked { background: @accent_soft; color: @fg; border-color: @accent; }
.chip.running { font-weight: 600; }
.code { font-size: 28px; font-weight: 600; letter-spacing: 4px; color: @fg; }
.status { background: @surface2; border: 1px solid @line; border-radius: 14px; padding: 10px 14px; color: @fg; }
scrollbar slider { min-width: 6px; min-height: 6px; margin: 0; }
entry, passwordentry {
  background: @bg; color: @fg; border: 1px solid @line_strong; border-radius: 10px; padding: 6px 10px; box-shadow: none;
}
entry:focus-within, passwordentry:focus-within { border-color: @accent; }

.tip {
  background: @surface; color: @fg; border: 1px solid @line_strong; border-radius: 10px;
  padding: 6px 10px; font-size: 12px; box-shadow: 0 8px 24px -10px rgba(3, 2, 8, 0.5);
}
.card.level { padding: 14px 16px; border-radius: 20px; }
.card.osd { padding: 12px 16px; border-radius: 18px; }
.card.osd image { color: @fg; }
.card.osd .value { color: @muted; font-size: 12px; font-feature-settings: "tnum"; }
progressbar trough { background: @surface3; border: none; border-radius: 999px; min-height: 6px; }
progressbar progress { background: @accent; border: none; border-radius: 999px; min-height: 6px; min-width: 6px; margin: 0; }
.slider-row { min-height: 34px; }
.slider-icon { min-width: 32px; color: @fg; }
.slider-row.muted .slider-icon image { color: @faint; }
button.slider-icon { padding: 2px 4px; min-height: 0; min-width: 0; }
.slider-row .value { color: @muted; font-size: 12px; font-feature-settings: "tnum"; }
.slider-row.muted .value { color: @faint; }
scale { padding: 6px 0; }
scale trough { background: @surface3; border: none; border-radius: 999px; min-height: 6px; }
scale highlight { background: @accent; border: none; border-radius: 999px; margin: 0; min-height: 6px; }
.slider-row.muted scale highlight { background: @faint; }
scale slider {
  background: @fg; border: 2px solid @surface; border-radius: 999px; min-width: 18px; min-height: 18px;
  margin: -7px 0; box-shadow: none;
}

list.clips { background: transparent; }
row.clip { background: transparent; border-radius: 12px; padding: 10px 12px; margin: 1px 0; color: @fg; outline: none; }
row.clip:hover { background: @surface2; }
row.clip:selected { background: @accent_soft; }
row.clip .text { color: @fg; font-size: 13px; }
row.clip .when { color: @faint; font-size: 11px; }
row.clip .thumb { border-radius: 8px; }
button.forget {
  background: transparent; background-image: none; border: none; box-shadow: none;
  color: @faint; padding: 2px 6px; min-height: 0; min-width: 0; font-size: 12px;
}
button.forget:hover { color: @fg; background: @surface3; }
.empty { padding: 18px 0; }

.switcher { background: alpha(@surface, 0.96); border: 1px solid @line_strong; border-radius: 28px; padding: 18px; }
.item { border-radius: 18px; padding: 14px; min-width: 128px; border: 2px solid transparent; }
.item.selected { background: @accent_soft; border-color: @accent; }
.item .icon { background: @surface3; border-radius: 16px; min-width: 64px; min-height: 64px; font-size: 26px; font-weight: 700; color: @fg; }
.item .preview { border-radius: 12px; border: 1px solid @line_strong; background: @surface3; }
.item .name { color: @fg; font-size: 13px; font-weight: 500; margin-top: 8px; }
.item .state { color: @faint; font-size: 11px; }
.switcher .divider { background: @line_strong; min-width: 1px; margin: 10px 6px; }

/* WadNotes (notes/): the panel on the right, its cards, and reminders. */
.pill.on { background: @accent_soft; border-color: alpha(@accent, 0.5); }
.pill.on image { color: @accent; }
.notes-card {
  background: alpha(@surface, 0.97); color: @fg; border: 1px solid @line_strong;
  border-radius: 20px; padding: 12px 14px; box-shadow: 0 16px 48px -20px rgba(3, 2, 8, 0.6);
}
.notes-card .heading { font-size: 14px; font-weight: 700; color: @fg; }
.notes-card .count { color: @faint; font-size: 12px; font-feature-settings: "tnum"; }
.scope-tag { color: @muted; font-size: 12px; }
.scope-tag.looking { color: @accent; }
.card.notes-menu { padding: 16px; border-radius: 20px; }
button.flat-icon {
  background: transparent; background-image: none; border: none; box-shadow: none;
  color: @muted; padding: 4px 6px; min-height: 0; min-width: 0; border-radius: 8px;
}
button.flat-icon:hover { background: @surface2; color: @fg; }
button.flat-icon image { color: inherit; }
button.scope-chip {
  background: @surface2; background-image: none; color: @fg; border: 1px solid @line; box-shadow: none;
  border-radius: 999px; padding: 3px 10px; min-height: 0; font-size: 13px; font-weight: 500;
}
button.scope-chip:hover { background: @surface3; }
button.link {
  background: transparent; background-image: none; border: none; box-shadow: none;
  color: @muted; padding: 0 2px; min-height: 0; font-size: 12px;
}
button.link:hover { color: @fg; }
button.row {
  background: transparent; background-image: none; border: none; box-shadow: none;
  border-radius: 12px; padding: 7px 8px; min-height: 0; color: @fg;
}
button.row:hover { background: @surface2; }
button.row.picked { background: @accent_soft; }
.row-title { font-size: 13px; font-weight: 500; color: @fg; }
.row-sub { font-size: 11px; color: @faint; }
.row-title.done { color: @faint; }
.key { font-family: monospace; font-size: 11px; color: @faint; }
.overdue { color: @danger; }
.empty-note { color: @faint; font-size: 12px; padding: 8px 4px; }
button.status {
  background-image: none; box-shadow: none; border: 1px solid @line; border-radius: 999px;
  padding: 1px 8px; min-height: 0; font-size: 11px; font-weight: 600;
  background: @surface3; color: @muted;
}
button.status.doing { background: @accent_soft; color: @accent; border-color: alpha(@accent, 0.4); }
button.status.done { background: transparent; color: @faint; }
.prio { font-size: 11px; font-weight: 700; }
.prio.high { color: @danger; }
.prio.medium { color: @warn; }
.prio.low { color: @faint; }
button.chip {
  background: @surface2; background-image: none; color: @muted; border: 1px solid @line; box-shadow: none;
  border-radius: 999px; padding: 2px 10px; min-height: 0; font-size: 12px; font-weight: 500;
}
button.chip:hover { color: @fg; background: @surface3; }
button.chip.on { background: @accent; color: @accent_fg; border-color: @accent; }
.field-label { color: @faint; font-size: 11px; font-weight: 600; letter-spacing: 0.5px; }
entry.title-entry { font-size: 15px; font-weight: 600; }
textview.md, textview.md text { background: @bg; color: @fg; font-family: monospace; font-size: 13px; }
textview.md { border: 1px solid @line_strong; border-radius: 10px; padding: 8px; }
.preview { font-size: 13px; color: @fg; }
.weekday { color: @faint; font-size: 11px; font-weight: 600; }
button.day {
  background: transparent; background-image: none; border: 1px solid transparent; box-shadow: none;
  border-radius: 10px; padding: 2px 0; min-height: 0; min-width: 40px; color: @fg;
}
button.day:hover { background: @surface2; }
button.day.other { color: @faint; }
button.day.today { border-color: alpha(@accent, 0.6); }
button.day.selected { background: @accent_soft; }
button.day .num { font-size: 12px; font-feature-settings: "tnum"; }
.dots { font-size: 7px; min-height: 8px; }
.stripe { min-width: 4px; border-radius: 2px; }
.when { color: @muted; font-size: 11px; font-feature-settings: "tnum"; }
.toast-row { background: @surface3; border-radius: 12px; padding: 6px 6px 6px 12px; }
.card.reminder { padding: 14px 16px; border-radius: 20px; }
calendar { background: @bg; color: @fg; border: 1px solid @line; border-radius: 12px; padding: 4px; }
calendar > header { border: none; }
calendar > header > button { background: transparent; background-image: none; border: none; box-shadow: none; color: @muted; }
calendar > grid > label.day-number { padding: 4px; border-radius: 8px; }
calendar > grid > label.day-number.other-month { color: @faint; }
calendar > grid > label.day-number:selected { background: @accent; color: @accent_fg; }
spinbutton { background: @bg; color: @fg; border: 1px solid @line_strong; border-radius: 10px; }
spinbutton button { background: transparent; background-image: none; border: none; box-shadow: none; color: @muted; }
switch { background: @surface3; border: 1px solid @line_strong; border-radius: 999px; }
switch:checked { background: @accent; }
switch slider { background: @fg; border-radius: 999px; box-shadow: none; border: none; }
switch:checked slider { background: @accent_fg; }
"#;

/// Each wadspace's colour (wad_notes::PALETTE) as classes: `.scope-N` on a
/// dot (●) colours its text, on a stripe its background.
fn scope_rules() -> String {
    wad_notes::PALETTE
        .iter()
        .enumerate()
        .map(|(i, (_, hex))| {
            format!(".dot.scope-{i} {{ color: {hex}; }}\n.stripe.scope-{i} {{ background: {hex}; }}\n")
        })
        .collect()
}

pub fn css(t: Theme) -> String {
    format!("{}{RULES}{}", palette(t), scope_rules())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes() {
        assert_eq!(Theme::parse("light\n"), Theme::Light);
        assert_eq!(Theme::parse("dark"), Theme::Dark);
        assert_eq!(Theme::parse("anything"), Theme::Dark);
        for t in [Theme::Dark, Theme::Light] {
            let css = css(t);
            // Every colour the rules use is defined.
            for name in [
                "bg",
                "surface",
                "surface2",
                "surface3",
                "line",
                "line_strong",
                "fg",
                "muted",
                "faint",
                "accent",
                "accent_fg",
                "accent_soft",
                "danger",
                "warn",
                "dim",
            ] {
                assert!(css.contains(&format!("@define-color {name} ")), "{t:?} lacks {name}");
            }
            assert!(css.contains(".dot.scope-11 { color: #10b981; }"), "the wadspaces' colours");
        }
    }
}
