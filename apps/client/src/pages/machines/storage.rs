//! What WadSpaces takes on this machine's disk (the web app's
//! MachineStorage, from wadd's /v1/storage): the disk (how full, by what),
//! and the wadspaces as the tree their images make: the base, the sets of
//! apps put together on it (shared: wadspaces with the same apps run on the
//! same one), and the wadspaces. So a set shows its own share (what it adds
//! to the base), and a wadspace its own files (its home and what it copied
//! in).
//!
//! Sets and wadspaces (with their files) are picked in the tree and deleted
//! from the disk, just what's picked: a set a wadspace that stays runs on
//! can't go. What's left over (sets nothing runs on, packages no wadspace
//! here needs, the old Builder's images) goes after the next build, or on
//! its own line now. The packages kept (what changes restack from, without
//! a download) are listed with the wadspaces they're for. wadd hands out
//! its last count at once and counts again in the background; its
//! `storage` event says when a new one is ready.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gtk::{glib, prelude::*};
use serde_json::Value;
use wad_gtk::Icon;
use wad_proto::v1::{Freed, ImageRow, LayerBytes, Storage, StorageDelete};
use wad_wadd_client::Method;

use super::tree::{NODE_W, Placed, Shares, Tree};
use super::{Container, THIS, get, machine, send};
use crate::launch::{ImageState, image_state};
use crate::modal::Modal;
use crate::store::{AppState, Topic, store};
use crate::ui::{self, Size, Variant, bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Base,
    Set,
    Wadspace,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Base => "Base",
            Kind::Set => "Apps",
            Kind::Wadspace => "Wadspace",
        }
    }

    fn class(self) -> &'static str {
        match self {
            Kind::Base => "base",
            Kind::Set => "template",
            Kind::Wadspace => "customization",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub key: String,
    pub kind: Kind,
    pub name: String,
    /// Its own share of the disk; None when it isn't on the machine.
    pub bytes: Option<u64>,
    pub parent: Option<String>,
    pub col: usize,
    /// A set: "update available", "building…". A wadspace: running,
    /// installed, not built here.
    pub note: Option<&'static str>,
    /// "on", "dim" or "warn".
    pub tone: &'static str,
    pub wadspace_id: Option<String>,
    pub installed: bool,
    pub running: bool,
}

