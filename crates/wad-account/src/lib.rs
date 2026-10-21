//! The signed-in person's WadSpaces account, from the native client: they
//! sign in to Firebase with their GitHub token (the same account, and uid,
//! as the web app gives them), and everything here acts as them, so
//! Firestore's rules apply exactly as they do to the web app.
//!
//! The session (its refresh token) is kept for this boot only, in
//! $XDG_RUNTIME_DIR: the app restarting keeps you signed in, a reboot
//! doesn't. Nothing of it goes to wadd.

pub mod config;
pub mod library;
pub mod signin;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use wad_firebase::{Firestore, Identity, Write};

pub use config::Config;
pub use library::Library;
pub use wad_firebase::values::rfc3339;

/// Usernames: what the rules allow.
pub fn username_ok(name: &str) -> bool {
    (3..=24).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
}

/// The avatar colours a new profile gets one of.
const COLORS: [&str; 6] = ["#c6ff1f", "#ff3d81", "#7c4dff", "#1de9b6", "#ffb020", "#40c4ff"];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Sign in first.")]
    SignedOut,
    #[error("New accounts are closed. Ask the owner to let your GitHub account in.")]
    Closed,
    #[error("This account is disabled.")]
    Disabled,
    #[error("Too many tries. Wait a minute and try again.")]
    TooMany,
    #[error("Can't reach the sign-in service. Check the connection.")]
    Offline,
    #[error("That username is taken.")]
    Taken,
    #[error("Usernames are 3-24 letters, digits, dots, dashes or underscores.")]
    BadUsername,
    /// A document that can't be saved as it is.
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Firebase(#[from] wad_firebase::Error),
}

impl Error {
    fn from_auth(e: wad_firebase::Error) -> Error {
        match &e {
            wad_firebase::Error::Network(_) => Error::Offline,
            wad_firebase::Error::Auth { message, .. } => match message.split(':').next().unwrap_or("").trim() {
                "ADMIN_ONLY_OPERATION" | "OPERATION_NOT_ALLOWED" => Error::Closed,
                "USER_DISABLED" => Error::Disabled,
                "TOO_MANY_ATTEMPTS_TRY_LATER" => Error::TooMany,
                "TOKEN_EXPIRED" | "INVALID_REFRESH_TOKEN" | "USER_NOT_FOUND" => Error::SignedOut,
                _ => Error::Firebase(e),
            },
            _ => Error::Firebase(e),
        }
    }
}

/// Who's signed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub uid: String,
    /// GitHub's numeric id for them.
    pub github_id: String,
    /// Their GitHub login.
    pub login: Option<String>,
    pub display_name: Option<String>,
    pub photo_url: Option<String>,
}

/// Their WadSpaces profile (profiles/{uid}).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    pub username: String,
    pub display_name: String,
    pub color: String,
    pub photo_url: Option<String>,
    pub onboarded: bool,
    /// What was last launched (the web app's LastSession): Jump back in.
    pub last_session: Option<Value>,
}

impl Profile {
    fn from_doc(d: &Value) -> Profile {
        let f = wad_firebase::values::from_fields(&d["fields"]);
        let s = |k: &str| f.get(k).and_then(Value::as_str).map(String::from);
        Profile {
            username: s("username").unwrap_or_default(),
            display_name: s("displayName").unwrap_or_default(),
            color: s("color").unwrap_or_else(|| "#999999".into()),
            photo_url: s("photoURL"),
            onboarded: f.get("onboarded").and_then(Value::as_bool).unwrap_or(true),
            last_session: f.get("lastSession").filter(|v| v.is_object()).cloned(),
        }
    }
}

/// What's kept for the boot: who, and the refresh token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    user: User,
    refresh_token: String,
}

struct Session {
    kept: Kept,
    id_token: String,
    expires: Instant,
}

pub struct Account {
    cfg: Config,
    http: reqwest::Client,
    identity: Identity,
    fs: Firestore,
    file: Option<PathBuf>,
    session: Mutex<Option<Session>>,
}

