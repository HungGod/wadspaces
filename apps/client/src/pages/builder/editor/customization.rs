//! The Customization editor (CustomizationEditor.tsx): the look (name,
//! description, background), its projects, and its keys: one per app (or
//! per group of WadBrowser's apps), new or picked from the keys there are
//! of the same kind. The projects and keys are the builder's parts
//! (parts/projects.rs, parts/keys.rs), editing the draft.

use std::rc::{Rc, Weak};

use gtk::prelude::*;
use serde_json::Value;
use wad_board::catalog::{self, KeyApp, KeyLine};
use wad_board::*;
use wad_wallpaper::{Paint, Wallpaper};

use super::{Editor, blank_look, field, live_box, scroll_into_view, scroller};
use crate::pages::builder::parts::{faint, keys, projects};
use crate::pages::builder::state;
use crate::ui;
use crate::{images, thumb};

fn draft_of(ed: &Editor) -> CustomizationModule {
    match ed.draft() {
        Module::Customization(c) => c,
        _ => unreachable!("the Customization editor edits a Customization"),
    }
}

fn look(d: &CustomizationModule) -> LookSub {
    look_of(d).cloned().unwrap_or_else(blank_look)
}

/// Its name, or "Untitled".
fn name_of(d: &CustomizationModule) -> String {
    let n = look(d).name.trim().to_string();
    if n.is_empty() { "Untitled".into() } else { n }
}

fn folders(d: &CustomizationModule) -> FoldersSub {
    folders_of(d).cloned().unwrap_or_else(|| FoldersSub { id: uid("folders"), origin: name_of(d), mounts: vec![] })
}

fn secrets(d: &CustomizationModule) -> SecretsSub {
    secrets_of(d).cloned().unwrap_or_else(|| SecretsSub {
        id: uid("secrets"),
        origin: name_of(d),
        by_slot: Default::default(),
        groups: None,
    })
}

/// The draft with `s` in it, in place of its kind.
fn with(d: &CustomizationModule, s: CustomSub) -> Module {
    let mut d = d.clone();
    d.subs.retain(|x| x.key() != s.key());
    d.subs.push(s);
    Module::Customization(d)
}

fn put(ed: &Rc<Editor>, s: CustomSub) {
    ed.set(with(&draft_of(ed), s));
}

/// Its sections, to open on: (focus, widget).
struct Sections {
    look: gtk::Box,
    folders: gtk::Box,
    secrets: gtk::Box,
}

