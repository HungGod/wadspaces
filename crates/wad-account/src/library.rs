//! The person's library from their account, as the native client shows it:
//! the Builder's documents (templates, Extensions layers, saved WadBrowser
//! setups, and their wadspaces: version 3's and design B's version 4, kept
//! apart), and their machines' heartbeats. Read as them (the rules decide),
//! kept on disk for the next start (offline too). Documents stay JSON:
//! wad-core's layers and v4 code reads them as the web app's wasm does.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wad_firebase::values::{doc_id, from_fields};

use crate::{Account, Error};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Library {
    pub templates: Vec<Value>,
    pub extensions: Vec<Value>,
    /// Extensions layers from before they had that name (read as Extensions).
    pub customizations: Vec<Value>,
    pub browsers: Vec<Value>,
    /// Version 3 wadspaces (the board's stacks: a template chain and an
    /// Extensions layer).
    pub wadspaces: Vec<Value>,
    /// Version 4 wadspaces (design B's Builder: each its own templates, on
    /// one machine). The same collection; apart here so the old board never
    /// sees them.
    #[serde(default)]
    pub v4: Vec<Value>,
    /// Each machine's last heartbeat (users/{uid}/machines).
    pub machines: Vec<Value>,
}

/// A version 4 wadspace's document (wad_core::v4::is_v4).
pub fn is_v4(w: &Value) -> bool {
    w.get("version").and_then(Value::as_u64) == Some(4)
}

impl Library {
    /// The shape wad-core's layers functions take: `{templates, extensions}`
    /// (the old customizations counted as Extensions).
    pub fn lib(&self) -> Value {
        let mut ext = self.extensions.clone();
        ext.extend(self.customizations.iter().filter(|c| !self.extensions.iter().any(|e| e["id"] == c["id"])).cloned());
        json!({ "templates": self.templates, "extensions": ext, "browsers": self.browsers })
    }

    /// Every wadspace, version 3's then version 4's.
    pub fn all_wadspaces(&self) -> impl Iterator<Item = &Value> {
        self.wadspaces.iter().chain(&self.v4)
    }

    /// The list a wadspace's document belongs in, by its version.
    pub fn wadspaces_for(&mut self, w: &Value) -> &mut Vec<Value> {
        if is_v4(w) { &mut self.v4 } else { &mut self.wadspaces }
    }
}

/// A document as the web app has it: its fields, and its id.
pub fn plain(doc: &Value) -> Value {
    let mut v = from_fields(&doc["fields"]);
    if let Value::Object(o) = &mut v {
        o.insert("id".into(), doc_id(doc).into());
    }
    v
}

