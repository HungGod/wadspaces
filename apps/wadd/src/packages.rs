//! Packages on this machine (wad_core::packages): CI's index of what it
//! published, what's here, and getting what a build needs here. For each
//! package, in order: the one here, else downloaded from CI's registry (the
//! packages are private: the machine's GitHub token signs in), else an older
//! one here, else built here from its recipe. Each is tagged by its image id
//! (local_ref).
//!
//! The index is fetched at most once an `index_max_age`, and saved in
//! state_dir/packages/index.json for when the network's out (building a
//! wadspace needs it anyway: there's no offline copy). A package is fetched
//! or built once at a time (two builds that need rust wait for one
//! download), and a download leaves the free space builds keep.
//!
//! `fetch` gets packages now rather than when a build needs them, as a job
//! that sends `packages` events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::task::JoinHandle;
use wad_core::packages as pk;
use wad_proto::v1::{
    BuildStatus, Event, PackageFetch, PackageFetchRequest, PackageGot, PackageHow, PackageList, PackageRow,
    PackageState,
};
use wad_proto::{ApiError, ErrorCode};

use crate::backend::Backend;
use crate::events::Bus;

/// A package's id (CI's and wadspaces-pkg's label).
pub const PKG_LABEL: &str = "io.wadspaces.pkg";
/// The base's ABI: packages built on a base with the same one fit it.
pub const ABI_LABEL: &str = "io.wadspaces.abi";
/// The ABI a package was built for.
pub const PKG_ABI_LABEL: &str = "io.wadspaces.pkg.abi";
const VERSION_LABEL: &str = "io.wadspaces.pkg.version";
/// A package built on this machine, not by CI.
pub const BUILT_HERE_LABEL: &str = "io.wadspaces.pkg.built-here";
/// The user name a registry sees: ghcr checks the token, not the name.
const REGISTRY_USER: &str = "wadspaces";
/// After the index couldn't be fetched, how long before trying again.
const RETRY_INDEX: Duration = Duration::from_secs(300);
const PUBLISH_EVERY: Duration = Duration::from_millis(500);

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, e.to_string())
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

pub struct Settings {
    /// Download what CI published; false: only what's here, or built here.
    pub pull: bool,
    pub index_url: String,
    pub index_max_age: Duration,
    pub state_dir: PathBuf,
    /// Free space a download or a build leaves (the builds' own).
    pub min_free_gb: u64,
}

impl Settings {
    pub fn new(config: &wad_config::Config) -> Self {
        let p = &config.packages;
        Self {
            pull: p.pull,
            index_url: p.index_url.clone(),
            index_max_age: Duration::from_secs(p.index_max_age_min * 60),
            state_dir: config.daemon.state_dir.clone(),
            min_free_gb: config.daemon.build_min_free_gb,
        }
    }
}

/// The index as it was got: from the network, or the last one saved.
#[derive(Clone)]
struct Index {
    doc: Value,
    from: &'static str,
    at: f64,
}

#[derive(Default)]
struct IndexState {
    got: Option<Index>,
    /// When fetching it last failed.
    failed: Option<Instant>,
}

/// A package here, as podman lists it.
#[derive(Debug, Clone)]
struct Local {
    image_id: String,
    tags: Vec<String>,
    version: String,
    size: u64,
    built_here: bool,
    created: f64,
}

/// A package a build can copy from: its image here, and how it got here.
#[derive(Debug, Clone, PartialEq)]
pub struct Got {
    pub id: String,
    /// localhost/wadspaces-pkg-<id>:<image id>
    pub image: String,
    pub how: PackageHow,
}

/// What `ensure` tells as it goes.
pub trait Watch: Send {
    /// It's on this package now.
    fn start(&mut self, _id: &str) {}
    fn line(&mut self, _line: &str) {}
    /// A download's bytes so far, and in all.
    fn bytes(&mut self, _id: &str, _done: u64, _total: u64) {}
    fn got(&mut self, _got: &Got) {}
}

/// Watching nothing.
pub struct Quiet;
impl Watch for Quiet {}