pub fn build(ed: &Rc<Editor>) -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.add_css_class("cz-editor");
    let sw = scroller(&content);

    // The look.
    let look_panel = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    look_panel.add_css_class("panel");
    look_panel.add_css_class("cz-look");
    let fields = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fields.set_hexpand(true);
    let d = draft_of(ed);
    let l = look(&d);
    let name = ui::entry("Name this wadspace");
    name.set_text(&l.name);
    let w = Rc::downgrade(ed);
    name.connect_changed(move |e| {
        if let Some(ed) = w.upgrade() {
            let d = draft_of(&ed);
            let mut l = look(&d);
            l.name = e.text().to_string();
            ed.set_quiet(with(&d, CustomSub::Look(l)));
        }
    });
    fields.append(&field("Name", &name));
    let desc = ui::entry("Optional");
    desc.set_text(&l.description);
    let w = Rc::downgrade(ed);
    desc.connect_changed(move |e| {
        if let Some(ed) = w.upgrade() {
            let d = draft_of(&ed);
            let mut l = look(&d);
            l.description = e.text().to_string();
            ed.set_quiet(with(&d, CustomSub::Look(l)));
        }
    });
    fields.append(&field("Description", &desc));
    let w = Rc::downgrade(ed);
    let picker = crate::wallpaper_picker::picker(l.wallpaper.clone(), move |v| {
        if let Some(ed) = w.upgrade() {
            let d = draft_of(&ed);
            let mut l = look(&d);
            l.wallpaper = v;
            put(&ed, CustomSub::Look(l));
        }
    });
    fields.append(&field("Background", &wad_gtk::Width::new(&picker, 456)));
    // Title bars on its windows: minimize, maximize and close (off: every
    // window fills the wadspace with none). Set when it next starts.
    let bars_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bars_row.add_css_class("cz-switch");
    let bars_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    bars_text.set_hexpand(true);
    let bars_title = gtk::Label::new(Some("Window title bars"));
    bars_title.set_xalign(0.0);
    let bars_hint = faint("Minimize, maximize and close on each window. Takes effect when the wadspace next starts.");
    bars_text.append(&bars_title);
    bars_text.append(&bars_hint);
    let bars = gtk::Switch::new();
    bars.set_valign(gtk::Align::Center);
    bars.set_active(l.title_bars);
    let w = Rc::downgrade(ed);
    bars.connect_active_notify(move |s| {
        if let Some(ed) = w.upgrade() {
            let d = draft_of(&ed);
            let mut l = look(&d);
            l.title_bars = s.is_active();
            put(&ed, CustomSub::Look(l));
        }
    });
    bars_row.append(&bars_text);
    bars_row.append(&bars);
    bars_row.set_margin_top(12);
    fields.append(&bars_row);
    look_panel.append(&fields);
    let mini = gtk::Box::new(gtk::Orientation::Vertical, 0);
    mini.set_valign(gtk::Align::Start);
    live_box(ed, &mini, |ed, b| b.append(&mini_desktop(&ed.live())));
    look_panel.append(&mini);
    content.append(&look_panel);

    // Projects and keys, side by side.
    let cols = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    cols.set_homogeneous(true);
    cols.add_css_class("cz-cols");
    let proj = gtk::Box::new(gtk::Orientation::Vertical, 0);
    proj.add_css_class("panel");
    proj.set_valign(gtk::Align::Start);
    let t = super::panel_title("Projects");
    t.set_margin_bottom(10);
    proj.append(&t);
    let draft = Rc::new(Draft(Rc::downgrade(ed)));
    let ph: Rc<dyn projects::Host> = draft.clone();
    let projects = Rc::new(projects::Projects::default());
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    list.add_css_class("cz-list");
    let (p, h) = (projects.clone(), ph.clone());
    live_box(ed, &list, move |_, b| projects::fill(&h, &p, b));
    proj.append(&list);
    cols.append(&proj);
    let keys_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    keys_panel.add_css_class("panel");
    keys_panel.set_valign(gtk::Align::Start);
    let t = super::panel_title("Secrets");
    t.set_margin_bottom(10);
    keys_panel.append(&t);
    let kh: Rc<dyn keys::Host> = draft;
    let keys = Rc::new(keys::Keys::default());
    let kb = gtk::Box::new(gtk::Orientation::Vertical, 10);
    kb.add_css_class("keys");
    let k = keys.clone();
    live_box(ed, &kb, move |_, b| keys::fill(&kh, &k, b));
    keys.show_in(&kb);
    keys_panel.append(&kb);
    cols.append(&keys_panel);
    content.append(&cols);

    let s = Sections { look: look_panel, folders: proj, secrets: keys_panel };
    let at = match ed.focus.as_deref() {
        Some("look") => Some(&s.look),
        Some("folders") => Some(&s.folders),
        Some("secrets") => Some(&s.secrets),
        _ => None,
    };
    if let Some(w) = at {
        w.add_css_class("focused");
        scroll_into_view(&sw, w);
    }
    projects::load_repos_once(&ph);
    sw.upcast()
}

/// The wadspace's desktop as it'd be: its background, every icon.
fn mini_desktop(stack: &Stack) -> gtk::Widget {
    let t = wad_gtk::DesktopThumb::new();
    t.add_css_class("mini-desktop");
    t.set_fixed_width(320);
    t.set_radius(12.0);
    t.set_screen(1920.0, 1080.0, thumb::ZOOM);
    let wall: Wallpaper = stack
        .look()
        .and_then(|l| serde_json::from_value(l.wallpaper.clone()).ok())
        .unwrap_or_else(|| Wallpaper { kind: wad_wallpaper::Kind::Gradient, value: BLANK_WALLPAPER.into() });
    let paint = wall.paint();
    t.set_wallpaper(&paint);
    if let Paint::Image(src) = &paint {
        images::wallpaper(src, images::later(&t, |t, tex| t.set_picture(Some(tex))));
    }
    let icons = catalog::desktop_of(stack);
    t.set_icons(icons.iter().map(|i| wad_gtk::DesktopIcon { label: i.label.clone(), texture: None }).collect());
    for (n, i) in icons.iter().enumerate() {
        images::desktop_icon(&i.icon, images::later(&t, move |t, tex| t.set_icon_texture(n, tex)));
    }
    t.upcast()
}

// ─────────────────────────── projects and keys ───────────────────────────

/// The editor's draft, as the projects and keys parts edit it.
struct Draft(Weak<Editor>);

impl Draft {
    fn ed(&self) -> Option<Rc<Editor>> {
        self.0.upgrade()
    }
}

impl projects::Host for Draft {
    fn projects(&self) -> Vec<Value> {
        self.ed().map(|ed| folders(&draft_of(&ed)).mounts).unwrap_or_default()
    }

