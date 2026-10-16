//! The Customization editor (CustomizationEditor.tsx): the look (name,
//! description, background), its projects, and its keys: one per app (or
//! per group of WadBrowser's apps), new or picked from the keys there are
//! of the same kind.
//!
//! Projects are cloned or copied in: GitHub repositories are cloned (New
//! project makes one on GitHub), and folders or files on this machine are
//! copied (wadd makes the copy now; the wadspace takes it in when it
//! starts). Or, when the user picks Mount instead of Copy, a folder or file
//! on this machine is mounted read-write: changes save to the machine, and
//! the wadspace has it only on this machine.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::{glib, prelude::*};
use serde_json::{Value, json};
use wad_board::catalog::{self, KeyLine};
use wad_board::*;
use wad_gtk::Icon;
use wad_proto::github::{Repo, Repos};
use wad_proto::v1::{Browse, CopyMade, Project, ProjectSource};
use wad_wadd_client::Method;
use wad_wallpaper::{Paint, Wallpaper};

use super::{Editor, blank_look, field, group_label, live_box, note, scroll_into_view, scroller};
use crate::drag::{self, Mods, Payload, Target, Verdict};
use crate::pages::builder::faces::{self, img};
use crate::pages::builder::state;
use crate::pages::machines::{get, send};
use crate::store::store;
use crate::ui::{self, Size, Variant};
use crate::{images, signins, thumb};

thread_local! {
    /// The machine's GitHub repositories (None: not read yet), read once
    /// the first time a Customization opens.
    static REPOS: RefCell<Option<Vec<Repo>>> = const { RefCell::new(None) };
}

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
    let projects = Rc::new(Projects::default());
    ed.keep(projects.clone());
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    list.add_css_class("cz-list");
    let p = projects.clone();
    live_box(ed, &list, move |ed, b| fill_projects(ed, &p, b));
    proj.append(&list);
    cols.append(&proj);
    let keys_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    keys_panel.add_css_class("panel");
    keys_panel.set_valign(gtk::Align::Start);
    let t = super::panel_title("Secrets");
    t.set_margin_bottom(10);
    keys_panel.append(&t);
    let keys = Rc::new(Keys::default());
    ed.keep(keys.clone());
    let kb = gtk::Box::new(gtk::Orientation::Vertical, 10);
    kb.add_css_class("keys");
    let k = keys.clone();
    live_box(ed, &kb, move |ed, b| fill_keys(ed, &k, b));
    *keys.view.borrow_mut() = kb.downgrade();
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
    if REPOS.with(|r| r.borrow().is_none()) {
        load_repos(Rc::downgrade(ed));
    }
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

// ─────────────────────────────── projects ───────────────────────────────

/// The projects part's own state.
#[derive(Default)]
struct Projects {
    /// New project's name, while it's being made.
    creating: RefCell<Option<String>>,
    problem: RefCell<String>,
    /// Copying in: (files or a folder, where it's at).
    picking: RefCell<Option<(bool, Option<String>)>>,
    picked: RefCell<Vec<String>>,
    copying: Cell<bool>,
    /// The picker mounts what's picked rather than copying it.
    mounting: Cell<bool>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// What a project is, to tell two apart: a repo, a mount (by what it
/// mounts), or a copy (by its id).
fn project_key(m: &Value) -> String {
    match s(m, "kind") {
        "github" => format!("github:{}", s(m, "repo").to_lowercase()),
        "mount" => format!("mount:{}", m["source"]["paths"][0].as_str().unwrap_or("")),
        _ => format!("copy:{}", s(m, "id")),
    }
}

/// A project's icon: a repo, a mount, or a copy.
fn project_icon(m: &Value) -> &'static str {
    match s(m, "kind") {
        "github" => "folder-git-2",
        "mount" => "folder-symlink",
        _ => "hard-drive",
    }
}

/// A folder name that's free at ~/Desktop: `want` cleaned, then "-2", "-3"…
fn free_as(want: &str, mounts: &[Value]) -> String {
    let clean = slug(want);
    let clean = if clean.is_empty() { "Project".to_string() } else { clean };
    let mut name = clean.clone();
    let mut n = 2;
    while mounts.iter().any(|m| s(m, "as").eq_ignore_ascii_case(&name)) {
        name = format!("{clean}-{n}");
        n += 1;
    }
    name
}

/// Runs of anything but letters, digits, `.`, `_` and `-` as one `-`, trimmed.
fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn size_of(bytes: u64) -> String {
    if bytes > 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else {
        format!("{} KB", ((bytes as f64 / 1e3).round() as u64).max(1))
    }
}

/// What a project is, in words.
fn project_text(m: &Value) -> String {
    if s(m, "kind") == "github" {
        return format!("cloned from {}", s(m, "repo"));
    }
    if s(m, "kind") == "mount" {
        let path = m["source"]["paths"][0].as_str().unwrap_or("");
        return format!("mounted from {path} · changes save to that machine");
    }
    let drive = &m["source"]["drive"];
    let from = if drive.is_object() {
        let label = if s(drive, "label").is_empty() { s(drive, "uuid") } else { s(drive, "label") };
        format!("the drive {label}")
    } else {
        "this machine".to_string()
    };
    let files = m["files"].as_u64().unwrap_or(0);
    let what = if files > 0 {
        format!(" · {files} file{}, {}", if files == 1 { "" } else { "s" }, size_of(m["bytes"].as_u64().unwrap_or(0)))
    } else {
        " at its first start".into()
    };
    format!("copied from {from}{what}")
}

fn project_face(m: &Value) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.add_css_class("mount-face");
    b.append(&Icon::new(project_icon(m), 13));
    let n = gtk::Label::new(Some(s(m, "as")));
    n.add_css_class("proj-name");
    b.append(&n);
    let t = gtk::Label::new(Some(&project_text(m)));
    t.add_css_class("faint");
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    t.set_xalign(0.0);
    t.set_hexpand(true);
    b.append(&t);
    b
}

fn add_mount(ed: &Rc<Editor>, m: Value) {
    let d = draft_of(ed);
    let mut f = folders(&d);
    let mut m = m;
    m["as"] = free_as(s(&m, "as"), &f.mounts).into();
    f.mounts.push(m);
    put(ed, CustomSub::Folders(f));
}

