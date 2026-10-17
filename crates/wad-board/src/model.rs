//! The board's model (the client's components/builder/model.ts). A stack is
//! one wadspace, built from up to three modules, bottom to top:
//!
//! - template: the base image plus apps, in desktop order. Templates stack
//!   too: one built on another has all of its apps and adds some.
//! - extensions: one submodule per app that has a customization layer (VS
//!   Code extensions, WadBrowser apps, command-line tools). Optional.
//! - customization: the look (name, description, background), projects and
//!   keys submodules.
//!
//! A stack keeps any number of templates and at most one of each other
//! layer. Grabbing a module takes it and every module above it.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

/// A field that's always one string: a module's `layer`.
macro_rules! tag {
    ($name:ident, $s:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct $name;
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str($s)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let v = String::deserialize(d)?;
                if v == $s { Ok($name) } else { Err(D::Error::custom(format!("not a {}", $s))) }
            }
        }
    };
}
tag!(TemplateTag, "template");
tag!(ExtensionsTag, "extensions");
tag!(CustomizationTag, "customization");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Template,
    Extensions,
    Customization,
}

impl Layer {
    pub const ALL: [Layer; 3] = [Layer::Template, Layer::Extensions, Layer::Customization];

    pub fn label(self) -> &'static str {
        match self {
            Layer::Template => "Template",
            Layer::Extensions => "Extensions",
            Layer::Customization => "Customization",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Layer::Template => "The apps",
            Layer::Extensions => "What the apps come set up with",
            Layer::Customization => "Name, background, projects and keys",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebApp {
    pub id: String,
    pub name: String,
    pub url: String,
}

// ── Extensions submodules (one per customizable app) ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VscodeSub {
    pub id: String,
    /// The module it was first made in.
    pub origin: String,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowserSub {
    pub id: String,
    pub origin: String,
    /// "browser" or "focus".
    pub mode: String,
    pub webapps: Vec<WebApp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsSub {
    pub id: String,
    pub origin: String,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "key")]
pub enum AppSub {
    #[serde(rename = "vscode")]
    Vscode(VscodeSub),
    #[serde(rename = "wadbrowser")]
    Browser(BrowserSub),
    #[serde(rename = "tools")]
    Tools(ToolsSub),
}

// ── Customization submodules ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookSub {
    pub id: String,
    pub origin: String,
    pub name: String,
    pub description: String,
    /// `{type, value}`.
    pub wallpaper: Value,
    /// Windows get title bars (minimize, maximize, close). Off: kept out of
    /// the JSON, as designs from before it have it.
    #[serde(default, rename = "titleBars", skip_serializing_if = "std::ops::Not::not")]
    pub title_bars: bool,
}

/// Projects: cloned or copied in (never mounted), each at ~/Desktop/<as>.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FoldersSub {
    pub id: String,
    pub origin: String,
    pub mounts: Vec<Value>,
}

/// One of WadBrowser's key groups: its apps ("browser", "web:<id>") and the
/// key they share.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyGroup {
    /// (Documents may have none: kept so, as the web app keeps them.)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsSub {
    pub id: String,
    pub origin: String,
    /// The keys of everything but WadBrowser: "app:vscode", "ext:<id>",
    /// "app:obsidian", "app:steam" → `{id, name, kind}`.
    pub by_slot: Map<String, Value>,
    /// WadBrowser's apps, grouped by the key they share (none: one group).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<KeyGroup>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "key")]
pub enum CustomSub {
    #[serde(rename = "look")]
    Look(LookSub),
    #[serde(rename = "folders")]
    Folders(FoldersSub),
    #[serde(rename = "secrets")]
    Secrets(SecretsSub),
}

/// Any submodule (what's dragged, or waits in Recently deleted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnySub {
    App(AppSub),
    Custom(CustomSub),
}

impl AppSub {
    pub fn id(&self) -> &str {
        match self {
            AppSub::Vscode(x) => &x.id,
            AppSub::Browser(x) => &x.id,
            AppSub::Tools(x) => &x.id,
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            AppSub::Vscode(_) => "vscode",
            AppSub::Browser(_) => "wadbrowser",
            AppSub::Tools(_) => "tools",
        }
    }

    fn set_id(&mut self, id: String) {
        match self {
            AppSub::Vscode(x) => x.id = id,
            AppSub::Browser(x) => x.id = id,
            AppSub::Tools(x) => x.id = id,
        }
    }
}

impl CustomSub {
    pub fn id(&self) -> &str {
        match self {
            CustomSub::Look(x) => &x.id,
            CustomSub::Folders(x) => &x.id,
            CustomSub::Secrets(x) => &x.id,
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            CustomSub::Look(_) => "look",
            CustomSub::Folders(_) => "folders",
            CustomSub::Secrets(_) => "secrets",
        }
    }

