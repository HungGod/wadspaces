//! The Cloudflare tunnel streams reach browsers through: cloudflared (its
//! systemd unit, the image's) carries https://*.<domain> from Cloudflare to
//! wadd's streaming proxy (proxy.rs) on this machine, which has no address
//! the internet can reach.
//!
//! The tunnel's token is /etc/cloudflared/token (baked into the image; a
//! new one can be put there through the API) and only ever that file: wadd
//! reads it for the tunnel's id and writes it when it's replaced, and never
//! logs it or hands it out.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::Value;
use wad_proto::v1::{SelfTest, TunnelStatus};
use wad_proto::{ApiError, ErrorCode};
use wad_systemd::Systemd;

/// The tunnel's token: the only place it's kept.
pub const TOKEN_FILE: &str = "/etc/cloudflared/token";
/// cloudflared, from Cloudflare's RPM.
pub const BINARY: &str = "/usr/bin/cloudflared";
pub const UNIT: &str = "cloudflared.service";
/// The proxy answers this host itself: the self-test fetches it from here,
/// through Cloudflare and the tunnel.
pub const SELFTEST_HOST: &str = "ws-selftest";
/// The proxy's answer to the self-test carries this header:
/// `<instance>:<nonce>`.
pub const SELFTEST_HEADER: &str = "x-wadspaces-selftest";

pub struct Tunnel {
    domain: String,
    /// cloudflared's metrics server (host:port).
    metrics: String,
    webrtc: bool,
    token_file: PathBuf,
    /// This run of wadd's proxy (the self-test tells it from another).
    instance: String,
    /// Where the self-test goes: its URL and the Host to send (tests point
    /// it at a local proxy).
    selftest: (String, Option<String>),
    /// Root, with the system manager: the token can be replaced and
    /// cloudflared restarted.
    can_change: bool,
    http: reqwest::Client,
    systemd: tokio::sync::OnceCell<Option<Systemd>>,
}

impl Tunnel {
    pub fn new(settings: &wad_config::Streaming, instance: String, can_change: bool) -> Self {
        Self {
            domain: settings.domain.clone(),
            metrics: settings.metrics.clone(),
            webrtc: settings.webrtc,
            token_file: TOKEN_FILE.into(),
            instance,
            selftest: (format!("https://{SELFTEST_HOST}.{}/", settings.domain), None),
            can_change,
            http: reqwest::Client::new(),
            systemd: tokio::sync::OnceCell::new(),
        }
    }

    /// Tests only: another token file and self-test target.
    pub fn for_tests(mut self, token_file: PathBuf, selftest: (String, Option<String>)) -> Self {
        self.token_file = token_file;
        self.selftest = selftest;
        self
    }

    /// The system manager, where cloudflared's unit is (from a laptop's
    /// wadd too: reading its state needs no rights).
    async fn systemd(&self) -> Option<&Systemd> {
        self.systemd
            .get_or_init(|| async {
                Systemd::system().await.inspect_err(|e| tracing::debug!("the system manager: {e}")).ok()
            })
            .await
            .as_ref()
    }

    pub async fn status(&self) -> TunnelStatus {
        let token_present = self.token_file.exists();
        let tunnel_id = std::fs::read_to_string(&self.token_file).ok().and_then(|t| tunnel_id(&t));
        let service = match self.systemd().await {
            Some(s) => s.active_state(UNIT).await.unwrap_or_else(|_| "unknown".into()),
            None => "unknown".into(),
        };
        let connections = self.connections().await;
        let connected = connections.is_some_and(|n| n > 0);
        TunnelStatus {
            cloudflared_installed: Path::new(BINARY).exists(),
            token_present,
            tunnel_id,
            ready: token_present && service == "active" && connected,
            service,
            connected,
            connections: connections.unwrap_or(0),
            domain: self.domain.clone(),
            webrtc: self.webrtc,
        }
    }