pub struct Packages {
    me: Weak<Self>,
    backend: Arc<dyn Backend>,
    http: reqwest::Client,
    bus: Bus,
    settings: Settings,
    index: tokio::sync::Mutex<IndexState>,
    /// One fetch or build of a package at a time.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// One package built here at a time, whichever (a build takes the
    /// machine's memory and processors; downloads go side by side).
    building_here: tokio::sync::Mutex<()>,
    fetch: Mutex<Option<PackageFetch>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Packages {
    pub fn new(backend: Arc<dyn Backend>, bus: Bus, settings: Settings) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            backend,
            http: reqwest::Client::new(),
            bus,
            settings,
            index: tokio::sync::Mutex::default(),
            locks: Mutex::default(),
            building_here: tokio::sync::Mutex::default(),
            fetch: Mutex::default(),
            task: Mutex::default(),
        })
    }

    fn saved_index(&self) -> PathBuf {
        self.settings.state_dir.join("packages/index.json")
    }

    /// CI's index: fetched again once it's older than `index_max_age`; when
    /// that fails, the last one fetched.
    async fn index(&self) -> Option<Index> {
        let mut st = self.index.lock().await;
        let fresh = st
            .got
            .as_ref()
            .is_some_and(|i| i.from == "network" && now() - i.at < self.settings.index_max_age.as_secs_f64());
        let resting = st.failed.is_some_and(|t| t.elapsed() < RETRY_INDEX);
        if !fresh && !resting {
            match self.download_index().await {
                Ok(doc) => {
                    if let Err(e) = save(&self.saved_index(), &doc) {
                        tracing::info!("saving the packages index: {e}");
                    }
                    st.got = Some(Index { doc, from: "network", at: now() });
                    st.failed = None;
                }
                Err(e) => {
                    tracing::info!("the packages index: {e}");
                    st.failed = Some(Instant::now());
                    if st.got.is_none() {
                        st.got = read_index(&self.saved_index(), "saved");
                    }
                }
            }
        }
        st.got.clone()
    }

    async fn download_index(&self) -> Result<Value, String> {
        let url = &self.settings.index_url;
        let doc: Value = if let Some(path) = url.strip_prefix("file://") {
            serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?)
                .map_err(|e| e.to_string())?
        } else {
            let res = self.http.get(url).timeout(Duration::from_secs(15)).send().await.map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("{url}: HTTP {}", res.status()));
            }
            res.json().await.map_err(|e| format!("{url}: {e}"))?
        };
        if doc["version"].as_u64() != Some(1) {
            return Err(format!("{url}: not an index WadSpaces reads"));
        }
        Ok(doc)
    }

    /// The base's ABI; None: it's from before packages, or it isn't here.
    async fn abi_of(&self, base: &str) -> Result<Option<String>, ApiError> {
        let labels = self.backend.image_labels(base).await.map_err(internal)?;
        Ok(labels.and_then(|l| l.get(ABI_LABEL).and_then(Value::as_str).filter(|a| !a.is_empty()).map(String::from)))
    }

    /// The packages here for `abi`, by id, newest first.
    async fn here(&self, abi: Option<&str>) -> Result<HashMap<String, Vec<Local>>, ApiError> {
        let mut out: HashMap<String, Vec<Local>> = HashMap::new();
        for i in self.backend.images_labelled(PKG_LABEL).await.map_err(internal)? {
            let label = |k: &str| i.labels.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            let (id, pkg_abi) = (label(PKG_LABEL), label(PKG_ABI_LABEL));
            if id.is_empty() || abi.is_some_and(|a| a != pkg_abi) {
                continue;
            }
            let list = out.entry(id).or_default();
            let image_id = i.id.trim_start_matches("sha256:");
            // One image under two names is one package.
            if let Some(same) = list.iter_mut().find(|l| l.image_id == image_id) {
                same.tags.extend(i.tags.iter().cloned());
                continue;
            }
            list.push(Local {
                image_id: i.id.trim_start_matches("sha256:").to_string(),
                tags: i.tags.clone(),
                version: label(VERSION_LABEL),
                size: i.size,
                built_here: label(BUILT_HERE_LABEL) == "true",
                created: i.created,
            });
        }
        for list in out.values_mut() {
            list.sort_by(|a, b| b.created.total_cmp(&a.created));
        }
        Ok(out)
    }

    /// GET /v1/packages: what CI published for the base, and what's here.
    pub async fn list(&self, base: &str) -> Result<PackageList, ApiError> {
        let abi = self.abi_of(base).await?;
        let index = self.index().await;
        let here = self.here(abi.as_deref()).await?;
        let published = match (&index, &abi) {
            (Some(i), Some(a)) => i.doc["abis"][a]["packages"].as_object().cloned().unwrap_or_default(),
            _ => Default::default(),
        };
        let mut ids: Vec<String> = published.keys().chain(here.keys()).cloned().collect();
        ids.sort();
        ids.dedup();
        let packages = pk::canonical(&ids)
            .into_iter()
            .map(|id| {
                let mine = here.get(&id).map(Vec::as_slice).unwrap_or_default();
                match published.get(&id) {
                    Some(e) => {
                        let want = e["imageId"].as_str().unwrap_or_default().trim_start_matches("sha256:");
                        let exact = mine.iter().find(|l| l.image_id == want);
                        PackageRow {
                            version: e["version"].as_str().unwrap_or_default().into(),
                            size_mb: e["size"].as_u64().unwrap_or(0) as f64 / 1e6,
                            needs: e["needs"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(String::from)
                                .collect(),
                            state: match (exact, mine.first()) {
                                (Some(_), _) => PackageState::Here,
                                (None, Some(_)) => PackageState::Older,
                                (None, None) => PackageState::Missing,
                            },
                            image: exact.or(mine.first()).map(|l| pk::local_ref(&id, &l.image_id)),
                            id,
                        }
                    }
                    None => {
                        let l = &mine[0];
                        PackageRow {
                            version: l.version.clone(),
                            size_mb: l.size as f64 / 1e6,
                            needs: pk::needs(&id).into_iter().map(String::from).collect(),
                            state: if l.built_here { PackageState::Built } else { PackageState::Here },
                            image: Some(pk::local_ref(&id, &l.image_id)),
                            id,
                        }
                    }
                }
            })
            .collect();
        Ok(PackageList {
            base: base.into(),
            abi,
            index_from: index.as_ref().map_or("none", |i| i.from).into(),
            index_at: index.as_ref().map(|i| i.at),
            packages,
            fetch: self.fetch.lock().unwrap().clone(),
        })
    }

    fn lock_for(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().unwrap().entry(id.to_string()).or_default().clone()
    }

    /// Each of `ids`, and what they need, onto this machine for `base`:
    /// what a build copies from, needs first.
    pub async fn ensure(&self, ids: &[String], base: &str, watch: &mut dyn Watch) -> Result<Vec<Got>, ApiError> {
        if let Some(bad) = ids.iter().find(|i| pk::source(i).is_none()) {
            return Err(ApiError::new(ErrorCode::BadRequest, format!("there's no package {bad:?}")));
        }
        let abi = self.abi_of(base).await?.ok_or_else(|| {
            ApiError::new(
                ErrorCode::Conflict,
                format!("{base} isn't here, or is from before packages (no {ABI_LABEL} label): update the base"),
            )
        })?;
        let index = self.index().await;
        let registry = index.as_ref().and_then(|i| i.doc["registry"].as_str()).unwrap_or(pk::REGISTRY).to_string();
        let mut got: Vec<Got> = vec![];
        for id in pk::closure(ids) {
            watch.start(&id);
            let lock = self.lock_for(&id);
            let _one = lock.lock().await;
            // After waiting: another build may have just got it.
            let here = self.here(Some(&abi)).await?.remove(&id).unwrap_or_default();
            let entry = index.as_ref().and_then(|i| pk::in_index(&i.doc, &abi, &id)).cloned();
            let g = self.one(&id, &abi, base, entry.as_ref(), &registry, &here, &got, watch).await?;
            watch.got(&g);
            got.push(g);
        }
        Ok(got)
    }

    /// One package, the first way that works.
    #[allow(clippy::too_many_arguments)]
    async fn one(
        &self,
        id: &str,
        abi: &str,
        base: &str,
        entry: Option<&Value>,
        registry: &str,
        here: &[Local],
        got: &[Got],
        watch: &mut dyn Watch,
    ) -> Result<Got, ApiError> {
        let use_local = |l: &Local| Got { id: id.into(), image: pk::local_ref(id, &l.image_id), how: PackageHow::Here };
        if let Some(e) = entry {
            let want = e["imageId"].as_str().unwrap_or_default().trim_start_matches("sha256:");
            if let Some(l) = here.iter().find(|l| l.image_id == want) {
                let g = use_local(l);
                self.name(&l.image_id, l, &g.image).await?;
                watch.line(&format!("» {id}: on this machine"));
                return Ok(g);
            }
            if self.settings.pull {
                match self.download(id, e, registry, watch).await {
                    Ok(image_id) => {
                        let image = pk::local_ref(id, &image_id);
                        self.backend.tag(&image_id, &image).await.map_err(internal)?;
                        return Ok(Got { id: id.into(), image, how: PackageHow::Fetched });
                    }
                    Err(why) => watch.line(&format!("» {id}: couldn't download it: {why}")),
                }
            }
        }
        if let Some(l) = here.first() {
            let g = use_local(l);
            self.name(&l.image_id, l, &g.image).await?;
            let older = if entry.is_some() { " (not CI's latest: a download gets that)" } else { "" };
            watch.line(&format!("» {id}: on this machine{older}"));
            return Ok(g);
        }
        self.build_here(id, abi, base, got, watch).await
    }

    /// Makes sure a package here has its local_ref name.
    async fn name(&self, image_id: &str, l: &Local, name: &str) -> Result<(), ApiError> {
        if !l.tags.iter().any(|t| t == name) {
            self.backend.tag(image_id, name).await.map_err(internal)?;
        }
        Ok(())
    }

    /// Room for `bytes` and what builds keep free.
    async fn room_for(&self, bytes: u64) -> Result<(), String> {
        match self.backend.free_gb().await {
            Some(free) if free - bytes as f64 / 1e9 < self.settings.min_free_gb as f64 => Err(format!(
                "only {free:.1} GB free: it needs {} and {} GB left over",
                gb(bytes),
                self.settings.min_free_gb
            )),
            _ => Ok(()),
        }
    }

    /// Downloads CI's package by its digest; returns its image id.
    async fn download(&self, id: &str, e: &Value, registry: &str, watch: &mut dyn Watch) -> Result<String, String> {
        let digest =
            e["digest"].as_str().filter(|d| d.starts_with("sha256:")).ok_or("the index has no digest for it")?;
        let size = e["size"].as_u64().unwrap_or(0);
        self.room_for(size).await?;
        let token = self.backend.github_token().await;
        if token.is_none() {
            return Err("there's no GitHub sign-in on this machine (sign in in the WadSpaces app)".into());
        }
        watch.line(&format!("» {id}: downloading {}", gb(size)));
        let reference = format!("{registry}/wadspaces-pkg-{id}@{digest}");
        let auth = token.map(|t| (REGISTRY_USER.to_string(), t));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let pull = self.backend.pull(
            &reference,
            auth,
            Box::new(move |d, t| {
                let _ = tx.send((d, t));
            }),
        );
        tokio::pin!(pull);
        let res = loop {
            tokio::select! {
                r = &mut pull => break r,
                Some((done, total)) = rx.recv() => watch.bytes(id, done, total),
            }
        };
        while let Ok((done, total)) = rx.try_recv() {
            watch.bytes(id, done, total);
        }
        let image_id = res?.trim_start_matches("sha256:").to_string();
        let want = e["imageId"].as_str().unwrap_or_default().trim_start_matches("sha256:");
        if !want.is_empty() && want != image_id {
            tracing::warn!("{id}: the index says image {want}, the registry gave {image_id}");
        }
        Ok(image_id)
    }

    /// Builds the package here from its recipe, on the packages it needs.
    async fn build_here(
        &self,
        id: &str,
        abi: &str,
        base: &str,
        got: &[Got],
        watch: &mut dyn Watch,
    ) -> Result<Got, ApiError> {
        let _one = match self.building_here.try_lock() {
            Ok(held) => held,
            Err(_) => {
                watch.line(&format!("» {id}: waiting for another package's build here"));
                self.building_here.lock().await
            }
        };
        self.room_for(0).await.map_err(|e| ApiError::new(ErrorCode::Conflict, e))?;
        let needs: Vec<(String, String)> = pk::closure(&pk::needs(id))
            .into_iter()
            .filter_map(|n| got.iter().find(|g| g.id == n).map(|g| (n, g.image.clone())))
            .collect();
        let file = pk::package_containerfile(id, base, &needs, false).map_err(internal)?;
        let context = wad_core::tar::tar(&[wad_core::tar::Entry { path: "Dockerfile", content: file.as_bytes() }])
            .map_err(internal)?;
        let tag = format!("localhost/wadspaces-pkg-{id}:building");
        // The Containerfile labels the package too; these say so whatever it does.
        let mut labels = serde_json::Map::new();
        labels.insert(PKG_LABEL.into(), id.into());
        labels.insert(PKG_ABI_LABEL.into(), abi.into());
        labels.insert(BUILT_HERE_LABEL.into(), "true".into());
        watch.line(&format!("» {id}: building it here"));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let build = self.backend.build(
            context,
            &tag,
            base,
            labels,
            Box::new(move |l| {
                let _ = tx.send(l.to_string());
            }),
        );
        tokio::pin!(build);
        let res = loop {
            tokio::select! {
                r = &mut build => break r,
                Some(l) = rx.recv() => watch.line(&l),
            }
        };
        while let Ok(l) = rx.try_recv() {
            watch.line(&l);
        }
        res.map_err(|e| ApiError::new(ErrorCode::Internal, format!("building the {id} package: {e}")))?;
        let info = self
            .backend
            .image_info(&tag)
            .await
            .map_err(internal)?
            .ok_or_else(|| internal(format!("{tag} isn't here after its build")))?;
        let image = pk::local_ref(id, &info.id);
        self.backend.tag(&info.id, &image).await.map_err(internal)?;
        self.backend.remove_image(&tag).await.map_err(internal)?;
        Ok(Got { id: id.into(), image, how: PackageHow::Built })
    }

    // ------------------------------------------------------------ fetches

    /// POST /v1/packages/fetch: packages onto this machine now, as a job.
    pub fn fetch(&self, req: PackageFetchRequest) -> Result<PackageFetch, ApiError> {
        if let Some(bad) = req.ids.iter().find(|i| pk::source(i).is_none()) {
            return Err(ApiError::new(ErrorCode::BadRequest, format!("there's no package {bad:?}")));
        }
        let base = req.base.clone().unwrap_or_else(|| wad_core::spec::DEFAULT_BASE_IMAGE.to_string());
        let job = {
            let mut f = self.fetch.lock().unwrap();
            if f.as_ref().is_some_and(|j| !j.status.finished()) {
                return Err(ApiError::new(ErrorCode::Conflict, "packages are being fetched already"));
            }
            let job = PackageFetch {
                ids: req.ids.clone(),
                status: BuildStatus::Building,
                current: None,
                done_bytes: 0,
                total_bytes: 0,
                got: vec![],
                error: None,
                started: now(),
                finished: None,
            };
            *f = Some(job.clone());
            job
        };
        self.bus.publish(Event::Packages(job.clone()));
        let me = self.me.upgrade().ok_or_else(|| internal("wadd is stopping"))?;
        *self.task.lock().unwrap() = Some(tokio::spawn(async move { me.run_fetch(req.ids, base).await }));
        Ok(job)
    }

    /// DELETE /v1/packages/fetch: stops it (what's downloaded so far stays).
    pub fn cancel_fetch(&self) -> bool {
        let Some(task) = self.task.lock().unwrap().take() else { return false };
        task.abort();
        self.finish(BuildStatus::Cancelled, None)
    }

    async fn run_fetch(self: Arc<Self>, ids: Vec<String>, base: String) {
        let ids = if ids.is_empty() {
            match self.list(&base).await {
                Ok(l) => l.packages.into_iter().filter(|p| p.state != PackageState::Built).map(|p| p.id).collect(),
                Err(e) => {
                    self.finish(BuildStatus::Error, Some(e.message));
                    return;
                }
            }
        } else {
            ids
        };
        // What it'll get, by the index's sizes: what isn't here yet.
        let sizes: HashMap<String, u64> = match self.list(&base).await {
            Ok(l) => {
                let want = pk::closure(&ids);
                l.packages
                    .iter()
                    .filter(|p| want.contains(&p.id) && p.state != PackageState::Here)
                    .map(|p| (p.id.clone(), (p.size_mb * 1e6) as u64))
                    .collect()
            }
            Err(_) => HashMap::new(),
        };
        let total = sizes.values().sum();
        self.update(|j| {
            j.ids = ids.clone();
            j.total_bytes = total;
        });
        let mut watch = FetchWatch { me: self.clone(), sizes, done: HashMap::new(), sent: Instant::now() };
        let res = self.ensure(&ids, &base, &mut watch).await;
        // Before it shows as finished, so a new fetch keeps its own task.
        self.task.lock().unwrap().take();
        match res {
            Ok(_) => self.finish(BuildStatus::Done, None),
            Err(e) => self.finish(BuildStatus::Error, Some(e.message)),
        };
    }

    fn update(&self, f: impl FnOnce(&mut PackageFetch)) {
        let job = {
            let mut held = self.fetch.lock().unwrap();
            let Some(j) = held.as_mut() else { return };
            f(j);
            j.clone()
        };
        self.bus.publish(Event::Packages(job));
    }

    /// Ends the fetch; false if none was going on.
    fn finish(&self, status: BuildStatus, error: Option<String>) -> bool {
        let running = self.fetch.lock().unwrap().as_ref().is_some_and(|j| !j.status.finished());
        if running {
            self.update(|j| {
                j.status = status;
                j.error = error;
                j.current = None;
                j.finished = Some(now());
            });
        }
        running
    }
}

