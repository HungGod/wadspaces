//! A wadspace's projects (the Customization editor's, and Builder B's
//! sidebar's): GitHub repositories cloned in (New project makes one on
//! GitHub), and folders or files on this machine copied in (wadd makes the
//! copy now; the wadspace takes it in when it starts). Or, when Mount is
//! picked instead of Copy, a folder or file on this machine mounted
//! read-write: changes save to the machine, and the wadspace has it only
//! there.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::{glib, prelude::*};
use serde_json::{Value, json};
use wad_board::uid;
use wad_gtk::Icon;
use wad_proto::github::{Repo, Repos};
use wad_proto::v1::{Browse, CopyMade, Project, ProjectSource};
use wad_wadd_client::Method;

use super::faint;
use crate::pages::builder::editor::{group_label, note};
use crate::pages::machines::{get, send};
use crate::store::store;
use crate::ui::{self, Size, Variant};

thread_local! {
    /// The machine's GitHub repositories (None: not read yet), read once
    /// the first time projects are shown.
    static REPOS: RefCell<Option<Vec<Repo>>> = const { RefCell::new(None) };
}

/// The wadspace whose projects these are.
pub trait Host {
    /// Its projects, as its customization keeps them.
    fn projects(&self) -> Vec<Value>;
    fn set_projects(&self, projects: Vec<Value>);
    /// Other wadspaces' projects, to add here too.
    fn elsewhere(&self) -> Vec<Value>;
    /// The projects, drawn again.
    fn redraw(&self);
}

/// The part's own state.
#[derive(Default)]
pub struct Projects {
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

fn with(w: &Weak<dyn Host>, f: impl FnOnce(&Rc<dyn Host>)) {
    if let Some(h) = w.upgrade() {
        f(&h);
    }
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
pub fn project_icon(m: &Value) -> &'static str {
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
pub fn project_text(m: &Value) -> String {
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

fn add_mount(host: &Rc<dyn Host>, m: Value) {
    let mut list = host.projects();
    let mut m = m;
    m["as"] = free_as(s(&m, "as"), &list).into();
    list.push(m);
    host.set_projects(list);
}

fn cloned(projects: &[Value], repo: &str) -> bool {
    projects.iter().any(|m| s(m, "kind") == "github" && s(m, "repo").eq_ignore_ascii_case(repo))
}

fn clone_repo(host: &Rc<dyn Host>, repo: &str) {
    if cloned(&host.projects(), repo) {
        return;
    }
    let as_ = repo.rsplit('/').next().unwrap_or(repo);
    add_mount(host, json!({ "id": uid("project"), "kind": "github", "repo": repo, "as": as_ }));
}

/// The machine's repositories, read the first time projects are shown.
pub fn load_repos_once(host: &Rc<dyn Host>) {
    if REPOS.with(|r| r.borrow().is_none()) {
        load_repos(Rc::downgrade(host));
    }
}

/// The machine's repositories, read (again): what the open part shows.
fn load_repos(w: Weak<dyn Host>) {
    glib::spawn_future_local(async move {
        let list = get::<Repos>("/v1/github/repos".into()).await.map(|r| r.repos).unwrap_or_default();
        REPOS.with(|r| r.replace(Some(list)));
        with(&w, |h| h.redraw());
    });
}

/// The projects: what it has, the machine's repositories, folders and files
/// from this machine, and what other wadspaces have.
pub fn fill(host: &Rc<dyn Host>, p: &Rc<Projects>, b: &gtk::Box) {
    let mounts = host.projects();
    let n = note(
        "Cloned or copied into the wadspace: the files live inside it, at ~/Desktop. A mounted folder stays \
         on this machine: changes save here, and the wadspace has it only here.",
    );
    n.add_css_class("proj-note");
    b.append(&n);
    for m in &mounts {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("cz-row");
        let face = project_face(m);
        face.set_hexpand(true);
        row.append(&face);
        let rm = ui::icon_button("trash", "Take it out of this wadspace (its files there stay)", 12);
        let (w, id) = (Rc::downgrade(host), s(m, "id").to_string());
        rm.connect_clicked(move |_| {
            with(&w, |h| {
                let mut list = h.projects();
                list.retain(|x| s(x, "id") != id);
                h.set_projects(list);
            });
        });
        row.append(&rm);
        b.append(&row);
    }
    if mounts.is_empty() {
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
                list.append(&repo_row(host, &mounts, &r));
            }
        }
    }
    b.append(&list);
    b.append(&new_project(host, p));

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
        let (w, p) = (Rc::downgrade(host), p.clone());
        btn.connect_clicked(move |_| {
            p.picking.replace(Some((want_files, None)));
            p.picked.borrow_mut().clear();
            with(&w, |h| h.redraw());
        });
        copy.append(btn);
    }
    b.append(&copy);

    // What other wadspaces have, to add here too.
    let mut seen: Vec<String> = mounts.iter().map(project_key).collect();
    let mut elsewhere: Vec<Value> = vec![];
    for m in host.elsewhere() {
        if !seen.contains(&project_key(&m)) {
            seen.push(project_key(&m));
            elsewhere.push(m);
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
            let w = Rc::downgrade(host);
            chip.connect_clicked(move |_| with(&w, |h| add_mount(h, m.clone())));
            sug.insert(&chip, -1);
        }
        b.append(&sug);
    }
    if p.picking.borrow().is_some() {
        b.append(&machine_picker(host, p));
    }
}

