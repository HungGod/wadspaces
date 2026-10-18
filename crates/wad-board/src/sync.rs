//! The board and the documents behind it (the client's builder/sync.ts).
//!
//! - [`to_board`]: the documents as stacks, a stack per wadspace in board
//!   order: its template chain, Extensions and customization as modules (a
//!   module that's a document says so: `ref`).
//! - [`plan`]: the writes that make the documents what the board shows, and
//!   the documents as they'll be. Templates and Extensions are shared by
//!   what makes their image: a module reuses its document while that still
//!   is what the module is; else one just like it; else it changes its own
//!   document when no other stack has it; else a new one is made. Wadspaces
//!   are written when they differ; a stack that's gone is deleted softly.
//! - [`plan_new`]: the writes that add one new stack and touch nothing else
//!   (the WadSpaces Android app's builder, which has no board).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::*;

/// The documents: templates, Extensions layers, wadspaces (as stored).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Docs {
    pub templates: Vec<Value>,
    pub extensions: Vec<Value>,
    pub wadspaces: Vec<Value>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn strs(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(String::from).collect()
}

/// A document's field is there and isn't null, false, 0 or "" (JS truthy).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(x)) => !x.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => true,
    }
}

fn alive(v: &Value) -> bool {
    !truthy(v.get("deletedAt"))
}

// ------------------------------------------------------------ documents → board

fn template_module(t: &Value, stack_id: &str) -> TemplateModule {
    TemplateModule {
        layer: TemplateTag,
        id: format!("{}@{stack_id}", s(t, "id")),
        doc: Some(s(t, "id").into()),
        name: s(t, "name").into(),
        description: s(t, "description").into(),
        apps: strs(t, "apps"),
    }
}

fn webapps_of(v: &Value) -> Vec<WebApp> {
    v.get("webapps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|w| WebApp { id: s(w, "id").into(), name: s(w, "name").into(), url: s(w, "url").into() })
        .collect()
}

/// An Extensions document's setups as submodules.
fn app_subs(e: &Value, origin: &str, base: &str) -> Vec<AppSub> {
    let mut subs = vec![];
    let webapps = webapps_of(e);
    let mode = match s(e, "browserMode") {
        "" => "browser",
        m => m,
    };
    if !webapps.is_empty() || mode == "focus" {
        subs.push(AppSub::Browser(BrowserSub {
            id: format!("{base}:wadbrowser"),
            origin: origin.into(),
            mode: mode.into(),
            webapps,
        }));
    }
    let extensions = strs(e, "extensions");
    if !extensions.is_empty() {
        subs.push(AppSub::Vscode(VscodeSub { id: format!("{base}:vscode"), origin: origin.into(), extensions }));
    }
    let tools = strs(e, "tools");
    if !tools.is_empty() {
        subs.push(AppSub::Tools(ToolsSub { id: format!("{base}:tools"), origin: origin.into(), tools }));
    }
    subs
}

fn extensions_module(e: &Value, stack_id: &str, doc: Option<&str>) -> ExtensionsModule {
    let base = format!("{}@{stack_id}", doc.unwrap_or("draft"));
    ExtensionsModule {
        layer: ExtensionsTag,
        id: format!("{}@{stack_id}", doc.unwrap_or("draft-extensions")),
        doc: doc.map(String::from),
        name: s(e, "name").into(),
        subs: app_subs(e, s(e, "name"), &base),
        autostart: Some(strs(e, "autostart")),
    }
}

fn customization_module(c: &Value, stack_id: &str) -> CustomizationModule {
    let id = format!("customization@{stack_id}");
    let name = s(c, "name");
    let secrets = &c["secrets"];
    let groups = secrets.get("groups").filter(|g| truthy(Some(g))).and_then(|g| serde_json::from_value(g.clone()).ok());
    CustomizationModule {
        layer: CustomizationTag,
        id: id.clone(),
        subs: vec![
            CustomSub::Look(LookSub {
                id: format!("{id}:look"),
                origin: name.into(),
                name: name.into(),
                description: s(c, "description").into(),
                wallpaper: c.get("wallpaper").cloned().unwrap_or(Value::Null),
                title_bars: c.get("titleBars").and_then(Value::as_bool).unwrap_or(false),
            }),
            CustomSub::Folders(FoldersSub {
                id: format!("{id}:folders"),
                origin: name.into(),
                mounts: c.get("projects").and_then(Value::as_array).cloned().unwrap_or_default(),
            }),
            CustomSub::Secrets(SecretsSub {
                id: format!("{id}:secrets"),
                origin: name.into(),
                by_slot: secrets.get("bySlot").and_then(Value::as_object).cloned().unwrap_or_default(),
                groups,
            }),
        ],
    }
}

/// The template documents a template builds on, bottom first (itself last).
fn chain<'a>(templates: &'a [Value], id: Option<&str>) -> Vec<&'a Value> {
    let mut out: Vec<&Value> = vec![];
    let find = |id: Option<&str>| templates.iter().find(|x| id.is_some() && x.get("id").and_then(Value::as_str) == id);
    let mut t = find(id);
    while let Some(x) = t {
        if out.len() >= 9 || out.iter().any(|o| std::ptr::eq(*o, x)) {
            break;
        }
        out.insert(0, x);
        t = find(x.get("parent").and_then(Value::as_str));
    }
    out
}

