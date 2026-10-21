//! Building on this machine: a wadspace's image is a set (sets.rs), its
//! packages stacked on the base, each build a job here. Each image is
//! labelled with its key and the id of the image it was built on, which is
//! how WadSpaces tells a built set from an old one. None of it adds a
//! workspace: a wadspace installs on its set (sets.rs, with `merged`).
//!
//! A few builds run at once; bases are never downloaded (they reach a
//! machine on the update drive), and a build needs some free disk. Progress
//! goes out as `build` events: the job, plus the log lines since the last
//! event.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use wad_proto::v1::{Build, BuildLog, BuildStatus, Event, Workspace};
use wad_proto::{ApiError, ErrorCode};

use crate::backend::Backend;
use crate::events::Bus;
use crate::icons::Icons;
use crate::joblog::Lines;

pub const MAX_CONTEXT_BYTES: usize = 64 * 1024 * 1024;
const MAX_LINES: usize = 4000;
const PUBLISH_EVERY: Duration = Duration::from_millis(250);
const KEEP_FINISHED: usize = 20;

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, msg)
}

/// "STEP 3/7: RUN ..." -> (3, 7)
fn step(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("STEP ")?;
    let (frac, _) = rest.split_once(':')?;
    let (a, b) = frac.split_once('/')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// The workspace list with `fresh` added, or merged into the one already
/// there (which keeps its icon and its extra volumes; its projects and
/// sign-ins are the design's); checked as a whole. Also: whether it replaced
/// one.
pub fn merged(current: &[Workspace], fresh: &Value) -> Result<(Vec<Workspace>, bool), String> {
    let id = fresh.get("id").and_then(Value::as_str).unwrap_or_default();
    if let Some(app) = current.iter().find(|w| w.id == id && w.kind == wad_proto::v1::Kind::App) {
        return Err(format!("{id:?} is {}, one of this machine's own apps: give the wadspace another id", app.name));
    }
    let mut list: Vec<Value> = current.iter().map(wad_store::workspaces::to_yaml).collect();
    let updated = match list.iter_mut().find(|w| w["id"] == id) {
        Some(old) => {
            let mut new = old.clone();
            for (k, v) in fresh.as_object().into_iter().flatten() {
                new[k] = v.clone();
            }
            let mut volumes: Vec<Value> = old["volumes"].as_array().cloned().unwrap_or_default();
            for v in fresh["volumes"].as_array().into_iter().flatten() {
                if !volumes.contains(v) {
                    volumes.push(v.clone());
                }
            }
            new["volumes"] = volumes.into();
            new["icon"] = old.get("icon").cloned().unwrap_or(Value::Null);
            // Its projects are the design's too.
            new["projects"] = fresh.get("projects").cloned().unwrap_or(serde_json::json!([]));
            // Its sign-ins are the design's: none left means none.
            if fresh.get("signins").is_none() {
                new.as_object_mut().expect("an object").remove("signins");
            }
            if new["icon"].is_null() {
                new.as_object_mut().expect("an object").remove("icon");
            }
            *old = new;
            true
        }
        None => {
            list.push(fresh.clone());
            false
        }
    };
    wad_store::workspaces::check_workspaces(list).map(|l| (l, updated)).map_err(|e| e.to_string())
}

/// The label a layer's image carries: its build folder's sha.
pub const SHA_LABEL: &str = "io.wadspaces.dockerfile-sha256";
/// The id of the image it was built FROM.
pub const FROM_LABEL: &str = "io.wadspaces.from";
/// "set:<key>" (what the image is).
pub const LAYER_LABEL: &str = "io.wadspaces.layer";

/// What a job builds: a set.
enum Work {
    Set(crate::sets::SetWork),
}

struct Job {
    build: Build,
    log: Lines,
    /// What to build; taken when it starts.
    work: Option<Work>,
    /// Which of how many images is building now (for the progress bar).
    group: u32,
    groups: u32,
}

pub struct Builds {
    me: Weak<Builds>,
    backend: Arc<dyn Backend>,
    bus: Bus,
    icons: Arc<Icons>,
    log_dir: PathBuf,
    min_free_gb: u64,
    jobs: Mutex<Vec<Job>>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    sent: Mutex<HashMap<String, u64>>,
    last_publish: Mutex<Option<Instant>>,
    /// Builds at once (the rest wait their turn, Queued).
    slots: tokio::sync::Semaphore,
    /// One build at a time on each image: two builds that both need a
    /// template take turns on it (the second finds it built, and skips it).
    images_busy: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Where sets get their packages.
    packages: Mutex<Option<Arc<crate::packages::Packages>>>,
    /// What runs here: the tidy-up after builds keeps what it needs.
    registry: Mutex<Option<Arc<crate::registry::Registry>>>,
    state_dir: PathBuf,
}

/// How many builds run at once: their downloads and builds side by side.
pub const AT_ONCE: usize = 3;

impl Builds {
    pub fn new(backend: Arc<dyn Backend>, bus: Bus, state_dir: &Path, min_free_gb: u64) -> Arc<Self> {
        Self::with_icons(backend, bus, state_dir, min_free_gb, Icons::new(&state_dir.join("webapp-icons")))
    }

    pub fn with_icons(
        backend: Arc<dyn Backend>,
        bus: Bus,
        state_dir: &Path,
        min_free_gb: u64,
        icons: Arc<Icons>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            backend,
            bus,
            icons,
            log_dir: state_dir.join("builds"),
            state_dir: state_dir.into(),
            min_free_gb,
            jobs: Mutex::default(),
            tasks: Mutex::default(),
            sent: Mutex::default(),
            last_publish: Mutex::default(),
            slots: tokio::sync::Semaphore::new(AT_ONCE),
            images_busy: Mutex::default(),
            packages: Mutex::default(),
            registry: Mutex::default(),
        })
    }

    /// What runs here, for the tidy-up after builds (none: it only removes
    /// older bases).
    pub fn set_registry(&self, r: Arc<crate::registry::Registry>) {
        *self.registry.lock().unwrap() = Some(r);
    }

    /// What the tidy-up keeps, unless a build or a download is going on.
    pub fn wanted(&self) -> Option<crate::tidy::Wanted> {
        if crate::tidy::busy(self) {
            return None;
        }
        let r = self.registry.lock().unwrap().clone()?;
        Some(crate::tidy::Wanted::here(&r, &self.state_dir))
    }

    /// Where set builds get their packages.
    pub fn set_packages(&self, p: Arc<crate::packages::Packages>) {
        *self.packages.lock().unwrap() = Some(p);
    }

    pub(crate) fn packages(&self) -> Option<Arc<crate::packages::Packages>> {
        self.packages.lock().unwrap().clone()
    }

    pub(crate) fn backend(&self) -> &Arc<dyn Backend> {
        &self.backend
    }

    /// Itself, for what outlives a call (a build's log lines).
    pub(crate) fn me(&self) -> Option<Arc<Self>> {
        self.me.upgrade()
    }

    /// Web apps' icons (and their cache), for builds and for WadSpaces Client.
    pub fn icons(&self) -> &Arc<Icons> {
        &self.icons
    }

    /// Builds a version 4 wadspace's set (sets.rs): `packages` (wad-core's
    /// v4::packages_of) on `base`. A build of the same set already going on
    /// is the one it answers.
    pub fn create_set(&self, name: &str, packages: Vec<String>, base: &str) -> Result<Build, ApiError> {
        if packages.is_empty() {
            return Err(bad(format!("{name} has no packages: it runs on the base")));
        }
        let key = format!("set:{}", wad_core::v4::set_key(&packages));
        if let Some(b) = self.list().into_iter().find(|b| b.ws_id == key && !b.status.finished()) {
            return Ok(b);
        }
        let image = wad_core::v4::image_ref(&key, base);
        // Its packages, chain steps and the set: groups move as it learns them.
        let groups = packages.len() as u32 + 2;
        let work = crate::sets::SetWork { key: key.clone(), packages, base: base.to_string() };
        Ok(self.start_job(key, name.to_string(), image, base.to_string(), Work::Set(work), groups))
    }

    fn start_job(&self, key: String, name: String, image: String, base: String, work: Work, groups: u32) -> Build {
        let id = wad_store::projects::new_id()[..12].to_lowercase();
        let build = Build {
            id: id.clone(),
            ws_id: key.clone(),
            name,
            status: BuildStatus::Queued,
            progress: 0.0,
            error: None,
            image,
            base_image: base,
            created: now(),
            started: None,
            finished: None,
            updated: false,
            restart_required: false,
            skipped: vec![],
            line_count: 0,
            layer: Some(key),
        };
        let log = Lines::new(&self.log_dir, &id, MAX_LINES);
        self.jobs.lock().unwrap().push(Job { build: build.clone(), log, work: Some(work), group: 0, groups });
        self.trim();
        self.publish(&id, true);
        let me = self.me.upgrade().expect("builds outlive their calls");
        let task_id = id.clone();
        self.tasks.lock().unwrap().insert(id, tokio::spawn(async move { me.run(task_id).await }));
        build
    }

    /// Whether a layer's image is building (or waiting to) now.
    pub fn building(&self, key: &str) -> bool {
        self.jobs.lock().unwrap().iter().any(|j| j.build.ws_id == key && !j.build.status.finished())
    }

    /// Keeps the last KEEP_FINISHED finished jobs.
    fn trim(&self) {
        let mut jobs = self.jobs.lock().unwrap();
        let finished: Vec<String> =
            jobs.iter().filter(|j| j.build.status.finished()).map(|j| j.build.id.clone()).collect();
        let cut = finished.len().saturating_sub(KEEP_FINISHED);
        for id in &finished[..cut] {
            jobs.retain(|j| &j.build.id != id);
            self.sent.lock().unwrap().remove(id);
        }
    }

    fn build_of(j: &Job) -> Build {
        Build { line_count: j.log.total(), ..j.build.clone() }
    }

    pub fn list(&self) -> Vec<Build> {
        self.jobs.lock().unwrap().iter().rev().map(Self::build_of).collect()
    }

    pub fn log(&self, id: &str, since: u64) -> Result<BuildLog, ApiError> {
        let jobs = self.jobs.lock().unwrap();
        let j = jobs.iter().find(|j| j.build.id == id).ok_or_else(|| no_build(id))?;
        Ok(BuildLog { build: Self::build_of(j), from: since, lines: j.log.since(since) })
    }

    pub fn cancel(&self, id: &str) -> Result<Build, ApiError> {
        let running = !self.log(id, 0)?.build.status.finished();
        if running {
            // Dropping the build closes podman's stream, which stops it.
            if let Some(t) = self.tasks.lock().unwrap().remove(id) {
                t.abort();
            }
            self.with(id, |b| {
                b.status = BuildStatus::Cancelled;
                b.finished = Some(now());
            });
            self.line(id, "✗ cancelled");
            self.publish(id, true);
        }
        self.log(id, 0).map(|l| l.build)
    }

    // ------------------------------------------------------------ output
    fn with<T>(&self, id: &str, f: impl FnOnce(&mut Build) -> T) -> Option<T> {
        self.jobs.lock().unwrap().iter_mut().find(|j| j.build.id == id).map(|j| f(&mut j.build))
    }

    pub(crate) fn line(&self, id: &str, text: &str) {
        if let Some(j) = self.jobs.lock().unwrap().iter_mut().find(|j| j.build.id == id) {
            j.log.push(text);
            if let Some((n, of)) = step(text).filter(|(_, of)| *of > 0) {
                // Each image of the job is its share of the bar.
                let done = f64::from(j.group) + f64::from(n - 1) / f64::from(of);
                j.build.progress = j.build.progress.max(done / f64::from(j.groups.max(1)) * 0.95);
            }
        }
        self.publish(id, false);
    }

    /// Events for this job (and any other with lines not sent yet), at most
    /// every PUBLISH_EVERY unless `force`.
    fn publish(&self, id: &str, force: bool) {
        {
            let mut last = self.last_publish.lock().unwrap();
            let now = Instant::now();
            if !force && last.is_some_and(|t| now - t < PUBLISH_EVERY) {
                return;
            }
            *last = Some(now);
        }
        let out: Vec<BuildLog> = {
            let jobs = self.jobs.lock().unwrap();
            let mut sent = self.sent.lock().unwrap();
            jobs.iter()
                .filter_map(|j| {
                    let from = sent.get(&j.build.id).copied().unwrap_or(0);
                    if j.build.id != id && from == j.log.total() {
                        return None;
                    }
                    sent.insert(j.build.id.clone(), j.log.total());
                    Some(BuildLog { build: Self::build_of(j), from, lines: j.log.since(from) })
                })
                .collect()
        };
        for l in out {
            self.bus.publish(Event::Build(l));
        }
    }

    // ----------------------------------------------------------- running
    async fn run(self: Arc<Self>, id: String) {
        let r = {
            let _turn = self.slots.acquire().await;
            self.go(&id).await
        };
        match r {
            Ok(tag) => {
                self.with(&id, |b| {
                    b.progress = 1.0;
                    b.status = BuildStatus::Done;
                });
                self.line(&id, &format!("✓ built {tag}"));
            }
            Err(e) => {
                tracing::warn!("build {id} failed: {e}");
                self.with(&id, |b| {
                    b.status = BuildStatus::Error;
                    b.error = Some(e.clone());
                });
                self.line(&id, &format!("✗ {e}"));
            }
        }
        self.with(&id, |b| b.finished = Some(now()));
        self.tasks.lock().unwrap().remove(&id);
        self.publish(&id, true);
        // The last build out tidies up: images nothing here needs any more
        // (sets nothing runs on, their steps and packages, older bases).
        if !crate::tidy::busy(&self)
            && let Ok(f) = crate::images::prune(self.backend.as_ref(), self.wanted().as_ref()).await
            && !f.removed.is_empty()
        {
            tracing::info!("after the builds: removed {} older images ({} MB)", f.removed.len(), f.bytes / 1_000_000);
            // Steps a build left free next to nothing: not worth a line.
            if f.bytes < 1_000_000 {
                return;
            }
            self.line(
                &id,
                &format!(
                    "» removed {} older image{} nothing needs, freeing {} MB",
                    f.removed.len(),
                    if f.removed.len() == 1 { "" } else { "s" },
                    f.bytes / 1_000_000
                ),
            );
            self.publish(&id, true);
        }
    }

    /// Which of how many parts of the job it's on (each its share of the
    /// bar), and how far into this one.
    pub(crate) fn at(&self, id: &str, group: u32, groups: u32, part: f64) {
        if let Some(j) = self.jobs.lock().unwrap().iter_mut().find(|j| j.build.id == id) {
            j.group = group;
            j.groups = groups.max(1);
            let done = (f64::from(group) + part.clamp(0.0, 1.0)) / f64::from(j.groups);
            j.build.progress = j.build.progress.max(done * 0.95);
        }
        self.publish(id, false);
    }

    /// The lock for building `tag`.
    pub(crate) fn image_lock(&self, tag: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.images_busy.lock().unwrap().entry(tag.to_string()).or_default().clone()
    }

    /// The build; the last image's tag when it's done.
    async fn go(&self, id: &str) -> Result<String, String> {
        let work = {
            let mut jobs = self.jobs.lock().unwrap();
            let j = jobs.iter_mut().find(|j| j.build.id == id).ok_or("the build is gone")?;
            j.build.status = BuildStatus::Building;
            j.build.started = Some(now());
            j.work.take().ok_or("the build already ran")?
        };
        self.publish(id, true);
        if let Some(free) = self.backend.free_gb().await
            && free < self.min_free_gb as f64
        {
            return Err(format!("only {free:.1} GB free; building needs at least {} GB", self.min_free_gb));
        }
        match work {
            Work::Set(work) => crate::sets::build(self, id, work).await,
        }
    }
}

fn no_build(id: &str) -> ApiError {
    ApiError::new(ErrorCode::NotFound, format!("no build {id:?}"))
}
