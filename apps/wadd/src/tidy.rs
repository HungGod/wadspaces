//! What builds leave that nothing here needs any more, and the tidy-up that
//! removes it: after the last build out, when Storage's "Delete them" asks,
//! and after things are taken off the disk. Never while a build or a
//! package download runs (one may be about to use what's left).
//!
//! Kept: every image a workspace here runs on, with everything it's built
//! from (its set's chain steps, podman's steps), and the newest copy of
//! each package the wadspaces installed here need, so a change to them
//! restacks from what's here, without a download. The rest of WadSpaces'
//! own goes: sets no workspace here runs on and older builds of sets, chain
//! and build steps nothing kept stands on, other packages (older versions,
//! ones no wadspace here needs, ones fetched ahead), and the old Builder's
//! template and Extensions images. An image that isn't WadSpaces' own, the
//! base and the stream sidecar stay.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::Value;
use wad_proto::v1::Freed;

use crate::backend::{Backend, ImageInfo};
use crate::builds::{Builds, FROM_LABEL, LAYER_LABEL};
use crate::packages::PKG_LABEL;
use crate::registry::Registry;
use crate::sets::CHAIN_LABEL;

/// What one of WadSpaces' images is to the tidy-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// A set ("set:<key>"), or an older build of one.
    Set(String),
    Chain,
    /// A package, by its id.
    Package(String),
    /// The old Builder's: "template:<id>" or "custom:<id>".
    Old(String),
    /// An untagged step under one of these.
    Step,
}

impl Part {
    /// How the Storage report names it (`LayerBytes::layer`).
    pub fn label(&self) -> String {
        match self {
            Part::Set(k) | Part::Old(k) => k.clone(),
            Part::Chain => "chain".into(),
            Part::Package(id) => format!("package:{id}"),
            Part::Step => "step".into(),
        }
    }
}

fn label<'a>(i: &'a ImageInfo, k: &str) -> &'a str {
    i.labels.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn short(id: &str) -> &str {
    id.trim_start_matches("sha256:")
}

/// What an image of WadSpaces' own is; None for any other image.
pub fn part_of(i: &ImageInfo) -> Option<Part> {
    let layer = label(i, LAYER_LABEL);
    if layer.starts_with("set:") {
        return Some(Part::Set(layer.into()));
    }
    if !layer.is_empty() {
        return Some(Part::Old(layer.into()));
    }
    if i.labels.contains_key(CHAIN_LABEL) {
        return Some(Part::Chain);
    }
    let pkg = label(i, PKG_LABEL);
    (!pkg.is_empty()).then(|| Part::Package(pkg.into()))
}

/// What the tidy-up keeps things for: the images workspaces here run on,
/// and the packages the wadspaces installed here need.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wanted {
    pub running_on: Vec<String>,
    pub needs: Vec<String>,
}

impl Wanted {
    pub fn here(registry: &Registry, state_dir: &Path) -> Self {
        let workspaces = registry.workspaces();
        let mut needs: Vec<String> = workspaces
            .iter()
            .filter_map(|w| crate::sets::installed_doc(state_dir, &w.id))
            .flat_map(|d| wad_core::v5::packages_of(&d))
            .collect();
        needs.sort();
        needs.dedup();
        Self { running_on: workspaces.into_iter().map(|w| w.image).collect(), needs }
    }
}

/// An image the tidy-up removes, and what only it takes (what it adds to
/// the image it's built on).
#[derive(Debug, Clone, PartialEq)]
pub struct Unused {
    pub image: ImageInfo,
    pub part: Part,
    pub bytes: u64,
}

/// The tidy-up's plan: what it keeps (image ids), and what it removes,
/// each before what it's built on.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub keep: HashSet<String>,
    pub unused: Vec<Unused>,
}