    /// cloudflared's links to Cloudflare (its /ready); None: it didn't answer.
    async fn connections(&self) -> Option<u32> {
        let url = format!("http://{}/ready", self.metrics);
        let r = self.http.get(&url).timeout(Duration::from_secs(2)).send().await.ok()?;
        let ok = r.status().is_success();
        let v: Value = r.json().await.ok()?;
        let n = v.get("readyConnections").and_then(Value::as_u64).unwrap_or(0) as u32;
        Some(if ok { n } else { 0 })
    }

    /// Fetches https://ws-selftest.<domain>/ from here three times: through
    /// Cloudflare and the tunnel back to this machine's proxy. Each answer
    /// must come from this wadd (another connector with the same token would
    /// get some of the traffic).
    pub async fn selftest(&self) -> SelfTest {
        let mut slowest = 0;
        let mut status = None;
        for _ in 0..3 {
            let nonce = crate::streams::random_hex(16);
            let mut req = self.http.get(format!("{}?n={nonce}", self.selftest.0)).timeout(Duration::from_secs(10));
            if let Some(host) = &self.selftest.1 {
                req = req.header(reqwest::header::HOST, host);
            }
            let t = Instant::now();
            let res = req.send().await;
            slowest = slowest.max(t.elapsed().as_millis() as u64);
            let r = match res {
                Ok(r) => r,
                Err(e) => return failed(None, slowest, why_unreachable(&e, &self.domain)),
            };
            status = Some(r.status().as_u16());
            let mark = r.headers().get(SELFTEST_HEADER).and_then(|v| v.to_str().ok()).map(String::from);
            match (r.status().as_u16(), mark) {
                (200, Some(m)) if m == format!("{}:{nonce}", self.instance) => {}
                (200, Some(m)) if m.ends_with(&format!(":{nonce}")) => {
                    return failed(
                        status,
                        slowest,
                        "Another machine answered too: something else runs this tunnel's connector with the same \
                         token (stop it, or replace the token)."
                            .into(),
                    );
                }
                (200, _) => {
                    return failed(
                        status,
                        slowest,
                        format!("{SELFTEST_HOST}.{} answered, but not as this machine's proxy.", self.domain),
                    );
                }
                (530, _) => {
                    return failed(
                        status,
                        slowest,
                        "Cloudflare can't reach the tunnel: cloudflared isn't connected.".into(),
                    );
                }
                (502..=504, _) => {
                    return failed(
                        status,
                        slowest,
                        "The tunnel is up, but this machine's streaming proxy didn't answer it (the tunnel's route \
                         should be http://127.0.0.1:8085)."
                            .into(),
                    );
                }
                (404, _) => {
                    return failed(
                        status,
                        slowest,
                        "Something else answered with 404: is the tunnel's route this proxy?".into(),
                    );
                }
                (s, _) => return failed(status, slowest, format!("Cloudflare answered {s}.")),
            }
        }
        SelfTest {
            ok: true,
            status,
            ms: slowest,
            detail: format!("{SELFTEST_HOST}.{} reached this machine through the tunnel.", self.domain),
        }
    }

    /// Puts a new token in /etc/cloudflared/token (the bare token, or the
    /// `cloudflared service install <token>` line Cloudflare shows) and
    /// restarts cloudflared with it.
    pub async fn replace_token(&self, input: &str) -> Result<TunnelStatus, ApiError> {
        if !self.can_change {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "only the machine's own wadd can change the tunnel's token",
            ));
        }
        let token = token_in(input).ok_or_else(|| {
            ApiError::new(ErrorCode::BadRequest, "that isn't a tunnel token (Cloudflare shows it as a long eyJ… line)")
        })?;
        write_token(&self.token_file, &token).map_err(|e| ApiError::new(ErrorCode::Internal, e))?;
        tracing::info!("the tunnel's token was replaced: restarting cloudflared");
        let systemd = self.systemd().await.ok_or_else(|| ApiError::new(ErrorCode::Offline, "no system manager"))?;
        systemd
            .restart(UNIT, Duration::from_secs(30))
            .await
            .map_err(|e| ApiError::new(ErrorCode::Internal, format!("restarting cloudflared: {e}")))?;
        Ok(self.status().await)
    }
}

fn failed(status: Option<u16>, ms: u64, detail: String) -> SelfTest {
    SelfTest { ok: false, status, ms, detail }
}