impl Node {
    /// What can be picked to delete: a built set (not the base), or a
    /// wadspace on this machine.
    pub fn pickable(&self) -> bool {
        match self.kind {
            Kind::Wadspace => self.installed,
            Kind::Base => false,
            _ => self.bytes.is_some(),
        }
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// The nodes: the base, the sets the wadspaces run on, the wadspaces (one
/// with no apps right on the base). `stacks`: (wadspace, its name).
pub fn tree_of(
    st: &Storage,
    stacks: &[(&Value, String)],
    containers: &[Container],
    note: impl Fn(&str) -> Option<(&'static str, &'static str)>,
) -> Vec<Node> {
    let own: HashMap<&str, u64> = st.layers.iter().map(|l| (l.layer.as_str(), l.own_bytes)).collect();
    let homes: HashMap<&str, u64> = st.homes.iter().map(|h| (h.workspace_id.as_str(), h.bytes)).collect();
    let copies: HashMap<&str, u64> = st.copies.iter().map(|c| (c.id.as_str(), c.bytes)).collect();
    let mut nodes: Vec<Node> = vec![];
    let mut add = |n: Node| {
        if !nodes.iter().any(|x| x.key == n.key) {
            nodes.push(n);
        }
    };
    add(Node {
        key: "base".into(),
        kind: Kind::Base,
        name: "WadSpaces base".into(),
        bytes: st.base.as_ref().map(|b| b.bytes),
        parent: None,
        col: 0,
        note: None,
        tone: "",
        wadspace_id: None,
        installed: false,
        running: false,
    });
    for (w, name) in stacks {
        let parent = wad_core::v5::image_key(w);
        if parent != "base" {
            let (note, tone) = note(&parent).map(|(n, t)| (Some(n), t)).unwrap_or((None, ""));
            let apps: Vec<String> = wad_core::v5::apps_of(w)
                .iter()
                .map(|e| wad_core::catalog::app(s(e, "app")).map_or_else(|| s(e, "app").to_string(), |a| a.name))
                .collect();
            add(Node {
                bytes: own.get(parent.as_str()).copied(),
                key: parent.clone(),
                kind: Kind::Set,
                name: apps.join(", "),
                parent: Some("base".into()),
                col: 1,
                note,
                tone,
                wadspace_id: None,
                installed: false,
                running: false,
            });
        }
        let id = s(w, "id");
        let c = containers.iter().find(|c| c.id == id);
        let copied: u64 = w["customization"]["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| s(p, "kind") == "copy")
            .map(|p| copies.get(s(p, "id")).copied().unwrap_or(0))
            .sum();
        let home = homes.get(id).copied();
        let running = c.is_some_and(|c| c.running);
        let (note, tone) = if running {
            ("running", "on")
        } else if c.is_some() {
            ("installed", "dim")
        } else {
            ("not built here", "dim")
        };
        add(Node {
            key: format!("ws:{id}"),
            kind: Kind::Wadspace,
            name: name.clone(),
            bytes: (c.is_some() || home.is_some()).then(|| home.unwrap_or(0) + copied),
            parent: Some(parent),
            col: 2,
            note: Some(note),
            tone,
            wadspace_id: Some(id.to_string()),
            installed: c.is_some(),
            running,
        });
    }
    nodes
}

/// Rows for the wadspaces (the leaves, in tree order), each node centred on
/// what's built on it: (row by key, rows, columns).
pub fn layout(nodes: &[Node]) -> (HashMap<String, f64>, usize, usize) {
    let mut kids: HashMap<&str, Vec<&Node>> = HashMap::new();
    for n in nodes {
        if let Some(p) = &n.parent {
            kids.entry(p.as_str()).or_default().push(n);
        }
    }
    for list in kids.values_mut() {
        list.sort_by_key(|n| n.name.to_lowercase());
    }
    fn place(key: &str, kids: &HashMap<&str, Vec<&Node>>, y: &mut HashMap<String, f64>, row: &mut usize) -> f64 {
        let list = kids.get(key).cloned().unwrap_or_default();
        if list.is_empty() {
            let r = *row as f64;
            y.insert(key.to_string(), r);
            *row += 1;
            return r;
        }
        let ys: Vec<f64> = list.iter().map(|k| place(&k.key, kids, y, row)).collect();
        let mid = (ys[0] + ys[ys.len() - 1]) / 2.0;
        y.insert(key.to_string(), mid);
        mid
    }
    let mut y = HashMap::new();
    let mut row = 0;
    // Only the base and what's built on it (nothing at all without wadspaces).
    if nodes.iter().any(|n| n.kind == Kind::Wadspace) {
        place("base", &kids, &mut y, &mut row);
    }
    (y, row, nodes.iter().map(|n| n.col).max().unwrap_or(0))
}

#[derive(Debug, Clone, PartialEq)]
pub enum OtherKind {
    Image,
    Volume,
    UpdateCopy,
}

/// Something in "Everything else" that can go.
#[derive(Debug, Clone, PartialEq)]
pub struct Other {
    pub key: String,
    pub kind: OtherKind,
    pub id: String,
    pub label: String,
    pub detail: String,
    pub bytes: u64,
}

/// What "Everything else" lets go: each deletable image and volume, and
/// the update copy.
pub fn other_picks(st: &Storage) -> Vec<Other> {
    let mut out = vec![];
    for p in &st.other {
        if p.key == "update-copy" && p.deletable {
            out.push(Other {
                key: "update-copy".into(),
                kind: OtherKind::UpdateCopy,
                id: String::new(),
                label: p.label.clone(),
                detail: p.detail.clone(),
                bytes: p.bytes,
            });
        }
        if p.key == "images" || p.key == "volumes" {
            let (pre, kind) = if p.key == "images" { ("img", OtherKind::Image) } else { ("vol", OtherKind::Volume) };
            out.extend(p.items.iter().filter(|i| i.deletable).map(|i| Other {
                key: format!("{pre}:{}", i.id),
                kind: kind.clone(),
                id: i.id.clone(),
                label: i.label.clone(),
                detail: i.detail.clone(),
                bytes: i.bytes,
            }));
        }
    }
    out
}

/// What's picked: what it frees, what stops it, what more could go.
pub struct Picks<'a> {
    pub layers: Vec<&'a Node>,
    pub wadspaces: Vec<&'a Node>,
    pub extras: Vec<Other>,
    /// Picked and running: stop them first.
    pub running: Vec<&'a Node>,
    /// On a picked layer's image, not picked.
    pub on_it: Vec<&'a Node>,
    /// Picked layers something unpicked is built on (their name goes,
    /// their files stay), with what.
    pub kept: Vec<(&'a Node, Vec<&'a Node>)>,
    pub frees: u64,
    /// Built on a picked layer and not picked yet.
    pub more: Vec<&'a Node>,
}

impl Picks<'_> {
    pub fn count(&self) -> usize {
        self.layers.len() + self.wadspaces.len() + self.extras.len()
    }

    pub fn blocked(&self) -> bool {
        !self.running.is_empty() || !self.on_it.is_empty()
    }

    pub fn request(&self) -> StorageDelete {
        StorageDelete {
            layers: self.layers.iter().map(|n| n.key.clone()).collect(),
            wadspaces: self.wadspaces.iter().filter_map(|n| n.wadspace_id.clone()).collect(),
            images: self.extras.iter().filter(|x| x.kind == OtherKind::Image).map(|x| x.id.clone()).collect(),
            volumes: self.extras.iter().filter(|x| x.kind == OtherKind::Volume).map(|x| x.id.clone()).collect(),
            update_copy: self.extras.iter().any(|x| x.kind == OtherKind::UpdateCopy),
        }
    }
}

pub fn picks<'a>(nodes: &'a [Node], st: &Storage, picked: &HashSet<String>) -> Picks<'a> {
    let kids = |key: &str| nodes.iter().filter(move |n| n.parent.as_deref() == Some(key)).collect::<Vec<_>>();
    fn descendants<'a>(nodes: &'a [Node], key: &str, out: &mut Vec<&'a Node>) {
        for k in nodes.iter().filter(|n| n.parent.as_deref() == Some(key)) {
            out.push(k);
            descendants(nodes, &k.key, out);
        }
    }
    let tree: Vec<&Node> = nodes.iter().filter(|n| picked.contains(&n.key) && n.pickable()).collect();
    let extras: Vec<Other> = other_picks(st).into_iter().filter(|x| picked.contains(&x.key)).collect();
    let layers: Vec<&Node> = tree.iter().copied().filter(|n| n.kind == Kind::Set).collect();
    let wadspaces: Vec<&Node> = tree.iter().copied().filter(|n| n.kind == Kind::Wadspace).collect();
    let running = wadspaces.iter().copied().filter(|n| n.running).collect();
    let on_it = layers
        .iter()
        .flat_map(|l| kids(&l.key))
        .filter(|k| k.kind == Kind::Wadspace && k.installed && !picked.contains(&k.key))
        .collect();
    let kept: Vec<(&Node, Vec<&Node>)> = layers
        .iter()
        .map(|l| {
            let by: Vec<&Node> = kids(&l.key)
                .into_iter()
                .filter(|k| k.kind != Kind::Wadspace && k.bytes.is_some() && !picked.contains(&k.key))
                .collect();
            (*l, by)
        })
        .filter(|(_, by)| !by.is_empty())
        .collect();
    let frees = wadspaces.iter().map(|n| n.bytes.unwrap_or(0)).sum::<u64>()
        + layers
            .iter()
            .filter(|l| !kept.iter().any(|(k, _)| k.key == l.key))
            .map(|n| n.bytes.unwrap_or(0))
            .sum::<u64>()
        + extras.iter().map(|x| x.bytes).sum::<u64>();
    let mut more: Vec<&Node> = vec![];
    for l in &layers {
        let mut d = vec![];
        descendants(nodes, &l.key, &mut d);
        for n in d {
            if n.pickable() && !picked.contains(&n.key) && !more.iter().any(|m| m.key == n.key) {
                more.push(n);
            }
        }
    }
    Picks { layers, wadspaces, extras, running, on_it, kept, frees, more }
}