/// The documents as the board shows them: a stack per wadspace (not
/// deleted ones), in order.
pub fn to_board(docs: &Docs) -> Vec<Stack> {
    let mut list: Vec<&Value> = docs.wadspaces.iter().filter(|w| alive(w)).collect();
    list.sort_by(|a, b| {
        let o = |w: &Value| w.get("order").and_then(Value::as_f64).unwrap_or(0.0);
        o(a).total_cmp(&o(b)).then_with(|| s(a, "createdAt").cmp(s(b, "createdAt")))
    });
    list.into_iter()
        .map(|w| {
            let id = s(w, "id");
            let tpl = w.get("template").and_then(Value::as_str).filter(|t| !t.is_empty());
            let mut templates: Vec<TemplateModule> =
                chain(&docs.templates, tpl).into_iter().map(|t| template_module(t, id)).collect();
            if let Some(d) = w.get("draftTemplate").filter(|d| truthy(Some(d))) {
                templates.push(TemplateModule {
                    layer: TemplateTag,
                    id: format!("draft-template@{id}"),
                    doc: None,
                    name: s(d, "name").into(),
                    description: s(d, "description").into(),
                    apps: vec![],
                });
            }
            let mut stack = Stack { id: id.into(), templates, extensions: None, customization: None };
            let ext_id = w.get("extensions").and_then(Value::as_str).filter(|e| !e.is_empty());
            let e = ext_id.and_then(|eid| docs.extensions.iter().find(|x| s(x, "id") == eid));
            if let Some(e) = e {
                stack.extensions = Some(extensions_module(e, id, Some(s(e, "id"))));
            } else if let Some(d) = w.get("draftExtensions").filter(|d| truthy(Some(d))) {
                stack.extensions = Some(extensions_module(d, id, None));
            }
            if let Some(c) = w.get("customization").filter(|c| truthy(Some(c))) {
                stack.customization = Some(customization_module(c, id));
            }
            stack
        })
        .collect()
}

// ------------------------------------------------------------ board → documents

/// An Extensions module's setups as a document has them.
pub fn setup_of_module(m: &ExtensionsModule) -> Value {
    let browser = browser_of(m);
    json!({
        "browserMode": browser.map(|b| b.mode.as_str()).unwrap_or("browser"),
        "webapps": browser.map(|b| b.webapps.iter().map(|w| json!({ "id": w.id, "name": w.name, "url": w.url })).collect::<Vec<_>>()).unwrap_or_default(),
        "extensions": vscode_of(m).map(|v| v.extensions.clone()).unwrap_or_default(),
        "tools": tools_of(m).map(|t| t.tools.clone()).unwrap_or_default(),
        "autostart": m.autostart.clone().unwrap_or_default(),
    })
}

/// A customization module as the wadspace keeps it.
pub fn customization_of(m: &CustomizationModule) -> Value {
    let look = look_of(m);
    let keys = secrets_of(m);
    let mut secrets = json!({ "bySlot": keys.map(|k| k.by_slot.clone()).unwrap_or_default() });
    if let Some(g) = keys.and_then(|k| k.groups.as_ref()) {
        secrets["groups"] = serde_json::to_value(g).unwrap_or_default();
    }
    let mut c = json!({
        "name": look.map(|l| l.name.as_str()).unwrap_or(""),
        "description": look.map(|l| l.description.as_str()).unwrap_or(""),
        "wallpaper": look.map(|l| l.wallpaper.clone()).unwrap_or_else(blank_wallpaper),
        "projects": folders_of(m).map(|f| f.mounts.clone()).unwrap_or_default(),
        "secrets": secrets,
    });
    // Only when on: a design from before it reads the same.
    if look.is_some_and(|l| l.title_bars) {
        c["titleBars"] = true.into();
    }
    c
}

