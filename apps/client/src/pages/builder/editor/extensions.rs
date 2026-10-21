//! The Extensions editor (ExtensionsEditor.tsx): a window per app the
//! template has that can be set up (WadBrowser's web apps, VS Code's
//! extensions, the command line's tools), holding what it's set up with.
//! On the left, single things to drag (or click) in: only for apps that are
//! there, and only what isn't in yet.

use std::rc::{Rc, Weak};

use gtk::prelude::*;
use wad_board::catalog::{self, app_sub_info};
use wad_board::*;
use wad_gtk::Icon;

use super::{Editor, field, group_label, icon_note, live_box, note, panel_title, scroll_into_view, scroller};
use crate::drag::{self, Item, Mods, Payload, Target, Verdict};
use crate::pages::builder::faces::{self, img};
use crate::ui::{self, Size, Variant};

/// Web apps worth offering even when no other wadspace has them.
pub(crate) const COMMON: [(&str, &str, &str); 5] = [
    ("github", "GitHub", "https://github.com"),
    ("gmail", "Gmail", "https://mail.google.com"),
    ("google-drive", "Google Drive", "https://drive.google.com"),
    ("claude", "Claude", "https://claude.ai"),
    ("youtube", "YouTube", "https://youtube.com"),
];

fn draft_of(ed: &Editor) -> ExtensionsModule {
    match ed.draft() {
        Module::Extensions(e) => e,
        _ => unreachable!("the Extensions editor edits Extensions"),
    }
}

/// The app setup an item goes in.
fn item_key(i: &Item) -> &'static str {
    match i {
        Item::WebApp(_) => "wadbrowser",
        Item::Extension(_) => "vscode",
        Item::Tool(_) => "tools",
    }
}

fn item_name(i: &Item) -> String {
    match i {
        Item::WebApp(w) => w.name.clone(),
        Item::Extension(id) => catalog::extension(id).name,
        Item::Tool(id) => catalog::tool_name(id),
    }
}

fn item_icon(i: &Item) -> String {
    match i {
        Item::WebApp(w) => catalog::site_icon(&w.url),
        Item::Extension(id) => catalog::extension(id).icon.unwrap_or_else(|| app_sub_info("vscode").2.into()),
        Item::Tool(_) => app_sub_info("tools").2.into(),
    }
}

fn item_face(i: &Item) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.add_css_class("item");
    b.append(&img(&item_icon(i), 16));
    b.append(&gtk::Label::new(Some(&item_name(i))));
    b
}

/// The setup of `key` the draft has, or an empty one.
fn sub(d: &ExtensionsModule, key: &str) -> AppSub {
    if let Some(s) = d.subs.iter().find(|s| s.key() == key) {
        return s.clone();
    }
    let (id, origin) = (uid(key), d.name.clone());
    match key {
        "vscode" => AppSub::Vscode(VscodeSub { id, origin, extensions: vec![] }),
        "wadbrowser" => AppSub::Browser(BrowserSub { id, origin, mode: "browser".into(), webapps: vec![] }),
        _ => AppSub::Tools(ToolsSub { id, origin, tools: vec![] }),
    }
}

/// The draft with `s` in it (or without its kind, when it's empty).
fn put(ed: &Rc<Editor>, s: AppSub) {
    let empty = match &s {
        AppSub::Vscode(v) => v.extensions.is_empty(),
        AppSub::Tools(t) => t.tools.is_empty(),
        AppSub::Browser(b) => b.webapps.is_empty() && b.mode == "browser",
    };
    let mut d = draft_of(ed);
    d.subs.retain(|x| x.key() != s.key());
    if !empty {
        d.subs.push(s);
    }
    ed.set(Module::Extensions(d));
}

fn has(d: &ExtensionsModule, i: &Item) -> bool {
    match i {
        Item::WebApp(w) => browser_of(d).is_some_and(|b| b.webapps.iter().any(|x| x.id == w.id)),
        Item::Extension(id) => vscode_of(d).is_some_and(|v| v.extensions.contains(id)),
        Item::Tool(id) => tools_of(d).is_some_and(|t| t.tools.contains(id)),
    }
}

fn add(ed: &Rc<Editor>, i: &Item) {
    let d = draft_of(ed);
    if has(&d, i) {
        return;
    }
    let mut s = sub(&d, item_key(i));
    match (&mut s, i) {
        (AppSub::Browser(b), Item::WebApp(w)) => b.webapps.push(w.clone()),
        (AppSub::Vscode(v), Item::Extension(id)) => v.extensions.push(id.clone()),
        (AppSub::Tools(t), Item::Tool(id)) => t.tools.push(id.clone()),
        _ => return,
    }
    put(ed, s);
}

