//! What WadSpaces takes on this machine's disk (GET /v1/storage): the base
//! image, each layer's image and its own share of it (what it adds to the
//! image under it: layers are shared, so that's what one costs), each
//! wadspace's home (its /config volume), and the files copied in for
//! wadspaces; with the disk's size and what's free. And everything else on
//! the disk, part by part: the system, the copy a stick update left, every
//! other image (older bases, images from before layers, build steps), what
//! older images share, running containers' changes, volumes no wadspace has,
//! logs, wadd's own files, people's files, and what's left over. Counting
//! files takes a while, so wadd keeps the last report and hands it out at
//! once: it measures again in the background when that's over a minute old
//! (or after something was deleted), one measure at a time, and says so
//! with a `storage` event when the new one is ready.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;

use serde_json::Value;
use wad_proto::v1::{
    CopyBytes, DiagDisk, Event, HomeBytes, ImageBytes, LayerBytes, OtherItem, OtherPart, Storage as Report,
};

use crate::backend::{Backend, ImageInfo};
use crate::events::Bus;
use crate::registry::Registry;

const FRESH: Duration = Duration::from_secs(60);
/// The first measure, once wadd has settled after starting.
const FIRST: Duration = Duration::from_secs(20);

/// Cheap to clone: one report and one measure shared by all.
#[derive(Clone)]
pub struct Storage(Arc<Inner>);

struct Inner {
    state: PathBuf,
    copies: PathBuf,
    /// The machine's own wadd (not one on a laptop): the system, the logs
    /// and people's files are its to count.
    machine: bool,
    backend: Arc<dyn Backend>,
    registry: Arc<Registry>,
    bus: Bus,
    last: Mutex<Option<(Instant, Report)>>,
    /// Something changed on the disk since the last report.
    stale: AtomicBool,
    /// Held while measuring: one at a time.
    measuring: tokio::sync::Mutex<()>,
    /// The system's size (it only changes with an update), by the versions
    /// deployed when it was counted.
    system: Mutex<Option<(Vec<String>, u64)>>,
}

/// The copy a stick update leaves (host/build.sh, import-updates).
pub const UPDATE_COPY: &str = "incoming";

impl Storage {
    /// `machine`: the machine's own wadd, which counts the whole disk.
    pub fn new(state_dir: &Path, machine: bool, backend: Arc<dyn Backend>, registry: Arc<Registry>, bus: Bus) -> Self {
        Self(Arc::new(Inner {
            state: state_dir.into(),
            copies: state_dir.join("copies"),
            machine,
            backend,
            registry,
            bus,
            last: Mutex::default(),
            stale: AtomicBool::new(false),
            measuring: tokio::sync::Mutex::default(),
            system: Mutex::default(),
        }))
    }

