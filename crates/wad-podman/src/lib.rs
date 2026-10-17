//! Podman's libpod REST API over its Unix socket (rootful
//! /run/podman/podman.sock, rootless $XDG_RUNTIME_DIR/podman/podman.sock).
//! Plain HTTP/1.1 and JSON: one connection per call, which podman handles
//! fine and keeps this simple. Only what wadd uses.

use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::UnixStream;

const API: &str = "/v5.0.0/libpod";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("can't reach podman ({0}): {1}")]
    Connect(String, #[source] std::io::Error),
    #[error("podman: {0}")]
    Http(String),
    /// Podman answered with an error status: what it said.
    #[error("{what}: HTTP {status}: {message}")]
    Status { what: String, status: u16, message: String },
    /// An error inside a streamed answer (a build).
    #[error("{0}")]
    Stream(String),
    #[error("podman's answer to {0} wasn't what was expected: {1}")]
    Decode(String, String),
}

#[derive(Debug, Clone)]
pub struct Podman {
    socket: PathBuf,
}

/// Percent-encodes one path segment (an image reference has / : @ in it).
/// A container's output without a terminal comes in frames: a stream byte,
/// three zero bytes, a big-endian u32 length, then that much output. Anything
/// else (a terminal's raw output) is taken as it is.
pub fn demux(b: &[u8]) -> String {
    let framed = b.len() >= 8 && matches!(b[0], 0..=2) && b[1..4] == [0, 0, 0];
    if !framed {
        return String::from_utf8_lossy(b).into_owned();
    }
    let mut out = Vec::with_capacity(b.len());
    let mut at = 0;
    while at + 8 <= b.len() {
        let n = u32::from_be_bytes([b[at + 4], b[at + 5], b[at + 6], b[at + 7]]) as usize;
        let end = (at + 8 + n).min(b.len());
        out.extend_from_slice(&b[at + 8..end]);
        at = end;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `podman system df -v`, in bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Df {
    /// All the images' layers together (each counted once).
    pub images_size: u64,
    pub images: Vec<DfImage>,
    /// Each container's id and its own changes (its writable layer).
    pub containers: Vec<(String, u64)>,
    pub volumes: Vec<DfVolume>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DfImage {
    pub id: String,
    /// "repo:tag", or empty for an image with no name.
    pub name: String,
    pub size: u64,
    /// What only this image has (no other image shares it).
    pub unique: u64,
    /// Containers made from it.
    pub containers: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DfVolume {
    pub name: String,
    pub size: u64,
    /// Containers using it.
    pub links: u64,
}

/// An image as `images_labelled` and `image_info` see it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    pub id: String,
    /// Unix seconds.
    pub created: f64,
    pub size: u64,
    pub labels: serde_json::Map<String, Value>,
    pub tags: Vec<String>,
}

/// A running container as `containers_labelled` sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerInfo {
    pub name: String,
    pub labels: serde_json::Map<String, Value>,
}

/// "2026-10-11T19:05:00.123456789Z" (or with an offset) → Unix seconds.
fn rfc3339_secs(s: &str) -> Option<f64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, dd) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (time, offset) = match rest.find(['Z', '+', '-']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "Z"),
    };
    let mut t = time.split(':');
    let (hh, mm) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let ss: f64 = t.next()?.parse().ok()?;
    let off = match offset {
        "Z" | "" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let (oh, om) = o[1..].split_once(':').unwrap_or((&o[1..], "0"));
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
        }
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + hh * 3600 + mm * 60 - off) as f64 + ss)
}