    fn set_id(&mut self, id: String) {
        match self {
            CustomSub::Look(x) => x.id = id,
            CustomSub::Folders(x) => x.id = id,
            CustomSub::Secrets(x) => x.id = id,
        }
    }
}

impl AnySub {
    pub fn id(&self) -> &str {
        match self {
            AnySub::App(x) => x.id(),
            AnySub::Custom(x) => x.id(),
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            AnySub::App(x) => x.key(),
            AnySub::Custom(x) => x.key(),
        }
    }

    /// The layer it goes in.
    pub fn parent(&self) -> Layer {
        sub_parent(self.key())
    }
}

/// The layer a submodule of this key goes in.
pub fn sub_parent(key: &str) -> Layer {
    if matches!(key, "vscode" | "wadbrowser" | "tools") { Layer::Extensions } else { Layer::Customization }
}

// ── Modules ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateModule {
    pub layer: TemplateTag,
    pub id: String,
    /// The template document it is (none yet: made when it's saved).
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub name: String,
    pub description: String,
    /// Catalog app ids, in desktop order.
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionsModule {
    pub layer: ExtensionsTag,
    pub id: String,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub name: String,
    pub subs: Vec<AppSub>,
    /// What opens at boot, kept as the document has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomizationModule {
    pub layer: CustomizationTag,
    pub id: String,
    pub subs: Vec<CustomSub>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Module {
    Template(TemplateModule),
    Extensions(ExtensionsModule),
    Customization(CustomizationModule),
}

impl Module {
    pub fn id(&self) -> &str {
        match self {
            Module::Template(m) => &m.id,
            Module::Extensions(m) => &m.id,
            Module::Customization(m) => &m.id,
        }
    }

    pub fn layer(&self) -> Layer {
        match self {
            Module::Template(_) => Layer::Template,
            Module::Extensions(_) => Layer::Extensions,
            Module::Customization(_) => Layer::Customization,
        }
    }

    /// The document it is (a customization is its stack's).
    pub fn doc(&self) -> Option<&str> {
        match self {
            Module::Template(m) => m.doc.as_deref(),
            Module::Extensions(m) => m.doc.as_deref(),
            Module::Customization(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Stack {
    pub id: String,
    /// Bottom first; each builds on the one below.
    pub templates: Vec<TemplateModule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionsModule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customization: Option<CustomizationModule>,
}

impl TemplateModule {
    pub fn new(id: impl Into<String>, name: impl Into<String>, apps: Vec<String>) -> Self {
        Self { layer: TemplateTag, id: id.into(), doc: None, name: name.into(), description: String::new(), apps }
    }
}

impl Stack {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), ..Default::default() }
    }

    /// The top template: the apps the wadspace's image ends up with.
    pub fn top(&self) -> Option<&TemplateModule> {
        self.templates.last()
    }

    pub fn apps(&self) -> &[String] {
        self.top().map(|t| t.apps.as_slice()).unwrap_or(&[])
    }

    /// Every module, bottom to top.
    pub fn modules(&self) -> Vec<Module> {
        let mut out: Vec<Module> = self.templates.iter().cloned().map(Module::Template).collect();
        out.extend(self.extensions.clone().map(Module::Extensions));
        out.extend(self.customization.clone().map(Module::Customization));
        out
    }

    pub fn has_app(&self, id: &str) -> bool {
        self.apps().iter().any(|a| a == id)
    }

    pub fn look(&self) -> Option<&LookSub> {
        self.customization.as_ref().and_then(look_of)
    }

    pub fn secrets(&self) -> Option<&SecretsSub> {
        self.customization.as_ref().and_then(secrets_of)
    }

    pub fn browser(&self) -> Option<&BrowserSub> {
        self.extensions.as_ref().and_then(browser_of)
    }

    pub fn vscode(&self) -> Option<&VscodeSub> {
        self.extensions.as_ref().and_then(vscode_of)
    }

    pub fn folders(&self) -> Option<&FoldersSub> {
        self.customization.as_ref().and_then(folders_of)
    }
}

pub fn look_of(m: &CustomizationModule) -> Option<&LookSub> {
    m.subs.iter().find_map(|x| if let CustomSub::Look(l) = x { Some(l) } else { None })
}

pub fn folders_of(m: &CustomizationModule) -> Option<&FoldersSub> {
    m.subs.iter().find_map(|x| if let CustomSub::Folders(l) = x { Some(l) } else { None })
}

pub fn secrets_of(m: &CustomizationModule) -> Option<&SecretsSub> {
    m.subs.iter().find_map(|x| if let CustomSub::Secrets(l) = x { Some(l) } else { None })
}

pub fn browser_of(m: &ExtensionsModule) -> Option<&BrowserSub> {
    m.subs.iter().find_map(|x| if let AppSub::Browser(l) = x { Some(l) } else { None })
}

pub fn vscode_of(m: &ExtensionsModule) -> Option<&VscodeSub> {
    m.subs.iter().find_map(|x| if let AppSub::Vscode(l) = x { Some(l) } else { None })
}

pub fn tools_of(m: &ExtensionsModule) -> Option<&ToolsSub> {
    m.subs.iter().find_map(|x| if let AppSub::Tools(l) = x { Some(l) } else { None })
}

/// A catalog app's name (else its id).
pub fn app_name(id: &str) -> String {
    wad_core::catalog::app(id).map(|a| a.name).unwrap_or_else(|| id.to_string())
}

/// Whether template `upper` can build on `lower`: it has all of lower's
/// apps and adds at least one.
pub fn builds_on(upper: &TemplateModule, lower: &TemplateModule) -> Result<(), String> {
    let missing: Vec<String> = lower.apps.iter().filter(|a| !upper.apps.contains(a)).map(|a| app_name(a)).collect();
    if !missing.is_empty() {
        return Err(format!("{} can't build on {}: it doesn't have {}", upper.name, lower.name, missing.join(", ")));
    }
    if upper.apps.len() == lower.apps.len() {
        return Err(format!("{} adds no apps to {}", upper.name, lower.name));
    }
    Ok(())
}

/// Where templates (bottom first) fit into a chain: the highest place they
/// build on what's below and what's above builds on them.
pub fn fit_templates(chain: &[TemplateModule], adding: &[TemplateModule]) -> Result<usize, String> {
    let mut first: Option<String> = None;
    for at in (0..=chain.len()).rev() {
        let below = at.checked_sub(1).and_then(|i| chain.get(i));
        let above = chain.get(at);
        let v1 = below.map(|b| builds_on(&adding[0], b)).unwrap_or(Ok(()));
        let v2 = above.map(|a| builds_on(a, &adding[adding.len() - 1])).unwrap_or(Ok(()));
        if v1.is_ok() && v2.is_ok() {
            return Ok(at);
        }
        if first.is_none() {
            first = v1.err().or(v2.err());
        }
    }
    Err(first.unwrap_or_else(|| "Doesn't fit".into()))
}

/// Something dropped in the Delete bucket, kept to drag back out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "lowercase")]
pub enum Deleted {
    Module { id: String, module: Module, from: String, at: f64 },
    Sub { id: String, sub: AnySub, from: String, at: f64 },
}

impl Deleted {
    pub fn id(&self) -> &str {
        match self {
            Deleted::Module { id, .. } | Deleted::Sub { id, .. } => id,
        }
    }
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A new id: `<prefix>-<time>-<n>` (the web app's uid).
pub fn uid(prefix: &str) -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("{prefix}-{}-{}", radix36(ms as u64), radix36(SEQ.fetch_add(1, Ordering::Relaxed)))
}

fn radix36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = vec![];
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

/// A module's name: a customization's is its look's.
pub fn module_name(m: &Module) -> String {
    match m {
        Module::Customization(c) => {
            look_of(c).map(|l| l.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Untitled".into())
        }
        Module::Template(t) => t.name.clone(),
        Module::Extensions(e) => e.name.clone(),
    }
}

/// The stack's display name: its customization's, else what's below.
pub fn stack_name(s: &Stack) -> String {
    let non_empty = |x: Option<&str>| x.filter(|n| !n.is_empty()).map(String::from);
    non_empty(s.look().map(|l| l.name.as_str()))
        .or_else(|| non_empty(s.extensions.as_ref().map(|e| e.name.as_str())))
        .or_else(|| non_empty(s.top().map(|t| t.name.as_str())))
        .unwrap_or_else(|| "Loose modules".into())
}

/// A new customization's background until one's picked (not one of the
/// choices, so it reads as "not set").
pub const BLANK_WALLPAPER: &str = "linear-gradient(160deg, #17112a 0%, #0e0919 100%)";

pub fn blank_wallpaper() -> Value {
    json!({ "type": "gradient", "value": BLANK_WALLPAPER })
}

/// Whether a wallpaper is the blank one (or none).
pub fn is_blank_wallpaper(w: Option<&Value>) -> bool {
    match w {
        None | Some(Value::Null) => true,
        Some(w) => w["value"] == BLANK_WALLPAPER,
    }
}

/// A module nothing's been put in yet.
pub fn is_blank(m: &Module) -> bool {
    match m {
        Module::Template(t) => t.apps.is_empty(),
        Module::Extensions(e) => e.subs.is_empty(),
        Module::Customization(c) => {
            let look = look_of(c);
            look.is_none_or(|l| l.name.trim().is_empty())
                && is_blank_wallpaper(look.map(|l| &l.wallpaper))
                && folders_of(c).is_none_or(|f| f.mounts.is_empty())
                && secrets_of(c).is_none_or(|s| s.by_slot.is_empty())
                && !secrets_of(c).and_then(|s| s.groups.as_ref()).is_some_and(|g| g.iter().any(|g| g.key.is_some()))
        }
    }
}

/// App setups in the stack's Extensions for apps its template doesn't have.
pub fn stray_apps(s: &Stack) -> Vec<String> {
    let mut out = vec![];
    for x in s.extensions.iter().flat_map(|e| e.subs.iter()) {
        let (id, name) = match x {
            AppSub::Vscode(_) => ("vscode", "VS Code"),
            AppSub::Browser(_) => ("wadbrowser", "WadBrowser"),
            AppSub::Tools(_) => continue,
        };
        if !s.has_app(id) {
            out.push(name.to_string());
        }
    }
    out
}

/// What a stack still needs before it can be built.
pub fn missing_for(s: &Stack) -> Vec<String> {
    let mut out = vec![];
    if s.templates.is_empty() {
        out.push("a template".into());
    } else if s.apps().is_empty() {
        out.push("an app in the template".into());
    } else {
        for a in stray_apps(s) {
            out.push(format!("{a} in the template (its Extensions set it up)"));
        }
    }
    if s.customization.is_none() {
        out.push("a customization".into());
    } else if s.look().is_none_or(|l| l.name.trim().is_empty()) {
        out.push("a name".into());
    }
    out
}

pub fn status_of(s: &Stack) -> &'static str {
    if s.templates.is_empty() {
        "no-template"
    } else if s.customization.is_none() {
        "no-customization"
    } else {
        "ready"
    }
}

/// A copy with new ids, so a clone never shares a module with its source.
pub fn clone_module(m: &Module, rename: bool) -> Module {
    let mut copy = m.clone();
    match &mut copy {
        Module::Template(t) => {
            t.id = uid("template");
            if rename {
                t.name = format!("{} copy", t.name);
            }
        }
        Module::Extensions(e) => {
            e.id = uid("extensions");
            e.subs = e.subs.iter().map(clone_app_sub).collect();
            if rename {
                e.name = format!("{} copy", e.name);
            }
        }
        Module::Customization(c) => {
            c.id = uid("customization");
            c.subs = c.subs.iter().map(clone_custom_sub).collect();
            if rename {
                for x in &mut c.subs {
                    if let CustomSub::Look(l) = x {
                        l.name = format!("{} copy", l.name);
                    }
                }
            }
        }
    }
    copy
}

pub fn clone_app_sub(s: &AppSub) -> AppSub {
    let mut c = s.clone();
    c.set_id(uid(s.key()));
    c
}

pub fn clone_custom_sub(s: &CustomSub) -> CustomSub {
    let mut c = s.clone();
    c.set_id(uid(s.key()));
    c
}

pub fn clone_sub(s: &AnySub) -> AnySub {
    match s {
        AnySub::App(a) => AnySub::App(clone_app_sub(a)),
        AnySub::Custom(c) => AnySub::Custom(clone_custom_sub(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_new_each_time_and_copies_get_their_own() {
        assert_ne!(uid("x"), uid("x"));
        let t = Module::Template(TemplateModule::new("t1", "Dev", vec!["vscode".into()]));
        let c = clone_module(&t, true);
        assert_ne!(c.id(), "t1");
        assert_eq!(module_name(&c), "Dev copy");
        assert_eq!(radix36(35), "z");
        assert_eq!(radix36(36), "10");
    }

    #[test]
    fn key_groups_without_ids_read_and_write_as_they_are() {
        let v = json!({ "id": "s", "origin": "S", "bySlot": {}, "groups": [{ "members": ["browser"], "key": { "id": "k" } }] });
        let sub: SecretsSub = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(sub.groups.as_ref().map(|g| g[0].members.len()), Some(1));
        assert_eq!(serde_json::to_value(&sub).unwrap(), v);
    }

    #[test]
    fn modules_read_and_write_as_the_web_app_keeps_them() {
        let v = json!({ "layer": "extensions", "id": "e", "name": "E", "subs": [
            { "key": "wadbrowser", "id": "b", "origin": "E", "mode": "focus", "webapps": [] },
            { "key": "tools", "id": "t", "origin": "E", "tools": ["go"] },
        ] });
        let m: Module = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(m.layer(), Layer::Extensions);
        assert_eq!(serde_json::to_value(&m).unwrap(), v);
        let t: Module = serde_json::from_value(
            json!({ "layer": "template", "id": "t", "name": "T", "description": "", "apps": [] }),
        )
        .unwrap();
        assert_eq!(t.layer(), Layer::Template);
    }
}
