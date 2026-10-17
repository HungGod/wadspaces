//! A fake machine for wadd's tests: podman, systemd and probes in memory.
#![allow(dead_code)]

pub mod firebase;
pub mod host;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use wad_proto::v1::{Display, Kind, Workspace};
use wadd::backend::{Backend, OnLine};

#[derive(Default)]
pub struct Machine {
    pub images: HashSet<String>,
    pub containers: HashMap<String, String>,
    pub calls: Vec<String>,
    pub units: Vec<(String, String)>,
    /// URLs that answer.
    pub answering: HashSet<String>,
    /// Polls before a URL answers.
    pub answer_after: usize,
    pub polls: usize,
    pub start_fails: Option<String>,
    pub free_gb: f64,
    /// Container id -> its wadspaces.id label.
    pub labels: HashMap<String, String>,
    /// Container -> all its labels (containers_labelled sees these).
    pub container_labels: HashMap<String, serde_json::Map<String, serde_json::Value>>,
    /// Image -> its labels (an image that's here and not listed has none).
    pub image_labels: HashMap<String, serde_json::Map<String, serde_json::Value>>,
    /// Volume -> where its files are.
    pub volumes: HashMap<String, String>,
    /// Times the units were written.
    pub installs: usize,
    /// Builds asked for: (tag, base, context).
    pub builds: Vec<(String, String, Vec<u8>)>,
    /// Each built image's id (a new one each build).
    pub image_ids: HashMap<String, String>,
    /// Images' sizes in bytes (else 2 GB).
    pub image_sizes: HashMap<String, u64>,
    /// Volumes' sizes in bytes (podman system df).
    pub volume_bytes: HashMap<String, u64>,
    /// What only an image has (podman system df's UniqueSize), and all the images' layers together.
    pub image_unique: HashMap<String, u64>,
    pub images_size: Option<u64>,
    pub build_fails: Option<String>,
    pub build_slow: bool,
    /// podman's secrets: name -> value.
    pub secrets: HashMap<String, Vec<u8>>,
    /// Apps' transient units that are up: unit -> its command line.
    pub apps: HashMap<String, Vec<String>>,
    /// This app's unit goes away just as it's stopped (it exited by itself).
    pub app_gone_at_stop: Option<String>,
    /// Running containers' first processes (a new one each start).
    pub pids: HashMap<String, u32>,
    pub next_pid: u32,
}

pub const TOKEN: &str = "ghp_TTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTT";

#[derive(Clone, Default)]
pub struct Fake(pub Arc<Mutex<Machine>>);

impl Fake {
    pub fn with(&self, f: impl FnOnce(&mut Machine)) {
        f(&mut self.0.lock().unwrap());
    }
    pub fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().calls.clone()
    }
}