    /// The first report, measured a little after wadd starts (so the Storage
    /// tab opens on one).
    pub fn start(&self) {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(FIRST).await;
            me.refresh();
        });
    }

    /// The last report, at once (`measuring` when a newer one is on its way);
    /// measured now when there's none yet, or `fresh`.
    pub async fn report(&self, fresh: bool) -> Report {
        if !fresh && let Some((at, r)) = self.0.last.lock().unwrap().clone() {
            let old = at.elapsed() >= FRESH || self.0.stale.load(Ordering::Relaxed);
            if old {
                self.refresh();
            }
            return Report { measuring: old || self.0.measuring.try_lock().is_err(), ..r };
        }
        self.measure_now().await
    }

    /// What changed on the disk (images removed): measured again, in the background.
    pub fn forget(&self) {
        self.0.stale.store(true, Ordering::Relaxed);
        self.refresh();
    }

    /// Measures in the background, unless that's happening already.
    fn refresh(&self) {
        if self.0.measuring.try_lock().is_err() {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            me.measure_now().await;
        });
    }

    /// A new report: this one, or the one a measure under way gives.
    async fn measure_now(&self) -> Report {
        let asked = Instant::now();
        let _one = self.0.measuring.lock().await;
        if let Some((at, r)) = self.0.last.lock().unwrap().clone()
            && at > asked
        {
            return r;
        }
        self.0.stale.store(false, Ordering::Relaxed);
        let r = self.measure().await;
        *self.0.last.lock().unwrap() = Some((Instant::now(), r.clone()));
        self.0.bus.publish(Event::Storage { measured_at: r.measured_at });
        r
    }

    async fn measure(&self) -> Report {
        let backend = &self.0.backend;
        // Every image's size from one list; podman is asked about the rest.
        let all = backend.images_all().await.unwrap_or_default();
        let sizes = Sizes::of(&all);
        let size_of = async |r: &str| match sizes.get(r) {
            Some(i) => Some(i.clone()),
            None => backend.image_info(r).await.ok().flatten().map(|i| (i.id, i.size)),
        };
        let base_ref = wad_core::spec::DEFAULT_BASE_IMAGE;
        let base = size_of(base_ref).await.map(|(id, bytes)| ImageBytes { image_ref: base_ref.into(), id, bytes });
        let mut layers = Vec::new();
        for row in
            crate::images::list(backend.as_ref()).await.unwrap_or_default().into_iter().filter(|r| r.layer != "base")
        {
            let Some((id, bytes)) = size_of(&row.image_ref).await else { continue };
            // What it adds to the image it's built on (that one as it was then).
            let under = size_of(&row.from).await.map(|(_, b)| b).unwrap_or(0);
            layers.push(LayerBytes {
                layer: row.layer,
                id,
                from: row.from,
                bytes,
                own_bytes: bytes.saturating_sub(under),
            });
        }
        let mut leftovers = Vec::new();
        for (layer, i) in crate::images::leftovers(backend.as_ref()).await.unwrap_or_default() {
            let from = i.labels.get(crate::builds::FROM_LABEL).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let under = size_of(&from).await.map(|(_, b)| b).unwrap_or(0);
            leftovers.push(LayerBytes {
                layer,
                id: i.id,
                from,
                bytes: i.size,
                own_bytes: i.size.saturating_sub(under),
            });
        }
        let df = backend.system_df().await.ok();
        let vols: Vec<(String, u64)> =
            df.as_ref().map(|d| d.volumes.iter().map(|v| (v.name.clone(), v.size)).collect()).unwrap_or_default();
        let homes: Vec<HomeBytes> = self
            .0
            .registry
            .workspaces()
            .iter()
            .filter_map(|w| {
                let volume = format!("wad-{}-config", w.id);
                let (_, bytes) = vols.iter().find(|(name, _)| *name == volume)?;
                Some(HomeBytes { workspace_id: w.id.clone(), volume, bytes: *bytes })
            })
            .collect();
        let disk = disk_of(backend).await;
        let folders = self.folders().await;
        let mut r = Report {
            disk,
            base,
            layers,
            leftovers,
            homes,
            copies: folders.copies.clone(),
            other: vec![],
            measured_at: now(),
            measuring: false,
        };
        r.other = self.other(&r, df.as_ref(), &all, &folders);
        r
    }

    /// The folders, counted on a thread of their own at idle priority (some
    /// hold many files; the machine's work comes first).
    async fn folders(&self) -> Folders {
        let (state, copies, machine) = (self.0.state.clone(), self.0.copies.clone(), self.0.machine);
        // The system only changes with an update: counted again when the
        // versions deployed change.
        let deployed = if machine { deployments() } else { vec![] };
        let known = self.0.system.lock().unwrap().clone().filter(|(d, _)| *d == deployed).map(|(_, b)| b);
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            idle_priority();
            let copies: Vec<CopyBytes> = std::fs::read_dir(&copies)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| Some(CopyBytes { id: e.file_name().into_string().ok()?, bytes: dir_bytes(&e.path()) }))
                .collect();
            let update = disk_bytes(&state.join(UPDATE_COPY), &mut HashSet::new());
            let mut seen = HashSet::new();
            let wadd = disk_bytes(&state, &mut seen)
                .saturating_sub(update)
                .saturating_sub(disk_bytes(&state.join("copies"), &mut HashSet::new()));
            let at = |p: &str| if machine { disk_bytes(Path::new(p), &mut HashSet::new()) } else { 0 };
            let system = known.unwrap_or_else(|| at("/sysroot/ostree"));
            let _ = tx.send(Folders {
                copies,
                system,
                update,
                wadd,
                logs: at("/var/log"),
                people: at("/var/home"),
                versions: 0,
            });
        });
        let mut f = rx.await.unwrap_or_default();
        f.versions = deployed.len();
        if machine {
            *self.0.system.lock().unwrap() = Some((deployed, f.system));
        }
        f
    }

    /// Everything else, part by part.
    fn other(&self, r: &Report, df: Option<&crate::backend::Df>, all: &[ImageInfo], f: &Folders) -> Vec<OtherPart> {
        let mut parts = vec![];
        let part = |key: &str, label: &str, detail: &str, bytes: u64| OtherPart {
            key: key.into(),
            label: label.into(),
            detail: detail.into(),
            bytes,
            items: vec![],
            deletable: false,
        };
        let short = |id: &str| id.trim_start_matches("sha256:").to_string();
        let (system, update, wadd, logs, people) = (f.system, f.update, f.wadd, f.logs, f.people);
        if system > 0 {
            let versions = f.versions;
            parts.push(part("system", "The WadSpaces system", &format!("The system itself: {versions} version{} kept, this one and the one before it (to go back to). It stays.", if versions == 1 { "" } else { "s" }), system));
        }
        if update > 0 {
            let waiting = self.0.state.join(UPDATE_COPY).join("pending").exists();
            parts.push(OtherPart {
                deletable: !waiting,
                ..part(
                    "update-copy",
                    "The last stick update's copy",
                    if waiting {
                        "Copied in by a stick update that's waiting to be applied at the next start."
                    } else {
                        "What the last stick update copied in, already applied: nothing needs it."
                    },
                    update,
                )
            });
        }
        // Images: what the layers above don't count, image by image.
        if let Some(df) = df {
            let mut counted: HashSet<String> = r.layers.iter().chain(&r.leftovers).map(|l| short(&l.id)).collect();
            if let Some(b) = &r.base {
                counted.insert(short(&b.id));
            }
            // What images here are built on: none of those can go.
            let under: HashSet<String> = all
                .iter()
                .filter_map(|a| a.labels.get(crate::builds::FROM_LABEL).and_then(Value::as_str))
                .map(short)
                .collect();
            // podman's steps under an image (`podman images` hides them too):
            // they share its layers, free nothing and go with it.
            let steps: HashSet<String> = all.iter().map(|a| short(&a.parent)).filter(|p| !p.is_empty()).collect();
            let mut items = vec![];
            for i in df
                .images
                .iter()
                .filter(|i| !counted.contains(&short(&i.id)) && !(i.name.is_empty() && steps.contains(&short(&i.id))))
            {
                let info = all.iter().find(|a| short(&a.id) == short(&i.id));
                let (label, detail, deletable) = classify(i, info, under.contains(&short(&i.id)));
                items.push(OtherItem {
                    id: short(&i.id),
                    label,
                    detail,
                    bytes: i.unique,
                    deletable: deletable && i.containers == 0,
                });
            }
            items.sort_by_key(|i| std::cmp::Reverse(i.bytes));
            let others: u64 = items.iter().map(|i| i.bytes).sum();
            if !items.is_empty() {
                parts.push(OtherPart { items, ..part("images", "Other images", "Images the wadspaces' layers don't count: older bases, images from before layers, steps builds left.", others) });
            }
            let ours = r.base.as_ref().map(|b| b.bytes).unwrap_or(0)
                + r.layers.iter().chain(&r.leftovers).map(|l| l.own_bytes).sum::<u64>();
            let shared = df.images_size.saturating_sub(ours + others);
            if shared > 0 {
                parts.push(part("shared", "Shared by older images", "Image layers older images still hold in common, such as an older base that templates built before the last base update still stand on. Update those templates and it goes.", shared));
            }
            let changes: u64 = df.containers.iter().map(|(_, b)| b).sum();
            if changes > 0 {
                parts.push(part(
                    "containers",
                    "Running wadspaces' changes",
                    "What running wadspaces changed outside their homes since they started: gone when they stop.",
                    changes,
                ));
            }
            let homes: HashSet<&str> = r.homes.iter().map(|h| h.volume.as_str()).collect();
            let mut vols: Vec<OtherItem> = df
                .volumes
                .iter()
                .filter(|v| !homes.contains(v.name.as_str()))
                .map(|v| {
                    let of = v.name.strip_prefix("wad-").and_then(|n| n.strip_suffix("-config"));
                    let display = v.name.strip_prefix("wad-").and_then(|n| n.strip_suffix("-display"));
                    OtherItem {
                        id: v.name.clone(),
                        label: match (of, display) {
                            (Some(id), _) => format!("{id}'s home"),
                            (_, Some(id)) => format!("{id}'s stream display"),
                            _ => v.name.clone(),
                        },
                        detail: if of.is_some() {
                            "A home whose wadspace isn't on this machine any more.".into()
                        } else if display.is_some() {
                            "Where a streamed wadspace and its sidecar meet (made again when it's next streamed)."
                                .into()
                        } else {
                            "A volume no wadspace here has.".into()
                        },
                        bytes: v.size,
                        deletable: v.links == 0,
                    }
                })
                .collect();
            vols.sort_by_key(|i| std::cmp::Reverse(i.bytes));
            if !vols.is_empty() {
                let bytes = vols.iter().map(|v| v.bytes).sum();
                parts.push(OtherPart { items: vols, ..part("volumes", "Homes left behind", "Files of wadspaces no longer on this machine (built again, a wadspace with the same id gets its home back).", bytes) });
            }
        }
        if logs > 0 {
            parts.push(part("logs", "Logs", "The system's logs (/var/log), kept and trimmed by the system.", logs));
        }
        parts.push(part(
            "wadd",
            "WadSpaces' own files",
            "wadd's records: build and launch logs, web apps' icons, saved sign-ins.",
            wadd,
        ));
        if people > 0 {
            parts.push(part(
                "people",
                "Your files",
                "Your files on the machine (/var/home): WadBrowser's downloads and profiles, the WadSpaces app's data.",
                people,
            ));
        }
        let used = r.disk.total_bytes.saturating_sub(r.disk.free_bytes);
        let listed = df.map(|d| d.images_size).unwrap_or(0)
            + r.homes.iter().map(|h| h.bytes).sum::<u64>()
            + r.copies.iter().map(|c| c.bytes).sum::<u64>()
            + parts.iter().filter(|p| !matches!(p.key.as_str(), "images" | "shared")).map(|p| p.bytes).sum::<u64>();
        let rest = used.saturating_sub(listed);
        if self.0.machine && rest > 0 {
            parts.push(part(
                "unaccounted",
                "Not listed above",
                "The disk's own bookkeeping, and files outside the folders above.",
                rest,
            ));
        }
        parts
    }
}