/// The image each one here is built on, by id: podman's record, else what
/// the image built on it says (`io.wadspaces.from`): podman records none
/// for a build's first step on the base.
pub fn parents(all: &[ImageInfo]) -> HashMap<String, String> {
    let ids: HashSet<&str> = all.iter().map(|i| short(&i.id)).collect();
    let mut out: HashMap<String, String> = all
        .iter()
        .filter(|i| !i.parent.is_empty() && ids.contains(short(&i.parent)))
        .map(|i| (short(&i.id).to_string(), short(&i.parent).to_string()))
        .collect();
    for c in all {
        let (step, from) = (short(&c.parent), short(label(c, FROM_LABEL)));
        if !c.parent.is_empty() && !from.is_empty() && from != step && ids.contains(from) && !out.contains_key(step) {
            out.insert(step.to_string(), from.to_string());
        }
    }
    out
}

/// What an image adds to the image it's built on (`parents`): its own
/// layers. Summed over images, each layer counts once.
pub fn own_bytes(all: &[ImageInfo], parents: &HashMap<String, String>, i: &ImageInfo) -> u64 {
    let under = parents.get(short(&i.id)).and_then(|p| all.iter().find(|x| short(&x.id) == p)).map_or(0, |p| p.size);
    i.size.saturating_sub(under)
}