fn remove(ed: &Rc<Editor>, i: &Item) {
    let mut s = sub(&draft_of(ed), item_key(i));
    match (&mut s, i) {
        (AppSub::Browser(b), Item::WebApp(w)) => b.webapps.retain(|x| x.id != w.id),
        (AppSub::Vscode(v), Item::Extension(id)) => v.extensions.retain(|x| x != id),
        (AppSub::Tools(t), Item::Tool(id)) => t.tools.retain(|x| x != id),
        _ => return,
    }
    put(ed, s);
}

/// A site typed in ("notion.so"): a web app named after it.
pub(crate) fn site_webapp(text: &str) -> Option<WebApp> {
    let text = text.trim();
    let url = if text.starts_with("http://") || text.starts_with("https://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?.split(':').next()?.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if host.is_empty() || host.contains(char::is_whitespace) {
        return None;
    }
    let first = host.split('.').next()?;
    let mut name: String = first.chars().take(1).flat_map(char::to_uppercase).collect();
    name.push_str(&first.chars().skip(1).collect::<String>());
    Some(WebApp { id: catalog::webapp_id(&name), name, url })
}

/// A marketplace id: publisher.name.
fn is_extension_id(s: &str) -> bool {
    let Some((publisher, name)) = s.split_once('.') else { return false };
    !publisher.is_empty()
        && !name.is_empty()
        && publisher.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The setups this template can have: WadBrowser's and VS Code's when it
/// has them; the command line's always.
fn keys_of(ed: &Editor) -> Vec<&'static str> {
    catalog::setups_for(ed.live().apps())
}

pub fn build(ed: &Rc<Editor>) -> gtk::Widget {
    let keys = keys_of(ed);
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    root.add_css_class("ext-editor");

    // What can go in, on the left.
    let pal = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pal.append(&panel_title("Add"));
    let n = note("Drag onto its app, or click.");
    n.set_margin_bottom(6);
    pal.append(&n);
    for k in &keys {
        pal.append(&palette_section(ed, k));
    }
    let pal_scroll = scroller(&pal);
    pal_scroll.add_css_class("panel");
    pal_scroll.add_css_class("palette-ext");
    root.append(&wad_gtk::Width::new(&pal_scroll, 300));

    // The windows.
    let main = gtk::Box::new(gtk::Orientation::Vertical, 4);
    main.add_css_class("ext-main");
    let name = ui::entry("Name");
    name.set_text(&draft_of(ed).name);
    let w = Rc::downgrade(ed);
    name.connect_changed(move |e| {
        if let Some(ed) = w.upgrade() {
            let mut d = draft_of(&ed);
            d.name = e.text().to_string();
            ed.set_quiet(Module::Extensions(d));
        }
    });
    let f = field("Name", &name);
    f.set_halign(gtk::Align::Start);
    name.set_width_chars(36);
    main.append(&f);
    let wins = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let main_scroll = scroller(&main);
    main_scroll.set_hexpand(true);
    for k in &keys {
        let w = big_win(ed, k);
        if ed.focus.as_deref() == Some(*k) {
            w.add_css_class("focused");
            scroll_into_view(&main_scroll, &w);
        }
        wins.append(&w);
    }
    main.append(&wins);
    let strays = gtk::Box::new(gtk::Orientation::Vertical, 0);
    live_box(ed, &strays, stray_notes);
    main.append(&strays);
    root.append(&main_scroll);
    root.upcast()
}

fn palette_section(ed: &Rc<Editor>, key: &'static str) -> gtk::Box {
    let s = gtk::Box::new(gtk::Orientation::Vertical, 6);
    s.add_css_class("pal-section");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.append(&img(app_sub_info(key).2, 14));
    head.append(&group_label(match key {
        "wadbrowser" => "Web apps",
        "vscode" => "VS Code extensions",
        _ => "Tools",
    }));
    s.append(&head);
    let items = gtk::FlowBox::new();
    items.add_css_class("pal-items");
    items.set_selection_mode(gtk::SelectionMode::None);
    items.set_column_spacing(5);
    items.set_row_spacing(5);
    items.set_max_children_per_line(20);
    items.set_homogeneous(false);
    s.append(&items);
    let wi = items.downgrade();
    let fill = move |ed: &Rc<Editor>| {
        let Some(items) = wi.upgrade() else { return };
        items.remove_all();
        let d = draft_of(ed);
        let left: Vec<Item> = offered(key).into_iter().filter(|i| !has(&d, i)).collect();
        if left.is_empty() {
            let e = gtk::Label::new(Some("All added"));
            e.add_css_class("empty-note");
            items.insert(&e, -1);
        }
        for i in left {
            items.insert(&pal_item(ed, &i), -1);
        }
    };
    fill(ed);
    ed.on_redraw(fill);
    // Something not offered: typed in.
    let form = match key {
        "wadbrowser" => Some(ui::entry("Another site, e.g. notion.so")),
        "vscode" => Some(ui::entry("Marketplace id, e.g. ms-toolsai.jupyter")),
        _ => None,
    };
    if let Some(e) = form {
        e.add_css_class("small");
        let w = Rc::downgrade(ed);
        e.connect_activate(move |e| {
            let Some(ed) = w.upgrade() else { return };
            let t = e.text().trim().to_string();
            let item = if key == "wadbrowser" {
                site_webapp(&t).map(Item::WebApp)
            } else {
                is_extension_id(&t).then(|| Item::Extension(t.clone()))
            };
            if let Some(i) = item {
                add(&ed, &i);
                e.set_text("");
            } else if key == "vscode" {
                e.set_text("");
            }
        });
        s.append(&e);
    }
    s
}