/// A fetch's progress, as `packages` events (a few a second at most). It
/// counts in the index's sizes (what a package takes here): a download's
/// own bytes are compressed, and a small one may not count them at all.
struct FetchWatch {
    me: Arc<Packages>,
    /// The packages it's getting, and their sizes.
    sizes: HashMap<String, u64>,
    done: HashMap<String, u64>,
    sent: Instant,
}

impl Watch for FetchWatch {
    fn start(&mut self, id: &str) {
        self.me.update(|j| j.current = Some(id.to_string()));
    }
    fn bytes(&mut self, id: &str, done: u64, total: u64) {
        let size = self.sizes.get(id).copied().unwrap_or(0);
        self.done.insert(id.to_string(), if total > 0 { (size as f64 * done as f64 / total as f64) as u64 } else { 0 });
        if self.sent.elapsed() >= PUBLISH_EVERY {
            self.sent = Instant::now();
            let sum = self.done.values().sum();
            self.me.update(|j| j.done_bytes = sum);
        }
    }
    fn got(&mut self, got: &Got) {
        if let Some(size) = self.sizes.get(&got.id) {
            self.done.insert(got.id.clone(), *size);
        }
        let sum = self.done.values().sum();
        self.me.update(|j| {
            j.done_bytes = sum;
            j.got.push(PackageGot { id: got.id.clone(), how: got.how });
        });
    }
}

fn read_index(path: &std::path::Path, from: &'static str) -> Option<Index> {
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let at = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64());
    (doc["version"].as_u64() == Some(1)).then_some(Index { doc, from, at })
}

fn save(path: &std::path::Path, doc: &Value) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(path))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(doc)?)?;
    std::fs::rename(&tmp, path)
}