/// What an image the layers don't count is, and whether it can go.
fn classify(
    i: &crate::backend::DfImage,
    info: Option<&crate::backend::ImageInfo>,
    built_on: bool,
) -> (String, String, bool) {
    let label = |k: &str| info.and_then(|x| x.labels.get(k)).and_then(Value::as_str).unwrap_or_default().to_string();
    let layer = label(crate::builds::LAYER_LABEL);
    if !layer.is_empty() {
        return (
            format!("An older build of {layer}"),
            "Something here is still built on it: it goes when that's updated or deleted.".into(),
            false,
        );
    }
    // Version 4 wadspaces' parts (sets.rs). They inherit the base's labels.
    let pkg = label(crate::packages::PKG_LABEL);
    if !pkg.is_empty() {
        return (
            format!("The {pkg} package"),
            "What version 4 wadspaces' images are put together from: it stays.".into(),
            false,
        );
    }
    let chain = label(crate::sets::CHAIN_LABEL);
    if !chain.is_empty() {
        return (
            format!("Packages stacked: {}", wad_core::packages::pkgs_of_label(&chain).join(", ")),
            "A step version 4 wadspaces' images are built from: the next build with these packages starts here.".into(),
            false,
        );
    }
    if label("build_version").contains("WadSpaces base") || i.name.starts_with("localhost/wadspaces-base") {
        return if built_on {
            (
                "An older base".into(),
                "Templates built before the last base update still stand on it: update them, and it can go.".into(),
                false,
            )
        } else {
            ("An older base".into(), "Nothing stands on it any more.".into(), true)
        };
    }
    // The stream sidecar (images/stream) comes on the drive with the base.
    if label("build_version").contains("WadSpaces stream") || i.name.starts_with("localhost/wadspaces-stream") {
        return if i.name.is_empty() {
            ("An older stream sidecar".into(), "Replaced by a newer one: nothing streams on it.".into(), true)
        } else {
            ("The stream sidecar".into(), "Streamed wadspaces draw on it (it comes with the base).".into(), false)
        };
    }
    if let Some(name) = i.name.strip_prefix("localhost/wadspaces-") {
        return (
            name.split(':').next().unwrap_or(name).to_string(),
            "From before the Builder's layers: nothing uses it.".into(),
            true,
        );
    }
    if !i.name.is_empty() {
        return (i.name.clone(), "An image podman fetched, to build or run something.".into(), true);
    }
    ("A build step".into(), "Left by a build; nothing names it.".into(), true)
}