/// What's offered for a setup: web apps the board's wadspaces have (and a
/// few common ones), the catalog's extensions (and the board's), its tools.
fn offered(key: &str) -> Vec<Item> {
    let stacks = crate::pages::builder::state::read(|s| s.board.stacks.clone());
    match key {
        "wadbrowser" => {
            let mut out: Vec<WebApp> = vec![];
            for st in &stacks {
                for w in st.browser().map(|b| b.webapps.as_slice()).unwrap_or(&[]) {
                    match out.iter_mut().find(|x| x.id == w.id) {
                        Some(x) => *x = w.clone(),
                        None => out.push(w.clone()),
                    }
                }
            }
            for (id, name, url) in COMMON {
                if !out.iter().any(|x| x.id == id) {
                    out.push(WebApp { id: id.into(), name: name.into(), url: url.into() });
                }
            }
            out.into_iter().map(Item::WebApp).collect()
        }
        "vscode" => {
            let mut ids: Vec<String> = catalog::extensions().into_iter().map(|e| e.id).collect();
            for st in &stacks {
                for id in st.vscode().map(|v| v.extensions.as_slice()).unwrap_or(&[]) {
                    if !ids.contains(id) {
                        ids.push(id.clone());
                    }
                }
            }
            ids.into_iter().map(Item::Extension).collect()
        }
        _ => catalog::tools().into_iter().map(|(id, _)| Item::Tool(id)).collect(),
    }
}

fn pal_item(ed: &Rc<Editor>, i: &Item) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    b.add_css_class("pal-item");
    b.append(&item_face(i));
    let plus = Icon::new("plus", 11);
    plus.add_css_class("pal-plus");
    plus.set_valign(gtk::Align::Center);
    b.append(&plus);
    b.set_tooltip_text(match i {
        Item::WebApp(w) => Some(w.url.as_str()),
        Item::Extension(id) => Some(id.as_str()),
        Item::Tool(_) => None,
    });
    let it = i.clone();
    drag::draggable(&b, move |_| Some((Payload::Item(it.clone()), faces::ghost(&item_face(&it)))));
    let click = gtk::GestureClick::new();
    let (w, it) = (Rc::downgrade(ed), i.clone());
    click.connect_released(move |_, _, _, _| {
        if let Some(ed) = w.upgrade() {
            add(&ed, &it);
        }
    });
    b.add_controller(click);
    b.upcast()
}

/// An app's window, holding what it's set up with.
fn big_win(ed: &Rc<Editor>, key: &'static str) -> gtk::Box {
    let (_, app, icon) = app_sub_info(key);
    let win = gtk::Box::new(gtk::Orientation::Vertical, 0);
    win.add_css_class("big-win");
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar.add_css_class("big-win-bar");
    bar.append(&img(icon, 18));
    let a = gtk::Label::new(Some(app));
    a.add_css_class("big-win-app");
    bar.append(&a);
    let count = gtk::Label::new(None);
    count.add_css_class("win-count");
    bar.append(&count);
    let spring = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spring.set_hexpand(true);
    bar.append(&spring);
    if key == "wadbrowser" {
        let mode = match sub(&draft_of(ed), key) {
            AppSub::Browser(b) => b.mode,
            _ => "browser".into(),
        };
        let w = Rc::downgrade(ed);
        let seg = ui::segmented(&[("browser", None, "Browser"), ("focus", None, "Focus")], &mode, move |m| {
            let Some(ed) = w.upgrade() else { return };
            if let AppSub::Browser(mut b) = sub(&draft_of(&ed), "wadbrowser") {
                b.mode = m.to_string();
                put(&ed, AppSub::Browser(b));
            }
        });
        seg.add_css_class("small-seg");
        seg.set_tooltip_text(Some("Browser: with an address bar · Focus: web apps only"));
        bar.append(&seg);
    }
    win.append(&bar);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("big-win-body");
    let items = gtk::FlowBox::new();
    items.add_css_class("win-items");
    items.set_selection_mode(gtk::SelectionMode::None);
    items.set_column_spacing(8);
    items.set_row_spacing(8);
    items.set_max_children_per_line(30);
    body.append(&items);
    win.append(&body);
    let (wi, wc) = (items.downgrade(), count.downgrade());
    let fill = move |ed: &Rc<Editor>| {
        let (Some(items), Some(count)) = (wi.upgrade(), wc.upgrade()) else { return };
        items.remove_all();
        let list: Vec<Item> = match sub(&draft_of(ed), key) {
            AppSub::Browser(b) => b.webapps.into_iter().map(Item::WebApp).collect(),
            AppSub::Vscode(v) => v.extensions.into_iter().map(Item::Extension).collect(),
            AppSub::Tools(t) => t.tools.into_iter().map(Item::Tool).collect(),
        };
        count.set_text(&list.len().to_string());
        if list.is_empty() {
            let e = gtk::Label::new(Some(match key {
                "wadbrowser" => "Drag web apps here",
                "vscode" => "Drag extensions here",
                _ => "Drag tools here",
            }));
            e.add_css_class("empty-note");
            items.insert(&e, -1);
        }
        for i in list {
            items.insert(&win_item(ed, &i), -1);
        }
    };
    fill(ed);
    ed.on_redraw(fill);
    drag::target(&win, ItemTarget { ed: Rc::downgrade(ed), key });
    win
}