fn same_set(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x))
}

/// The setups that make an Extensions layer what it is (one missing reads
/// as null, as JSON.stringify writes it).
fn setup_key(e: &Value) -> Value {
    let f = |k: &str| e.get(k).cloned().unwrap_or(Value::Null);
    json!([f("browserMode"), f("webapps"), f("extensions"), f("tools"), f("autostart")])
}

/// An id from a name, free among these (deleted ones too): the web app's
/// freeId.
pub fn free_id<'a>(name: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    let base = slug(name);
    let mut id = base.clone();
    let mut n = 2;
    while taken.clone().any(|t| t == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// The web app's slug: lowercase, runs of anything else as one dash, no
/// dash at either end, at most 40, else "item".
pub fn slug(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::new();
    let mut dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let out = out.strip_prefix('-').unwrap_or(&out);
    let out = out.strip_suffix('-').unwrap_or(out);
    let cut: String = out.chars().take(40).collect();
    if cut.is_empty() { "item".into() } else { cut }
}

/// Documents by id, in the order they came (a Map's).
#[derive(Default)]
struct Ordered {
    list: Vec<(String, Value)>,
}

impl Ordered {
    fn from(docs: &[Value]) -> Self {
        let mut o = Ordered::default();
        for d in docs {
            o.set(d.clone());
        }
        o
    }

    fn get(&self, id: &str) -> Option<&Value> {
        self.list.iter().find(|(k, _)| k == id).map(|(_, v)| v)
    }

    fn set(&mut self, d: Value) {
        let id = s(&d, "id").to_string();
        match self.list.iter_mut().find(|(k, _)| *k == id) {
            Some((_, v)) => *v = d,
            None => self.list.push((id, d)),
        }
    }

    fn values(&self) -> impl Iterator<Item = &Value> + Clone {
        self.list.iter().map(|(_, v)| v)
    }
}

/// What to write so the documents are what the board shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub templates: Vec<Value>,
    pub extensions: Vec<Value>,
    pub wadspaces: Vec<Value>,
    /// Wadspaces (stacks) gone from the board: deleted softly.
    pub deleted: Vec<String>,
    /// Each module's document, by module id.
    pub refs: Map<String, Value>,
    /// The documents once written.
    pub docs: Docs,
}

impl Plan {
    /// Whether it writes anything.
    pub fn writes(&self) -> bool {
        !(self.templates.is_empty()
            && self.extensions.is_empty()
            && self.wadspaces.is_empty()
            && self.deleted.is_empty())
    }
}

fn merge(base: &Value, over: &Value) -> Value {
    let mut out = base.as_object().cloned().unwrap_or_default();
    for (k, v) in over.as_object().into_iter().flatten() {
        out.insert(k.clone(), v.clone());
    }
    Value::Object(out)
}

fn put(list: &mut Vec<Value>, d: Value) {
    let id = s(&d, "id").to_string();
    list.retain(|x| s(x, "id") != id);
    list.push(d);
}