/// What the folder walk found.
#[derive(Default)]
struct Folders {
    copies: Vec<CopyBytes>,
    system: u64,
    update: u64,
    wadd: u64,
    logs: u64,
    people: u64,
    /// The system's versions deployed.
    versions: usize,
}

/// Images' ids and sizes, by tag and by id.
struct Sizes(HashMap<String, (String, u64)>);

impl Sizes {
    fn of(all: &[ImageInfo]) -> Self {
        let mut m = HashMap::new();
        for i in all {
            let v = (i.id.clone(), i.size);
            m.insert(i.id.trim_start_matches("sha256:").to_string(), v.clone());
            for t in &i.tags {
                m.insert(t.clone(), v.clone());
            }
        }
        Self(m)
    }

    fn get(&self, r: &str) -> Option<&(String, u64)> {
        if r.is_empty() {
            return None;
        }
        self.0.get(r).or_else(|| self.0.get(r.trim_start_matches("sha256:")))
    }
}

/// The system's versions deployed (ostree), by name.
fn deployments() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir("/sysroot/ostree/deploy")
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|d| {
            std::fs::read_dir(d.path().join("deploy"))
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
        })
        .collect();
    out.sort();
    out
}

/// This thread reads the disk only when nothing else does, and yields the CPU.
fn idle_priority() {
    const IOPRIO_WHO_PROCESS: libc::c_int = 1;
    const IOPRIO_CLASS_IDLE: libc::c_int = 3;
    // SAFETY: plain syscalls on this thread (tid 0 / who 0: the caller).
    unsafe {
        libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, 0, IOPRIO_CLASS_IDLE << 13);
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
}

