//! wadd, from the HUD: its event stream (followed on a thread, delivered to
//! the GTK loop through a channel) and the few calls the buttons make. The
//! Rust wadd's /v1, over its Unix socket (/run/wadd/wadd.sock, or
//! WADD_SOCKET), spoken as plain HTTP/1.1: one connection per call.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::model::{Brightness, WifiNetwork};

pub enum Incoming {
    /// A wadd event: its name and data.
    Event(String, Value),
    /// The stream dropped (it reconnects by itself).
    Disconnected,
}

#[derive(Clone)]
pub struct Wadd {
    socket: PathBuf,
}

/// An answer: its status, and the body still to read.
struct Answer {
    status: u16,
    body: Box<dyn Read + Send>,
}

impl Answer {
    fn text(mut self, limit: u64) -> Result<(u16, Vec<u8>), String> {
        let mut out = Vec::new();
        (&mut self.body).take(limit).read_to_end(&mut out).map_err(|e| e.to_string())?;
        Ok((self.status, out))
    }
}

impl Wadd {
    pub fn from_env() -> Self {
        let socket = std::env::var_os("WADD_SOCKET").map(PathBuf::from).unwrap_or_else(|| "/run/wadd/wadd.sock".into());
        Self { socket }
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> Result<Answer, String> {
        let mut s = UnixStream::connect(&self.socket).map_err(|e| format!("can't reach wadd: {e}"))?;
        s.set_read_timeout(timeout).map_err(|e| e.to_string())?;
        s.set_write_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: wadd\r\nConnection: close\r\nAccept: */*\r\n");
        if body.is_some() {
            head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", payload.len()));
        }
        head.push_str("\r\n");
        head.push_str(&payload);
        // One write. wadd may answer (and close) before reading a body its
        // endpoint doesn't take: a failed write then still has an answer.
        let sent = s.write_all(head.as_bytes());
        match read_answer(BufReader::new(s)) {
            Ok(a) => Ok(a),
            Err(e) => Err(sent.err().map(|w| w.to_string()).unwrap_or(e)),
        }
    }

    /// wadd's message for a failed call, else the status.
    fn failure(status: u16, body: &[u8]) -> String {
        serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(String::from))
            .unwrap_or_else(|| format!("wadd answered {status}"))
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let (status, body) = self.call("GET", path, None, Some(Duration::from_secs(40)))?.text(16 << 20)?;
        if !(200..300).contains(&status) {
            return Err(Self::failure(status, &body));
        }
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    }

    fn post(&self, path: &str, body: Value) -> Result<(), String> {
        let (status, text) = self.call("POST", path, Some(&body), Some(Duration::from_secs(40)))?.text(1 << 20)?;
        if (200..300).contains(&status) { Ok(()) } else { Err(Self::failure(status, &text)) }
    }

    fn put<T: DeserializeOwned>(&self, path: &str, body: Value) -> Result<T, String> {
        let (status, text) = self.call("PUT", path, Some(&body), Some(Duration::from_secs(10)))?.text(1 << 20)?;
        if !(200..300).contains(&status) {
            return Err(Self::failure(status, &text));
        }
        serde_json::from_slice(&text).map_err(|e| e.to_string())
    }

    pub fn brightness(&self) -> Result<Brightness, String> {
        self.get("/v1/screen/brightness")
    }

    pub fn set_brightness(&self, percent: u8) -> Result<Brightness, String> {
        self.put("/v1/screen/brightness", json!({ "percent": percent }))
    }

    /// WadSpaces Client on screen (what Super+0 does).
    pub fn home(&self) -> Result<(), String> {
        self.post("/v1/view/home", json!({}))
    }

    pub fn power(&self, action: &str) -> Result<(), String> {
        self.post("/v1/power", json!({ "action": action }))
    }

    pub fn wifi_scan(&self) -> Result<Vec<WifiNetwork>, String> {
        self.get("/v1/network/wifi")
    }

    pub fn wifi_connect(&self, ssid: &str, password: Option<&str>) -> Result<(), String> {
        let mut body = json!({ "ssid": ssid });
        if let Some(p) = password.filter(|p| !p.is_empty()) {
            body["password"] = p.into();
        }
        self.post("/v1/network/wifi/connect", body)
    }

    pub fn wifi_disconnect(&self) -> Result<(), String> {
        self.post("/v1/network/wifi/disconnect", json!({}))
    }

    pub fn devices(&self) -> Result<Vec<wad_proto::v1::Device>, String> {
        self.get("/v1/devices")
    }

    /// Hands a device to a wadspace (on), or takes it back (off).
    pub fn device_to(&self, key: &str, wadspace: &str, on: bool) -> Result<(), String> {
        let how = if on { "attach" } else { "detach" };
        self.post(&format!("/v1/devices/{key}/{how}"), json!({ "wadspace": wadspace }))
    }

    /// Takes a drive out of its wadspaces and turns it off (safe to unplug).
    pub fn device_eject(&self, key: &str) -> Result<(), String> {
        self.post(&format!("/v1/devices/{key}/eject"), json!({}))
    }

    /// Where a device goes (the answer for a new one: none keeps it here).
    pub fn device_remembered(&self, key: &str, wadspaces: &[String]) -> Result<(), String> {
        self.put::<Value>(&format!("/v1/devices/{key}"), json!({ "remembered": wadspaces })).map(drop)
    }

    pub fn bluetooth(&self) -> Result<wad_proto::v1::BluetoothStatus, String> {
        self.get("/v1/bluetooth")
    }

    /// "power" or "scan", on or off.
    pub fn bluetooth_switch(&self, what: &str, on: bool) -> Result<(), String> {
        self.post(&format!("/v1/bluetooth/{what}"), json!({ "on": on }))
    }

    /// pair | connect | disconnect | forget.
    pub fn bluetooth_device(&self, address: &str, action: &str) -> Result<(), String> {
        self.post(&format!("/v1/bluetooth/{address}/{action}"), json!({}))
    }

    pub fn bluetooth_answer(&self, id: u64, accept: bool, pin: Option<String>) -> Result<(), String> {
        let mut body = json!({ "id": id, "accept": accept });
        if let Some(p) = pin {
            body["pin"] = p.into();
        }
        self.post("/v1/bluetooth/prompt", body)
    }

    /// Trust one of the owner's devices on the tailnet to manage the machine.
    pub fn tailnet_trust(&self, id: &str) -> Result<(), String> {
        self.put::<Value>(&format!("/v1/tailnet/devices/{id}"), json!({})).map(drop)
    }

    /// "Not now" to a device on the tailnet asking.
    pub fn tailnet_dismiss(&self, id: &str) -> Result<(), String> {
        self.post(&format!("/v1/tailnet/asks/{id}/dismiss"), json!({}))
    }

    /// A workspace's icon (`/v1/workspaces/<id>/icon`), as bytes.
    pub fn bytes(&self, path: &str) -> Option<Vec<u8>> {
        let (status, body) = self.call("GET", path, None, Some(Duration::from_secs(20))).ok()?.text(8 << 20).ok()?;
        (status == 200).then_some(body)
    }

    /// Follows wadd's server-sent events for as long as the HUD runs.
    pub fn follow(&self, tx: async_channel::Sender<Incoming>) {
        let me = self.clone();
        std::thread::spawn(move || {
            loop {
                let _ = me.stream_once(&tx);
                if tx.send_blocking(Incoming::Disconnected).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    fn stream_once(&self, tx: &async_channel::Sender<Incoming>) -> Result<(), String> {
        let a = self.call("GET", "/v1/events", None, None)?;
        if a.status != 200 {
            return Err(format!("status {}", a.status));
        }
        for (name, data) in Sse::new(BufReader::new(a.body)) {
            let value = serde_json::from_str(&data).unwrap_or(Value::Null);
            if tx.send_blocking(Incoming::Event(name, value)).is_err() {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// Reads the status line and headers; the body as sent (chunked or not).
fn read_answer<R: BufRead + Send + 'static>(mut r: R) -> Result<Answer, String> {
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("not an HTTP answer")?;
    let (mut chunked, mut length) = (false, None::<u64>);
    loop {
        line.clear();
        if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("the answer ended early".into());
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked") {
                chunked = true;
            } else if k == "content-length" {
                length = v.parse().ok();
            }
        }
    }
    let body: Box<dyn Read + Send> = if chunked {
        Box::new(Chunked { inner: r, left: 0, done: false })
    } else if let Some(n) = length {
        Box::new(r.take(n))
    } else {
        Box::new(r)
    };
    Ok(Answer { status, body })
}

/// A chunked body, as its plain bytes.
struct Chunked<R> {
    inner: R,
    /// Bytes left in this chunk.
    left: u64,
    done: bool,
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let mut size = String::new();
            if self.inner.read_line(&mut size)? == 0 {
                return Err(bad("the body ended early"));
            }
            let hex = size.trim().split(';').next().unwrap_or("");
            self.left = u64::from_str_radix(hex, 16).map_err(|_| bad("bad chunk size"))?;
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let want = buf.len().min(self.left as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(bad("the body ended early"));
        }
        self.left -= n as u64;
        if self.left == 0 {
            let mut crlf = String::new();
            self.inner.read_line(&mut crlf)?;
        }
        Ok(n)
    }
}

/// Server-sent events from a body: (name, data), until it ends.
struct Sse<R> {
    lines: std::io::Lines<R>,
}

impl<R: BufRead> Sse<R> {
    fn new(r: R) -> Self {
        Self { lines: r.lines() }
    }
}

impl<R: BufRead> Iterator for Sse<R> {
    type Item = (String, String);

    fn next(&mut self) -> Option<Self::Item> {
        let (mut name, mut data) = (String::new(), Vec::<String>::new());
        for line in self.lines.by_ref() {
            let line = line.ok()?;
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                if !data.is_empty() {
                    let ev = if name.is_empty() { "message".to_string() } else { name };
                    return Some((ev, data.join("\n")));
                }
                name.clear();
            } else if let Some(v) = line.strip_prefix("event:") {
                name = v.trim().into();
            } else if let Some(v) = line.strip_prefix("data:") {
                data.push(v.strip_prefix(' ').unwrap_or(v).into());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(raw: &'static [u8]) -> Answer {
        read_answer(BufReader::new(raw)).unwrap()
    }

    #[test]
    fn plain_and_chunked_bodies() {
        let (s, b) = answer(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhelloEXTRA").text(100).unwrap();
        assert_eq!((s, b.as_slice()), (200, &b"hello"[..]));
        let raw = b"HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"me\r\n0e;x=1\r\nssage\":\"gone\"}\r\n0\r\n\r\n";
        let (s, b) = answer(raw).text(100).unwrap();
        assert_eq!(s, 404);
        assert_eq!(Wadd::failure(s, &b), "gone");
    }

    #[test]
    fn events_from_a_chunked_stream() {
        let raw = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n1b\r\nevent: session\ndata: null\n\n\r\n17\r\n: keepalive\n\ndata: {\"a\"\r\n5\r\n:1}\n\n\r\n0\r\n\r\n";
        let a = answer(raw);
        let got: Vec<_> = Sse::new(BufReader::new(a.body)).collect();
        assert_eq!(got, [("session".to_string(), "null".to_string()), ("message".into(), "{\"a\":1}".into())]);
    }

    /// Against a real wadd (no machine under it), on a temporary socket.
    #[tokio::test(flavor = "multi_thread")]
    async fn against_wadd() {
        use wad_config::{Config, Profile};
        let d = tempfile::tempdir().unwrap();
        let sock = d.path().join("wadd.sock");
        let mut cfg = Config::defaults(Profile::User);
        cfg.daemon.state_dir = d.path().join("state");
        cfg.daemon.vendor_workspaces = d.path().join("vendor");
        cfg.daemon.projects_dir = d.path().join("projects");
        cfg.display.enabled = false;
        cfg.cloud.enabled = false;
        let icons = cfg.daemon.state_dir.join("icons");
        std::fs::create_dir_all(&icons).unwrap();
        std::fs::write(icons.join("w.png"), b"\x89PNG-ish").unwrap();
        std::fs::write(icons.join("w-dark.png"), b"\x89PNG-dark").unwrap();
        let ws = serde_json::json!([{"id": "writing", "name": "Writing", "image": "i", "display": "host", "port": null,
            "hotkey": 1, "icon": icons.join("w.png"), "enabled": true, "autostart": false, "containerName": "wad-writing",
            "containerPort": 3000, "env": [], "secrets": [], "volumes": [], "devices": [], "shmSize": null, "projects": []}]);
        std::fs::write(cfg.daemon.state_dir.join("workspaces.json"), ws.to_string()).unwrap();
        let offline = std::sync::Arc::new(wadd::backend::Offline("no machine in tests".into()));
        let server = wadd::Server::new(
            &cfg,
            Profile::User,
            wadd::Listen::Path(sock.clone()),
            wadd::logbuf::LogBuffer::new(100),
            offline,
        )
        .unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(server.run(async move {
            let _ = stopped.await;
        }));
        for _ in 0..100 {
            if sock.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let w = Wadd { socket: sock };
        let got = tokio::task::spawn_blocking(move || {
            let (tx, rx) = async_channel::unbounded();
            w.follow(tx);
            w.home().unwrap();
            assert_eq!(w.bytes("/v1/workspaces/writing/icon").as_deref(), Some(&b"\x89PNG-ish"[..]));
            assert_eq!(w.bytes("/v1/workspaces/nope/icon"), None);
            // A theme's own variant when there's one beside it, else the icon.
            assert_eq!(w.bytes("/v1/workspaces/writing/icon?theme=dark").as_deref(), Some(&b"\x89PNG-dark"[..]));
            assert_eq!(w.bytes("/v1/workspaces/writing/icon?theme=light").as_deref(), Some(&b"\x89PNG-ish"[..]));
            assert_eq!(w.bytes("/v1/workspaces/writing/icon?theme=../x").as_deref(), Some(&b"\x89PNG-ish"[..]));
            // The laptop's own screen is never set from a test: only read.
            w.brightness().unwrap();
            // No NetworkManager here: wadd says so, in words.
            let e = w.wifi_scan().unwrap_err();
            assert!(!e.is_empty() && !e.starts_with("wadd answered"), "{e}");
            // A free session with Writing, which can't run here (no podman):
            // Super+Tab only offers what runs, so with WadSpaces Client alone
            // the switcher doesn't open.
            w.post("/v1/session", json!({"workspaces": ["writing"], "minutes": null})).unwrap();
            w.post("/v1/carousel/next", json!({})).unwrap();
            let mut names = vec![];
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                match rx.try_recv() {
                    Ok(Incoming::Event(name, data)) => {
                        if name == "carousel" {
                            let c: crate::model::Carousel = serde_json::from_value(data.clone()).unwrap();
                            assert!(!c.open, "a switcher with nothing to switch to: {c:?}");
                        }
                        names.push(format!("{name} {data}"));
                    }
                    Ok(Incoming::Disconnected) => names.push("disconnected".into()),
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
            names
        })
        .await
        .unwrap();
        assert!(got.iter().any(|n| n.starts_with("session ")), "{got:?}");
        // The view and the workspaces come too (WadNotes follows them).
        assert!(got.iter().any(|n| n.starts_with("view ")), "{got:?}");
        assert!(got.iter().any(|n| n.starts_with("workspaces ")), "{got:?}");
        drop(stop);
    }
}