fn repo_row(host: &Rc<dyn Host>, projects: &[Value], r: &Repo) -> gtk::Button {
    let on = cloned(projects, &r.full_name);
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
    let (w, repo) = (Rc::downgrade(host), r.full_name.clone());
    b.connect_clicked(move |_| with(&w, |h| clone_repo(h, &repo)));
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

fn new_project(host: &Rc<dyn Host>, p: &Rc<Projects>) -> gtk::Widget {
    let Some(text) = p.creating.borrow().clone() else {
        let b = ui::button("New project", Some("plus"), Variant::Ghost, Size::Sm);
        b.add_css_class("new-proj-btn");
        b.set_halign(gtk::Align::Start);
        b.set_margin_top(4);
        let (w, p) = (Rc::downgrade(host), p.clone());
        b.connect_clicked(move |_| {
            p.creating.replace(Some(String::new()));
            p.problem.borrow_mut().clear();
            with(&w, |h| h.redraw());
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
        let (w, p) = (Rc::downgrade(host), p.clone());
        move || {
            let name = p.creating.borrow().clone().unwrap_or_default();
            let (w, p) = (w.clone(), p.clone());
            glib::spawn_future_local(async move {
                let made = create_project(&name, w.clone()).await;
                with(&w, |h| match made {
                    Ok(repo) => {
                        p.creating.replace(None);
                        p.problem.borrow_mut().clear();
                        clone_repo(h, &repo);
                    }
                    Err(why) => {
                        p.problem.replace(why);
                        h.redraw();
                    }
                });
            });
        }
    };
    let g = go.clone();
    e.connect_activate(move |_| g());
    make.connect_clicked(move |_| go());
    let (w, p) = (Rc::downgrade(host), p.clone());
    cancel.connect_clicked(move |_| {
        p.creating.replace(None);
        p.problem.borrow_mut().clear();
        with(&w, |h| h.redraw());
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
async fn create_project(name: &str, host: Weak<dyn Host>) -> Result<String, String> {
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
    load_repos(host);
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
fn machine_picker(host: &Rc<dyn Host>, p: &Rc<Projects>) -> gtk::Widget {
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
    let (w, pp) = (Rc::downgrade(host), p.clone());
    x.connect_clicked(move |_| {
        pp.picking.replace(None);
        with(&w, |h| h.redraw());
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
        let (w, pp) = (Rc::downgrade(host), p.clone());
        b.connect_toggled(move |b| {
            if b.is_active() && pp.mounting.get() != yes {
                pp.mounting.set(yes);
                with(&w, |h| h.redraw());
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
    let (w, pp, lw, gw) = (Rc::downgrade(host), p.clone(), list.downgrade(), go.downgrade());
    glib::spawn_future_local(async move {
        let r = get::<Browse>(url).await;
        let (Some(list), Some(go), Some(host)) = (lw.upgrade(), gw.upgrade(), w.upgrade()) else { return };
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
            let (w, pp) = (Rc::downgrade(&host), pp.clone());
            move |_: &gtk::Button| {
                let files = pp.picking.borrow().as_ref().is_some_and(|x| x.0);
                pp.picking.replace(Some((files, to.clone())));
                pp.picked.borrow_mut().clear();
                with(&w, |h| h.redraw());
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
        let (w, pp, here) = (Rc::downgrade(&host), pp.clone(), b.path.clone());
        go.connect_clicked(move |_| {
            let Some(h) = w.upgrade() else { return };
            let paths = if files { pp.picked.borrow().clone() } else { here.iter().cloned().collect() };
            if pp.mounting.get() {
                mount_in(&h, &pp, paths);
            } else {
                copy_in(&h, &pp, paths);
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
fn mount_in(host: &Rc<dyn Host>, p: &Rc<Projects>, paths: Vec<String>) {
    p.picking.replace(None);
    if let [path] = paths.as_slice() {
        add_mount(
            host,
            json!({
                "id": uid("mount"), "kind": "mount",
                "source": { "machineId": this_machine(), "paths": [path] }, "as": picked_name(&paths),
            }),
        );
    } else {
        host.redraw();
    }
}

/// Paths on this machine: wadd copies them now, and the wadspace takes the
/// copy in when it starts.
fn copy_in(host: &Rc<dyn Host>, p: &Rc<Projects>, paths: Vec<String>) {
    p.picking.replace(None);
    if paths.is_empty() {
        host.redraw();
        return;
    }
    p.copying.set(true);
    host.redraw();
    let (w, p) = (Rc::downgrade(host), p.clone());
    glib::spawn_future_local(async move {
        let made = send::<CopyMade>(Method::POST, "/v1/copies".into(), Some(json!({ "paths": paths }))).await;
        p.copying.set(false);
        with(&w, |h| match made {
            Ok(c) => add_mount(
                h,
                json!({
                    "id": c.id, "kind": "copy", "source": { "machineId": this_machine(), "paths": paths },
                    "as": picked_name(&paths), "files": c.files, "bytes": c.bytes,
                }),
            ),
            Err(e) => {
                crate::toast::error("Couldn't copy that", &e);
                h.redraw();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