/// The disk podman keeps its images on (else /).
async fn disk_of(backend: &Arc<dyn Backend>) -> DiagDisk {
    let root = backend
        .podman_info()
        .await
        .ok()
        .and_then(|i| i.pointer("/store/graphRoot").and_then(|v| v.as_str()).map(String::from))
        .filter(|p| Path::new(p).is_dir())
        .unwrap_or_else(|| "/".into());
    let (free, total) = match nix::sys::statvfs::statvfs(root.as_str()) {
        Ok(s) => (s.blocks_available() * s.fragment_size(), s.blocks() * s.fragment_size()),
        Err(_) => (0, 0),
    };
    DiagDisk { path: root, free_bytes: free, total_bytes: total }
}

/// What the files under `dir` take on the disk (their blocks, a file
/// hard-linked in several places counted once; links not followed).
fn disk_bytes(dir: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
    let mut total = 0;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(m) = e.path().symlink_metadata() else { continue };
            if m.is_dir() {
                todo.push(e.path());
            } else if m.nlink() <= 1 || seen.insert((m.dev(), m.ino())) {
                total += m.blocks() * 512;
            }
        }
    }
    total
}

/// The bytes of the files under `dir` (links not followed).
pub fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => todo.push(e.path()),
                Ok(t) if t.is_file() => total += e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => {}
            }
        }
    }
    total
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}