fn cloned(d: &CustomizationModule, repo: &str) -> bool {
    folders(d).mounts.iter().any(|m| s(m, "kind") == "github" && s(m, "repo").eq_ignore_ascii_case(repo))
}

fn clone_repo(ed: &Rc<Editor>, repo: &str) {
    if cloned(&draft_of(ed), repo) {
        return;
    }
    let as_ = repo.rsplit('/').next().unwrap_or(repo);
    add_mount(ed, json!({ "id": uid("project"), "kind": "github", "repo": repo, "as": as_ }));
}

/// The machine's repositories, read (again): what the open editor shows.
fn load_repos(ed: Weak<Editor>) {
    glib::spawn_future_local(async move {
        let list = get::<Repos>("/v1/github/repos".into()).await.map(|r| r.repos).unwrap_or_default();
        REPOS.with(|r| r.replace(Some(list)));
        if let Some(ed) = ed.upgrade() {
            ed.redraw();
        }
    });
}

fn fill_projects(ed: &Rc<Editor>, p: &Rc<Projects>, b: &gtk::Box) {
    let d = draft_of(ed);
    let f = folders(&d);
    let n = note(
        "Cloned or copied into the wadspace: the files live inside it, at ~/Desktop. A mounted folder stays \
         on this machine: changes save here, and the wadspace has it only here.",
    );
    n.add_css_class("proj-note");
    b.append(&n);
    for m in &f.mounts {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("cz-row");
        let face = project_face(m);
        face.set_hexpand(true);
        row.append(&face);
        let rm = ui::icon_button("trash", "Take it out of this wadspace (its files there stay)", 12);
        let (w, id) = (Rc::downgrade(ed), s(m, "id").to_string());
        rm.connect_clicked(move |_| {
            if let Some(ed) = w.upgrade() {
                let mut f = folders(&draft_of(&ed));
                f.mounts.retain(|x| s(x, "id") != id);
                put(&ed, CustomSub::Folders(f));
            }
        });
        row.append(&rm);
        b.append(&row);
    }
    if f.mounts.is_empty() {
        let e = gtk::Label::new(Some("No projects yet."));
        e.add_css_class("empty-note");
        e.set_xalign(0.0);
        b.append(&e);
    }

    let g = group_label("Your GitHub repositories");
    g.set_margin_top(8);
    b.append(&g);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 5);
    list.add_css_class("gh-list");
    match REPOS.with(|r| r.borrow().clone()) {
        None => list.append(&faint("Loading…")),
        Some(rs) if rs.is_empty() => {
            list.append(&faint("None to show: sign this machine in to GitHub (Secrets, GitHub sign-in)."))
        }
        Some(rs) => {
            for r in rs {
                list.append(&repo_row(ed, &d, &r));
            }
        }
    }
    b.append(&list);
    b.append(&new_project(ed, p));

    let g = group_label("From this machine");
    g.set_margin_top(8);
    b.append(&g);
    let copy = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    copy.add_css_class("copy-row");
    let busy = p.copying.get();
    let folder =
        ui::button(if busy { "Copying…" } else { "Add a folder" }, Some("hard-drive"), Variant::Ghost, Size::Sm);
    let files = ui::button("Add files", Some("plus"), Variant::Ghost, Size::Sm);
    for (btn, want_files) in [(&folder, false), (&files, true)] {
        btn.set_sensitive(!busy);
        let (w, p) = (Rc::downgrade(ed), p.clone());
        btn.connect_clicked(move |_| {
            p.picking.replace(Some((want_files, None)));
            p.picked.borrow_mut().clear();
            if let Some(ed) = w.upgrade() {
                ed.redraw();
            }
        });
        copy.append(btn);
    }
    b.append(&copy);

    // What other wadspaces have, to add here too.
    let mut seen: Vec<String> = f.mounts.iter().map(project_key).collect();
    let mut elsewhere: Vec<Value> = vec![];
    for st in state::read(|s| s.board.stacks.clone()) {
        if st.id == ed.stack_id {
            continue;
        }
        for m in st.folders().map(|f| f.mounts.clone()).unwrap_or_default() {
            if !seen.contains(&project_key(&m)) {
                seen.push(project_key(&m));
                elsewhere.push(m);
            }
        }
    }
    if !elsewhere.is_empty() {
        let sug = gtk::FlowBox::new();
        sug.add_css_class("suggest");
        sug.set_selection_mode(gtk::SelectionMode::None);
        sug.set_max_children_per_line(12);
        sug.set_column_spacing(5);
        sug.set_row_spacing(5);
        let l = gtk::Label::new(Some("In other wadspaces:"));
        l.add_css_class("faint");
        sug.insert(&l, -1);
        for m in elsewhere {
            let chip = ui::button(s(&m, "as"), Some(project_icon(&m)), Variant::Ghost, Size::Sm);
            chip.add_css_class("sug");
            chip.set_tooltip_text(Some(&match s(&m, "kind") {
                "github" => format!("Clone {}", s(&m, "repo")),
                "mount" => format!("Mount {} here too", s(&m, "as")),
                _ => format!("Copy {} in too", s(&m, "as")),
            }));
            let w = Rc::downgrade(ed);
            chip.connect_clicked(move |_| {
                if let Some(ed) = w.upgrade() {
                    add_mount(&ed, m.clone());
                }
            });
            sug.insert(&chip, -1);
        }
        b.append(&sug);
    }
    if p.picking.borrow().is_some() {
        b.append(&machine_picker(ed, p));
    }
}

fn faint(text: &str) -> gtk::Label {
    let l = ui::wrapping(text);
    l.add_css_class("faint");
    l.add_css_class("small");
    l
}

fn repo_row(ed: &Rc<Editor>, d: &CustomizationModule, r: &Repo) -> gtk::Button {
    let on = cloned(d, &r.full_name);
    let b = gtk::Button::new();
    b.add_css_class("gh-proj");
    if on {
        b.add_css_class("on");
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&Icon::new("folder-git-2", 13));
    let n = gtk::Label::new(Some(&r.name));
    n.add_css_class("gh-name");
    n.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&n);
    if r.private {
        let p = gtk::Label::new(Some("private"));
        p.add_css_class("gh-private");
        row.append(&p);
    }
    let meta = gtk::Label::new(Some(&r.pushed_at.as_deref().map(day).unwrap_or_default()));
    meta.add_css_class("gh-meta");
    meta.set_hexpand(true);
    meta.set_xalign(1.0);
    row.append(&meta);
    let act = gtk::Label::new(Some(if on { "Cloned" } else { "Clone" }));
    act.add_css_class("gh-act");
    row.append(&act);
    b.set_child(Some(&row));
    b.set_tooltip_text(Some(if r.description.is_empty() { &r.full_name } else { &r.description }));
    b.set_sensitive(!on);
    let (w, repo) = (Rc::downgrade(ed), r.full_name.clone());
    b.connect_clicked(move |_| {
        if let Some(ed) = w.upgrade() {
            clone_repo(&ed, &repo);
        }
    });
    b
}