/// What a delete or prune freed, as a toast: (title, body, failed).
/// What's left over, said in a line (none: nothing is).
pub fn leftover_text(left: &[LayerBytes]) -> Option<String> {
    if left.is_empty() {
        return None;
    }
    let n = |p: &str| left.iter().filter(|l| l.layer.starts_with(p)).count();
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let (sets, pkgs, old) = (n("set:"), n("package:"), n("template:") + n("custom:"));
    let mut what = vec![];
    if sets > 0 {
        what.push(plural(sets, "set no wadspace here runs on", "sets no wadspace here runs on"));
    }
    if pkgs > 0 {
        what.push(plural(pkgs, "package no wadspace here needs", "packages no wadspace here needs"));
    }
    if old > 0 {
        what.push("the old Builder's images".into());
    }
    if what.is_empty() {
        what.push("steps builds left".into());
    }
    let what = match what.len() {
        1 => what.remove(0),
        _ => {
            let last = what.pop().unwrap_or_default();
            format!("{} and {last}", what.join(", "))
        }
    };
    let total: u64 = left.iter().map(|l| l.own_bytes).sum();
    Some(format!("Left over: {what}, {}. It goes after the next build, or now.", bytes(total)))
}

/// The packages kept, each with what it's for: (name, detail, bytes).
/// `wadspaces`: (wadspace, its name), each on its set.
pub fn package_rows(st: &Storage, wadspaces: &[(&Value, String)]) -> Vec<(String, String, u64)> {
    st.packages
        .iter()
        .map(|p| {
            let mut names: Vec<&str> = wadspaces
                .iter()
                .filter(|(w, _)| p.sets.contains(&wad_core::v5::image_key(w)))
                .map(|(_, n)| n.as_str())
                .collect();
            names.sort();
            names.dedup();
            let detail = if names.is_empty() {
                format!("{} · kept for a wadspace whose image isn't built yet", p.version)
            } else {
                format!("{} · in {}", p.version, names.join(", "))
            };
            (p.name.clone(), detail, p.bytes)
        })
        .collect()
}

pub fn freed_text(f: &Freed) -> (String, String, bool) {
    let plural = |n: usize, one: &'static str, many: &'static str| if n == 1 { one } else { many };
    let mut what = vec![];
    if !f.removed.is_empty() {
        what.push(format!("{} image{}", f.removed.len(), plural(f.removed.len(), "", "s")));
    }
    if !f.uninstalled.is_empty() {
        what.push(format!("{} and {} files", f.uninstalled.join(", "), plural(f.uninstalled.len(), "its", "their")));
    }
    let mut body = if what.is_empty() {
        "Nothing needed deleting.".to_string()
    } else {
        format!("Deleted {}.", what.join(" and "))
    };
    if !f.kept.is_empty() {
        let n = f.kept.len();
        body.push_str(&format!(
            " {} kept {} files: an image here is built on {}.",
            f.kept.join(", "),
            plural(n, "its", "their"),
            plural(n, "it", "them")
        ));
    }
    if !f.failed.is_empty() {
        body.push_str(&format!(" Not deleted: {}.", f.failed.join("; ")));
    }
    (format!("Freed about {}", bytes(f.bytes)), body, !f.failed.is_empty())
}

// ------------------------------------------------------------------ view

thread_local! {
    static VIEW: RefCell<std::rc::Weak<View>> = const { RefCell::new(std::rc::Weak::new()) };
}

/// The packages' card opened (snapshots).
pub fn show_packages() {
    if let Some(v) = VIEW.with(|w| w.borrow().upgrade()) {
        v.packages_open.set(true);
        v.render();
    }
}

struct View {
    storage: RefCell<Option<Storage>>,
    images: RefCell<HashMap<String, ImageRow>>,
    error: RefCell<Option<String>>,
    busy: Cell<bool>,
    picked: RefCell<HashSet<String>>,
    report_open: Cell<bool>,
    packages_open: Cell<bool>,
    /// The layers building now (their notes say so).
    building: RefCell<Vec<String>>,
    body: gtk::Box,
    refresh: gtk::Button,
}

pub fn view() -> gtk::Widget {
    let p = ui::panel(
        "Storage",
        Some("hard-drive"),
        Some(
            "What your wadspaces take on This machine's disk. Wadspaces with the same apps share their image: each \
             shows what it adds to the base. Click apps or wadspaces to delete them from this machine.",
        ),
    );
    let refresh = ui::icon_button("refresh-cw", "Measure again", 16);
    p.actions.append(&refresh);
    p.body.add_css_class("panel-pad");
    p.body.set_spacing(24);
    let v = Rc::new(View {
        storage: RefCell::default(),
        images: RefCell::default(),
        error: RefCell::default(),
        busy: Cell::new(false),
        picked: RefCell::default(),
        report_open: Cell::new(false),
        packages_open: Cell::new(false),
        building: RefCell::default(),
        body: p.body.clone(),
        refresh: refresh.clone(),
    });
    VIEW.with(|w| w.replace(Rc::downgrade(&v)));
    let vv = v.clone();
    refresh.connect_clicked(move |_| vv.clone().load(true));
    // A new count from wadd (and the first look).
    let vv = v.clone();
    store().watch(&p.root, Topic::STORAGE | Topic::WADD, move |_| vv.clone().load(false));
    let vv = v.clone();
    store().watch(&p.root, Topic::LIBRARY | Topic::STATES | Topic::WORKSPACES, move |_| vv.render());
    // Builds: only one starting or ending changes a note (not its progress).
    let vv = v.clone();
    store().watch(&p.root, Topic::ACTIVITY, move |s| {
        let building: Vec<String> =
            s.builds.iter().filter(|(b, _)| !b.status.finished()).filter_map(|(b, _)| b.layer.clone()).collect();
        if *vv.building.borrow() != building {
            vv.building.replace(building);
            vv.render();
        }
    });
    p.root.upcast()
}

impl View {
    fn load(self: Rc<Self>, fresh: bool) {
        if self.busy.replace(true) {
            return;
        }
        self.spin();
        glib::spawn_future_local(async move {
            let path = if fresh { "/v1/storage?fresh=true" } else { "/v1/storage" };
            match get::<Storage>(path.into()).await {
                Ok(st) => {
                    self.storage.replace(Some(st));
                    self.error.replace(None);
                }
                Err(e) => {
                    self.error.replace(Some(e));
                }
            }
            if let Ok(rows) = get::<Vec<ImageRow>>("/v1/images".into()).await {
                self.images.replace(rows.into_iter().map(|r| (r.layer.clone(), r)).collect());
            }
            self.busy.set(false);
            self.spin();
            self.render();
        });
    }