impl Account {
    /// Everything, read now (and kept for next time).
    pub async fn library(&self) -> Result<Library, Error> {
        let uid = self.user().map(|u| u.uid).ok_or(Error::SignedOut)?;
        let tok = self.id_token().await?;
        let fs = self.firestore();
        let list = |c: &'static str| {
            let (tok, uid) = (tok.clone(), uid.clone());
            async move { fs.list(&tok, &format!("users/{uid}/{c}")).await.map(|d| d.iter().map(plain).collect::<Vec<_>>()) }
        };
        let owned = json!({
            "from": [{ "collectionId": "wadspaces" }],
            "where": { "fieldFilter": { "field": { "fieldPath": "owner" }, "op": "EQUAL", "value": { "stringValue": uid } } },
        });
        let (templates, extensions, customizations, browsers, machines, wadspaces) = tokio_join(
            list("templates"),
            list("extensions"),
            list("customizations"),
            list("browsers"),
            list("machines"),
            async { fs.query_root(&tok, owned).await.map(|d| d.iter().map(plain).collect::<Vec<_>>()) },
        )
        .await;
        let (v4, wadspaces): (Vec<Value>, Vec<Value>) = wadspaces?.into_iter().partition(is_v4);
        let lib = Library {
            templates: templates?,
            extensions: extensions?,
            customizations: customizations?,
            browsers: browsers?,
            wadspaces,
            v4,
            machines: machines?,
        };
        if let Some(p) = self.library_file() {
            let _ = crate::write_private(&p, &serde_json::to_vec(&lib).expect("JSON"));
        }
        Ok(lib)
    }

    /// The machines' heartbeats alone (the Machines page reads them often).
    pub async fn machines(&self) -> Result<Vec<Value>, Error> {
        let uid = self.user().map(|u| u.uid).ok_or(Error::SignedOut)?;
        let tok = self.id_token().await?;
        Ok(self.firestore().list(&tok, &format!("users/{uid}/machines")).await?.iter().map(plain).collect())
    }

    /// A wadspace (one of the board's stacks) saved as it is: at the top
    /// level, theirs, shared with no one (as the web app keeps them).
    pub async fn save_wadspace(&self, w: &Value) -> Result<(), Error> {
        let uid = self.user().map(|u| u.uid).ok_or(Error::SignedOut)?;
        let mut data = w.as_object().cloned().unwrap_or_default();
        let id = data.remove("id").and_then(|v| v.as_str().map(String::from)).ok_or(Error::Invalid("no id".into()))?;
        data.insert("owner".into(), uid.into());
        data.insert("sharedWith".into(), json!([]));
        let tok = self.id_token().await?;
        Ok(self.firestore().set(&tok, &format!("wadspaces/{id}"), &data).await?)
    }

    /// One of their documents (`templates`, `extensions`, `browsers`) saved
    /// as it is.
    pub async fn save_doc(&self, collection: &str, d: &Value) -> Result<(), Error> {
        let uid = self.user().map(|u| u.uid).ok_or(Error::SignedOut)?;
        let mut data = d.as_object().cloned().unwrap_or_default();
        let id = data.remove("id").and_then(|v| v.as_str().map(String::from)).ok_or(Error::Invalid("no id".into()))?;
        let tok = self.id_token().await?;
        Ok(self.firestore().set(&tok, &format!("users/{uid}/{collection}/{id}"), &data).await?)
    }

    /// One of their documents (`templates`, `extensions`, `browsers`)
    /// deleted for good.
    pub async fn delete_doc(&self, collection: &str, id: &str) -> Result<(), Error> {
        let uid = self.user().map(|u| u.uid).ok_or(Error::SignedOut)?;
        let tok = self.id_token().await?;
        Ok(self.firestore().delete(&tok, &format!("users/{uid}/{collection}/{id}")).await?)
    }

    /// A wadspace deleted for good.
    pub async fn delete_wadspace(&self, id: &str) -> Result<(), Error> {
        let tok = self.id_token().await?;
        Ok(self.firestore().delete(&tok, &format!("wadspaces/{id}")).await?)
    }

    /// The library as last read on this machine (for a quick start, or offline).
    pub fn cached_library(&self) -> Option<Library> {
        serde_json::from_slice(&std::fs::read(self.library_file()?).ok()?).ok()
    }

    /// ~/.local/share/io.wadspaces.client/account/<uid>/library.json
    fn library_file(&self) -> Option<PathBuf> {
        self.data_file("library.json")
    }

    /// A file of theirs on this machine:
    /// ~/.local/share/io.wadspaces.client/account/<uid>/<name>.
    pub fn data_file(&self, name: &str) -> Option<PathBuf> {
        let uid = self.user()?.uid;
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(data.join("io.wadspaces.client/account").join(uid).join(name))
    }
}

/// Six reads at once.
async fn tokio_join<A, B, C, D, E, F>(
    a: impl Future<Output = A>,
    b: impl Future<Output = B>,
    c: impl Future<Output = C>,
    d: impl Future<Output = D>,
    e: impl Future<Output = E>,
    f: impl Future<Output = F>,
) -> (A, B, C, D, E, F) {
    futures_util::join!(a, b, c, d, e, f)
}