fn win_item(ed: &Rc<Editor>, i: &Item) -> gtk::Widget {
    let o = gtk::Overlay::new();
    o.add_css_class("win-item");
    let face = item_face(i);
    o.set_child(Some(&face));
    if let Item::WebApp(w) = i {
        o.set_tooltip_text(Some(&w.url));
    }
    let x = gtk::Button::new();
    x.add_css_class("item-x");
    x.set_child(Some(&Icon::new("x", 10)));
    x.set_tooltip_text(Some("Remove"));
    x.set_halign(gtk::Align::End);
    x.set_valign(gtk::Align::Start);
    let (w, it) = (Rc::downgrade(ed), i.clone());
    x.connect_clicked(move |_| {
        if let Some(ed) = w.upgrade() {
            remove(&ed, &it);
        }
    });
    o.add_overlay(&x);
    o.upcast()
}

/// Setups moved in for an app this template doesn't have: they do nothing.
fn stray_notes(ed: &Rc<Editor>, b: &gtk::Box) {
    let live = ed.live();
    for s in draft_of(ed).subs {
        let Err(why) = catalog::sub_fits(s.key(), Some(live.apps())) else { continue };
        let row = icon_note(
            "triangle-alert",
            &format!("{}: {why}, so it does nothing here.", app_sub_info(s.key()).0),
            "warn-note",
        );
        row.add_css_class("block");
        let rm = ui::button("Remove", Some("trash"), Variant::Ghost, Size::Sm);
        let (w, id) = (Rc::downgrade(ed), s.id().to_string());
        rm.connect_clicked(move |_| {
            if let Some(ed) = w.upgrade() {
                let mut d = draft_of(&ed);
                d.subs.retain(|x| x.id() != id);
                ed.set(Module::Extensions(d));
            }
        });
        row.append(&rm);
        b.append(&row);
    }
}

/// An app's window: what goes in it, dropped on it.
struct ItemTarget {
    ed: Weak<Editor>,
    key: &'static str,
}

impl Target for ItemTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let Payload::Item(i) = p else { return None };
        if item_key(i) == self.key {
            Verdict::yes(format!("Add {} to {}", item_name(i), app_sub_info(self.key).1))
        } else {
            Verdict::no(format!("That goes in {}", app_sub_info(item_key(i)).1))
        }
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let (Payload::Item(i), Some(ed)) = (p, self.ed.upgrade()) {
            add(&ed, i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_typed_in_is_a_web_app_named_after_it() {
        let w = site_webapp("notion.so").unwrap();
        assert_eq!((w.id.as_str(), w.name.as_str(), w.url.as_str()), ("notion", "Notion", "https://notion.so"));
        let w = site_webapp("https://www.figma.com/files").unwrap();
        assert_eq!((w.name.as_str(), w.url.as_str()), ("Figma", "https://www.figma.com/files"));
        assert!(site_webapp("").is_none());
        assert!(site_webapp("not a site").is_none());
    }

    #[test]
    fn extensions_are_added_by_marketplace_id() {
        assert!(is_extension_id("ms-toolsai.jupyter"));
        assert!(is_extension_id("rust-lang.rust-analyzer"));
        assert!(!is_extension_id("jupyter"));
        assert!(!is_extension_id(".jupyter"));
        assert!(!is_extension_id("ms toolsai.jupyter"));
    }
}
