//! A wadspace's projects (the Builder sidebar's Customization): what it
//! has; the machine's GitHub repositories, one bar each (Clone into it, or
//! Mount: cloned on its machine, in ~/GitHub); a new project (made on
//! GitHub); and from its machine a new folder, or one there, mounted
//! read-write in its home: changes save there, and only that machine's
//! wadspaces have it.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::{glib, prelude::*};
use serde_json::{Value, json};
use wad_board::uid;
use wad_gtk::Icon;
use wad_proto::github::{Repo, Repos};
use wad_proto::v1::{Project, ProjectSource};
use wad_wadd_client::Method;

use super::faint;
use super::{group_label, note};
use crate::pages::machines::{get, send};
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
    /// Its machine's id, and whether that's this machine.
    fn machine(&self) -> (String, bool);
    /// The projects, drawn again.
    fn redraw(&self);
}

/// From the machine: a new folder being named, or its folders listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Local {
    Create,
    Existing,
}

/// The part's own state.
#[derive(Default)]
pub struct Projects {
    /// New project's name, while it's being made.
    creating: RefCell<Option<String>>,
    problem: RefCell<String>,
    local: RefCell<Option<Local>>,
    /// The folders in this machine's home (None: not read yet).
    folders: RefCell<Option<Vec<String>>>,
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

/// A GitHub project cloned on its machine and mounted, not cloned inside.
fn mounted_repo(m: &Value) -> bool {
    s(m, "kind") == "github" && m["mounted"] == true
}

/// What a project is, in words.
pub fn project_text(m: &Value) -> String {
    if mounted_repo(m) {
        return format!("{} cloned on its machine in ~/GitHub, mounted · changes save there", s(m, "repo"));
    }
    if s(m, "kind") == "github" {
        return format!("cloned from {}", s(m, "repo"));
    }
    if s(m, "kind") == "mount" {
        let path = m["source"]["paths"][0].as_str().unwrap_or("");
        let made = if m["make"] == true { ", a folder made for it" } else { "" };
        return format!("mounted from {path}{made} · changes save to that machine");
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

/// The project a repo is, if it's in.
fn repo_in<'a>(projects: &'a [Value], repo: &str) -> Option<&'a Value> {
    projects.iter().find(|m| s(m, "kind") == "github" && s(m, "repo").eq_ignore_ascii_case(repo))
}

/// A repo in: cloned inside the wadspace, or (`mounted`) cloned on its
/// machine, in ~/GitHub, and mounted.
fn clone_repo(host: &Rc<dyn Host>, repo: &str, mounted: bool) {
    if repo_in(&host.projects(), repo).is_some() {
        return;
    }
    let as_ = repo.rsplit('/').next().unwrap_or(repo);
    let mut m = json!({ "id": uid("project"), "kind": "github", "repo": repo, "as": as_ });
    if mounted {
        m["mounted"] = true.into();
    }
    add_mount(host, m);
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

/// The projects: what it has, the machine's repositories, a new project,
/// and folders from its machine.
pub fn fill(host: &Rc<dyn Host>, p: &Rc<Projects>, b: &gtk::Box) {
    let mounts = host.projects();
    let n = note(
        "Cloned into the wadspace: the files live inside it, at ~/Desktop. A mounted folder stays on its \
         machine: changes save there, and the wadspace has it only there. A mounted repo is cloned on the \
         wadspace's machine, in ~/GitHub: its wadspaces there share that clone.",
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
        rm.connect_clicked(move |_| with(&w, |h| take_out(h, &id)));
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

    let (_, here) = host.machine();
    let g = group_label(if here { "From this machine" } else { "From its machine" });
    g.set_margin_top(8);
    b.append(&g);
    if !here {
        b.append(&faint("Its folders are mounted on its own machine: pick them in WadSpaces there."));
        return;
    }
    let local = *p.local.borrow();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    for (what, icon, label) in
        [(Local::Create, "plus", "Create new folder"), (Local::Existing, "folder-open", "Add existing")]
    {
        let btn = ui::button(label, Some(icon), Variant::Ghost, Size::Sm);
        if local == Some(what) {
            btn.add_css_class("on");
        }
        let (w, p) = (Rc::downgrade(host), p.clone());
        btn.connect_clicked(move |_| {
            let now = *p.local.borrow();
            p.local.replace(if now == Some(what) { None } else { Some(what) });
            if what == Local::Existing {
                load_folders(w.clone(), p.clone());
            }
            with(&w, |h| h.redraw());
        });
        row.append(&btn);
    }
    b.append(&row);
    match local {
        Some(Local::Create) => b.append(&new_folder(host, p)),
        Some(Local::Existing) => b.append(&existing_folders(host, p, &mounts)),
        None => {}
    }
}

/// A project out of the wadspace (it waits in Recently deleted).
fn take_out(host: &Rc<dyn Host>, id: &str) {
    let mut list = host.projects();
    list.retain(|x| s(x, "id") != id);
    host.set_projects(list);
}

/// A folder in the machine's home, mounted (`make`: made for it when it
/// isn't there yet).
fn mount_home(host: &Rc<dyn Host>, name: &str, make: bool) {
    let path = format!("~/{name}");
    if host.projects().iter().any(|m| s(m, "kind") == "mount" && m["source"]["paths"][0] == path.as_str()) {
        return;
    }
    let (machine, _) = host.machine();
    let mut m = json!({
        "id": format!("mount-{}", slug(name).to_lowercase()),
        "kind": "mount",
        "as": name,
        "source": { "machineId": machine, "paths": [path] },
    });
    if make {
        m["make"] = true.into();
    }
    add_mount(host, m);
}

/// Create new folder: its name, then a folder of that name in the home
/// (made at the wadspace's next start), mounted.
fn new_folder(host: &Rc<dyn Host>, p: &Rc<Projects>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let e = ui::entry("A name for the folder, in your home");
    e.add_css_class("small");
    e.set_hexpand(true);
    row.append(&e);
    let go = ui::button("Create", None, Variant::Secondary, Size::Sm);
    row.append(&go);
    let create = {
        let (w, p, e) = (Rc::downgrade(host), p.clone(), e.downgrade());
        move || {
            let Some(e) = e.upgrade() else { return };
            let name = slug(e.text().trim());
            if name.is_empty() || name == "." || name == ".." {
                return;
            }
            p.local.replace(None);
            with(&w, |h| mount_home(h, &name, true));
        }
    };
    let c = create.clone();
    e.connect_activate(move |_| c());
    go.connect_clicked(move |_| create());
    e.grab_focus();
    row
}

/// The folders in this machine's home, read (again).
fn load_folders(w: Weak<dyn Host>, p: Rc<Projects>) {
    glib::spawn_future_local(async move {
        let list = get::<Value>("/v1/files?root=home&path=".into()).await.ok();
        let names: Vec<String> = list
            .iter()
            .flat_map(|l| l["entries"].as_array().into_iter().flatten())
            .filter(|e| e["dir"] == true)
            .filter_map(|e| e["name"].as_str())
            .filter(|n| !n.starts_with('.'))
            .map(String::from)
            .collect();
        p.folders.replace(Some(names));
        with(&w, |h| h.redraw());
    });
}

/// Add existing: the home's folders, each mounted with a click (those in
/// already dimmed).
fn existing_folders(host: &Rc<dyn Host>, p: &Rc<Projects>, mounts: &[Value]) -> gtk::Box {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("local-list");
    let Some(folders) = p.folders.borrow().clone() else {
        list.append(&faint("Reading…"));
        return list;
    };
    if folders.is_empty() {
        list.append(&faint("No folders in your home."));
    }
    for f in folders {
        let path = format!("~/{f}");
        let on = mounts.iter().any(|m| s(m, "kind") == "mount" && m["source"]["paths"][0] == path.as_str());
        let b = gtk::Button::new();
        b.add_css_class("local-row");
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        r.append(&Icon::new("folder", 14));
        let n = gtk::Label::new(Some(&f));
        n.set_hexpand(true);
        n.set_xalign(0.0);
        n.set_ellipsize(gtk::pango::EllipsizeMode::End);
        r.append(&n);
        let d = gtk::Label::new(Some(if on { "Added" } else { &path }));
        d.add_css_class("faint");
        d.add_css_class("tiny");
        r.append(&d);
        b.set_child(Some(&r));
        b.set_sensitive(!on);
        b.set_tooltip_text(Some(&format!("Mount {path}: changes save to this machine")));
        let w = Rc::downgrade(host);
        b.connect_clicked(move |_| with(&w, |h| mount_home(h, &f, false)));
        list.append(&b);
    }
    list
}

/// A repository, one bar: Clone it into the wadspace, or Mount it (cloned
/// on its machine); once it's in, Cloned or Mounted, and Remove.
fn repo_row(host: &Rc<dyn Host>, projects: &[Value], r: &Repo) -> gtk::Box {
    let had = repo_in(projects, &r.full_name).cloned();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.add_css_class("gh-proj");
    if had.is_some() {
        row.add_css_class("on");
    }
    row.append(&Icon::new("folder-git-2", 13));
    let n = gtk::Label::new(Some(&r.name));
    n.add_css_class("gh-name");
    n.set_ellipsize(gtk::pango::EllipsizeMode::End);
    n.set_hexpand(true);
    n.set_xalign(0.0);
    row.append(&n);
    row.set_tooltip_text(Some(&if r.description.is_empty() {
        r.full_name.clone()
    } else {
        format!("{}: {}", r.full_name, r.description)
    }));
    let opt = |text: &str, tip: &str| {
        let b = gtk::Button::new();
        b.add_css_class("gh-opt");
        b.set_child(Some(&gtk::Label::new(Some(text))));
        b.set_tooltip_text(Some(tip));
        b
    };
    match had {
        Some(m) => {
            let done = gtk::Label::new(Some(if mounted_repo(&m) { "Mounted" } else { "Cloned" }));
            done.add_css_class("gh-done");
            row.append(&done);
            let rm = opt("Remove", "Take it out of this wadspace (its files there stay)");
            let (w, id) = (Rc::downgrade(host), s(&m, "id").to_string());
            rm.connect_clicked(move |_| with(&w, |h| take_out(h, &id)));
            row.append(&rm);
        }
        None => {
            let clone = opt("Clone", "Clone it into the wadspace: its files live there");
            let (w, repo) = (Rc::downgrade(host), r.full_name.clone());
            clone.connect_clicked(move |_| with(&w, |h| clone_repo(h, &repo, false)));
            row.append(&clone);
            let mount = opt(
                "Mount",
                &format!(
                    "Clone it on the wadspace's machine, in ~/GitHub/{}, and mount it: changes save there, and \
                     its wadspaces there share that clone",
                    r.full_name
                ),
            );
            let (w, repo) = (Rc::downgrade(host), r.full_name.clone());
            mount.connect_clicked(move |_| with(&w, |h| clone_repo(h, &repo, true)));
            row.append(&mount);
        }
    }
    row
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
                        clone_repo(h, &repo, false);
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
            project_text(&json!({ "kind": "github", "repo": "me/site", "mounted": true })),
            "me/site cloned on its machine in ~/GitHub, mounted · changes save there"
        );
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
        assert_eq!(project_icon(&thesis), "folder-symlink");
        let made = json!({ "kind": "mount", "make": true, "source": { "paths": ["~/Thesis"] } });
        assert_eq!(project_text(&made), "mounted from ~/Thesis, a folder made for it · changes save to that machine");
    }
}