#[async_trait]
impl Backend for Fake {
    async fn available(&self) -> bool {
        true
    }
    async fn container(&self, name: &str) -> Result<String, String> {
        Ok(self.0.lock().unwrap().containers.get(name).cloned().unwrap_or_else(|| "missing".into()))
    }
    async fn image_exists(&self, image: &str) -> Result<bool, String> {
        Ok(self.0.lock().unwrap().images.contains(image))
    }
    async fn start(&self, unit: &str) -> Result<(), String> {
        let fail = self.0.lock().unwrap().start_fails.clone();
        self.with(|m| m.calls.push(format!("start {unit}")));
        if let Some(e) = fail {
            return Err(e);
        }
        let name = unit.trim_end_matches(".service").to_string();
        self.with(|m| {
            m.next_pid += 1;
            m.pids.insert(name.clone(), 4000 + m.next_pid);
            m.containers.insert(name, "running".into());
        });
        Ok(())
    }
    async fn stop(&self, unit: &str) -> Result<(), String> {
        if self.0.lock().unwrap().app_gone_at_stop.as_deref() == Some(unit) {
            // It ended by itself just now: systemd collected its unit.
            self.with(|m| {
                m.apps.remove(unit);
                m.app_gone_at_stop = None;
            });
            return Err(format!("Unit {unit} not loaded."));
        }
        self.with(|m| {
            m.calls.push(format!("stop {unit}"));
            m.containers.remove(unit.trim_end_matches(".service"));
            m.pids.remove(unit.trim_end_matches(".service"));
            m.apps.remove(unit);
        });
        Ok(())
    }
    async fn start_app(&self, unit: &str, ws: &Workspace) -> Result<(), String> {
        let fail = self.0.lock().unwrap().start_fails.clone();
        self.with(|m| m.calls.push(format!("app {unit} {}", ws.exec.join(" "))));
        if let Some(e) = fail {
            return Err(e);
        }
        if self.0.lock().unwrap().apps.contains_key(unit) {
            return Err(format!("Unit {unit} was already loaded")); // as systemd says
        }
        self.with(|m| {
            m.apps.insert(unit.into(), ws.exec.clone());
        });
        Ok(())
    }
    async fn container_pid(&self, name: &str) -> Result<Option<u32>, String> {
        let m = self.0.lock().unwrap();
        Ok(m.pids.get(name).copied().filter(|_| m.containers.get(name).is_some_and(|s| s == "running")))
    }
    async fn unit_state(&self, unit: &str) -> Result<String, String> {
        Ok(if self.0.lock().unwrap().apps.contains_key(unit) { "active" } else { "inactive" }.into())
    }
    async fn restart(&self, unit: &str) -> Result<(), String> {
        self.with(|m| {
            m.calls.push(format!("restart {unit}"));
            m.containers.insert(unit.trim_end_matches(".service").into(), "running".into());
        });
        Ok(())
    }
    async fn install_units(&self, units: Vec<(String, String)>) -> Result<bool, String> {
        self.with(|m| {
            m.units = units;
            m.installs += 1;
        });
        Ok(true)
    }
    async fn http_ok(&self, url: &str) -> bool {
        let mut m = self.0.lock().unwrap();
        m.polls += 1;
        m.answering.contains(url) && m.polls > m.answer_after
    }
    async fn image_labels(&self, image: &str) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
        let m = self.0.lock().unwrap();
        Ok(m.images.contains(image).then(|| m.image_labels.get(image).cloned().unwrap_or_default()))
    }
    async fn volume_mountpoint(&self, name: &str) -> Option<String> {
        self.0.lock().unwrap().volumes.get(name).cloned()
    }
    async fn github_token(&self) -> Option<String> {
        let v = self.0.lock().unwrap().secrets.get("github_token").cloned()?;
        Some(String::from_utf8_lossy(&v).trim().to_string()).filter(|t| !t.is_empty())
    }
    async fn secret_names(&self) -> Result<Vec<String>, String> {
        let mut n: Vec<String> = self.0.lock().unwrap().secrets.keys().cloned().collect();
        n.sort();
        Ok(n)
    }
    async fn secret_value(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().secrets.get(name).map(|v| String::from_utf8_lossy(v).into_owned()))
    }
    async fn containers_labelled(&self, label: &str) -> Result<Vec<wadd::backend::ContainerInfo>, String> {
        let (key, value) = label.split_once('=').unwrap_or((label, ""));
        let m = self.0.lock().unwrap();
        let mut out: Vec<wadd::backend::ContainerInfo> = m
            .container_labels
            .iter()
            .filter(|(name, _)| m.containers.get(*name).is_some_and(|s| s == "running"))
            .filter(|(_, l)| l.get(key).and_then(|v| v.as_str()).is_some_and(|v| value.is_empty() || v == value))
            .map(|(name, labels)| wadd::backend::ContainerInfo { name: name.clone(), labels: labels.clone() })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
    async fn stop_container(&self, name: &str) -> Result<bool, String> {
        self.with(|m| m.calls.push(format!("stop container {name}")));
        Ok(self.0.lock().unwrap().containers.remove(name).is_some())
    }
    async fn create_secret(&self, name: &str, value: &[u8]) -> Result<(), String> {
        if value.is_empty() {
            return Err("secret data must be larger than 0".into()); // as podman says
        }
        self.with(|m| {
            m.secrets.insert(name.into(), value.to_vec());
        });
        Ok(())
    }
    async fn delete_secret(&self, name: &str) -> Result<bool, String> {
        Ok(self.0.lock().unwrap().secrets.remove(name).is_some())
    }
    async fn build(
        &self,
        context: Vec<u8>,
        tag: &str,
        base: &str,
        labels: serde_json::Map<String, serde_json::Value>,
        mut on_line: OnLine,
    ) -> Result<(), String> {
        let (fails, slow) = {
            let mut m = self.0.lock().unwrap();
            m.builds.push((tag.into(), base.into(), context));
            (m.build_fails.clone(), m.build_slow)
        };
        on_line(&format!("STEP 1/2: FROM {base}"));
        if slow {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        if let Some(e) = fails {
            return Err(e);
        }
        on_line("STEP 2/2: COPY root/ /");
        on_line("--> 0123abcd");
        self.with(|m| {
            m.images.insert(tag.into());
            // A build keeps its base's labels (FROM), then adds its own.
            let mut all = m.image_labels.get(base).cloned().unwrap_or_default();
            all.extend(labels);
            m.image_labels.insert(tag.into(), all);
            let n = m.builds.len();
            m.image_ids.insert(tag.into(), format!("{n:0>64}"));
        });
        Ok(())
    }
    async fn volume_sizes(&self) -> Result<Vec<(String, u64)>, String> {
        Ok(self.0.lock().unwrap().volume_bytes.iter().map(|(k, v)| (k.clone(), *v)).collect())
    }
    async fn image_info(&self, image: &str) -> Result<Option<wadd::backend::ImageInfo>, String> {
        let m = self.0.lock().unwrap();
        let id_of = |name: &str| m.image_ids.get(name).cloned().unwrap_or_else(|| format!("{:x<64}", name.len()));
        // By name, or by id as podman takes either.
        let image = m.images.iter().find(|n| *n == image || id_of(n) == image).cloned().unwrap_or_default();
        Ok(m.images.contains(&image).then(|| wadd::backend::ImageInfo {
            id: id_of(&image),
            created: 1_700_000_000.0,
            size: m.image_sizes.get(&image).copied().unwrap_or(2_000_000_000),
            labels: m.image_labels.get(&image).cloned().unwrap_or_default(),
            tags: vec![image],
        }))
    }
    async fn images_labelled(&self, label: &str) -> Result<Vec<wadd::backend::ImageInfo>, String> {
        let names: Vec<String> = {
            let m = self.0.lock().unwrap();
            let mut n: Vec<String> = m
                .images
                .iter()
                .filter(|i| m.image_labels.get(*i).is_some_and(|l| l.contains_key(label)))
                .cloned()
                .collect();
            n.sort();
            n
        };
        let mut out = vec![];
        for n in names {
            out.extend(self.image_info(&n).await?);
        }
        Ok(out)
    }
    async fn remove_image(&self, image: &str) -> Result<bool, String> {
        self.with(|m| m.calls.push(format!("rmi {image}")));
        let mut m = self.0.lock().unwrap();
        // An image built on it keeps it: only the name goes (as podman untags).
        let id = m.image_ids.get(image).cloned().unwrap_or_else(|| format!("{:x<64}", image.len()));
        let needed = m.image_labels.iter().any(|(n, l)| {
            n != image && m.images.contains(n) && l.get("io.wadspaces.from").and_then(|v| v.as_str()) == Some(&id)
        });
        if needed && m.images.remove(image) {
            let left = format!("<none>@{id}");
            if let Some(v) = m.image_ids.remove(image) {
                m.image_ids.insert(left.clone(), v);
            }
            if let Some(v) = m.image_labels.remove(image) {
                m.image_labels.insert(left.clone(), v);
            }
            if let Some(v) = m.image_sizes.remove(image) {
                m.image_sizes.insert(left.clone(), v);
            }
            m.images.insert(left);
            return Ok(true);
        }
        Ok(m.images.remove(image))
    }
    async fn remove_image_id(&self, id: &str) -> Result<bool, String> {
        self.with(|m| m.calls.push(format!("rmi --id {id}")));
        let mut m = self.0.lock().unwrap();
        let id_of =
            |m: &Machine, name: &str| m.image_ids.get(name).cloned().unwrap_or_else(|| format!("{:x<64}", name.len()));
        let Some(name) = m.images.iter().find(|n| id_of(&m, n) == id).cloned() else { return Ok(false) };
        if m.image_labels.iter().any(|(n, l)| {
            *n != name && m.images.contains(n) && l.get("io.wadspaces.from").and_then(|v| v.as_str()) == Some(id)
        }) {
            return Err(format!("image {id} has dependent child images"));
        }
        m.images.remove(&name);
        Ok(true)
    }
    async fn system_df(&self) -> Result<wadd::backend::Df, String> {
        let m = self.0.lock().unwrap();
        let id_of = |name: &str| m.image_ids.get(name).cloned().unwrap_or_else(|| format!("{:x<64}", name.len()));
        let size = |name: &str| m.image_sizes.get(name).copied().unwrap_or(2_000_000_000);
        let images: Vec<wadd::backend::DfImage> = m
            .images
            .iter()
            .map(|n| wadd::backend::DfImage {
                id: id_of(n),
                name: if n.starts_with("<none>") { String::new() } else { n.clone() },
                size: size(n),
                unique: m.image_unique.get(n).copied().unwrap_or(0),
                containers: 0,
            })
            .collect();
        Ok(wadd::backend::Df {
            images_size: m.images_size.unwrap_or_else(|| images.iter().map(|i| i.unique).sum()),
            images,
            containers: vec![],
            volumes: m
                .volume_bytes
                .iter()
                .map(|(k, v)| wadd::backend::DfVolume { name: k.clone(), size: *v, links: 0 })
                .collect(),
        })
    }
    async fn images_all(&self) -> Result<Vec<wadd::backend::ImageInfo>, String> {
        let names: Vec<String> = self.0.lock().unwrap().images.iter().cloned().collect();
        let mut out = vec![];
        for n in names {
            out.extend(self.image_info(&n).await?);
        }
        Ok(out)
    }
    async fn remove_volume(&self, name: &str) -> Result<bool, String> {
        self.with(|m| m.calls.push(format!("volume rm {name}")));
        Ok(self.0.lock().unwrap().volume_bytes.remove(name).is_some())
    }
    async fn workspace_of(&self, container: &str) -> Option<String> {
        self.0.lock().unwrap().labels.get(container).cloned()
    }
    async fn free_gb(&self) -> Option<f64> {
        Some(self.0.lock().unwrap().free_gb)
    }
}

/// An app (a program on the machine): `exec` and nothing of a container's.
pub fn app(id: &str, exec: &str) -> Workspace {
    Workspace {
        kind: Kind::App,
        exec: vec![exec.into()],
        image: String::new(),
        env: vec![],
        ..ws(id, "", Display::Host, None)
    }
}

pub fn ws(id: &str, image: &str, display: Display, port: Option<u16>) -> Workspace {
    Workspace {
        id: id.into(),
        name: id.to_uppercase(),
        kind: Kind::Container,
        exec: vec![],
        image: image.into(),
        display,
        port,
        hotkey: None,
        icon: None,
        enabled: true,
        autostart: false,
        container_name: format!("wad-{id}"),
        container_port: 3000,
        env: vec![("PUID".into(), "1000".into())],
        secrets: vec![],
        volumes: vec![],
        devices: vec![],
        shm_size: Some("1g".into()),
        projects: vec![],
        signins: vec![],
        ephemeral: false,
        desktop: vec![],
    }
}

/// The file names in a tar (a build folder the fake was given).
pub fn tar_names(tar: &[u8]) -> Vec<String> {
    let mut out = vec![];
    let mut at = 0;
    while at + 512 <= tar.len() && tar[at] != 0 {
        let head = &tar[at..at + 512];
        let name = String::from_utf8_lossy(&head[..100]).trim_end_matches('\0').to_string();
        let size =
            usize::from_str_radix(String::from_utf8_lossy(&head[124..135]).trim_matches(['\0', ' ']), 8).unwrap_or(0);
        out.push(name);
        at += 512 + size.div_ceil(512) * 512;
    }
    out
}