/// "2026-10-14T…" as the date, the way this machine writes dates.
fn day(iso: &str) -> String {
    glib::DateTime::from_iso8601(iso, None)
        .ok()
        .and_then(|d| d.to_local().ok())
        .and_then(|d| d.format("%x").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn new_project(ed: &Rc<Editor>, p: &Rc<Projects>) -> gtk::Widget {
    let Some(text) = p.creating.borrow().clone() else {
        let b = ui::button("New project", Some("plus"), Variant::Ghost, Size::Sm);
        b.add_css_class("new-proj-btn");
        b.set_halign(gtk::Align::Start);
        b.set_margin_top(4);
        let (w, p) = (Rc::downgrade(ed), p.clone());
        b.connect_clicked(move |_| {
            p.creating.replace(Some(String::new()));
            p.problem.borrow_mut().clear();
            if let Some(ed) = w.upgrade() {
                ed.redraw();
            }
        });
        return b.upcast();
    };
    let form = gtk::Box::new(gtk::Orientation::Vertical, 4);
    form.set_margin_top(4);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let e = ui::entry("Repository name");
    e.add_css_class("small");
    e.add_css_class("esc-local");
    e.set_hexpand(true);
    e.set_text(&text);
    row.append(&e);
    let make = ui::button("Create and clone", None, Variant::Secondary, Size::Sm);
    row.append(&make);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Sm);
    row.append(&cancel);
    form.append(&row);
    let problem = p.problem.borrow().clone();
    let hint = gtk::Label::new(Some(if !problem.is_empty() {
        &problem
    } else if text.trim().is_empty() {
        ""
    } else {
        "Makes a private repository on GitHub"
    }));
    hint.add_css_class(if problem.is_empty() { "faint" } else { "name-clash" });
    hint.add_css_class("small");
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    form.append(&hint);
    let (pp, h) = (p.clone(), hint.downgrade());
    e.connect_changed(move |e| {
        pp.creating.replace(Some(e.text().to_string()));
        pp.problem.borrow_mut().clear();
        if let Some(h) = h.upgrade() {
            h.remove_css_class("name-clash");
            h.add_css_class("faint");
            h.set_text(if e.text().trim().is_empty() { "" } else { "Makes a private repository on GitHub" });
        }
    });
    let go = {
        let (w, p) = (Rc::downgrade(ed), p.clone());
        move || {
            let name = p.creating.borrow().clone().unwrap_or_default();
            let (w, p) = (w.clone(), p.clone());
            glib::spawn_future_local(async move {
                let made = create_project(&name).await;
                let Some(ed) = w.upgrade() else { return };
                match made {
                    Ok(repo) => {
                        p.creating.replace(None);
                        p.problem.borrow_mut().clear();
                        clone_repo(&ed, &repo);
                    }
                    Err(why) => {
                        p.problem.replace(why);
                        ed.redraw();
                    }
                }
            });
        }
    };
    let g = go.clone();
    e.connect_activate(move |_| g());
    make.connect_clicked(move |_| go());
    let (w, p) = (Rc::downgrade(ed), p.clone());
    cancel.connect_clicked(move |_| {
        p.creating.replace(None);
        p.problem.borrow_mut().clear();
        if let Some(ed) = w.upgrade() {
            ed.redraw();
        }
    });
    if text.is_empty() {
        let e = e.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(e) = e.upgrade() {
                e.grab_focus();
            }
        });
    }
    form.upcast()
}

/// Makes a private repository on GitHub (on this machine): its owner/name,
/// or why it can't.
async fn create_project(name: &str) -> Result<String, String> {
    let name = name.trim();
    let slug = slug(name);
    if slug.is_empty() {
        return Err("Give it a name".into());
    }
    let have = REPOS.with(|r| r.borrow().iter().flatten().any(|x| x.name.eq_ignore_ascii_case(&slug)));
    if have {
        return Err(format!("You already have a repository called {slug}"));
    }
    let p = send::<Project>(Method::POST, "/v1/github/repos".into(), Some(json!({ "name": slug, "private": true })))
        .await?;
    let repo = match p.source {
        Some(ProjectSource::Git { url, .. }) => wad_core::projects::repo_full_name(&url),
        _ => slug,
    };
    crate::toast::show(&format!("Made {repo} on GitHub"), None, crate::toast::Tone::Info);
    REPOS.with(|r| r.replace(None));
    load_repos(super::current().as_ref().map(Rc::downgrade).unwrap_or_default());
    Ok(repo)
}

/// What the picker's button says: what's picked, copied or mounted.
fn go_text(files: bool, mounting: bool, picked: usize) -> String {
    match (files, mounting) {
        (false, false) => "Copy this folder".into(),
        (false, true) => "Mount this folder".into(),
        (true, false) if picked == 0 => "Copy files".into(),
        (true, false) => format!("Copy {picked} file{}", if picked == 1 { "" } else { "s" }),
        (true, true) if picked > 1 => "Mount one file at a time".into(),
        (true, true) => "Mount this file".into(),
    }
}

/// Whether the picker's button can go with these files picked: any to copy,
/// one to mount.
fn go_ready(mounting: bool, picked: usize) -> bool {
    if mounting { picked == 1 } else { picked > 0 }
}