pub fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Podman {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into() }
    }

    async fn send(&self, method: Method, path: &str, body: Option<(Bytes, &str)>) -> Result<Response<Incoming>, Error> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|e| Error::Connect(self.socket.display().to_string(), e))?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| Error::Http(e.to_string()))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let mut req = Request::builder().method(method).uri(format!("{API}{path}")).header("host", "podman");
        let body = match body {
            Some((bytes, content_type)) => {
                req = req.header("content-type", content_type);
                bytes
            }
            None => Bytes::new(),
        };
        let req = req.body(Full::new(body)).map_err(|e| Error::Http(e.to_string()))?;
        sender.send_request(req).await.map_err(|e| Error::Http(e.to_string()))
    }

    async fn body(res: Response<Incoming>) -> Result<Bytes, Error> {
        Ok(res.into_body().collect().await.map_err(|e| Error::Http(e.to_string()))?.to_bytes())
    }

    async fn status_error(what: &str, res: Response<Incoming>) -> Error {
        let status = res.status().as_u16();
        let body = Self::body(res).await.unwrap_or_default();
        let message = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(String::from))
            .unwrap_or_else(|| String::from_utf8_lossy(&body).trim().to_string());
        Error::Status { what: what.into(), status, message }
    }

    async fn get_json(&self, what: &str, path: &str) -> Result<Option<Value>, Error> {
        let res = self.send(Method::GET, path, None).await?;
        match res.status() {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => {
                let body = Self::body(res).await?;
                serde_json::from_slice(&body).map(Some).map_err(|e| Error::Decode(what.into(), e.to_string()))
            }
            _ => Err(Self::status_error(what, res).await),
        }
    }

    /// podman's own description (`podman info`).
    pub async fn info(&self) -> Result<Value, Error> {
        Ok(self.get_json("info", "/info").await?.unwrap_or(Value::Null))
    }

    /// A container's last `tail` lines of output (stdout and stderr). None if
    /// there's no such container.
    pub async fn container_logs(&self, name: &str, tail: usize) -> Result<Option<String>, Error> {
        let path = format!("/containers/{}/logs?stdout=true&stderr=true&tail={tail}", segment(name));
        let res = self.send(Method::GET, &path, None).await?;
        match res.status() {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => Ok(Some(demux(&Self::body(res).await?))),
            _ => Err(Self::status_error(&format!("logs {name}"), res).await),
        }
    }

    /// Whether podman answers (within 3 s).
    pub async fn ping(&self) -> bool {
        matches!(
            tokio::time::timeout(Duration::from_secs(3), self.send(Method::GET, "/_ping", None)).await,
            Ok(Ok(r)) if r.status() == StatusCode::OK
        )
    }

    /// Where podman keeps its images (to see which layers it has).
    pub async fn graph_root(&self) -> Result<Option<String>, Error> {
        let info = self.info().await?;
        Ok(info.pointer("/store/graphRoot").and_then(Value::as_str).map(String::from))
    }

    pub async fn image_exists(&self, image: &str) -> Result<bool, Error> {
        let res = self.send(Method::GET, &format!("/images/{}/exists", segment(image)), None).await?;
        match res.status() {
            StatusCode::NO_CONTENT | StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            _ => Err(Self::status_error(&format!("image exists {image}"), res).await),
        }
    }

    /// An image's labels; None if it isn't here.
    pub async fn image_labels(&self, image: &str) -> Result<Option<serde_json::Map<String, Value>>, Error> {
        let what = format!("inspect image {image}");
        Ok(self.get_json(&what, &format!("/images/{}/json", segment(image))).await?.map(|info| {
            info.get("Labels")
                .or_else(|| info.pointer("/Config/Labels"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        }))
    }

    /// An image's id, when it was made (Unix seconds), its size in bytes and
    /// its labels; None if it isn't here.
    pub async fn image_info(&self, image: &str) -> Result<Option<ImageInfo>, Error> {
        let what = format!("inspect image {image}");
        Ok(self.get_json(&what, &format!("/images/{}/json", segment(image))).await?.map(|info| ImageInfo {
            id: info.get("Id").and_then(Value::as_str).unwrap_or_default().to_string(),
            created: info
                .get("Created")
                .and_then(Value::as_str)
                .and_then(rfc3339_secs)
                .or_else(|| info.get("Created").and_then(Value::as_f64))
                .unwrap_or(0.0),
            size: info.get("Size").and_then(Value::as_u64).unwrap_or(0),
            labels: info
                .get("Labels")
                .or_else(|| info.pointer("/Config/Labels"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            tags: info
                .get("RepoTags")
                .and_then(Value::as_array)
                .map(|t| t.iter().filter_map(Value::as_str).map(String::from).collect())
                .unwrap_or_default(),
        }))
    }

    /// The images that have a label, with their tags (`podman images --filter label=`).
    pub async fn images_labelled(&self, label: &str) -> Result<Vec<ImageInfo>, Error> {
        let filters = serde_json::json!({ "label": [label] }).to_string();
        let path = format!("/images/json?filters={}", segment(&filters));
        let list = self.get_json("list images", &path).await?.unwrap_or(Value::Null);
        Ok(list
            .as_array()
            .into_iter()
            .flatten()
            .map(|i| ImageInfo {
                id: i.get("Id").and_then(Value::as_str).unwrap_or_default().to_string(),
                created: i.get("Created").and_then(Value::as_f64).unwrap_or(0.0),
                size: i.get("Size").and_then(Value::as_u64).unwrap_or(0),
                labels: i.get("Labels").and_then(Value::as_object).cloned().unwrap_or_default(),
                tags: i
                    .get("RepoTags")
                    .or_else(|| i.get("Names"))
                    .and_then(Value::as_array)
                    .map(|t| t.iter().filter_map(Value::as_str).map(String::from).collect())
                    .unwrap_or_default(),
            })
            .collect())
    }

    /// Removes an image by name; when other images are built on it, only
    /// its name goes (they keep its layers). False if there was none.
    pub async fn remove_image(&self, image: &str) -> Result<bool, Error> {
        let res = self.send(Method::DELETE, &format!("/images/{}", segment(image)), None).await?;
        match res.status() {
            StatusCode::NOT_FOUND => Ok(false),
            s if s.is_success() => Ok(true),
            StatusCode::CONFLICT => {
                let (repo, tag) = image.rsplit_once(':').unwrap_or((image, "latest"));
                let path = format!("/images/{}/untag?repo={}&tag={}", segment(image), segment(repo), segment(tag));
                let res = self.send(Method::POST, &path, None).await?;
                if res.status().is_success() {
                    Ok(true)
                } else {
                    Err(Self::status_error(&format!("untag {image}"), res).await)
                }
            }
            _ => Err(Self::status_error(&format!("remove {image}"), res).await),
        }
    }

    /// Removes an image by id, all its names with it; an error while
    /// anything uses it (a container, an image built on it). False if there was none.
    pub async fn remove_image_id(&self, id: &str) -> Result<bool, Error> {
        let res = self.send(Method::DELETE, &format!("/images/{}", segment(id)), None).await?;
        match res.status() {
            StatusCode::NOT_FOUND => Ok(false),
            s if s.is_success() => Ok(true),
            _ => Err(Self::status_error(&format!("remove image {id}"), res).await),
        }
    }

    pub async fn inspect_container(&self, name: &str) -> Result<Option<Value>, Error> {
        self.get_json(&format!("inspect {name}"), &format!("/containers/{}/json", segment(name))).await
    }

    /// The running containers that carry a label (`podman ps --filter label=`).
    pub async fn containers_labelled(&self, label: &str) -> Result<Vec<ContainerInfo>, Error> {
        let filters = serde_json::json!({ "label": [label] }).to_string();
        let path = format!("/containers/json?filters={}", segment(&filters));
        let list = self.get_json("list containers", &path).await?.unwrap_or(Value::Null);
        Ok(list
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| ContainerInfo {
                name: c
                    .get("Names")
                    .and_then(Value::as_array)
                    .and_then(|n| n.first())
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim_start_matches('/')
                    .to_string(),
                labels: c.get("Labels").and_then(Value::as_object).cloned().unwrap_or_default(),
            })
            .collect())
    }

    /// Stops a container (`timeout` seconds before it's killed). False if
    /// there was none.
    pub async fn stop_container(&self, name: &str, timeout: u32) -> Result<bool, Error> {
        let path = format!("/containers/{}/stop?timeout={timeout}", segment(name));
        let res = self.send(Method::POST, &path, None).await?;
        match res.status() {
            StatusCode::NOT_FOUND => Ok(false),
            // 304: it had stopped already.
            s if s.is_success() || s == StatusCode::NOT_MODIFIED => Ok(true),
            _ => Err(Self::status_error(&format!("stop {name}"), res).await),
        }
    }

    /// running | exited | created | paused | ... ; "missing" when there's no such
    /// container (quadlet runs them with --rm, so a stopped one is gone).
    pub async fn container_status(&self, name: &str) -> Result<String, Error> {
        Ok(match self.inspect_container(name).await? {
            None => "missing".into(),
            Some(info) => info.pointer("/State/Status").and_then(Value::as_str).unwrap_or("unknown").to_lowercase(),
        })
    }

    /// Each named volume's size in bytes (`podman system df -v`: podman
    /// counts the files, so it takes a moment).
    pub async fn volume_sizes(&self) -> Result<Vec<(String, u64)>, Error> {
        Ok(self.system_df().await?.volumes.into_iter().map(|v| (v.name, v.size)).collect())
    }

    /// What podman keeps on the disk (`podman system df -v`): every image
    /// (all of it, and what only it has), the containers' own changes, the volumes.
    pub async fn system_df(&self) -> Result<Df, Error> {
        let df = self.get_json("disk usage", "/system/df").await?.unwrap_or(Value::Null);
        let list = |k: &str| df.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Ok(Df {
            images_size: n(&df, "ImagesSize"),
            images: list("Images")
                .iter()
                .map(|i| DfImage {
                    id: s(i, "ImageID"),
                    name: match (s(i, "Repository"), s(i, "Tag")) {
                        (r, _) if r.is_empty() || r == "<none>" => String::new(),
                        (r, t) => format!("{r}:{t}"),
                    },
                    size: n(i, "Size"),
                    unique: n(i, "UniqueSize"),
                    containers: n(i, "Containers"),
                })
                .collect(),
            containers: list("Containers").iter().map(|c| (s(c, "ContainerID"), n(c, "RWSize"))).collect(),
            volumes: list("Volumes")
                .iter()
                .map(|v| DfVolume { name: s(v, "VolumeName"), size: n(v, "Size"), links: n(v, "Links") })
                .collect(),
        })
    }

    /// Every image here, with its labels and names (intermediate ones too).
    pub async fn images_all(&self) -> Result<Vec<ImageInfo>, Error> {
        let list = self.get_json("list images", "/images/json?all=true").await?.unwrap_or(Value::Null);
        Ok(list
            .as_array()
            .into_iter()
            .flatten()
            .map(|i| ImageInfo {
                id: i.get("Id").and_then(Value::as_str).unwrap_or_default().to_string(),
                created: i.get("Created").and_then(Value::as_f64).unwrap_or(0.0),
                size: i.get("Size").and_then(Value::as_u64).unwrap_or(0),
                labels: i.get("Labels").and_then(Value::as_object).cloned().unwrap_or_default(),
                tags: i
                    .get("RepoTags")
                    .or_else(|| i.get("Names"))
                    .and_then(Value::as_array)
                    .map(|t| t.iter().filter_map(Value::as_str).map(String::from).collect())
                    .unwrap_or_default(),
            })
            .collect())
    }

    /// Removes a named volume and its files. False if there was none.
    pub async fn remove_volume(&self, name: &str) -> Result<bool, Error> {
        let res = self.send(Method::DELETE, &format!("/volumes/{}", segment(name)), None).await?;
        match res.status() {
            StatusCode::NOT_FOUND => Ok(false),
            s if s.is_success() => Ok(true),
            _ => Err(Self::status_error(&format!("remove volume {name}"), res).await),
        }
    }

    /// Where a named volume's files are on the host; None if there's no such volume.
    pub async fn volume_mountpoint(&self, name: &str) -> Result<Option<String>, Error> {
        let info =
            self.get_json(&format!("inspect volume {name}"), &format!("/volumes/{}/json", segment(name))).await?;
        Ok(info.and_then(|i| i.get("Mountpoint").and_then(Value::as_str).filter(|m| !m.is_empty()).map(String::from)))
    }

    /// Builds an image from a tar build context; returns the image id. Each
    /// output line goes to `on_line`. podman answers 200 even when the build
    /// fails, so errors come from the stream. Dropping the future closes the
    /// stream, which stops the build.
    pub async fn build(
        &self,
        context: Vec<u8>,
        tag: &str,
        buildargs: &serde_json::Map<String, Value>,
        labels: &serde_json::Map<String, Value>,
        mut on_line: impl FnMut(&str),
    ) -> Result<String, Error> {
        let args = serde_json::to_string(buildargs).expect("json");
        let mut path = format!(
            "/build?t={}&layers=true&rm=true&forcerm=true&pull=false&buildargs={}",
            segment(tag),
            segment(&args)
        );
        if !labels.is_empty() {
            path.push_str(&format!("&labels={}", segment(&serde_json::to_string(labels).expect("json"))));
        }
        let res = self.send(Method::POST, &path, Some((Bytes::from(context), "application/x-tar"))).await?;
        if !res.status().is_success() {
            return Err(Self::status_error(&format!("build {tag}"), res).await);
        }
        let mut body = res.into_body();
        let mut buf: Vec<u8> = Vec::new();
        let mut image_id = String::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| Error::Http(e.to_string()))?;
            let Ok(data) = frame.into_data() else { continue };
            buf.extend_from_slice(&data);
            while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                let Ok(msg) = serde_json::from_slice::<Value>(&line) else { continue };
                let err = msg
                    .pointer("/errorDetail/message")
                    .or_else(|| msg.get("error"))
                    .and_then(Value::as_str)
                    .filter(|e| !e.is_empty());
                if let Some(e) = err {
                    return Err(Error::Stream(e.trim().to_string()));
                }
                let text = msg.get("stream").and_then(Value::as_str).unwrap_or_default();
                let t = text.trim();
                if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
                    image_id = t.into();
                    continue;
                }
                for part in text.trim_end_matches('\n').split('\n') {
                    on_line(part);
                }
            }
        }
        Ok(image_id)
    }

    // ---------------------------------------------------------- secrets
    /// A secret's value, for wadd's own use (git's token); never sent out.
    /// None if there's no such secret.
    pub async fn secret_value(&self, name: &str) -> Result<Option<String>, Error> {
        let path = format!("/secrets/{}/json?showsecret=true", segment(name));
        let info = self.get_json(&format!("read secret {name}"), &path).await?;
        Ok(info.and_then(|i| i.get("SecretData").and_then(Value::as_str).map(String::from)))
    }

    /// The names of podman's secrets.
    pub async fn secret_names(&self) -> Result<Vec<String>, Error> {
        let list = self.get_json("list secrets", "/secrets/json").await?.unwrap_or(Value::Null);
        Ok(list
            .as_array()
            .map(|a| {
                a.iter().filter_map(|s| s.pointer("/Spec/Name").and_then(Value::as_str).map(String::from)).collect()
            })
            .unwrap_or_default())
    }

    pub async fn delete_secret(&self, name: &str) -> Result<bool, Error> {
        let res = self.send(Method::DELETE, &format!("/secrets/{}", segment(name)), None).await?;
        match res.status() {
            StatusCode::NOT_FOUND => Ok(false),
            s if s.is_success() => Ok(true),
            _ => Err(Self::status_error(&format!("delete secret {name}"), res).await),
        }
    }

    /// Creates (replacing) a secret. The value only ever goes to podman.
    pub async fn create_secret(&self, name: &str, value: &[u8]) -> Result<(), Error> {
        self.delete_secret(name).await?;
        let path = format!("/secrets/create?name={}", segment(name));
        let res =
            self.send(Method::POST, &path, Some((Bytes::copy_from_slice(value), "application/octet-stream"))).await?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(Self::status_error(&format!("create secret {name}"), res).await)
        }
    }
}

#[cfg(test)]
mod tests;