    /// The refresh button turns while measuring.
    fn spin(&self) {
        let on = self.busy.get() || self.storage.borrow().as_ref().is_some_and(|s| s.measuring);
        ui::set_icon(&self.refresh, if on { "loader-circle" } else { "refresh-cw" });
        if on {
            self.refresh.add_css_class("spinning");
        } else {
            self.refresh.remove_css_class("spinning");
        }
        self.refresh.set_sensitive(!self.busy.get());
    }

    fn toggle(self: &Rc<Self>, key: &str) {
        {
            let mut p = self.picked.borrow_mut();
            if !p.remove(key) {
                p.insert(key.to_string());
            }
        }
        self.render();
    }

    fn nodes(&self, s: &AppState, st: &Storage) -> Vec<Node> {
        if s.library.is_none() {
            return vec![];
        }
        let stacks: Vec<(&Value, String)> = s.wadspaces().into_iter().map(|w| (w, s.wadspace_name(w))).collect();
        let containers = machine(s, THIS).map(|m| m.containers).unwrap_or_default();
        let images = self.images.borrow();
        let building = self.building.borrow();
        tree_of(st, &stacks, &containers, |key| {
            if building.iter().any(|b| b == key) {
                return Some(("building…", "on"));
            }
            match image_state(key, &images) {
                ImageState::Update => Some(("update available", "warn")),
                _ => None,
            }
        })
    }

    fn render(self: &Rc<Self>) {
        ui::clear(&self.body);
        if let Some(e) = self.error.borrow().as_ref() {
            let n = ui::notice(e, true);
            n.set_margin_top(0);
            self.body.append(&n);
            return;
        }
        let st = self.storage.borrow();
        let Some(st) = st.as_ref() else {
            let l = gtk::Label::new(Some("Measuring…"));
            l.add_css_class("small-muted-14");
            l.set_xalign(0.0);
            self.body.append(&l);
            return;
        };
        let nodes = store().read_now(|s| self.nodes(s, st));
        self.body.append(&self.disk_bar(st));
        if let Some(w) = self.other(st) {
            self.body.append(&w);
        }
        self.body.append(&self.tree(&nodes));
        if let Some(w) = self.packages(st) {
            self.body.append(&w);
        }
        let picked = self.picked.borrow().clone();
        if !picked.is_empty() {
            self.body.append(&self.picked_bar(st, &nodes, &picked));
        }
    }

    /// The disk: images, wadspaces' files, copies, everything else, free.
    fn disk_bar(self: &Rc<Self>, st: &Storage) -> gtk::Widget {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let packages: u64 = st.packages.iter().map(|p| p.bytes).sum();
        // Each layer once (a wadd from before counted sets on their own).
        let images = if st.images_bytes > 0 {
            st.images_bytes.saturating_sub(packages)
        } else {
            st.base.as_ref().map(|b| b.bytes).unwrap_or(0) + st.layers.iter().map(|l| l.own_bytes).sum::<u64>()
        };
        let homes: u64 = st.homes.iter().map(|h| h.bytes).sum();
        let copies: u64 = st.copies.iter().map(|c| c.bytes).sum();
        let total = st.disk.total_bytes.max(1);
        let used = st.disk.total_bytes.saturating_sub(st.disk.free_bytes);
        let other = used.saturating_sub(images + packages + homes + copies);
        let parts = [
            ("Images", images, "template"),
            ("Packages", packages, "packages"),
            ("Wadspaces' files", homes, "customization"),
            ("Copied in", copies, "extensions"),
            ("Everything else", other, "else"),
        ];
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let uses = gtk::Label::new(Some(&format!("WadSpaces uses {}", bytes(images + packages + homes + copies))));
        uses.add_css_class("strong");
        uses.set_hexpand(true);
        uses.set_xalign(0.0);
        head.append(&uses);
        let free =
            gtk::Label::new(Some(&format!("{} free of {}", bytes(st.disk.free_bytes), bytes(st.disk.total_bytes))));
        free.add_css_class("mono-muted");
        head.append(&free);
        col.append(&head);
        let bar = Shares::new(
            parts
                .iter()
                .filter(|(_, b, _)| *b > 0)
                .map(|(label, b, class)| {
                    let w = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    w.add_css_class("share");
                    w.add_css_class(class);
                    w.set_tooltip_text(Some(&format!("{label}: {}", bytes(*b))));
                    (w.upcast::<gtk::Widget>(), *b as f64 / total as f64)
                })
                .collect(),
        );
        bar.add_css_class("disk-bar");
        col.append(&bar);
        let legend = gtk::FlowBox::new();
        legend.add_css_class("legend");
        legend.set_selection_mode(gtk::SelectionMode::None);
        legend.set_max_children_per_line(8);
        legend.set_column_spacing(20);
        legend.set_row_spacing(6);
        let key = |label: &str, b: u64, class: &str| {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let sw = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            sw.add_css_class("swatch");
            sw.add_css_class(class);
            sw.set_valign(gtk::Align::Center);
            row.append(&sw);
            let l = gtk::Label::new(Some(label));
            l.add_css_class("small-muted");
            row.append(&l);
            let n = gtk::Label::new(Some(&bytes(b)));
            n.add_css_class("mono-fg");
            row.append(&n);
            row
        };
        for (label, b, class) in parts {
            legend.insert(&key(label, b, class), -1);
        }
        legend.insert(&key("Free", st.disk.free_bytes, "free"), -1);
        col.append(&legend);
        if let Some(text) = leftover_text(&st.leftovers) {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("old-builds");
            let l = ui::wrapping(&text);
            l.add_css_class("small-muted");
            row.append(&l);
            let go = ui::button("Delete them", Some("trash"), Variant::Ghost, Size::Sm);
            let me = self.clone();
            go.connect_clicked(move |b| {
                b.set_sensitive(false);
                let (me, b) = (me.clone(), b.clone());
                glib::spawn_future_local(async move {
                    match send::<Freed>(Method::POST, "/v1/storage/prune".into(), Some(serde_json::json!({}))).await {
                        Ok(f) => me.clone().freed(&f),
                        Err(e) => crate::toast::error("Couldn't delete what's left over", &e),
                    }
                    b.set_sensitive(true);
                });
            });
            row.append(&go);
            col.append(&row);
        }
        col.upcast()
    }

    fn freed(self: Rc<Self>, f: &Freed) {
        self.picked.borrow_mut().clear();
        let (title, body, failed) = freed_text(f);
        crate::toast::show(
            &title,
            Some(&body),
            if failed { crate::toast::Tone::Error } else { crate::toast::Tone::Success },
        );
        self.load(true);
    }