/// Folders (and files) on this machine, inside the places wadd lets you
/// copy from: a folder, or files, to copy in or (Mount) to mount.
fn machine_picker(ed: &Rc<Editor>, p: &Rc<Projects>) -> gtk::Widget {
    let (files, at) = p.picking.borrow().clone().unwrap_or((false, None));
    let mounting = p.mounting.get();
    let pick = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pick.add_css_class("picker");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("picker-head");
    head.append(&gtk::Label::new(Some(if files { "Add files" } else { "Add a folder" })));
    let path = gtk::Label::new(Some(at.as_deref().unwrap_or("Places you can copy from")));
    path.add_css_class("picker-path");
    path.set_hexpand(true);
    path.set_xalign(0.0);
    path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    head.append(&path);
    let x = ui::icon_button("x", "Close", 13);
    let (w, pp) = (Rc::downgrade(ed), p.clone());
    x.connect_clicked(move |_| {
        pp.picking.replace(None);
        if let Some(ed) = w.upgrade() {
            ed.redraw();
        }
    });
    head.append(&x);
    pick.append(&head);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("picker-list");
    list.append(&faint("Loading…"));
    let ls = gtk::ScrolledWindow::new();
    ls.set_hscrollbar_policy(gtk::PolicyType::Never);
    ls.set_max_content_height(220);
    ls.set_propagate_natural_height(true);
    ls.set_child(Some(&list));
    pick.append(&ls);
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.add_css_class("picker-foot");
    // Copy (the default) or Mount: the machine's own folder, read-write.
    let how = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    how.add_css_class("linked");
    how.add_css_class("copy-or-mount");
    let copy_b = gtk::ToggleButton::with_label("Copy");
    let mount_b = gtk::ToggleButton::with_label("Mount");
    mount_b.set_group(Some(&copy_b));
    copy_b.set_tooltip_text(Some("Copied into the wadspace: its own files, wherever it starts"));
    mount_b.set_tooltip_text(Some("This machine's own folder or file: changes save here, and only here has it"));
    (if mounting { &mount_b } else { &copy_b }).set_active(true);
    for (b, yes) in [(&copy_b, false), (&mount_b, true)] {
        b.add_css_class("small");
        let (w, pp) = (Rc::downgrade(ed), p.clone());
        b.connect_toggled(move |b| {
            if b.is_active() && pp.mounting.get() != yes {
                pp.mounting.set(yes);
                if let Some(ed) = w.upgrade() {
                    ed.redraw();
                }
            }
        });
        how.append(b);
    }
    foot.append(&how);
    let picked_now = p.picked.borrow().len();
    let go = ui::button(&go_text(files, mounting, picked_now), None, Variant::Secondary, Size::Sm);
    go.set_hexpand(true);
    go.set_halign(gtk::Align::End);
    go.set_sensitive(files && go_ready(mounting, picked_now));
    foot.append(&go);
    pick.append(&foot);

    let mut q = vec![];
    if let Some(a) = &at {
        q.push(format!("path={}", ui::enc(a)));
    }
    if files {
        q.push("files=true".into());
    }
    let url = if q.is_empty() { "/v1/browse".to_string() } else { format!("/v1/browse?{}", q.join("&")) };
    let (w, pp, lw, gw) = (Rc::downgrade(ed), p.clone(), list.downgrade(), go.downgrade());
    glib::spawn_future_local(async move {
        let r = get::<Browse>(url).await;
        let (Some(list), Some(go), Some(ed)) = (lw.upgrade(), gw.upgrade(), w.upgrade()) else { return };
        ui::clear(&list);
        let b = match r {
            Ok(b) => b,
            Err(e) => {
                let l = faint(&e);
                l.add_css_class("name-clash");
                list.append(&l);
                return;
            }
        };
        let row = |icon: &'static str, text: &str| {
            let btn = gtk::Button::new();
            btn.add_css_class("picker-row");
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            r.append(&Icon::new(icon, 13));
            let l = gtk::Label::new(Some(text));
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            r.append(&l);
            btn.set_child(Some(&r));
            btn
        };
        let goto = |to: Option<String>| {
            let (w, pp) = (Rc::downgrade(&ed), pp.clone());
            move |_: &gtk::Button| {
                let files = pp.picking.borrow().as_ref().is_some_and(|x| x.0);
                pp.picking.replace(Some((files, to.clone())));
                pp.picked.borrow_mut().clear();
                if let Some(ed) = w.upgrade() {
                    ed.redraw();
                }
            }
        };
        if b.path.is_some() {
            let up = row("arrow-up", "Up");
            up.connect_clicked(goto(b.parent.clone()));
            list.append(&up);
        }
        for d in &b.dirs {
            let r = row("folder", &d.name);
            r.connect_clicked(goto(Some(d.path.clone())));
            list.append(&r);
        }
        if files {
            for f in &b.files {
                let c = gtk::CheckButton::with_label(&f.name);
                c.add_css_class("picker-row");
                c.set_active(pp.picked.borrow().contains(&f.path));
                let (pp2, path, gw) = (pp.clone(), f.path.clone(), go.downgrade());
                c.connect_toggled(move |c| {
                    let mut p = pp2.picked.borrow_mut();
                    p.retain(|x| *x != path);
                    if c.is_active() {
                        p.push(path.clone());
                    }
                    if let Some(go) = gw.upgrade() {
                        let mounting = pp2.mounting.get();
                        go.set_sensitive(go_ready(mounting, p.len()));
                        ui::set_text(&go, &go_text(true, mounting, p.len()));
                    }
                });
                list.append(&c);
            }
        }
        if b.dirs.is_empty() && !(files && !b.files.is_empty()) {
            list.append(&faint("Nothing here."));
        }
        if !files {
            go.set_sensitive(b.path.is_some());
        }
        let (w, pp, here) = (Rc::downgrade(&ed), pp.clone(), b.path.clone());
        go.connect_clicked(move |_| {
            let Some(ed) = w.upgrade() else { return };
            let paths = if files { pp.picked.borrow().clone() } else { here.iter().cloned().collect() };
            if pp.mounting.get() {
                mount_in(&ed, &pp, paths);
            } else {
                copy_in(&ed, &pp, paths);
            }
        });
    });
    pick.upcast()
}

/// This machine's id, as projects from it name it.
fn this_machine() -> String {
    store().read_now(|s| s.cloud.as_ref().and_then(|c| c.machine_id.clone())).unwrap_or_else(|| "local".into())
}

/// A name for what's at these paths: the one's own name, or "Files".
fn picked_name(paths: &[String]) -> String {
    if paths.len() == 1 {
        paths[0].trim_end_matches('/').rsplit('/').next().unwrap_or("Files").to_string()
    } else {
        "Files".into()
    }
}