/// What to write so the documents are what `stacks` shows; `owner` and
/// `now` stamp new and changed ones.
pub fn plan(stacks: &[Stack], docs: &Docs, owner: &str, now: &str) -> Plan {
    let mut templates = Ordered::from(&docs.templates);
    let mut extensions = Ordered::from(&docs.extensions);
    let mut out = Plan { docs: docs.clone(), ..Default::default() };
    // How many stacks (but `stack_id`) have a module that's this document.
    let others_with = |doc: &str, stack_id: &str, layer: Layer| {
        stacks
            .iter()
            .filter(|s| {
                s.id != stack_id
                    && match layer {
                        Layer::Template => s.templates.iter().any(|t| t.doc.as_deref() == Some(doc)),
                        _ => s.extensions.as_ref().and_then(|e| e.doc.as_deref()) == Some(doc),
                    }
            })
            .count()
    };
    let name_or = |n: &str| if n.is_empty() { "layer".to_string() } else { n.to_string() };

    for (i, st) in stacks.iter().enumerate() {
        // The template chain, bottom up; a blank one on top waits as a draft.
        let mut parent: Option<String> = None;
        let mut draft_template: Option<Value> = None;
        if let Some(top) = st.templates.last().filter(|t| t.apps.is_empty()) {
            draft_template = Some(json!({ "name": top.name, "description": top.description }));
        }
        for m in st.templates.iter().filter(|t| !t.apps.is_empty()) {
            let mut want = json!({ "name": m.name, "description": m.description, "apps": m.apps });
            if let Some(p) = &parent {
                want["parent"] = p.clone().into();
            }
            let is_it = |t: &Value| {
                alive(t)
                    && same_set(&strs(t, "apps"), &m.apps)
                    && t.get("parent").and_then(Value::as_str).filter(|p| !p.is_empty()).map(String::from) == parent
            };
            let cur = m.doc.as_deref().and_then(|d| templates.get(d)).cloned();
            let twin = templates.values().find(|t| is_it(t)).cloned();
            let id: String;
            if let Some(cur) = cur.as_ref().filter(|c| is_it(c)) {
                id = s(cur, "id").into();
                if s(cur, "name") != m.name || s(cur, "description") != m.description || strs(cur, "apps") != m.apps {
                    let mut d = merge(cur, &want);
                    d["updatedAt"] = now.into();
                    templates.set(d.clone());
                    put(&mut out.templates, d);
                }
            } else if let Some(twin) = twin {
                id = s(&twin, "id").into();
                if s(&twin, "name") != m.name || s(&twin, "description") != m.description {
                    let mut d = twin.clone();
                    d["name"] = m.name.clone().into();
                    d["description"] = m.description.clone().into();
                    d["updatedAt"] = now.into();
                    templates.set(d.clone());
                    put(&mut out.templates, d);
                }
            } else if let Some(cur) =
                cur.as_ref().filter(|c| alive(c) && others_with(s(c, "id"), &st.id, Layer::Template) == 0)
            {
                id = s(cur, "id").into();
                let mut rest = cur.clone();
                if let Some(o) = rest.as_object_mut() {
                    o.remove("parent");
                }
                let mut d = merge(&rest, &want);
                d["updatedAt"] = now.into();
                templates.set(d.clone());
                put(&mut out.templates, d);
            } else {
                id = free_id(&name_or(&m.name), templates.values().map(|t| s(t, "id")));
                let mut d = json!({ "id": id, "owner": owner });
                d = merge(&d, &want);
                d["createdAt"] = now.into();
                d["updatedAt"] = now.into();
                templates.set(d.clone());
                put(&mut out.templates, d);
            }
            out.refs.insert(m.id.clone(), id.clone().into());
            parent = Some(id);
        }

        // Its Extensions, made on the top template; with no template yet,
        // kept in the stack.
        let mut ext_id: Option<String> = None;
        let mut draft_extensions: Option<Value> = None;
        if let Some(em) = &st.extensions {
            let setup = setup_of_module(em);
            match &parent {
                None => {
                    draft_extensions = Some(merge(&json!({ "name": em.name }), &setup));
                }
                Some(p) => {
                    let is_it = |e: &Value| alive(e) && s(e, "templateId") == p && setup_key(e) == setup_key(&setup);
                    let cur = em.doc.as_deref().and_then(|d| extensions.get(d)).cloned();
                    let twin = extensions.values().find(|e| is_it(e)).cloned();
                    let id: String;
                    if let Some(cur) = cur.as_ref().filter(|c| is_it(c)) {
                        id = s(cur, "id").into();
                        if s(cur, "name") != em.name {
                            let mut d = cur.clone();
                            d["name"] = em.name.clone().into();
                            d["updatedAt"] = now.into();
                            extensions.set(d.clone());
                            put(&mut out.extensions, d);
                        }
                    } else if let Some(twin) = twin {
                        id = s(&twin, "id").into();
                        if s(&twin, "name") != em.name {
                            let mut d = twin.clone();
                            d["name"] = em.name.clone().into();
                            d["updatedAt"] = now.into();
                            extensions.set(d.clone());
                            put(&mut out.extensions, d);
                        }
                    } else if let Some(cur) =
                        cur.as_ref().filter(|c| alive(c) && others_with(s(c, "id"), &st.id, Layer::Extensions) == 0)
                    {
                        id = s(cur, "id").into();
                        let mut d = merge(cur, &json!({ "name": em.name, "templateId": p }));
                        d = merge(&d, &setup);
                        d["updatedAt"] = now.into();
                        extensions.set(d.clone());
                        put(&mut out.extensions, d);
                    } else {
                        id = free_id(&name_or(&em.name), extensions.values().map(|e| s(e, "id")));
                        let mut d =
                            json!({ "id": id, "owner": owner, "name": em.name, "description": "", "templateId": p });
                        d = merge(&d, &setup);
                        d["createdAt"] = now.into();
                        d["updatedAt"] = now.into();
                        extensions.set(d.clone());
                        put(&mut out.extensions, d);
                    }
                    out.refs.insert(em.id.clone(), id.clone().into());
                    ext_id = Some(id);
                }
            }
        }

        // The wadspace: the stack itself.
        let was = docs.wadspaces.iter().find(|w| s(w, "id") == st.id);
        let mut next = Map::new();
        next.insert("version".into(), 3.into());
        next.insert("id".into(), st.id.clone().into());
        next.insert("owner".into(), was.and_then(|w| w.get("owner").cloned()).unwrap_or_else(|| owner.into()));
        if let Some(sw) = was.and_then(|w| w.get("sharedWith")).filter(|v| truthy(Some(v))) {
            next.insert("sharedWith".into(), sw.clone());
        }
        next.insert("order".into(), (i + 1).into());
        if let Some(p) = &parent {
            next.insert("template".into(), p.clone().into());
        }
        if let Some(d) = draft_template {
            next.insert("draftTemplate".into(), d);
        }
        if let Some(e) = &ext_id {
            next.insert("extensions".into(), e.clone().into());
        }
        if let Some(d) = draft_extensions {
            next.insert("draftExtensions".into(), d);
        }
        if let Some(c) = &st.customization {
            next.insert("customization".into(), customization_of(c));
        }
        next.insert("createdAt".into(), was.and_then(|w| w.get("createdAt").cloned()).unwrap_or_else(|| now.into()));
        next.insert("updatedAt".into(), was.and_then(|w| w.get("updatedAt").cloned()).unwrap_or_else(|| now.into()));
        let mut a = next.clone();
        a.remove("updatedAt");
        let b = was.map(|w| {
            let mut b = w.as_object().cloned().unwrap_or_default();
            b.remove("updatedAt");
            b.remove("deletedAt");
            b
        });
        let changed = match (was, b) {
            (Some(w), Some(b)) => !alive(w) || Value::Object(a) != Value::Object(b),
            _ => true,
        };
        if changed {
            next.insert("updatedAt".into(), now.into());
            out.wadspaces.push(Value::Object(next));
        }
    }

    let on_board: Vec<&str> = stacks.iter().map(|s| s.id.as_str()).collect();
    out.deleted = docs
        .wadspaces
        .iter()
        .filter(|w| alive(w) && !on_board.contains(&s(w, "id")))
        .map(|w| s(w, "id").to_string())
        .collect();
    let mut wadspaces = Ordered::from(&docs.wadspaces);
    for w in &out.wadspaces {
        wadspaces.set(w.clone());
    }
    for id in &out.deleted {
        if let Some(mut w) = wadspaces.get(id).cloned() {
            w["deletedAt"] = now.into();
            wadspaces.set(w);
        }
    }
    out.docs = Docs {
        templates: templates.values().cloned().collect(),
        extensions: extensions.values().cloned().collect(),
        wadspaces: wadspaces.values().cloned().collect(),
    };
    out
}