    fn check(self: &Rc<Self>, key: &str, label: &str) -> gtk::CheckButton {
        let c = gtk::CheckButton::new();
        c.add_css_class("pick-box");
        c.set_active(self.picked.borrow().contains(key));
        c.set_tooltip_text(Some(&format!("Pick {label} to delete")));
        c.set_valign(gtk::Align::Start);
        let (me, key) = (self.clone(), key.to_string());
        c.connect_toggled(move |_| me.toggle(&key));
        c
    }

    /// "Everything else", part by part: what each is, how big, and where it
    /// can go, a box to pick it.
    /// The packages kept: closed, a line; open, each with what it's for.
    fn packages(self: &Rc<Self>, st: &Storage) -> Option<gtk::Widget> {
        if st.packages.is_empty() {
            return None;
        }
        let rows = store().read_now(|s| {
            let ws: Vec<(&Value, String)> = s.wadspaces().into_iter().map(|w| (w, s.wadspace_name(w))).collect();
            package_rows(st, &ws)
        });
        let total: u64 = rows.iter().map(|r| r.2).sum();
        let me = self.clone();
        let show = move |open: bool| {
            me.packages_open.set(open);
            me.render();
        };
        if !self.packages_open.get() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("other-closed");
            let t = format!(
                " · {} in {}, kept so a change to a wadspace's apps rebuilds without a download",
                bytes(total),
                rows.len()
            );
            let l = gtk::Label::new(None);
            l.set_markup(&format!("<b>Packages</b><span alpha=\"62%\">{}</span>", glib::markup_escape_text(&t)));
            l.set_hexpand(true);
            l.set_xalign(0.0);
            l.set_wrap(true);
            row.append(&l);
            let b = ui::button("Show", Some("layers"), Variant::Secondary, Size::Sm);
            b.set_valign(gtk::Align::Center);
            b.connect_clicked(move |_| show(true));
            row.append(&b);
            return Some(row.upcast());
        }
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("other-open");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        head.add_css_class("other-head");
        let t = gtk::Label::new(Some("Packages: each app's files, kept for the wadspaces here"));
        t.add_css_class("strong");
        t.set_hexpand(true);
        t.set_xalign(0.0);
        t.set_wrap(true);
        head.append(&t);
        let n = gtk::Label::new(Some(&bytes(total)));
        n.add_css_class("mono-muted");
        head.append(&n);
        let hide = ui::button("Hide", Some("chevron-up"), Variant::Ghost, Size::Sm);
        hide.connect_clicked(move |_| show(false));
        head.append(&hide);
        card.append(&head);
        for (name, detail, b) in rows {
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            top.add_css_class("other-part");
            let sq = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            sq.add_css_class("swatch");
            sq.add_css_class("packages");
            sq.set_valign(gtk::Align::Start);
            sq.set_margin_top(4);
            sq.set_margin_start(3);
            sq.set_margin_end(3);
            top.append(&sq);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_hexpand(true);
            let l = gtk::Label::new(Some(&name));
            l.add_css_class("medium");
            l.set_xalign(0.0);
            text.append(&l);
            let d = ui::wrapping(&detail);
            d.add_css_class("small-muted");
            text.append(&d);
            top.append(&text);
            let n = gtk::Label::new(Some(&bytes(b)));
            n.add_css_class("mono-14");
            n.set_valign(gtk::Align::Start);
            top.append(&n);
            card.append(&top);
        }
        Some(card.upcast())
    }

    fn other(self: &Rc<Self>, st: &Storage) -> Option<gtk::Widget> {
        let mut parts = st.other.clone();
        if parts.is_empty() {
            return None;
        }
        parts.sort_by_key(|p| std::cmp::Reverse(p.bytes));
        let freeable = other_picks(st);
        let total: u64 = parts.iter().map(|p| p.bytes).sum();
        let me = self.clone();
        let report = move |open: bool| {
            me.report_open.set(open);
            // Opened: everything in it that can go, ticked; closed: unticked.
            let keys: Vec<String> =
                me.storage.borrow().as_ref().map(other_picks).unwrap_or_default().into_iter().map(|x| x.key).collect();
            {
                let mut p = me.picked.borrow_mut();
                for k in keys {
                    if open {
                        p.insert(k);
                    } else {
                        p.remove(&k);
                    }
                }
            }
            me.render();
        };
        if !self.report_open.get() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("other-closed");
            let mut t = format!(" · {} in {} parts", bytes(total), parts.len());
            if !freeable.is_empty() {
                t.push_str(&format!(", {} of it can go", bytes(freeable.iter().map(|x| x.bytes).sum())));
            }
            let l = gtk::Label::new(None);
            l.set_markup(&format!("<b>Everything else</b><span alpha=\"62%\">{}</span>", glib::markup_escape_text(&t)));
            l.set_hexpand(true);
            l.set_xalign(0.0);
            l.set_wrap(true);
            row.append(&l);
            let b = ui::button("Report", Some("file-search"), Variant::Secondary, Size::Sm);
            b.set_valign(gtk::Align::Center);
            b.connect_clicked(move |_| report(true));
            row.append(&b);
            return Some(row.upcast());
        }
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("other-open");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        head.add_css_class("other-head");
        let t = gtk::Label::new(Some("Everything else, part by part"));
        t.add_css_class("strong");
        t.set_hexpand(true);
        t.set_xalign(0.0);
        head.append(&t);
        let n = gtk::Label::new(Some(&bytes(total)));
        n.add_css_class("mono-muted");
        head.append(&n);
        let hide = ui::button("Hide", Some("chevron-up"), Variant::Ghost, Size::Sm);
        hide.connect_clicked(move |_| report(false));
        head.append(&hide);
        card.append(&head);
        for p in &parts {
            let li = gtk::Box::new(gtk::Orientation::Vertical, 8);
            li.add_css_class("other-part");
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            if p.key == "update-copy" && p.deletable {
                top.append(&self.check("update-copy", &p.label));
            } else {
                let sq = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                sq.add_css_class("swatch");
                sq.add_css_class("else");
                sq.set_valign(gtk::Align::Start);
                sq.set_margin_top(4);
                sq.set_margin_start(3);
                sq.set_margin_end(3);
                top.append(&sq);
            }
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_hexpand(true);
            let l = gtk::Label::new(Some(&p.label));
            l.add_css_class("medium");
            l.set_xalign(0.0);
            text.append(&l);
            let d = ui::wrapping(&p.detail);
            d.add_css_class("small-muted");
            text.append(&d);
            top.append(&text);
            let b = gtk::Label::new(Some(&bytes(p.bytes)));
            b.add_css_class("mono-14");
            b.set_valign(gtk::Align::Start);
            top.append(&b);
            li.append(&top);
            if !p.items.is_empty() {
                let items = gtk::Box::new(gtk::Orientation::Vertical, 0);
                items.add_css_class("other-items");
                for i in &p.items {
                    let r = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                    r.add_css_class("other-item");
                    let key = format!("{}:{}", if p.key == "images" { "img" } else { "vol" }, i.id);
                    if i.deletable {
                        r.append(&self.check(&key, &i.label));
                    } else {
                        let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                        gap.set_size_request(16, -1);
                        gap.set_tooltip_text(Some("Something still needs it"));
                        r.append(&gap);
                    }
                    let tx = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    tx.set_hexpand(true);
                    let a = gtk::Label::new(Some(&i.label));
                    a.add_css_class("tiny-strong");
                    a.set_xalign(0.0);
                    a.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    tx.append(&a);
                    let b = gtk::Label::new(Some(&i.detail));
                    b.add_css_class("tiny-muted");
                    b.set_xalign(0.0);
                    b.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    tx.append(&b);
                    r.append(&tx);
                    let n = gtk::Label::new(Some(&bytes(i.bytes)));
                    n.add_css_class("mono-12");
                    r.append(&n);
                    items.append(&r);
                }
                li.append(&items);
            }
            card.append(&li);
        }
        Some(card.upcast())
    }

    fn tree(self: &Rc<Self>, nodes: &[Node]) -> gtk::Widget {
        let (rows_of, rows, cols) = layout(nodes);
        if rows == 0 {
            let l = gtk::Label::new(Some("No wadspaces yet: make one in the Builder."));
            l.add_css_class("small-muted-14");
            l.set_xalign(0.0);
            return l.upcast();
        }
        let picked = self.picked.borrow().clone();
        let mut placed: Vec<Placed> = vec![];
        let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.key.as_str(), i)).collect();
        for n in nodes {
            let (w, stripe) = self.node(n, picked.contains(&n.key));
            placed.push(Placed {
                widget: w,
                col: n.col,
                row: rows_of.get(&n.key).copied().unwrap_or(0.0),
                parent: n.parent.as_deref().and_then(|p| index.get(p).copied()),
                stripe,
                absent: n.bytes.is_none(),
            });
        }
        let tree = Tree::new(placed, cols, rows);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_hscrollbar_policy(gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_child(Some(&tree));
        scroll.upcast()
    }

    /// A node: its kind's stripe, kind and name, its share and note; picked,
    /// a check in its corner. (The node, and the stripe its edge is coloured
    /// by.)
    fn node(self: &Rc<Self>, n: &Node, on: bool) -> (gtk::Widget, gtk::Widget) {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let stripe = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stripe.add_css_class("stripe");
        content.append(&stripe);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let kind = gtk::Label::new(Some(&n.kind.label().to_uppercase()));
        kind.add_css_class("node-kind");
        kind.set_xalign(0.0);
        text.append(&kind);
        let name = gtk::Label::new(Some(&n.name));
        name.add_css_class("node-name");
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(1);
        text.append(&name);
        content.append(&text);
        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.set_valign(gtk::Align::Center);
        right.set_margin_start(8);
        let b = gtk::Label::new(Some(&n.bytes.map(bytes).unwrap_or_else(|| "—".into())));
        b.add_css_class("mono-12");
        b.set_xalign(1.0);
        right.append(&b);
        if let Some(note) = n.note {
            let l = gtk::Label::new(Some(note));
            l.add_css_class("node-note");
            if !n.tone.is_empty() {
                l.add_css_class(n.tone);
            }
            l.set_xalign(1.0);
            right.append(&l);
        }
        content.append(&right);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&content));
        if on {
            let c = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            c.add_css_class("picked-check");
            c.set_halign(gtk::Align::End);
            c.set_valign(gtk::Align::Start);
            c.append(&Icon::new("check", 10));
            overlay.add_overlay(&c);
        }
        let w: gtk::Widget = if n.pickable() {
            let b = gtk::Button::new();
            b.set_child(Some(&overlay));
            b.set_tooltip_text(Some(if on {
                "Picked to delete: click to leave it"
            } else {
                "Click to pick it to delete"
            }));
            let (me, key) = (self.clone(), n.key.clone());
            b.connect_clicked(move |_| me.toggle(&key));
            b.upcast()
        } else {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
            b.append(&overlay);
            if n.kind == Kind::Base {
                b.set_tooltip_text(Some("The base every wadspace runs on: it stays"));
            } else if n.bytes.is_none() {
                b.set_tooltip_text(Some("Not on this machine"));
            }
            b.upcast()
        };
        w.add_css_class("tree-node");
        w.add_css_class(n.kind.class());
        if on {
            w.add_css_class("picked");
        }
        if n.bytes.is_none() && n.kind != Kind::Wadspace {
            w.add_css_class("absent");
        }
        w.set_size_request(NODE_W, -1);
        (w, stripe.upcast())
    }

    /// What's picked: what it frees, what stops it, and the delete (after a
    /// look at what goes).
    fn picked_bar(self: &Rc<Self>, st: &Storage, nodes: &[Node], picked: &HashSet<String>) -> gtk::Widget {
        let p = picks(nodes, st, picked);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.add_css_class("picked-bar");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let l = gtk::Label::new(None);
        l.set_markup(&format!(
            "<b>{} picked</b><span alpha=\"62%\"> · frees about {}</span>",
            p.count(),
            glib::markup_escape_text(&bytes(p.frees))
        ));
        l.set_hexpand(true);
        l.set_xalign(0.0);
        row.append(&l);
        let add = |me: &Rc<Self>, keys: Vec<String>| {
            let me = me.clone();
            move || {
                me.picked.borrow_mut().extend(keys.iter().cloned());
                me.render();
            }
        };
        if !p.more.is_empty() {
            let b = ui::button(
                &format!(
                    "Pick what's built on {} (+{})",
                    if p.layers.len() == 1 { "it" } else { "them" },
                    p.more.len()
                ),
                None,
                Variant::Ghost,
                Size::Sm,
            );
            b.set_tooltip_text(Some(&p.more.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", ")));
            let f = add(self, p.more.iter().map(|n| n.key.clone()).collect());
            b.connect_clicked(move |_| f());
            row.append(&b);
        }
        let clear = ui::button("Clear", Some("x"), Variant::Ghost, Size::Sm);
        let me = self.clone();
        clear.connect_clicked(move |_| {
            me.picked.borrow_mut().clear();
            me.render();
        });
        row.append(&clear);
        let del = ui::button("Delete…", Some("trash"), Variant::Danger, Size::Sm);
        del.set_sensitive(!p.blocked());
        let (me, req) = (self.clone(), p.request());
        let lines = confirm_lines(&p);
        let frees = p.frees;
        del.connect_clicked(move |_| me.confirm(req.clone(), lines.clone(), frees));
        row.append(&del);
        card.append(&row);
        let notes = gtk::Box::new(gtk::Orientation::Vertical, 4);
        for n in &p.running {
            let l = ui::wrapping(&format!("{} is running: stop it first.", n.name));
            l.add_css_class("small-danger");
            notes.append(&l);
        }
        for n in &p.on_it {
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let l = gtk::Label::new(Some(&format!("{} runs on a picked image.", n.name)));
            l.add_css_class("small-danger");
            r.append(&l);
            let b = gtk::Button::with_label("Pick it too");
            b.add_css_class("text-link");
            let f = add(self, vec![n.key.clone()]);
            b.connect_clicked(move |_| f());
            r.append(&b);
            notes.append(&r);
        }
        for (l, by) in &p.kept {
            let names: Vec<&str> = by.iter().map(|k| k.name.as_str()).collect();
            let t = ui::wrapping(&format!(
                "{}'s files stay while {} {} built on it: only its name goes.",
                l.name,
                names.join(", "),
                if by.len() == 1 { "is" } else { "are" }
            ));
            t.add_css_class("small-muted");
            notes.append(&t);
        }
        if notes.first_child().is_some() {
            card.append(&notes);
        }
        card.upcast()
    }

    fn confirm(self: &Rc<Self>, req: StorageDelete, lines: Vec<(String, String, String)>, frees: u64) {
        let m = Modal::open(
            "Delete from this machine?",
            Some("Only from this machine's disk: your library keeps every design, to build again."),
            560,
        );
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        for (name, kind, detail) in lines {
            let li = gtk::Box::new(gtk::Orientation::Vertical, 2);
            li.add_css_class("confirm-item");
            let t = gtk::Label::new(None);
            t.set_markup(&format!(
                "<b>{}</b> <span alpha=\"62%\">({})</span>",
                glib::markup_escape_text(&name),
                glib::markup_escape_text(&kind)
            ));
            t.set_xalign(0.0);
            t.set_wrap(true);
            li.append(&t);
            let d = ui::wrapping(&detail);
            d.add_css_class("small-muted");
            li.append(&d);
            list.append(&li);
        }
        m.body().append(&list);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_halign(gtk::Align::End);
        row.set_margin_top(20);
        let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
        let mm = m.clone();
        cancel.connect_clicked(move |_| mm.close());
        let go =
            ui::button(&format!("Delete, freeing about {}", bytes(frees)), Some("trash"), Variant::Danger, Size::Md);
        let (mm, me) = (m.clone(), self.clone());
        go.connect_clicked(move |b| {
            b.set_sensitive(false);
            let body = serde_json::to_value(&req).unwrap_or_default();
            let (mm, me, b) = (mm.clone(), me.clone(), b.clone());
            glib::spawn_future_local(async move {
                match send::<Freed>(Method::POST, "/v1/storage/delete".into(), Some(body)).await {
                    Ok(f) => {
                        mm.close();
                        me.freed(&f);
                    }
                    Err(e) => crate::toast::error("Couldn't delete them", &e),
                }
                b.set_sensitive(true);
            });
        });
        row.append(&cancel);
        row.append(&go);
        m.body().append(&row);
    }
}