/// A folder or file on this machine, mounted read-write when the wadspace
/// starts here (wadd finds it then; nothing is made now).
fn mount_in(ed: &Rc<Editor>, p: &Rc<Projects>, paths: Vec<String>) {
    p.picking.replace(None);
    if let [path] = paths.as_slice() {
        add_mount(
            ed,
            json!({
                "id": uid("mount"), "kind": "mount",
                "source": { "machineId": this_machine(), "paths": [path] }, "as": picked_name(&paths),
            }),
        );
    } else {
        ed.redraw();
    }
}

/// Paths on this machine: wadd copies them now, and the wadspace takes the
/// copy in when it starts.
fn copy_in(ed: &Rc<Editor>, p: &Rc<Projects>, paths: Vec<String>) {
    p.picking.replace(None);
    if paths.is_empty() {
        ed.redraw();
        return;
    }
    p.copying.set(true);
    ed.redraw();
    let (w, p) = (Rc::downgrade(ed), p.clone());
    glib::spawn_future_local(async move {
        let made = send::<CopyMade>(Method::POST, "/v1/copies".into(), Some(json!({ "paths": paths }))).await;
        p.copying.set(false);
        let Some(ed) = w.upgrade() else { return };
        match made {
            Ok(c) => {
                add_mount(
                    &ed,
                    json!({
                        "id": c.id, "kind": "copy", "source": { "machineId": this_machine(), "paths": paths },
                        "as": picked_name(&paths), "files": c.files, "bytes": c.bytes,
                    }),
                );
            }
            Err(e) => {
                crate::toast::error("Couldn't copy that", &e);
                ed.redraw();
            }
        }
    });
}

// ─────────────────────────────── keys ───────────────────────────────

/// The keys part's own state: WadBrowser's apps Ctrl+clicked to move
/// together.
#[derive(Default)]
struct Keys {
    picked: RefCell<Vec<String>>,
    view: RefCell<glib::WeakRef<gtk::Box>>,
}

impl Keys {
    /// Just the keys, drawn again.
    fn redraw(self: &Rc<Self>, ed: &Rc<Editor>) {
        if let Some(b) = self.view.borrow().upgrade() {
            ui::clear(&b);
            fill_keys(ed, self, &b);
        }
    }
}

/// The keys there are: the machine's saved sign-ins (and those the
/// library's wadspaces name), as `{id, name, kind}`.
fn known() -> Vec<Value> {
    store()
        .read_now(signins::known)
        .into_iter()
        .map(|(id, name, kind)| json!({ "id": id, "name": name, "kind": kind }))
        .collect()
}