/// The plan for the images here (podman's list, steps too).
pub fn plan(all: &[ImageInfo], wanted: &Wanted) -> Plan {
    let by_id: HashMap<&str, &ImageInfo> = all.iter().map(|i| (short(&i.id), i)).collect();
    let parents = parents(all);
    let parent = |i: &ImageInfo| parents.get(short(&i.id)).and_then(|p| by_id.get(p.as_str()).copied());
    let mut keep: HashSet<String> = HashSet::new();
    let walk = |from: &ImageInfo, keep: &mut HashSet<String>| {
        let mut at = Some(from);
        while let Some(i) = at {
            if !keep.insert(short(&i.id).to_string()) {
                break;
            }
            at = parent(i);
        }
    };
    for i in all {
        let runs = |r: &String| i.tags.contains(r) || short(r) == short(&i.id);
        if wanted.running_on.iter().any(runs) {
            walk(i, &mut keep);
        }
    }
    // Each package needed: its newest copy here.
    for id in &wanted.needs {
        let newest = all
            .iter()
            .filter(|i| !i.tags.is_empty() && part_of(i).is_some_and(|p| p == Part::Package(id.clone())))
            .max_by(|a, b| a.created.total_cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
        if let Some(i) = newest {
            walk(i, &mut keep);
        }
    }
    // Ours that nothing keeps, and the untagged steps under them.
    let mut gone: HashMap<String, Part> = HashMap::new();
    for i in all.iter().filter(|i| !keep.contains(short(&i.id))) {
        let Some(p) = part_of(i) else { continue };
        gone.insert(short(&i.id).to_string(), p);
        let mut at = parent(i);
        while let Some(a) = at {
            let id = short(&a.id);
            if keep.contains(id) || !a.tags.is_empty() || part_of(a).is_some() || gone.contains_key(id) {
                break;
            }
            gone.insert(id.to_string(), Part::Step);
            at = parent(a);
        }
    }
    // Each one once nothing left is built on it (podman refuses one that has).
    let mut left: HashSet<&str> = by_id.keys().copied().collect();
    let mut unused = vec![];
    loop {
        let mut next: Vec<&str> = gone
            .keys()
            .map(String::as_str)
            .filter(|id| left.contains(id))
            .filter(|id| !parents.iter().any(|(c, p)| p == id && left.contains(c.as_str())))
            .collect();
        if next.is_empty() {
            break;
        }
        next.sort();
        for id in next {
            left.remove(id);
            let image = by_id[id].clone();
            unused.push(Unused { bytes: own_bytes(all, &parents, &image), part: gone[id].clone(), image });
        }
    }
    Plan { keep, unused }
}

/// Whether a build or a package download is going on (the tidy-up waits).
pub fn busy(builds: &Builds) -> bool {
    builds.list().iter().any(|b| !b.status.finished()) || builds.packages().is_some_and(|p| p.fetching())
}

/// The tidy-up: what nothing here needs, removed. One something still
/// uses (a container made from it) stays, and so does what it's built on.
pub async fn run(backend: &dyn Backend, wanted: &Wanted) -> Freed {
    let mut freed = Freed::default();
    let all = match backend.images_all().await {
        Ok(a) => a,
        Err(e) => {
            tracing::info!("the tidy-up: {e}");
            return freed;
        }
    };
    for u in plan(&all, wanted).unused {
        let name = u.image.tags.first().cloned().unwrap_or_else(|| short(&u.image.id).chars().take(12).collect());
        match backend.remove_image_id(&u.image.id).await {
            Ok(true) => {
                freed.bytes += u.bytes;
                // Steps go with what they're under: not counted on their own.
                if u.part != Part::Step {
                    freed.removed.push(name);
                }
            }
            Ok(false) => {}
            Err(e) => tracing::info!("{name} stays for now: {e}"),
        }
    }
    if !freed.removed.is_empty() {
        tracing::info!("tidied up {} image(s), {} MB", freed.removed.len(), freed.bytes / 1_000_000);
    }
    freed
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn img(id: &str, parent: &str, size: u64, tag: &str, labels: Value) -> ImageInfo {
        ImageInfo {
            id: format!("sha256:{id}"),
            created: size as f64,
            size,
            labels: labels.as_object().cloned().unwrap_or_default(),
            tags: if tag.is_empty() { vec![] } else { vec![tag.into()] },
            parent: if parent.is_empty() { String::new() } else { format!("sha256:{parent}") },
        }
    }

    fn pkg(id: &str) -> Value {
        json!({ "io.wadspaces.pkg": id })
    }

    fn chain() -> Value {
        json!({ "io.wadspaces.chain": "x" })
    }

    fn layer(l: &str) -> Value {
        json!({ "io.wadspaces.layer": l, "io.wadspaces.chain": "x" })
    }

    /// The base; two package versions of VS Code, Tiled (built here, on a
    /// step), Obsidian; a set a workspace runs on (A, on chains C1 and C2)
    /// and an older build of it; a set nothing runs on (O, on C3 over C1);
    /// the old Builder's template and Extensions; someone's image on C3.
    fn store() -> Vec<ImageInfo> {
        vec![
            img("base", "", 1000, "localhost/wadspaces-base:trixie", json!({})),
            img("code1", "", 300, "localhost/wadspaces-pkg-vscode:code1", pkg("vscode")),
            img("code2", "", 310, "localhost/wadspaces-pkg-vscode:code2", pkg("vscode")),
            img("tstep", "", 100, "", json!({})),
            img("tiled", "tstep", 100, "localhost/wadspaces-pkg-tiled:tiled", pkg("tiled")),
            img("obs", "", 200, "localhost/wadspaces-pkg-obsidian:obs", pkg("obsidian")),
            img("s1", "base", 1310, "", json!({})),
            img("c1", "s1", 1310, "localhost/wadspaces-chain:c1", chain()),
            img("c2", "c1", 1410, "localhost/wadspaces-chain:c2", chain()),
            img("sa", "c2", 1412, "", chain()),
            img("a", "sa", 1412, "localhost/wadspaces-set-aaaaaaaaaaaa:latest", layer("set:aaaaaaaaaaaa")),
            img("aold", "c2", 1411, "", layer("set:aaaaaaaaaaaa")),
            img("c3", "c1", 1510, "localhost/wadspaces-chain:c3", chain()),
            img("so", "c3", 1515, "", chain()),
            img("o", "so", 1515, "localhost/wadspaces-set-bbbbbbbbbbbb:latest", layer("set:bbbbbbbbbbbb")),
            img(
                "t",
                "base",
                1600,
                "localhost/wadspaces-template-dev:latest",
                json!({ "io.wadspaces.layer": "template:dev" }),
            ),
            img("x", "t", 1700, "localhost/wadspaces-custom-x:latest", json!({ "io.wadspaces.layer": "custom:x" })),
            img("busybox", "", 5, "docker.io/library/busybox:latest", json!({})),
            img("mine", "c3", 1520, "localhost/mine:latest", json!({})),
        ]
    }

    fn wanted() -> Wanted {
        Wanted {
            running_on: vec!["localhost/wadspaces-set-aaaaaaaaaaaa:latest".into(), "localhost/gone:1".into()],
            needs: vec!["tiled".into(), "vscode".into(), "rust".into()],
        }
    }

    #[test]
    fn what_runs_and_the_newest_packages_needed_stay_and_the_rest_goes_children_first() {
        let all = store();
        let p = plan(&all, &wanted());
        for k in ["a", "sa", "c2", "c1", "s1", "base", "code2", "tiled", "tstep"] {
            assert!(p.keep.contains(k), "{k} is kept");
        }
        let gone: Vec<(&str, String)> =
            p.unused.iter().map(|u| (u.image.id.trim_start_matches("sha256:"), u.part.label())).collect();
        let ids: Vec<&str> = gone.iter().map(|(i, _)| *i).collect();
        // C3 stays: someone's image is built on it (so its set's steps go, not it).
        for k in ["aold", "code1", "obs", "o", "so", "x", "t"] {
            assert!(ids.contains(&k), "{k} goes: {gone:?}");
        }
        assert_eq!(ids.len(), 7, "{gone:?}");
        let at = |k: &str| ids.iter().position(|i| *i == k).unwrap();
        assert!(at("x") < at("t") && at("o") < at("so"), "children first: {ids:?}");
        let part = |k: &str| gone.iter().find(|(i, _)| *i == k).unwrap().1.clone();
        assert_eq!(part("o"), "set:bbbbbbbbbbbb");
        assert_eq!(part("so"), "chain");
        assert_eq!(part("code1"), "package:vscode");
        assert_eq!(part("t"), "template:dev");
        // What only each takes: what it adds to the image under it.
        let bytes = |k: &str| p.unused.iter().find(|u| u.image.id.ends_with(k)).unwrap().bytes;
        assert_eq!((bytes("o"), bytes("so"), bytes("code1"), bytes("x"), bytes("t")), (0, 5, 300, 100, 600));
    }

    #[test]
    fn a_first_step_podman_kept_no_parent_for_is_on_what_the_image_above_says() {
        // podman's first step on the base, with no parent; the chain over it
        // says it was built on the base.
        let all = vec![
            img("base", "", 1000, "localhost/wadspaces-base:trixie", json!({})),
            img("s1", "", 1400, "", chain()),
            img(
                "c1",
                "s1",
                1400,
                "localhost/wadspaces-chain:c1",
                json!({ "io.wadspaces.chain": "x", "io.wadspaces.from": "sha256:base" }),
            ),
        ];
        let p = parents(&all);
        assert_eq!(p.get("s1").map(String::as_str), Some("base"));
        let total: u64 = all.iter().map(|i| own_bytes(&all, &p, i)).sum();
        assert_eq!(total, 1400, "each layer once");
    }

    #[test]
    fn untagged_steps_under_what_goes_go_too() {
        let mut all = store();
        // Tiled isn't needed: it goes, and the step it was built on.
        let p = plan(&all, &Wanted { needs: vec!["vscode".into()], ..wanted() });
        let part = |p: &Plan, k: &str| p.unused.iter().find(|u| u.image.id.ends_with(k)).map(|u| u.part.clone());
        assert_eq!(part(&p, "tiled"), Some(Part::Package("tiled".into())));
        assert_eq!(part(&p, "tstep"), Some(Part::Step));
        // Nothing runs: every set and chain goes, the base and others' images stay.
        all.retain(|i| !i.id.ends_with("mine"));
        let p = plan(&all, &Wanted::default());
        let ids: Vec<&str> = p.unused.iter().map(|u| u.image.id.trim_start_matches("sha256:")).collect();
        for k in ["a", "sa", "c2", "c1", "s1", "c3", "code2", "tiled"] {
            assert!(ids.contains(&k), "{k} goes: {ids:?}");
        }
        assert!(!ids.contains(&"base") && !ids.contains(&"busybox"));
    }
}
