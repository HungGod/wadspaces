//! The person's library from their account, as the native client shows it:
//! their wadspaces (version 4 documents: each its own templates, on one
//! machine), their saved WadBrowser setups, and their machines' heartbeats.
//! Read as them (the rules decide), kept on disk for the next start (offline
//! too). Documents stay JSON: wad-core's v4 code reads them as the web app's
//! wasm does.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wad_firebase::values::{doc_id, from_fields};

use crate::{Account, Error};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Library {
    pub browsers: Vec<Value>,
    /// Their wadspaces (version 4: anything older is never shown).
    pub wadspaces: Vec<Value>,
    /// Each machine's last heartbeat (users/{uid}/machines).
    pub machines: Vec<Value>,
}

/// A version 4 wadspace's document (wad_core::v4::is_v4).
pub fn is_v4(w: &Value) -> bool {
    w.get("version").and_then(Value::as_u64) == Some(4)
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
        let (browsers, machines, wadspaces) = futures_util::join!(list("browsers"), list("machines"), async {
            fs.query_root(&tok, owned).await.map(|d| d.iter().map(plain).collect::<Vec<_>>())
        },);
        let lib = Library {
            browsers: browsers?,
            wadspaces: wadspaces?.into_iter().filter(is_v4).collect(),
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

    /// A wadspace saved as it is: at the top level, theirs, shared with no
    /// one (as the web app keeps them).
    pub async fn save_wadspace(&self, w: &Value) -> Result<(), Error> {
        let uid = self.user().map(|u| u.uid).ok_or(Error::SignedOut)?;
        let mut data = w.as_object().cloned().unwrap_or_default();
        let id = data.remove("id").and_then(|v| v.as_str().map(String::from)).ok_or(Error::Invalid("no id".into()))?;
        data.insert("owner".into(), uid.into());
        data.insert("sharedWith".into(), json!([]));
        let tok = self.id_token().await?;
        Ok(self.firestore().set(&tok, &format!("wadspaces/{id}"), &data).await?)
    }

    /// One of their documents (`browsers`) saved as it is.
    pub async fn save_doc(&self, collection: &str, d: &Value) -> Result<(), Error> {
        let uid = self.user().map(|u| u.uid).ok_or(Error::SignedOut)?;
        let mut data = d.as_object().cloned().unwrap_or_default();
        let id = data.remove("id").and_then(|v| v.as_str().map(String::from)).ok_or(Error::Invalid("no id".into()))?;
        let tok = self.id_token().await?;
        Ok(self.firestore().set(&tok, &format!("users/{uid}/{collection}/{id}"), &data).await?)
    }

    /// A wadspace deleted for good.
    pub async fn delete_wadspace(&self, id: &str) -> Result<(), Error> {
        let tok = self.id_token().await?;
        Ok(self.firestore().delete(&tok, &format!("wadspaces/{id}")).await?)
    }

    /// The library as last read on this machine (for a quick start, or
    /// offline). One kept before version 4 shows none of its old wadspaces.
    pub fn cached_library(&self) -> Option<Library> {
        let mut lib: Library = serde_json::from_slice(&std::fs::read(self.library_file()?).ok()?).ok()?;
        lib.wadspaces.retain(is_v4);
        Some(lib)
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