fn apps_word(n: usize) -> String {
    if n == 1 { "it".into() } else { format!("these {n}") }
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

/// WadBrowser's apps move to another group (or a new one). A group they
/// leave empty goes; an empty group made with New group stays until it's
/// filled or removed.
fn regrouped(groups: &[KeyGroup], members: &[String], to: Option<&str>, new_id: &str) -> Vec<KeyGroup> {
    let moved: Vec<KeyGroup> = groups
        .iter()
        .map(|g| KeyGroup {
            members: g.members.iter().filter(|m| !members.contains(m)).cloned().collect(),
            ..g.clone()
        })
        .collect();
    let next: Vec<KeyGroup> = match to {
        Some(to) => moved
            .into_iter()
            .map(|mut g| {
                if g.id == to {
                    g.members.extend(members.iter().cloned());
                }
                g
            })
            .collect(),
        None => {
            let mut m = moved;
            m.push(KeyGroup { id: new_id.into(), members: members.to_vec(), key: None });
            m
        }
    };
    next.into_iter()
        .enumerate()
        .filter(|(i, g)| !g.members.is_empty() || groups.get(*i).is_some_and(|o| o.members.is_empty()))
        .map(|(_, g)| g)
        .collect()
}

fn regroup(ed: &Rc<Editor>, keys: &Keys, members: &[String], to: Option<&str>) {
    let d = draft_of(ed);
    let mut sub = secrets(&d);
    let groups = catalog::key_groups_of(&ed.live(), Some(&sub));
    sub.groups = Some(regrouped(&groups, members, to, &uid("group")));
    keys.picked.borrow_mut().clear();
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

fn wire(on: bool) -> gtk::Box {
    let w = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    w.add_css_class("wire");
    if on {
        w.add_css_class("on");
    }
    w.set_hexpand(true);
    w.set_valign(gtk::Align::Center);
    w
}

fn fill_keys(ed: &Rc<Editor>, keys: &Rc<Keys>, b: &gtk::Box) {
    let live = ed.live();
    let d = draft_of(ed);
    let sub = secrets(&d);
    let apps = catalog::key_map(&live, Some(&sub));
    if apps.is_empty() {
        let e = gtk::Label::new(Some("Nothing in this wadspace keeps a key."));
        e.add_css_class("empty-note");
        e.set_xalign(0.0);
        b.append(&e);
        return;
    }
    for a in apps {
        let app = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        app.add_css_class("key-app");
        let head = gtk::Box::new(gtk::Orientation::Vertical, 5);
        head.add_css_class("key-app-head");
        let i = img(&a.icon, 34);
        i.set_halign(gtk::Align::Center);
        head.append(&i);
        let n = gtk::Label::new(Some(&a.name));
        n.set_wrap(true);
        n.set_justify(gtk::Justification::Center);
        head.append(&n);
        app.append(&wad_gtk::Width::new(&head, 96));
        let lines = gtk::Box::new(gtk::Orientation::Vertical, 6);
        lines.add_css_class("key-lines");
        lines.set_hexpand(true);
        for l in &a.lines {
            let value = catalog::key_of(Some(&sub), l).cloned();
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            line.add_css_class("key-line");
            let (w, l2) = (Rc::downgrade(ed), l.clone());
            let on_set = move |k: Option<Value>| {
                if let Some(ed) = w.upgrade() {
                    set_key(&ed, &l2, k);
                }
            };
            if let (Some(g), Some(members)) = (&l.group, &l.members) {
                line.add_css_class("group");
                let src = gtk::FlowBox::new();
                src.add_css_class("members");
                src.set_selection_mode(gtk::SelectionMode::None);
                src.set_max_children_per_line(8);
                src.set_column_spacing(4);
                src.set_row_spacing(4);
                src.set_hexpand(false);
                src.set_valign(gtk::Align::Center);
                for m in members {
                    src.insert(&member(ed, keys, m, &g.id), -1);
                }
                if members.is_empty() {
                    line.add_css_class("empty");
                    let h = gtk::Label::new(Some("Drag apps here"));
                    h.add_css_class("drop-here");
                    src.insert(&h, -1);
                }
                line.append(&src);
                line.append(&wire(value.is_some()));
                line.append(&key_picker(ed, "web", value.clone(), l.app_id.clone(), on_set));
                if members.is_empty() {
                    let x = ui::icon_button("x", "Remove this empty group", 13);
                    let (w, id) = (Rc::downgrade(ed), g.id.clone());
                    x.connect_clicked(move |_| {
                        if let Some(ed) = w.upgrade() {
                            edit_groups(&ed, |gs| gs.retain(|g| g.id != id));
                        }
                    });
                    line.append(&x);
                }
                drag::target(
                    &line,
                    GroupTarget {
                        ed: Rc::downgrade(ed),
                        keys: keys.clone(),
                        id: g.id.clone(),
                        members: g.members.clone(),
                        key: value.as_ref().map(|k| k["name"].as_str().unwrap_or_default().to_string()),
                    },
                );
            } else {
                let src = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                src.add_css_class("key-src");
                src.append(&img(&l.icon, 18));
                src.append(&gtk::Label::new(Some(&l.label)));
                line.append(&src);
                line.append(&wire(value.is_some()));
                line.append(&key_picker(ed, &l.kind, value, l.app_id.clone(), on_set));
            }
            lines.append(&line);
        }
        if a.id == "wadbrowser" {
            let ng = gtk::Button::new();
            ng.add_css_class("key-line");
            ng.add_css_class("new-group");
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            r.set_halign(gtk::Align::Center);
            r.append(&Icon::new("plus", 13));
            r.append(&gtk::Label::new(Some("New group")));
            ng.set_child(Some(&r));
            let w = Rc::downgrade(ed);
            ng.connect_clicked(move |_| {
                if let Some(ed) = w.upgrade() {
                    edit_groups(&ed, |gs| gs.push(KeyGroup { id: uid("group"), members: vec![], key: None }));
                }
            });
            drag::target(&ng, NewGroupTarget { ed: Rc::downgrade(ed), keys: keys.clone() });
            lines.append(&ng);
        }
        app.append(&lines);
        b.append(&app);
    }
    let n = gtk::Label::new(Some("No key: that app is signed out when the wadspace stops."));
    n.add_css_class("faint");
    n.add_css_class("small");
    n.set_xalign(0.0);
    b.append(&n);
}

fn member_face(m: &catalog::BrowserItem, picked: bool, n: usize) -> gtk::Box {
    let f = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    f.add_css_class("member");
    if picked {
        f.add_css_class("picked");
    }
    f.append(&img(&m.icon, 15));
    f.append(&gtk::Label::new(Some(&m.name)));
    if n > 1 {
        let more = gtk::Label::new(Some(&format!("+{}", n - 1)));
        more.add_css_class("member-more");
        f.append(&more);
    }
    f
}

/// One of WadBrowser's apps in a key group: drag it to another group;
/// Ctrl+click picks several to drag together.
fn member(ed: &Rc<Editor>, keys: &Rc<Keys>, m: &catalog::BrowserItem, group: &str) -> gtk::Widget {
    let on = keys.picked.borrow().contains(&m.id);
    let f = member_face(m, on, 1);
    f.set_tooltip_text(Some("Drag to another group · Ctrl+click to pick several"));
    let (k, mm, g) = (keys.clone(), m.clone(), group.to_string());
    drag::draggable(&f, move |_| {
        let picked = k.picked.borrow().clone();
        let members = if picked.contains(&mm.id) { picked } else { vec![mm.id.clone()] };
        let ghost = faces::ghost(&member_face(&mm, true, members.len()));
        Some((Payload::Member { members, group: g.clone() }, ghost))
    });
    let click = gtk::GestureClick::new();
    let (w, k, id) = (Rc::downgrade(ed), keys.clone(), m.id.clone());
    click.connect_released(move |g, _, _, _| {
        if !g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            return;
        }
        {
            let mut p = k.picked.borrow_mut();
            if p.contains(&id) {
                p.retain(|x| *x != id);
            } else {
                p.push(id.clone());
            }
        }
        if let Some(ed) = w.upgrade() {
            k.redraw(&ed);
        }
    });
    f.add_controller(click);
    f.upcast()
}

/// A key: "No key", or its name. Click it for a new key or one of the same
/// kind; click its name to rename it (everywhere it's kept).
fn key_picker(
    ed: &Rc<Editor>,
    kind: &str,
    value: Option<Value>,
    app_id: String,
    on_set: impl Fn(Option<Value>) + 'static,
) -> gtk::Widget {
    let slot = gtk::Box::new(gtk::Orientation::Vertical, 2);
    slot.add_css_class("key-pick");
    slot.set_valign(gtk::Align::Center);
    let known = known();
    let id = value.as_ref().and_then(|v| v["id"].as_str()).map(String::from);
    // The machine's name for it (renamed on the Secrets page, a wadspace may keep the old one).
    let shown = id
        .as_ref()
        .and_then(|id| known.iter().find(|k| k["id"] == id.as_str()))
        .or(value.as_ref())
        .and_then(|k| k["name"].as_str())
        .unwrap_or_default()
        .to_string();
    let btn = gtk::MenuButton::new();
    btn.add_css_class("key-btn");
    if value.is_some() {
        btn.add_css_class("set");
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&Icon::new("key-round", 13));
    let name = gtk::Label::new(Some(if value.is_some() { &shown } else { "No key" }));
    name.add_css_class("secret-name");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(22);
    row.append(&name);
    btn.set_child(Some(&row));
    btn.set_always_show_arrow(false);
    btn.set_tooltip_text(Some(&if value.is_some() {
        format!("{} · click the name to rename it, the key for more", signins::kind_label(kind))
    } else {
        signins::kind_label(kind)
    }));
    let on_set: Rc<dyn Fn(Option<Value>)> = Rc::new(on_set);

    // The menu: a new key, the keys there are of this kind, no key (made
    // as it opens: names and keys as they are then).
    let pop = gtk::Popover::new();
    pop.add_css_class("wad-menu");
    pop.add_css_class("key-menu");
    pop.set_has_arrow(false);
    pop.set_position(gtk::PositionType::Bottom);
    let (w, kind_s, id2, has) = (Rc::downgrade(ed), kind.to_string(), id.clone(), value.is_some());
    pop.connect_show(move |p| {
        super::menu_opened(p);
        if let Some(ed) = w.upgrade() {
            fill_menu(p, &ed, &kind_s, id2.as_deref(), has, &app_id, &on_set);
        }
    });
    btn.set_popover(Some(&pop));
    slot.append(&btn);

    // Its name renames it, in place.
    if let Some(id) = id {
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (w, sw, bw) = (Rc::downgrade(ed), slot.downgrade(), btn.downgrade());
        let nw = name.downgrade();
        click.connect_pressed(move |g, _, x, y| {
            let (Some(ed), Some(slot), Some(btn), Some(name)) = (w.upgrade(), sw.upgrade(), bw.upgrade(), nw.upgrade())
            else {
                return;
            };
            // Only on the name (the key icon opens the menu).
            let on_name = btn
                .compute_point(&name, &gtk::graphene::Point::new(x as f32, y as f32))
                .is_some_and(|p| name.contains(p.x() as f64, p.y() as f64));
            if !on_name {
                return;
            }
            g.set_state(gtk::EventSequenceState::Claimed);
            renaming(&ed, &slot, &btn, &id, &shown);
        });
        btn.add_controller(click);
    }
    slot.upcast()
}

/// What a new key is called: <Wadspace>-<what signs in with it>.
fn new_key_name(d: &CustomizationModule, app_id: &str) -> String {
    let base: String = name_of(d).chars().filter(|c| !c.is_whitespace()).collect();
    format!("{}-{app_id}", if base.is_empty() { "Wadspace" } else { &base })
}

/// The key menu's choices, as they are now.
fn fill_menu(
    pop: &gtk::Popover,
    ed: &Rc<Editor>,
    kind: &str,
    id: Option<&str>,
    has: bool,
    app_id: &str,
    on_set: &Rc<dyn Fn(Option<Value>)>,
) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let item = |icon: &'static str, text: &str, extra: Option<&str>| {
        let b = gtk::Button::new();
        b.add_css_class("menu-item");
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        r.append(&Icon::new(icon, 12));
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        r.append(&l);
        if let Some(x) = extra {
            let f = gtk::Label::new(Some(x));
            f.add_css_class("faint");
            f.set_ellipsize(gtk::pango::EllipsizeMode::End);
            r.append(&f);
        }
        b.set_child(Some(&r));
        b
    };
    let default = new_key_name(&draft_of(ed), app_id);
    let new = item("plus", "New key", Some(&default));
    let (os, pw, kind_s) = (on_set.clone(), pop.downgrade(), kind.to_string());
    new.connect_clicked(move |_| {
        if let Some(p) = pw.upgrade() {
            p.popdown();
        }
        let (os, kind, def) = (os.clone(), kind_s.clone(), default.clone());
        glib::spawn_future_local(async move {
            match create_key(&kind, &def).await {
                Ok(k) => os(Some(k)),
                Err(e) => crate::toast::error("Couldn't make the key", &e),
            }
        });
    });
    col.append(&new);
    let stacks = state::read(|s| s.board.stacks.clone());
    let existing: Vec<Value> =
        board::keys_of_kind(&known(), &stacks, kind).into_iter().filter(|k| id.is_none_or(|i| k["id"] != i)).collect();
    if !existing.is_empty() {
        let g = group_label(&format!("Your {} keys", signins::kind_label(kind)));
        g.set_margin_top(6);
        g.set_margin_start(8);
        col.append(&g);
    }
    for k in existing {
        let b = item("key-round", k["name"].as_str().unwrap_or_default(), None);
        let (os, pw) = (on_set.clone(), pop.downgrade());
        b.connect_clicked(move |_| {
            if let Some(p) = pw.upgrade() {
                p.popdown();
            }
            os(Some(k.clone()));
        });
        col.append(&b);
    }
    if has {
        let b = item("x", "No key", None);
        b.add_css_class("danger");
        let (os, pw) = (on_set.clone(), pop.downgrade());
        b.connect_clicked(move |_| {
            if let Some(p) = pw.upgrade() {
                p.popdown();
            }
            os(None);
        });
        col.append(&b);
    }
    pop.set_child(Some(&col));
}

