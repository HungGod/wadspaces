//! The machine's secrets: podman's secret store, which the workspaces mount
//! (quadlet's Secret=). Values only ever go to podman; wadd keeps a note of
//! where each one came from (<state_dir>/secrets.json):
//!   - account: synced from the owner's account (users/{uid}/secrets), with
//!     a digest of the value last synced, so only changes are written, and
//!     one deleted in the account is deleted here;
//!   - placeholder: one a workspace names that isn't here yet, so its
//!     container can still start (podman refuses a missing secret); a real
//!     value replaces it;
//!   - local: set on this machine (the app's GitHub sign-in).
//!
//! Linking to another owner forgets everything that came from the last one.
//!
//! Names starting `wad_` are wadd's own (a stream's password, the TURN key):
//! never listed, set, synced or deleted here, nor mounted by a design.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wad_proto::v1::{SecretInfo, SecretOrigin, SecretsSynced};
use wad_proto::{ApiError, ErrorCode};

use crate::backend::Backend;

/// What a placeholder holds (podman won't keep an empty secret): a line that
/// reads as nothing once trimmed.
pub const PLACEHOLDER: &[u8] = b"\n";

/// wadd's own secret (streams.rs): out of everyone else's reach.
pub fn reserved(name: &str) -> bool {
    name.starts_with("wad_")
}

pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn digest(value: &[u8]) -> String {
    Sha256::digest(value).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Book {
    /// name -> digest of the value last synced from the account.
    #[serde(default)]
    account: BTreeMap<String, String>,
    #[serde(default)]
    placeholders: BTreeSet<String>,
}

pub struct Secrets {
    backend: Arc<dyn Backend>,
    file: PathBuf,
    /// One change at a time (the book and podman agree).
    lock: tokio::sync::Mutex<()>,
}

impl Secrets {
    pub fn new(backend: Arc<dyn Backend>, state_dir: &std::path::Path) -> Self {
        Self { backend, file: state_dir.join("secrets.json"), lock: tokio::sync::Mutex::new(()) }
    }

    fn book(&self) -> Book {
        std::fs::read(&self.file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, book: &Book) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(d) = self.file.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = self.file.with_extension("json.tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        f.write_all(&serde_json::to_vec_pretty(book).expect("json")).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.file).map_err(|e| format!("{}: {e}", self.file.display()))
    }

    /// What's here, and where each came from (no values).
    pub async fn list(&self) -> Result<Vec<SecretInfo>, ApiError> {
        let names = self.backend.secret_names().await.map_err(|e| ApiError::new(ErrorCode::Offline, e))?;
        let book = self.book();
        Ok(names
            .into_iter()
            .filter(|name| !reserved(name))
            .map(|name| {
                let origin = if book.placeholders.contains(&name) {
                    SecretOrigin::Placeholder
                } else if book.account.contains_key(&name) {
                    SecretOrigin::Account
                } else {
                    SecretOrigin::Local
                };
                SecretInfo { name, origin }
            })
            .collect())
    }

    /// Sets a secret here (it's this machine's own from now on).
    pub async fn set(&self, name: &str, value: &[u8]) -> Result<(), ApiError> {
        if !valid_name(name) || reserved(name) {
            return Err(ApiError::new(ErrorCode::BadRequest, format!("bad secret name {name:?}")));
        }
        if value.is_empty() {
            return Err(ApiError::new(ErrorCode::BadRequest, "a secret needs a value"));
        }
        let _one = self.lock.lock().await;
        self.backend.create_secret(name, value).await.map_err(|e| ApiError::new(ErrorCode::Upstream, e))?;
        let mut book = self.book();
        book.placeholders.remove(name);
        book.account.remove(name);
        self.save(&book).map_err(|e| ApiError::new(ErrorCode::Internal, e))
    }

    pub async fn delete(&self, name: &str) -> Result<bool, ApiError> {
        if reserved(name) {
            return Err(ApiError::new(ErrorCode::BadRequest, format!("bad secret name {name:?}")));
        }
        let _one = self.lock.lock().await;
        let gone = self.backend.delete_secret(name).await.map_err(|e| ApiError::new(ErrorCode::Upstream, e))?;
        let mut book = self.book();
        book.placeholders.remove(name);
        book.account.remove(name);
        self.save(&book).map_err(|e| ApiError::new(ErrorCode::Internal, e))?;
        Ok(gone)
    }

    /// The account's secrets, into podman: written when new or changed since
    /// the last sync (or missing here); removed when the account no longer
    /// has one this sync put here.
    pub async fn sync_account(&self, account: &[(String, String)]) -> Result<SecretsSynced, String> {
        let _one = self.lock.lock().await;
        let mut book = self.book();
        let here: BTreeSet<String> = self.backend.secret_names().await?.into_iter().collect();
        let mut out = SecretsSynced::default();
        for (name, value) in account {
            if !valid_name(name) || reserved(name) || value.is_empty() {
                continue;
            }
            let d = digest(value.as_bytes());
            if book.account.get(name) == Some(&d) && here.contains(name) && !book.placeholders.contains(name) {
                out.unchanged.push(name.clone());
                continue;
            }
            self.backend.create_secret(name, value.as_bytes()).await?;
            book.placeholders.remove(name);
            if book.account.insert(name.clone(), d).is_some() {
                out.updated.push(name.clone())
            } else {
                out.added.push(name.clone())
            }
        }
        let gone: Vec<String> =
            book.account.keys().filter(|n| !account.iter().any(|(a, _)| a == *n)).cloned().collect();
        for name in gone {
            self.backend.delete_secret(&name).await?;
            book.account.remove(&name);
            out.removed.push(name);
        }
        self.save(&book)?;
        Ok(out)
    }

    /// Placeholders for the secrets these names ask for that aren't here.
    pub async fn ensure(&self, needed: &[String]) -> Result<Vec<String>, String> {
        let _one = self.lock.lock().await;
        let here: BTreeSet<String> = self.backend.secret_names().await?.into_iter().collect();
        let mut book = self.book();
        let mut made = vec![];
        for name in needed.iter().filter(|n| valid_name(n) && !reserved(n) && !here.contains(*n)) {
            self.backend.create_secret(name, PLACEHOLDER).await?;
            book.placeholders.insert(name.clone());
            made.push(name.clone());
        }
        if !made.is_empty() {
            tracing::info!("placeholder secrets for {} (set them in WadSpaces)", made.join(", "));
            self.save(&book)?;
        }
        Ok(made)
    }

    /// Another owner now: everything the last one's account put here goes,
    /// and their GitHub token.
    pub async fn forget_account(&self) -> Result<Vec<String>, String> {
        let _one = self.lock.lock().await;
        let mut book = self.book();
        let mut gone = vec![];
        let mut names: Vec<String> = book.account.keys().cloned().collect();
        if !names.iter().any(|n| n == "github_token") {
            names.push("github_token".into());
        }
        for name in names {
            if self.backend.delete_secret(&name).await? {
                gone.push(name.clone());
            }
            book.account.remove(&name);
        }
        self.save(&book)?;
        Ok(gone)
    }
}
