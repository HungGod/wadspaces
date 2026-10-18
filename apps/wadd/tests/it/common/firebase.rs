//! A Firebase for tests: enrollMachine, Auth's custom-token sign-in, refresh
//! and token lookup, and a Firestore keeping documents in memory (REST form).
//! Tokens say who they are: `id:<uid>:<mid>:<n>`, `rt:<uid>:<mid>:<n>`, and a
//! person's `user:<uid>`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use wad_firebase::values::{from_fields, to_fields};

#[derive(Default)]
pub struct Fb {
    /// Link codes -> the owner who made them.
    pub codes: HashMap<String, String>,
    /// Document path -> its fields (REST form).
    pub docs: BTreeMap<String, Value>,
    pub next_machine: u32,
    pub refreshes: u32,
    /// Every enrollMachine call's data.
    pub enrolls: Vec<Value>,
    /// Answer every Firestore call with this status.
    pub down: Option<u16>,
    /// accounts:lookup calls.
    pub lookups: u32,
}

#[derive(Clone, Default)]
pub struct FakeFirebase(pub Arc<Mutex<Fb>>);

impl FakeFirebase {
    /// A document as plain JSON (timestamps as RFC 3339 text).
    pub fn doc(&self, path: &str) -> Option<Value> {
        self.0.lock().unwrap().docs.get(path).map(from_fields)
    }

    /// Writes a document as the owner (or a function) would.
    pub fn put(&self, path: &str, data: Value) {
        self.0.lock().unwrap().docs.insert(path.into(), to_fields(data.as_object().unwrap()));
    }

    pub fn code(&self, code: &str, uid: &str) {
        self.0.lock().unwrap().codes.insert(code.into(), uid.into());
    }

    pub async fn serve(&self) -> String {
        let app = Router::new()
            .route("/functions/enrollMachine", post(enroll))
            .route("/identitytoolkit/{op}", post(sign_in))
            .route("/identitytoolkit/accounts:lookup", post(lookup))
            .route("/securetoken/token", post(refresh))
            .route("/v1/projects/{p}/databases/{db}/documents/{*rest}", any(firestore))
            .with_state(self.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }
}

fn who(tok: &str) -> Option<(String, String)> {
    let mut p = tok.split(':');
    let (_, uid, mid) = (p.next()?, p.next()?, p.next()?);
    Some((uid.into(), mid.into()))
}

async fn enroll(State(fb): State<FakeFirebase>, Json(b): Json<Value>) -> Response {
    let d = b["data"].clone();
    let mut f = fb.0.lock().unwrap();
    f.enrolls.push(d.clone());
    let code = d["code"].as_str().unwrap_or_default().to_string();
    let Some(uid) = f.codes.remove(&code) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"status": "NOT_FOUND", "message": "Unknown enrollment code."}})),
        )
            .into_response();
    };
    // The same owner relinking keeps the machine; another owner gets a new one.
    let previous = d.get("previousIdToken").and_then(Value::as_str).and_then(who);
    let (mid, relink) = match previous {
        Some((puid, pmid)) if puid == uid => (pmid, "same"),
        Some((puid, pmid)) => {
            f.docs.remove(&format!("users/{puid}/machines/{pmid}"));
            f.next_machine += 1;
            (format!("m{}", f.next_machine), "new-owner")
        }
        None => {
            f.next_machine += 1;
            (format!("m{}", f.next_machine), "new")
        }
    };
    f.docs
        .entry(format!("users/{uid}/machines/{mid}"))
        .or_insert_with(|| to_fields(json!({"name": d["machineName"]}).as_object().unwrap()));
    Json(json!({"result": {"machineId": mid, "ownerUid": uid, "customToken": format!("ct:{uid}:{mid}"), "projectId": "demo", "apiKey": "key", "relink": relink}})).into_response()
}