/// What the confirmation lists: (name, what it is, what happens to it).
fn confirm_lines(p: &Picks) -> Vec<(String, String, String)> {
    let mut out = vec![];
    for n in &p.wadspaces {
        out.push((
            n.name.clone(),
            "Customization".into(),
            format!(
                "Taken off this machine, and its files here deleted for good: what's in its home and the copies only it had{}.",
                n.bytes.filter(|b| *b > 0).map(|b| format!(" ({})", bytes(b))).unwrap_or_default()
            ),
        ));
    }
    for n in &p.layers {
        let by = p.kept.iter().find(|(k, _)| k.key == n.key).map(|(_, by)| by.clone()).unwrap_or_default();
        let detail = if by.is_empty() {
            format!(
                "Its image deleted, and its older builds{}. Built again when a wadspace on it is built.",
                n.bytes.filter(|b| *b > 0).map(|b| format!(" ({})", bytes(b))).unwrap_or_default()
            )
        } else {
            format!(
                "Its name goes; its files stay while {} {} built on it.",
                by.iter().map(|k| k.name.as_str()).collect::<Vec<_>>().join(", "),
                if by.len() == 1 { "is" } else { "are" }
            )
        };
        out.push((n.name.clone(), n.kind.label().into(), detail));
    }
    for x in &p.extras {
        out.push((x.label.clone(), bytes(x.bytes), x.detail.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wad_proto::v1::{DiagDisk, HomeBytes, ImageBytes, LayerBytes, OtherItem, OtherPart};

    fn storage() -> Storage {
        Storage {
            disk: DiagDisk { path: "/".into(), free_bytes: 48_000_000_000, total_bytes: 120_000_000_000 },
            base: Some(ImageBytes { image_ref: "base".into(), id: "b".into(), bytes: 3_100_000_000 }),
            layers: vec![LayerBytes {
                layer: set("vscode"),
                id: "i1".into(),
                from: "b".into(),
                bytes: 0,
                own_bytes: 2_000_000_000,
            }],
            leftovers: vec![],
            packages: vec![],
            images_bytes: 0,
            homes: vec![HomeBytes { workspace_id: "vanua".into(), volume: "v".into(), bytes: 900_000_000 }],
            copies: vec![],
            other: vec![OtherPart {
                key: "volumes".into(),
                label: "Homes left behind".into(),
                detail: "".into(),
                bytes: 1_900_000_000,
                items: vec![OtherItem {
                    id: "wad-old-config".into(),
                    label: "old's home".into(),
                    detail: "".into(),
                    bytes: 1_900_000_000,
                    deletable: true,
                }],
                deletable: false,
            }],
            measured_at: 0.0,
            measuring: false,
        }
    }

    /// A wadspace with these apps in one template.
    fn wadspace(id: &str, apps: &[&str]) -> Value {
        let apps: Vec<Value> = apps.iter().map(|a| json!({ "app": a, "extras": [] })).collect();
        json!({ "version": 4, "id": id, "machine": "m", "templates": [{ "id": "t", "name": "T", "apps": apps }],
            "customization": { "name": id } })
    }

    /// The set those apps run on.
    fn set(app: &str) -> String {
        wad_core::v5::image_key(&wadspace("x", &[app]))
    }

    fn running(id: &str) -> Container {
        Container {
            id: id.into(),
            name: id.into(),
            running: true,
            stream: false,
            since: 0.0,
            phase: "ready".into(),
            error: None,
            on_screen: false,
            hotkey: None,
        }
    }

    #[test]
    fn the_tree_runs_from_the_base_through_the_sets_to_the_wadspaces() {
        let vanua = wadspace("vanua", &["vscode"]);
        let cosmic = wadspace("cosmic", &["obsidian"]);
        let bare = wadspace("bare", &[]);
        let stacks = vec![(&vanua, "Vanua".to_string()), (&cosmic, "Cosmic".to_string()), (&bare, "Bare".to_string())];
        let nodes = tree_of(&storage(), &stacks, &[running("vanua")], |_| None);
        let keys: Vec<&str> = nodes.iter().map(|n| n.key.as_str()).collect();
        let (code, notes) = (set("vscode"), set("obsidian"));
        assert_eq!(keys, vec!["base", &code, "ws:vanua", &notes, "ws:cosmic", "ws:bare"]);
        assert_eq!((nodes[1].name.as_str(), nodes[1].bytes), ("VS Code", Some(2_000_000_000)));
        let ws = &nodes[2];
        assert_eq!((ws.col, ws.bytes, ws.note, ws.running), (2, Some(900_000_000), Some("running"), true));
        // Not built: no share, and nothing to pick.
        assert_eq!(nodes[3].bytes, None);
        assert!(!nodes[3].pickable() && !nodes[0].pickable() && !nodes[4].pickable());
        // No apps: right on the base.
        assert_eq!(nodes[5].parent.as_deref(), Some("base"));
        let (rows, n, cols) = layout(&nodes);
        assert_eq!((n, cols), (3, 2));
        assert_eq!(rows["ws:bare"], 0.0);
        assert_eq!(rows["base"], 1.0);
    }

    #[test]
    fn picks_say_what_they_free_and_what_stops_them() {
        let vanua = wadspace("vanua", &["vscode"]);
        let stacks = vec![(&vanua, "Vanua".to_string())];
        let st = storage();
        let code = set("vscode");
        // Vanua stopped: its set and an old home go; Vanua could too.
        let nodes = tree_of(&st, &stacks, &[Container { running: false, ..running("vanua") }], |_| None);
        let picked: HashSet<String> = [code.clone(), "vol:wad-old-config".to_string()].into();
        let p = picks(&nodes, &st, &picked);
        assert_eq!(p.count(), 2);
        assert!(p.kept.is_empty());
        assert_eq!(p.frees, 3_900_000_000);
        assert_eq!(p.more.iter().map(|n| n.key.as_str()).collect::<Vec<_>>(), vec!["ws:vanua"]);
        assert!(p.blocked(), "Vanua's installed on it: pick it too");
        // Vanua picked while it runs: blocked.
        let nodes = tree_of(&st, &stacks, &[running("vanua")], |_| None);
        let picked: HashSet<String> = [code.clone(), "ws:vanua".to_string()].into();
        let p = picks(&nodes, &st, &picked);
        assert!(p.blocked());
        assert_eq!(p.frees, 2_900_000_000);
        let r = p.request();
        assert_eq!((r.layers, r.wadspaces), (vec![code], vec!["vanua".to_string()]));
    }

    #[test]
    fn whats_left_over_says_what_it_is() {
        assert_eq!(leftover_text(&[]), None);
        let l = |layer: &str, b: u64| LayerBytes {
            layer: layer.into(),
            id: "i".into(),
            from: String::new(),
            bytes: b,
            own_bytes: b,
        };
        let left = [
            l("set:aaaaaaaaaaaa", 2_000_000_000),
            l("step", 0),
            l("package:tiled", 100_000_000),
            l("package:obsidian", 200_000_000),
            l("template:dev", 600_000_000),
        ];
        assert_eq!(
            leftover_text(&left).unwrap(),
            format!(
                "Left over: 1 set no wadspace here runs on, 2 packages no wadspace here needs and the old Builder's \
                 images, {}. It goes after the next build, or now.",
                bytes(2_900_000_000)
            )
        );
        assert!(leftover_text(&[l("chain", 5)]).unwrap().starts_with("Left over: steps builds left,"));
    }

    #[test]
    fn packages_say_which_wadspaces_theyre_in() {
        let vanua = wadspace("vanua", &["vscode"]);
        let mut st = storage();
        let pkg = |id: &str, name: &str, sets: Vec<String>| wad_proto::v1::PackageBytes {
            id: id.into(),
            name: name.into(),
            version: "cccccccccccc".into(),
            bytes: 1_000_000_000,
            sets,
        };
        st.packages =
            vec![pkg("vscode", "VS Code", vec![wad_core::v5::image_key(&vanua)]), pkg("tiled", "Tiled", vec![])];
        let rows = package_rows(&st, &[(&vanua, "Vanua".to_string())]);
        assert_eq!(rows[0], ("VS Code".into(), "cccccccccccc · in Vanua".into(), 1_000_000_000));
        assert_eq!(rows[1].1, "cccccccccccc · kept for a wadspace whose image isn't built yet");
    }

    #[test]
    fn what_was_freed_reads_as_the_web_app_says_it() {
        let f = Freed {
            removed: vec!["set:0123456789ab".into(), "x".into()],
            kept: vec!["Dev".into()],
            uninstalled: vec!["Vanua".into()],
            failed: vec![],
            bytes: 2_500_000_000,
        };
        let (title, body, failed) = freed_text(&f);
        assert_eq!(title, "Freed about 2.5 GB");
        assert_eq!(body, "Deleted 2 images and Vanua and its files. Dev kept its files: an image here is built on it.");
        assert!(!failed);
    }
}