    fn set_projects(&self, list: Vec<Value>) {
        if let Some(ed) = self.ed() {
            let mut f = folders(&draft_of(&ed));
            f.mounts = list;
            put(&ed, CustomSub::Folders(f));
        }
    }

    fn elsewhere(&self) -> Vec<Value> {
        let Some(ed) = self.ed() else { return vec![] };
        state::read(|s| s.board.stacks.clone())
            .iter()
            .filter(|st| st.id != ed.stack_id)
            .flat_map(|st| st.folders().map(|f| f.mounts.clone()).unwrap_or_default())
            .collect()
    }

    fn redraw(&self) {
        if let Some(ed) = self.ed() {
            ed.redraw();
        }
    }
}

impl keys::Host for Draft {
    fn key_map(&self) -> Vec<KeyApp> {
        self.ed().map(|ed| catalog::key_map(&ed.live(), Some(&secrets(&draft_of(&ed))))).unwrap_or_default()
    }

    fn key_of(&self, line: &KeyLine) -> Option<Value> {
        let ed = self.ed()?;
        catalog::key_of(Some(&secrets(&draft_of(&ed))), line).cloned()
    }

    fn set_key(&self, line: &KeyLine, key: Option<Value>) {
        if let Some(ed) = self.ed() {
            set_key(&ed, line, key);
        }
    }

    fn regroup(&self, members: &[String], to: Option<&str>) {
        let Some(ed) = self.ed() else { return };
        let mut sub = secrets(&draft_of(&ed));
        let groups = catalog::key_groups_of(&ed.live(), Some(&sub));
        sub.groups = Some(keys::regrouped(&groups, members, to, &keys::new_group_id()));
        put(&ed, CustomSub::Secrets(sub));
    }

    fn new_group(&self) {
        if let Some(ed) = self.ed() {
            edit_groups(&ed, |gs| gs.push(KeyGroup { id: keys::new_group_id(), members: vec![], key: None }));
        }
    }

    fn remove_group(&self, id: &str) {
        if let Some(ed) = self.ed() {
            edit_groups(&ed, |gs| gs.retain(|g| g.id != id));
        }
    }

    fn rename_key(&self, id: &str, to: &str, known: &[Value]) -> Result<(), String> {
        state::edit(|b| b.rename_key(id, to, known))?;
        if let Some(ed) = self.ed() {
            rename_in_draft(&ed, id, to);
        }
        Ok(())
    }

    fn keys_of_kind(&self, known: &[Value], kind: &str) -> Vec<Value> {
        board::keys_of_kind(known, &state::read(|s| s.board.stacks.clone()), kind)
    }

    fn new_key_name(&self, app_id: &str) -> String {
        self.ed().map(|ed| new_key_name(&draft_of(&ed), app_id)).unwrap_or_default()
    }
}

fn set_key(ed: &Rc<Editor>, line: &KeyLine, key: Option<Value>) {
    let d = draft_of(ed);
    let mut sub = secrets(&d);
    match &line.group {
        Some(g) => {
            let live = ed.live();
            let mut groups = catalog::key_groups_of(&live, Some(&sub));
            for x in &mut groups {
                if x.id == g.id {
                    x.key = key.clone();
                }
            }
            sub.groups = Some(groups);
        }
        None => {
            match key {
                Some(k) => sub.by_slot.insert(line.slot.clone(), k),
                None => sub.by_slot.remove(&line.slot),
            };
        }
    }
    put(ed, CustomSub::Secrets(sub));
}

fn edit_groups(ed: &Rc<Editor>, f: impl FnOnce(&mut Vec<KeyGroup>)) {
    let d = draft_of(ed);
    let mut sub = secrets(&d);
    let mut groups = catalog::key_groups_of(&ed.live(), Some(&sub));
    f(&mut groups);
    sub.groups = Some(groups);
    put(ed, CustomSub::Secrets(sub));
}

/// A key renamed, wherever the draft keeps it.
fn rename_in_draft(ed: &Rc<Editor>, id: &str, to: &str) {
    let d = draft_of(ed);
    let mut sub = secrets(&d);
    let swap = |k: &mut Value| {
        if k["id"] == id {
            k["name"] = to.into();
        }
    };
    sub.by_slot.values_mut().for_each(swap);
    for g in sub.groups.iter_mut().flatten() {
        if let Some(k) = g.key.as_mut() {
            swap(k);
        }
    }
    put(ed, CustomSub::Secrets(sub));
}

/// What a new key is called: <Wadspace>-<what signs in with it>.
fn new_key_name(d: &CustomizationModule, app_id: &str) -> String {
    let base: String = name_of(d).chars().filter(|c| !c.is_whitespace()).collect();
    format!("{}-{app_id}", if base.is_empty() { "Wadspace" } else { &base })
}