/// A new key (a saved sign-in on this machine) of a kind, named after
/// `base` (made free).
async fn create_key(kind: &str, base: &str) -> Result<Value, String> {
    let stacks = state::read(|s| s.board.stacks.clone());
    let known = known();
    let mut taken: Vec<String> = known.iter().filter_map(|k| k["name"].as_str()).map(str::to_lowercase).collect();
    taken.extend(
        board::keys_of_kind(&[], &stacks, kind).iter().filter_map(|k| k["name"].as_str()).map(str::to_lowercase),
    );
    let mut name = base.to_string();
    let mut n = 2;
    while taken.contains(&name.to_lowercase()) {
        name = format!("{base}-{n}");
        n += 1;
    }
    let made = signins::create(name, kind.to_string()).await?;
    Ok(json!({ "id": made.id, "name": made.name, "kind": kind }))
}

/// The key's name as a field: Enter saves; clicking away saves too, unless
/// the name's taken; Esc lets go.
fn renaming(ed: &Rc<Editor>, slot: &gtk::Box, btn: &gtk::MenuButton, id: &str, shown: &str) {
    btn.set_visible(false);
    let edit = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    edit.add_css_class("key-edit");
    edit.append(&Icon::new("key-round", 13));
    let e = gtk::Entry::new();
    e.add_css_class("esc-local");
    e.set_text(shown);
    e.set_width_chars(22);
    edit.append(&e);
    slot.append(&edit);
    let problem = gtk::Label::new(None);
    problem.add_css_class("name-clash");
    problem.add_css_class("small");
    problem.set_visible(false);
    slot.append(&problem);
    e.grab_focus();
    let done = Rc::new(Cell::new(false));
    let stop = {
        let (btn, edit, problem, done) = (btn.clone(), edit.clone(), problem.clone(), done.clone());
        Rc::new(move || {
            if done.replace(true) {
                return;
            }
            if let Some(p) = edit.parent().and_downcast::<gtk::Box>() {
                p.remove(&edit);
                p.remove(&problem);
            }
            btn.set_visible(true);
        })
    };
    let rename = {
        let (w, e, id, shown, stop, problem, done) = (
            Rc::downgrade(ed),
            e.clone(),
            id.to_string(),
            shown.to_string(),
            stop.clone(),
            problem.clone(),
            done.clone(),
        );
        Rc::new(move |keep_on_clash: bool| {
            if done.get() {
                return;
            }
            let to = e.text().trim().to_string();
            if to.is_empty() || to == shown {
                return stop();
            }
            let known = known();
            match state::edit(|b| b.rename_key(&id, &to, &known)) {
                Err(why) if keep_on_clash => {
                    problem.set_text(&why);
                    problem.set_visible(true);
                    e.add_css_class("clash");
                }
                Err(_) => stop(),
                Ok(()) => {
                    // On this machine too, where it's one.
                    let (id2, to2) = (id.clone(), to.clone());
                    glib::spawn_future_local(async move {
                        let r = send::<Value>(Method::PUT, format!("/v1/signins/{id2}"), Some(json!({ "name": to2 })));
                        match r.await {
                            Ok(_) => signins::load(),
                            Err(e) if crate::fake::on() => tracing::debug!("rename on the machine: {e}"),
                            Err(e) => crate::toast::error("Couldn't rename it on the machine", &e),
                        }
                    });
                    stop();
                    if let Some(ed) = w.upgrade() {
                        rename_in_draft(&ed, &id, &to);
                    }
                }
            }
        })
    };
    let r = rename.clone();
    e.connect_activate(move |_| r(true));
    let pw = problem.downgrade();
    e.connect_changed(move |e| {
        e.remove_css_class("clash");
        if let Some(p) = pw.upgrade() {
            p.set_visible(false);
        }
    });
    let keys = gtk::EventControllerKey::new();
    let s2 = stop.clone();
    keys.connect_key_pressed(move |_, k, _, _| {
        if k == gtk::gdk::Key::Escape {
            s2();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    e.add_controller(keys);
    let focus = gtk::EventControllerFocus::new();
    let r = rename.clone();
    focus.connect_leave(move |_| {
        let r = r.clone();
        glib::idle_add_local_once(move || r(false));
    });
    e.add_controller(focus);
}

/// A WadBrowser key group: apps dropped on it join it (and share its key).
struct GroupTarget {
    ed: Weak<Editor>,
    keys: Rc<Keys>,
    id: String,
    members: Vec<String>,
    key: Option<String>,
}

impl Target for GroupTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let Payload::Member { members, group } = p else { return None };
        if *group == self.id && members.iter().all(|m| self.members.contains(m)) {
            return None;
        }
        let n = apps_word(members.len());
        Verdict::yes(match &self.key {
            Some(k) => format!("Share {k} ({n})"),
            None => format!("Join this group ({n})"),
        })
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let (Payload::Member { members, .. }, Some(ed)) = (p, self.ed.upgrade()) {
            regroup(&ed, &self.keys, members, Some(&self.id));
        }
    }
}

/// New group: apps dropped on it start a group of their own.
struct NewGroupTarget {
    ed: Weak<Editor>,
    keys: Rc<Keys>,
}

impl Target for NewGroupTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let Payload::Member { members, .. } = p else { return None };
        Verdict::yes(format!("A new group, with a key of its own ({})", apps_word(members.len())))
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let (Payload::Member { members, .. }, Some(ed)) = (p, self.ed.upgrade()) {
            regroup(&ed, &self.keys, members, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(id: &str, members: &[&str]) -> KeyGroup {
        KeyGroup { id: id.into(), members: members.iter().map(|s| s.to_string()).collect(), key: None }
    }

    fn ids(gs: &[KeyGroup]) -> Vec<(String, Vec<String>)> {
        gs.iter().map(|g| (g.id.clone(), g.members.clone())).collect()
    }

    #[test]
    fn apps_move_between_key_groups() {
        let groups = vec![g("a", &["browser", "web:gmail"]), g("b", &["web:github"]), g("empty", &[])];
        // Into another group: the one it leaves empty goes; the empty one made by hand stays.
        let r = regrouped(&groups, &["web:github".into()], Some("a"), "n");
        assert_eq!(
            ids(&r),
            vec![
                ("a".into(), vec!["browser".into(), "web:gmail".into(), "web:github".into()]),
                ("empty".into(), vec![])
            ]
        );
        // Into a new group of their own.
        let r = regrouped(&groups, &["browser".into()], None, "n");
        assert_eq!(r.last().map(|g| (g.id.as_str(), g.members.clone())), Some(("n", vec!["browser".to_string()])));
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn projects_get_free_names_and_say_what_they_are() {
        let mounts = vec![json!({ "as": "notes" }), json!({ "as": "notes-2" })];
        assert_eq!(free_as("notes", &mounts), "notes-3");
        assert_eq!(free_as("My Notes!", &[]), "My-Notes");
        assert_eq!(free_as("???", &[]), "Project");
        assert_eq!(project_text(&json!({ "kind": "github", "repo": "me/site" })), "cloned from me/site");
        assert_eq!(
            project_text(&json!({ "kind": "copy", "files": 3, "bytes": 2_500_000, "source": {} })),
            "copied from this machine · 3 files, 2.5 MB"
        );
        assert_eq!(
            project_text(&json!({ "kind": "copy", "source": { "drive": { "label": "", "uuid": "AB-12" } } })),
            "copied from the drive AB-12 at its first start"
        );
        assert_eq!(size_of(200), "1 KB");
        let thesis = json!({ "kind": "mount", "source": { "machineId": "m1", "paths": ["/var/home/wad/Thesis"] } });
        assert_eq!(project_text(&thesis), "mounted from /var/home/wad/Thesis · changes save to that machine");
        assert_eq!(
            (project_icon(&thesis), project_key(&thesis).as_str()),
            ("folder-symlink", "mount:/var/home/wad/Thesis")
        );
    }

    #[test]
    fn the_picker_copies_any_files_but_mounts_one() {
        assert_eq!(
            (go_text(false, false, 0), go_text(false, true, 0)),
            ("Copy this folder".into(), "Mount this folder".into())
        );
        assert_eq!((go_text(true, false, 3), go_ready(false, 3)), ("Copy 3 files".into(), true));
        assert_eq!((go_text(true, true, 1), go_ready(true, 1)), ("Mount this file".into(), true));
        assert_eq!((go_text(true, true, 2), go_ready(true, 2)), ("Mount one file at a time".into(), false));
        assert_eq!(picked_name(&["/var/home/wad/Thesis/".into()]), "Thesis");
    }
}