/// A person's ID token (`user:<uid>`): whose it is.
async fn lookup(State(fb): State<FakeFirebase>, Json(b): Json<Value>) -> Response {
    fb.0.lock().unwrap().lookups += 1;
    match b["idToken"].as_str().and_then(|t| t.strip_prefix("user:")) {
        Some(uid) => Json(json!({"users": [{"localId": uid}]})).into_response(),
        None => (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": "INVALID_ID_TOKEN"}}))).into_response(),
    }
}

async fn sign_in(Path(op): Path<String>, Json(b): Json<Value>) -> Response {
    assert_eq!(op, "accounts:signInWithCustomToken");
    let (uid, mid) = who(b["token"].as_str().unwrap_or_default()).unwrap();
    Json(json!({"idToken": format!("id:{uid}:{mid}:0"), "refreshToken": format!("rt:{uid}:{mid}:0"), "expiresIn": "3600"})).into_response()
}

/// application/x-www-form-urlencoded, as reqwest writes it.
fn form(body: &[u8]) -> HashMap<String, String> {
    let decode = |s: &str| {
        let s = s.replace('+', " ");
        let b = s.as_bytes();
        let mut out = vec![];
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%'
                && i + 2 < b.len()
                && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    String::from_utf8_lossy(body)
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

async fn refresh(State(fb): State<FakeFirebase>, body: axum::body::Bytes) -> Response {
    let f = form(&body);
    let Some((uid, mid)) = who(f.get("refresh_token").map(String::as_str).unwrap_or_default()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": "INVALID_REFRESH_TOKEN"}}))).into_response();
    };
    let n = {
        let mut g = fb.0.lock().unwrap();
        g.refreshes += 1;
        g.refreshes
    };
    // A rotated refresh token, as Google sometimes gives.
    Json(json!({"id_token": format!("id:{uid}:{mid}:{n}"), "refresh_token": format!("rt:{uid}:{mid}:{n}"), "expires_in": "3600"})).into_response()
}

async fn firestore(
    State(fb): State<FakeFirebase>,
    Path((_, _, rest)): Path<(String, String, String)>,
    Query(q): Query<Vec<(String, String)>>,
    headers: HeaderMap,
    method: axum::http::Method,
    body: axum::body::Bytes,
) -> Response {
    let auth = headers.get("authorization").and_then(|h| h.to_str().ok()).unwrap_or_default();
    if !auth.starts_with("Bearer id:") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut f = fb.0.lock().unwrap();
    if let Some(s) = f.down {
        return StatusCode::from_u16(s).unwrap().into_response();
    }
    let name = |p: &str| format!("projects/demo/databases/(default)/documents/{p}");
    let doc = |p: &str, fields: &Value| json!({"name": name(p), "fields": fields});
    if let Some(parent) = rest.strip_suffix(":runQuery") {
        // Only what the relay asks: a subcollection where status == <value>.
        let b: Value = serde_json::from_slice(&body).unwrap();
        let q = &b["structuredQuery"];
        let coll = q["from"][0]["collectionId"].as_str().unwrap();
        let want = &q["where"]["fieldFilter"]["value"];
        let prefix = format!("{parent}/{coll}/");
        let rows: Vec<Value> = f
            .docs
            .iter()
            .filter(|(p, d)| p.starts_with(&prefix) && !p[prefix.len()..].contains('/') && &d["status"] == want)
            .map(|(p, d)| json!({"document": doc(p, d)}))
            .collect();
        return Json(Value::Array(rows)).into_response();
    }
    let depth = rest.split('/').count();
    match method.as_str() {
        "GET" if depth % 2 == 1 => {
            let prefix = format!("{rest}/");
            let docs: Vec<Value> = f
                .docs
                .iter()
                .filter(|(p, _)| p.starts_with(&prefix) && !p[prefix.len()..].contains('/'))
                .map(|(p, d)| doc(p, d))
                .collect();
            Json(json!({"documents": docs})).into_response()
        }
        "GET" => match f.docs.get(&rest) {
            Some(d) => Json(doc(&rest, d)).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        "PATCH" => {
            let b: Value = serde_json::from_slice(&body).unwrap();
            let mask: Vec<String> =
                q.iter().filter(|(k, _)| k == "updateMask.fieldPaths").map(|(_, v)| v.clone()).collect();
            if mask.is_empty() {
                // No mask: the document is replaced, as Firestore does.
                f.docs.insert(rest.clone(), b["fields"].clone());
            } else {
                let entry = f.docs.entry(rest.clone()).or_insert_with(|| json!({}));
                for k in mask {
                    match b["fields"].get(&k) {
                        Some(v) => entry[&k] = v.clone(),
                        None => {
                            entry.as_object_mut().unwrap().remove(&k);
                        }
                    }
                }
            }
            Json(doc(&rest, &f.docs[&rest])).into_response()
        }
        "DELETE" => {
            f.docs.remove(&rest);
            Json(json!({})).into_response()
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}