/// The writes that add one new stack (`stack`, not on the board yet) to
/// the documents, and nothing else: [`plan`] over the board with it last,
/// kept to this stack's own writes (a whole board's save renumbers, renames
/// and deletes what isn't on it). First, a new module that has a live twin
/// (a template with the same apps on the same parent; Extensions with the
/// same setups on the same template) takes that document as it is, name
/// and apps' order and all, so saving changes nothing of it. The stack goes after the last one.
pub fn plan_new(stack: &Stack, docs: &Docs, owner: &str, now: &str) -> Plan {
    let mut stack = stack.clone();
    // Twins, bottom up: a module that's a document (or a twin's) is its
    // parent's for the one above; past a new one, nothing has a twin.
    let mut parent: Option<Option<String>> = Some(None);
    for m in stack.templates.iter_mut().filter(|t| !t.apps.is_empty()) {
        let Some(under) = parent.clone() else { break };
        if m.doc.is_none()
            && let Some(t) = docs.templates.iter().find(|t| {
                alive(t)
                    && same_set(&strs(t, "apps"), &m.apps)
                    && t.get("parent").and_then(Value::as_str).filter(|p| !p.is_empty()).map(String::from) == under
            })
        {
            m.doc = Some(s(t, "id").into());
            m.name = s(t, "name").into();
            m.description = s(t, "description").into();
            // Its desktop's order too.
            m.apps = strs(t, "apps");
        }
        parent = m.doc.clone().map(Some);
    }
    if let (Some(Some(tid)), Some(em)) = (parent, stack.extensions.as_mut())
        && em.doc.is_none()
    {
        let want = setup_key(&setup_of_module(em));
        if let Some(e) = docs.extensions.iter().find(|e| alive(e) && s(e, "templateId") == tid && setup_key(e) == want)
        {
            em.doc = Some(s(e, "id").into());
            em.name = s(e, "name").into();
        }
    }

    let mut stacks = to_board(docs);
    stacks.retain(|st| st.id != stack.id);
    stacks.push(stack.clone());
    let all = plan(&stacks, docs, owner, now);

    // Only this stack's.
    let mine: Vec<String> =
        stack.templates.iter().map(|t| t.id.clone()).chain(stack.extensions.iter().map(|e| e.id.clone())).collect();
    let refs: Map<String, Value> = all.refs.into_iter().filter(|(k, _)| mine.contains(k)).collect();
    let ids: Vec<&str> = refs.values().filter_map(Value::as_str).collect();
    let order = docs
        .wadspaces
        .iter()
        .filter(|w| alive(w) && s(w, "id") != stack.id)
        .filter_map(|w| w.get("order").and_then(Value::as_f64))
        .fold(0.0, f64::max) as i64
        + 1;
    let mut out = Plan {
        templates: all.templates.into_iter().filter(|t| ids.contains(&s(t, "id"))).collect(),
        extensions: all.extensions.into_iter().filter(|e| ids.contains(&s(e, "id"))).collect(),
        wadspaces: all
            .wadspaces
            .into_iter()
            .filter(|w| s(w, "id") == stack.id)
            .map(|mut w| {
                w["order"] = order.into();
                w
            })
            .collect(),
        deleted: vec![],
        refs,
        docs: docs.clone(),
    };
    for t in &out.templates {
        put(&mut out.docs.templates, t.clone());
    }
    for e in &out.extensions {
        put(&mut out.docs.extensions, e.clone());
    }
    for w in &out.wadspaces {
        put(&mut out.docs.wadspaces, w.clone());
    }
    out
}