fn why_unreachable(e: &reqwest::Error, domain: &str) -> String {
    let text = format!("{e:?}");
    if e.is_timeout() {
        format!("{SELFTEST_HOST}.{domain} didn't answer within 10 s.")
    } else if text.contains("dns") || text.contains("failed to lookup") {
        format!(
            "{SELFTEST_HOST}.{domain} has no address: its DNS (the *.{domain} record) isn't there, or this machine is offline."
        )
    } else if e.is_connect() {
        "Couldn't connect to Cloudflare: is this machine online?".into()
    } else {
        format!("The request failed: {e}")
    }
}

/// What a tunnel token holds: base64 of {"a": account, "t": tunnel id,
/// "s": secret}. The tunnel's id, if it is one.
pub fn tunnel_id(token: &str) -> Option<String> {
    let token = token.trim();
    let bytes = [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        &base64::engine::general_purpose::URL_SAFE,
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ]
    .iter()
    .find_map(|e| e.decode(token).ok())?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let field = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    field("a")?;
    field("s")?;
    let t = field("t")?;
    let uuid = t.len() == 36
        && t.chars()
            .enumerate()
            .all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() });
    uuid.then(|| t.to_lowercase())
}

/// The token in what was pasted: the whole of it, or the word in it that
/// is one (the `cloudflared service install <token>` line).
pub fn token_in(input: &str) -> Option<String> {
    input.split_whitespace().rev().find(|w| tunnel_id(w).is_some()).map(String::from)
}

/// Writes the token file whole or not at all: a new file beside it (0600,
/// root's), synced, then renamed over it.
fn write_token(path: &Path, token: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("no folder for the token")?;
    let tmp = dir.join(format!(".token.{}", crate::streams::random_hex(4)));
    let r = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(token.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(t: &str) -> String {
        let json = format!(r#"{{"a":"0123456789abcdef0123456789abcdef","t":"{t}","s":"c2VjcmV0c2VjcmV0c2VjcmV0"}}"#);
        base64::engine::general_purpose::STANDARD.encode(json)
    }

    #[test]
    fn the_tunnel_id_comes_from_the_token() {
        let id = "8f0e5a2c-1b3d-4e6f-9a7b-0c1d2e3f4a5b";
        assert_eq!(tunnel_id(&token(id)).as_deref(), Some(id));
        assert_eq!(tunnel_id(&format!("  {}\n", token(id))).as_deref(), Some(id));
        assert_eq!(tunnel_id(&token("not-a-uuid")), None);
        assert_eq!(tunnel_id("eyJhIjoiYiJ9"), None); // {"a":"b"}
        assert_eq!(tunnel_id("hello"), None);
        assert_eq!(tunnel_id(""), None);
    }

    #[test]
    fn a_pasted_install_line_gives_its_token() {
        let t = token("8f0e5a2c-1b3d-4e6f-9a7b-0c1d2e3f4a5b");
        assert_eq!(token_in(&t).as_deref(), Some(t.as_str()));
        assert_eq!(token_in(&format!("sudo cloudflared service install {t}\n")).as_deref(), Some(t.as_str()));
        assert_eq!(token_in("cloudflared service install nope"), None);
    }

    #[test]
    fn the_token_is_written_whole_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        std::fs::write(&p, "old").unwrap();
        write_token(&p, "new-token").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new-token\n");
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        // Nothing left beside it.
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn status_without_cloudflared_or_a_token() {
        let d = tempfile::tempdir().unwrap();
        let s = wad_config::Streaming { metrics: "127.0.0.1:9".into(), ..Default::default() };
        let t =
            Tunnel::new(&s, "i".into(), false).for_tests(d.path().join("token"), ("http://127.0.0.1:9/".into(), None));
        let st = t.status().await;
        assert!(!st.token_present && !st.connected && !st.ready);
        assert_eq!((st.tunnel_id, st.connections, st.domain.as_str()), (None, 0, "wadspaces.com"));
        let e = t.replace_token(&token("8f0e5a2c-1b3d-4e6f-9a7b-0c1d2e3f4a5b")).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::Forbidden);
    }
}