impl Account {
    /// With the session kept in `file` (None: not kept).
    pub fn new(cfg: Config, file: Option<PathBuf>) -> Result<Account, Error> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().expect("a client");
        let (identity, fs) = match &cfg.emulator {
            Some(host) => (
                Identity::emulator(http.clone(), &format!("{host}:9099")),
                Firestore::with_base(http.clone(), &format!("http://{host}:8090"), &cfg.project_id)?,
            ),
            None => (Identity::new(http.clone()), Firestore::new(http.clone(), &cfg.project_id)?),
        };
        Ok(Self::with(cfg, http, identity, fs, file))
    }

    /// Against test servers.
    pub fn with(
        cfg: Config,
        http: reqwest::Client,
        identity: Identity,
        fs: Firestore,
        file: Option<PathBuf>,
    ) -> Account {
        Account { cfg, http, identity, fs, file, session: Mutex::new(None) }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn user(&self) -> Option<User> {
        self.session.lock().unwrap().as_ref().map(|s| s.kept.user.clone())
    }

    /// Signs in with a GitHub access token (the sign-in page's).
    pub async fn sign_in_github(&self, access_token: &str) -> Result<User, Error> {
        let got = self
            .identity
            .sign_in_with_idp(&self.cfg.api_key, "github.com", access_token)
            .await
            .map_err(Error::from_auth)?;
        let user = User {
            uid: got.uid,
            github_id: got.provider_uid,
            login: got.screen_name,
            display_name: got.display_name,
            photo_url: got.photo_url,
        };
        self.keep(
            Kept { user: user.clone(), refresh_token: got.tokens.refresh_token },
            got.tokens.id_token,
            got.tokens.expires_in,
        );
        tracing::info!(uid = %user.uid, login = ?user.login, "signed in");
        Ok(user)
    }

    /// Against the emulators only: signs in as a made-up GitHub user (the
    /// Auth emulator can't take GitHub's tokens).
    pub async fn sign_in_emulated(&self, login: &str, github_id: &str) -> Result<User, Error> {
        if self.cfg.emulator.is_none() {
            return Err(Error::Firebase(wad_firebase::Error::BadProject("not the emulators".into())));
        }
        let claims = json!({ "sub": github_id, "screen_name": login, "name": login });
        let got = self
            .identity
            .sign_in_with_fake_idp(&self.cfg.api_key, "github.com", &claims)
            .await
            .map_err(Error::from_auth)?;
        let user = User {
            uid: got.uid,
            github_id: got.provider_uid,
            login: got.screen_name,
            display_name: got.display_name,
            photo_url: got.photo_url,
        };
        self.keep(
            Kept { user: user.clone(), refresh_token: got.tokens.refresh_token },
            got.tokens.id_token,
            got.tokens.expires_in,
        );
        Ok(user)
    }

    /// Signed in earlier this boot (the app restarted): back in, if the
    /// session still holds.
    pub async fn restore(&self) -> Option<User> {
        let kept: Kept = serde_json::from_slice(&std::fs::read(self.file.as_ref()?).ok()?).ok()?;
        match self.identity.refresh(&self.cfg.api_key, &kept.refresh_token).await {
            Ok(t) => {
                let user = kept.user.clone();
                self.keep(Kept { refresh_token: t.refresh_token, ..kept }, t.id_token, t.expires_in);
                Some(user)
            }
            Err(wad_firebase::Error::Network(_)) => {
                // Offline: still them; the token is fetched when it's needed.
                let user = kept.user.clone();
                *self.session.lock().unwrap() =
                    Some(Session { kept, id_token: String::new(), expires: Instant::now() });
                Some(user)
            }
            Err(e) => {
                tracing::info!("the last sign-in no longer holds: {e}");
                self.forget();
                None
            }
        }
    }

    pub fn sign_out(&self) {
        self.forget();
    }

    fn forget(&self) {
        self.session.lock().unwrap().take();
        if let Some(f) = &self.file {
            let _ = std::fs::remove_file(f);
        }
    }

    fn keep(&self, kept: Kept, id_token: String, expires_in: u64) {
        if let Some(f) = &self.file
            && let Err(e) = write_private(f, &serde_json::to_vec(&kept).expect("JSON"))
        {
            tracing::warn!("{}: {e}", f.display());
        }
        let expires = Instant::now() + Duration::from_secs(expires_in);
        *self.session.lock().unwrap() = Some(Session { kept, id_token, expires });
    }

    /// An ID token good for a few minutes more (refreshed when it isn't).
    pub async fn id_token(&self) -> Result<String, Error> {
        let (refresh, fresh) = {
            let s = self.session.lock().unwrap();
            let s = s.as_ref().ok_or(Error::SignedOut)?;
            (
                s.kept.refresh_token.clone(),
                (s.expires > Instant::now() + Duration::from_secs(300)).then(|| s.id_token.clone()),
            )
        };
        if let Some(t) = fresh {
            return Ok(t);
        }
        let t = self.identity.refresh(&self.cfg.api_key, &refresh).await.map_err(Error::from_auth)?;
        let mut s = self.session.lock().unwrap();
        let s = s.as_mut().ok_or(Error::SignedOut)?;
        s.kept.refresh_token = t.refresh_token;
        s.id_token = t.id_token.clone();
        s.expires = Instant::now() + Duration::from_secs(t.expires_in);
        if let Some(f) = &self.file {
            let _ = write_private(f, &serde_json::to_vec(&s.kept).expect("JSON"));
        }
        Ok(t.id_token)
    }

    fn uid(&self) -> Result<String, Error> {
        self.user().map(|u| u.uid).ok_or(Error::SignedOut)
    }

    /// Their profile; None until they've a username.
    pub async fn profile(&self) -> Result<Option<Profile>, Error> {
        let (uid, tok) = (self.uid()?, self.id_token().await?);
        Ok(self.fs.get(&tok, &format!("profiles/{uid}")).await?.as_ref().map(Profile::from_doc))
    }

    /// First sign-in: a profile named after the GitHub login when that's a
    /// free username (else None, and they pick one).
    pub async fn ensure_profile(&self) -> Result<Option<Profile>, Error> {
        if let Some(p) = self.profile().await? {
            return Ok(Some(p));
        }
        let user = self.user().ok_or(Error::SignedOut)?;
        let want = self.suggested_username();
        if !username_ok(&want) || self.username_taken(&want).await? {
            return Ok(None);
        }
        let name = user.display_name.or(user.login).unwrap_or_else(|| want.clone());
        match self.claim_username(&want, &name).await {
            Ok(p) => Ok(Some(p)),
            Err(Error::Taken) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Their GitHub login as a username would be.
    pub fn suggested_username(&self) -> String {
        let login = self.user().and_then(|u| u.login).unwrap_or_default().to_lowercase();
        login.chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(*c)).take(24).collect()
    }

    /// Someone else has it.
    pub async fn username_taken(&self, name: &str) -> Result<bool, Error> {
        let tok = self.id_token().await?;
        let me = self.uid()?;
        Ok(match self.fs.get(&tok, &format!("usernames/{}", name.to_lowercase())).await? {
            Some(d) => {
                wad_firebase::values::from_fields(&d["fields"]).get("uid").and_then(Value::as_str) != Some(me.as_str())
            }
            None => false,
        })
    }

    /// They've had the tour (or skipped it): it doesn't start again.
    pub async fn set_onboarded(&self) -> Result<(), Error> {
        let user = self.user().ok_or(Error::SignedOut)?;
        let tok = self.id_token().await?;
        let mut data = serde_json::Map::new();
        data.insert("onboarded".into(), true.into());
        Ok(self.fs.patch(&tok, &format!("profiles/{}", user.uid), &data, &[]).await?)
    }

    /// Claims a username: usernames/{name} and profiles/{uid} together, or
    /// neither (someone took it first).
    pub async fn claim_username(&self, username: &str, display_name: &str) -> Result<Profile, Error> {
        let name = username.trim().to_lowercase();
        if !username_ok(&name) {
            return Err(Error::BadUsername);
        }
        let user = self.user().ok_or(Error::SignedOut)?;
        if self.username_taken(&name).await? {
            return Err(Error::Taken);
        }
        let color = COLORS[user.uid.chars().map(|c| c as usize).sum::<usize>() % COLORS.len()];
        let display = Some(display_name.trim()).filter(|d| !d.is_empty()).unwrap_or(&name).to_string();
        let obj = |v: Value| -> Map<String, Value> { v.as_object().cloned().unwrap_or_default() };
        let writes = [
            // Only if it's free (or already theirs, from a try that half-failed).
            Write::set(format!("usernames/{name}"), obj(json!({ "uid": user.uid }))),
            Write::set(
                format!("profiles/{}", user.uid),
                obj(json!({
                    "username": name, "displayName": display, "color": color,
                    "photoURL": user.photo_url, "onboarded": false,
                })),
            )
            .now("createdAt"),
        ];
        let tok = self.id_token().await?;
        self.fs.commit(&tok, &writes).await.map_err(|e| match e {
            wad_firebase::Error::Denied | wad_firebase::Error::Precondition(_) => Error::Taken,
            e => Error::Firebase(e),
        })?;
        Ok(Profile {
            username: name,
            display_name: display,
            color: color.into(),
            photo_url: user.photo_url,
            onboarded: false,
            last_session: None,
        })
    }

    /// A one-time code for linking a machine (enrollMachine redeems it
    /// within 15 minutes; wadd's /v1/cloud/link takes it).
    pub async fn create_enroll_code(&self, machine_name: &str) -> Result<String, Error> {
        let uid = self.uid()?;
        let code = enroll_code();
        let data = json!({ "uid": uid, "used": false, "machineName": machine_name });
        let tok = self.id_token().await?;
        self.fs
            .commit(
                &tok,
                &[Write::set(format!("enrollCodes/{code}"), data.as_object().cloned().unwrap_or_default())
                    .now("createdAt")],
            )
            .await?;
        Ok(code)
    }

    /// A command for one of their machines (its relay runs it within a few
    /// seconds): start, stop, restart a wadspace there, and the like.
    pub async fn send_command(&self, machine_id: &str, kind: &str, ws_id: Option<&str>) -> Result<String, Error> {
        let uid = self.uid()?;
        let id = doc_id_random();
        let mut data = json!({ "type": kind, "status": "pending" });
        if let Some(w) = ws_id {
            data["wsId"] = w.into();
        }
        let path = format!("users/{uid}/machines/{machine_id}/commands/{id}");
        let tok = self.id_token().await?;
        self.fs
            .commit(&tok, &[Write::set(path, data.as_object().cloned().unwrap_or_default()).now("createdAt")])
            .await?;
        Ok(id)
    }

    /// Their GitHub token, for their other machines (users/{uid}/secrets).
    pub async fn save_github_token(&self, token: &str) -> Result<(), Error> {
        let (uid, tok) = (self.uid()?, self.id_token().await?);
        self.fs.set_strings(&tok, &format!("users/{uid}/secrets/github_token"), &[("value", token)]).await?;
        Ok(())
    }

    /// For the library and relay calls to come.
    pub fn firestore(&self) -> &Firestore {
        &self.fs
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }
}

/// A new document's id, as Firestore makes them (20 letters and digits).
fn doc_id_random() -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 20];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut bytes);
    }
    bytes.iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect()
}

/// Eight characters with no 0/O or 1/I (the web app's newEnrollCode).
fn enroll_code() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut bytes);
    }
    bytes.iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect()
}

/// Written so only its owner can read it.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests;