/// The stacks with each module's document noted.
pub fn with_refs(stacks: &[Stack], refs: &HashMap<String, String>) -> Vec<Stack> {
    stacks
        .iter()
        .map(|st| {
            let mut st = st.clone();
            for t in &mut st.templates {
                if let Some(r) = refs.get(&t.id) {
                    t.doc = Some(r.clone());
                }
            }
            if let Some(e) = st.extensions.as_mut()
                && let Some(r) = refs.get(&e.id)
            {
                e.doc = Some(r.clone());
            }
            st
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_free_ids_are_the_web_apps() {
        assert_eq!(slug("  Game Dev + Art!  "), "game-dev-art");
        assert_eq!(slug("✓✓"), "item");
        assert_eq!(slug(&"a".repeat(50)).len(), 40);
        let taken = ["dev", "dev-2"];
        assert_eq!(free_id("Dev", taken.iter().copied()), "dev-3");
    }

    #[test]
    fn title_bars_are_kept_only_when_on() {
        let c = json!({ "name": "Writing", "wallpaper": { "type": "color", "value": "#000" }, "titleBars": true });
        let m = customization_module(&c, "s1");
        assert_eq!(customization_of(&m)["titleBars"], true);
        let off = customization_module(&json!({ "name": "Writing" }), "s1");
        assert!(customization_of(&off).get("titleBars").is_none(), "a design from before reads the same");
    }
}
