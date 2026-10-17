//! The board's edits (the pure part of the client's builder/store.ts):
//! placing modules and submodules, new stacks, deleting into Recently
//! deleted and back, selection, saving an editor's module, renaming (shared
//! templates and Extensions together), keys, fixes, undo. The app saves the
//! stacks after each change ([`Board::version`] moves) and shows the notes
//! an edit returns as toasts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalog::Fix;
use crate::model::*;

/// Where dragged modules come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum From {
    #[serde(rename_all = "camelCase")]
    Stack { stack_id: String },
    #[serde(rename_all = "camelCase")]
    Deleted { deleted_id: String },
}

/// Where a dragged submodule comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SubFrom {
    #[serde(rename_all = "camelCase")]
    Module { stack_id: String, layer: Layer },
    #[serde(rename_all = "camelCase")]
    Deleted { deleted_id: String },
}

/// A toast: what happened, and whether Undo goes with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub text: String,
    pub undo: bool,
}

fn note(text: impl Into<String>, undo: bool) -> Option<Note> {
    Some(Note { text: text.into(), undo })
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// The stack without these modules (by id); a stack left empty goes.
pub fn without(stacks: &[Stack], stack_id: &str, ids: &[String]) -> Vec<Stack> {
    stacks
        .iter()
        .map(|s| {
            if s.id != stack_id {
                return s.clone();
            }
            let mut n = s.clone();
            n.templates.retain(|t| !ids.contains(&t.id));
            if n.extensions.as_ref().is_some_and(|e| ids.contains(&e.id)) {
                n.extensions = None;
            }
            if n.customization.as_ref().is_some_and(|c| ids.contains(&c.id)) {
                n.customization = None;
            }
            n
        })
        .filter(|s| !s.modules().is_empty())
        .collect()
}

/// `stack` with `modules` added: a layer it has is replaced; templates go
/// where they build on what's below, and a template that doesn't fit
/// replaces the stack's templates. (The stack, what it replaced.)
pub fn place(stack: &Stack, modules: &[Module]) -> Result<(Stack, Vec<Module>), String> {
    let adding: Vec<TemplateModule> =
        modules.iter().filter_map(|m| if let Module::Template(t) = m { Some(t.clone()) } else { None }).collect();
    for i in 1..adding.len() {
        builds_on(&adding[i], &adding[i - 1])?;
    }
    let mut next = stack.clone();
    let mut replaced: Vec<Module> = vec![];
    for m in modules {
        match m {
            Module::Template(_) => {}
            Module::Extensions(e) => {
                if let Some(there) = &stack.extensions {
                    replaced.push(Module::Extensions(there.clone()));
                }
                next.extensions = Some(e.clone());
            }
            Module::Customization(c) => {
                if let Some(there) = &stack.customization {
                    replaced.push(Module::Customization(there.clone()));
                }
                next.customization = Some(c.clone());
            }
        }
    }
    if !adding.is_empty() {
        match fit_templates(&next.templates, &adding) {
            Ok(at) => {
                next.templates.splice(at..at, adding);
            }
            Err(_) => {
                replaced.extend(next.templates.iter().cloned().map(Module::Template));
                next.templates = adding;
            }
        }
    }
    Ok((next, replaced))
}

/// "its Extensions", "its templates"…: what a drop would replace.
pub fn replacing(ms: &[Module]) -> String {
    let mut words: Vec<&str> = vec![];
    for m in ms {
        let w = match m.layer() {
            Layer::Template => "templates",
            Layer::Extensions => "Extensions",
            Layer::Customization => "Customization",
        };
        if !words.contains(&w) {
            words.push(w);
        }
    }
    words.iter().map(|w| format!("its {w}")).collect::<Vec<_>>().join(" and ")
}

fn apps_sig(t: &TemplateModule) -> String {
    let mut a = t.apps.clone();
    a.sort();
    a.join(",")
}

/// Copies of `base` with these modules on top; a dragged template the same
/// as one in `base` folds into it.
pub fn on_base(modules: &[Module], base: &[TemplateModule]) -> Vec<Module> {
    let have: Vec<String> = base.iter().map(apps_sig).collect();
    let mut out: Vec<Module> = base.iter().map(|t| clone_module(&Module::Template(t.clone()), false)).collect();
    out.extend(modules.iter().filter(|m| !matches!(m, Module::Template(t) if have.contains(&apps_sig(t)))).cloned());
    out
}

/// A new stack holding these modules.
pub fn fresh(modules: &[Module]) -> Stack {
    match place(&Stack::new(uid("stack")), modules) {
        Ok((s, _)) => s,
        Err(_) => Stack::new(uid("stack")),
    }
}

/// A copied customization's name: the original's with " - n", n counting
/// the copies.
pub fn copy_name(name: &str, taken: &[String]) -> String {
    let base = strip_counter(name);
    if base.trim().is_empty() {
        return name.to_string();
    }
    let prefix = format!("{base} - ");
    let n = taken
        .iter()
        .filter_map(|t| t.strip_prefix(&prefix))
        .filter(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|rest| rest.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("{base} - {}", n + 1)
}

/// `name` without a " - <digits>" end.
fn strip_counter(name: &str) -> &str {
    if let Some(i) = name.rfind(" - ") {
        let rest = &name[i + 3..];
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return &name[..i];
        }
    }
    name
}

/// Templates with the same apps are one template (the same image).
pub fn same_apps(a: &TemplateModule, b: &TemplateModule) -> bool {
    a.apps.len() == b.apps.len() && a.apps.iter().all(|x| b.apps.contains(x))
}

/// Renames a shared template everywhere it's used: the stacks, and how
/// many it touched.
pub fn rename_template(stacks: &[Stack], t: &TemplateModule, name: &str) -> (Vec<Stack>, usize) {
    let mut count = 0;
    let next = stacks
        .iter()
        .map(|st| {
            if !st.templates.iter().any(|x| same_apps(x, t)) {
                return st.clone();
            }
            count += 1;
            let mut st = st.clone();
            for x in &mut st.templates {
                if same_apps(x, t) {
                    x.name = name.to_string();
                }
            }
            st
        })
        .collect();
    (next, count)
}

/// What makes two Extensions modules the same layer (the same image): the
/// template's apps and their app setups.
pub fn ext_sig(s: &Stack) -> Option<String> {
    let e = s.extensions.as_ref()?;
    let sorted = |xs: &[String]| {
        let mut v = xs.to_vec();
        v.sort();
        v.join(",")
    };
    let mut subs = e.subs.clone();
    subs.sort_by(|a, b| a.key().cmp(b.key()));
    let parts: Vec<String> = subs
        .iter()
        .map(|x| match x {
            AppSub::Vscode(v) => format!("vscode:{}", sorted(&v.extensions)),
            AppSub::Tools(t) => format!("tools:{}", sorted(&t.tools)),
            AppSub::Browser(b) => format!(
                "wadbrowser:{}:{}",
                b.mode,
                sorted(&b.webapps.iter().map(|w| format!("{}={}", w.id, w.url)).collect::<Vec<_>>())
            ),
        })
        .collect();
    Some(format!("{}#{}", sorted(s.apps()), parts.join("|")))
}

/// Why `name` can't be `m`'s (in stack `stack_id`), or None when it can.
/// Names are unique per kind, but shared modules share theirs.
pub fn name_clash(stacks: &[Stack], stack_id: &str, m: &Module, name: &str) -> Option<String> {
    let want = name.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    let same = |n: Option<&str>| n.unwrap_or("").trim().to_lowercase() == want;
    match m {
        Module::Customization(c) => stacks
            .iter()
            .find(|st| {
                st.id != stack_id
                    && st.customization.as_ref().is_some_and(|x| x.id != c.id)
                    && same(st.look().map(|l| l.name.as_str()))
            })
            .map(|o| format!("{} already uses that name", stack_name(o))),
        Module::Extensions(e) => {
            let sig = stacks.iter().find(|st| st.id == stack_id).and_then(|home| {
                let mut h = home.clone();
                h.extensions = Some(e.clone());
                ext_sig(&h)
            });
            stacks
                .iter()
                .find(|st| {
                    st.extensions.as_ref().is_some_and(|x| x.id != e.id && same(Some(&x.name))) && ext_sig(st) != sig
                })
                .map(|o| format!("{}'s Extensions already use that name", stack_name(o)))
        }
        Module::Template(t) => stacks
            .iter()
            .flat_map(|st| st.templates.iter())
            .find(|x| x.id != t.id && same(Some(&x.name)) && !same_apps(x, t))
            .map(|o| format!("Another template (with different apps) is already called {}", o.name)),
    }
}

/// The modules a grab takes: the one grabbed and every module on top of it,
/// or (only) just it.
pub fn grab(stack: &Stack, module_id: &str, only: bool) -> Vec<Module> {
    let all = stack.modules();
    match all.iter().position(|m| m.id() == module_id) {
        None => vec![],
        Some(at) if only => vec![all[at].clone()],
        Some(at) => all[at..].to_vec(),
    }
}

/// Every key there is of a kind: those given and the ones the stacks keep
/// (`{id, name, kind}`).
pub fn keys_of_kind(keys: &[Value], stacks: &[Stack], kind: &str) -> Vec<Value> {
    let mut out: Vec<(String, Value)> = vec![];
    let mut add = |k: &Value| {
        if k.get("kind").and_then(Value::as_str) != Some(kind) {
            return;
        }
        let id = k.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        match out.iter_mut().find(|(i, _)| *i == id) {
            Some((_, v)) => *v = k.clone(),
            None => out.push((id, k.clone())),
        }
    };
    keys.iter().for_each(&mut add);
    for st in stacks {
        if let Some(sub) = st.secrets() {
            sub.by_slot.values().for_each(&mut add);
            for g in sub.groups.iter().flatten() {
                if let Some(k) = &g.key {
                    add(k);
                }
            }
        }
    }
    out.into_iter().map(|(_, v)| v).collect()
}

/// The templates on the board a new template has all the apps of, as the
/// longest chain each building on the one before.
pub fn chain_under(t: &TemplateModule, stacks: &[Stack]) -> Vec<TemplateModule> {
    let within = |a: &TemplateModule, b: &[String]| a.apps.len() < b.len() && a.apps.iter().all(|x| b.contains(x));
    let mut found: Vec<(String, TemplateModule)> = vec![];
    for s in stacks {
        for x in &s.templates {
            if !x.apps.is_empty() && within(x, &t.apps) && !found.iter().any(|(k, _)| *k == apps_sig(x)) {
                found.push((apps_sig(x), x.clone()));
            }
        }
    }
    let mut c: Vec<TemplateModule> = found.into_iter().map(|(_, x)| x).collect();
    c.sort_by_key(|x| x.apps.len());
    let mut best: Vec<Vec<TemplateModule>> = c.iter().map(|x| vec![x.clone()]).collect();
    for i in 0..c.len() {
        for j in 0..i {
            if within(&c[j], &c[i].apps) && best[j].len() + 1 > best[i].len() {
                let mut b = best[j].clone();
                b.push(c[i].clone());
                best[i] = b;
            }
        }
    }
    best.into_iter().fold(vec![], |a, b| {
        let longer = b.len() > a.len();
        let tie_more_apps = b.len() == a.len()
            && b.last().map(|x| x.apps.len()).unwrap_or(0) > a.last().map(|x| x.apps.len()).unwrap_or(0);
        if longer || tie_more_apps { b } else { a }
    })
}

/// A copied Extensions module's name: shared with a stack it's the same
/// layer as, else "<original> - n".
fn name_copied_extensions(stacks: Vec<Stack>, stack_id: &str, ext_id: Option<&str>) -> Vec<Stack> {
    let Some(ext_id) = ext_id else { return stacks };
    let Some(st) = stacks.iter().find(|x| x.id == stack_id) else { return stacks };
    let Some(e) = st.extensions.as_ref().filter(|e| e.id == ext_id) else { return stacks };
    let sig = ext_sig(st);
    let others: Vec<&Stack> = stacks.iter().filter(|x| x.id != stack_id).collect();
    let name = match others.iter().find(|x| ext_sig(x) == sig) {
        Some(twin) => twin.extensions.as_ref().map(|t| t.name.clone()).unwrap_or_default(),
        None => copy_name(
            &e.name,
            &others
                .iter()
                .map(|x| x.extensions.as_ref().map(|e| e.name.clone()).unwrap_or_default())
                .collect::<Vec<_>>(),
        ),
    };
    stacks
        .into_iter()
        .map(|mut x| {
            if x.id == stack_id
                && let Some(e) = x.extensions.as_mut()
            {
                e.name = name.clone();
            }
            x
        })
        .collect()
}

/// A copied customization, named "<original> - n".
fn renamed(m: Module, stacks: &[Stack]) -> Module {
    let Module::Customization(mut c) = m else { return m };
    let taken: Vec<String> = stacks.iter().map(|s| s.look().map(|l| l.name.clone()).unwrap_or_default()).collect();
    for x in &mut c.subs {
        if let CustomSub::Look(l) = x
            && !l.name.trim().is_empty()
        {
            l.name = copy_name(&l.name, &taken);
        }
    }
    Module::Customization(c)
}

/// The modules as they land: moved as they are, or copied (new ids; copied
/// customizations renamed).
fn landing(modules: &[Module], from: &From, moving: bool, stacks: &[Stack]) -> Vec<Module> {
    if moving || matches!(from, From::Deleted { .. }) {
        return modules.to_vec();
    }
    modules.iter().map(|m| renamed(clone_module(m, false), stacks)).collect()
}

fn title(m: &Module) -> String {
    format!("{} ({})", module_name(m), m.layer().label().to_lowercase())
}

/// The board's state.
#[derive(Debug, Clone, Default)]
pub struct Board {
    pub stacks: Vec<Stack>,
    /// Recently deleted, newest first.
    pub deleted: Vec<Deleted>,
    past: Vec<(Vec<Stack>, Vec<Deleted>)>,
    /// Modules picked out with Ctrl+click.
    pub selected: Vec<String>,
    /// Modules waiting on "Are you sure?" (a whole stack among them).
    pub confirming: Option<Vec<String>>,
    /// Moves with every change to the stacks (they're to be saved).
    pub version: u64,
}

impl Board {
    pub fn new(stacks: Vec<Stack>, deleted: Vec<Deleted>) -> Self {
        Board { stacks, deleted, ..Default::default() }
    }

    /// Every change goes through here, so it can be undone.
    fn commit(&mut self, stacks: Vec<Stack>, deleted: Option<Vec<Deleted>>) {
        let keep = self.past.len().saturating_sub(40);
        self.past.drain(..keep);
        self.past.push((self.stacks.clone(), self.deleted.clone()));
        self.stacks = stacks;
        if let Some(d) = deleted {
            self.deleted = d;
        }
        self.version += 1;
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some((stacks, deleted)) = self.past.pop() else { return false };
        self.stacks = stacks;
        self.deleted = deleted;
        self.version += 1;
        true
    }

    pub fn stack(&self, id: &str) -> Option<&Stack> {
        self.stacks.iter().find(|s| s.id == id)
    }

    fn take_from_deleted(&self, from_deleted: Option<&str>) -> Vec<Deleted> {
        match from_deleted {
            Some(id) => self.deleted.iter().filter(|d| d.id() != id).cloned().collect(),
            None => self.deleted.clone(),
        }
    }

    fn source_name(&self, stack_id: Option<&str>) -> String {
        match stack_id {
            Some(id) => stack_name(self.stack(id).unwrap_or(&Stack::default())),
            None => "Recently deleted".into(),
        }
    }

    /// Whether `modules` can go on `stack_id`: Ok(what they'd replace there).
    pub fn can_place(&self, modules: &[Module], stack_id: &str, from: &From) -> Result<Option<String>, Option<String>> {
        let stack = self.stack(stack_id).ok_or(None)?;
        if matches!(from, From::Stack { stack_id: f } if f == stack_id) {
            return Err(Some("Already in this stack".into()));
        }
        let (_, replaced) = place(stack, modules).map_err(Some)?;
        Ok((!replaced.is_empty()).then(|| replacing(&replaced)))
    }

    /// Copies them onto the stack, or moves them there with `moving`.
    pub fn place_modules(&mut self, modules: &[Module], from: &From, stack_id: &str, moving: bool) -> Option<Note> {
        let landed = landing(modules, from, moving, &self.stacks);
        let mut list = self.stacks.clone();
        if let From::Stack { stack_id: f } = from
            && moving
        {
            list = without(&list, f, &modules.iter().map(|m| m.id().to_string()).collect::<Vec<_>>());
        }
        let mut gone: Vec<Deleted> = vec![];
        list = list
            .into_iter()
            .map(|st| {
                if st.id != stack_id {
                    return st;
                }
                match place(&st, &landed) {
                    Ok((next, replaced)) => {
                        // What it replaced waits in Recently deleted.
                        gone = replaced
                            .into_iter()
                            .map(|module| Deleted::Module { id: uid("d"), module, from: stack_name(&st), at: now_ms() })
                            .collect();
                        next
                    }
                    Err(_) => st,
                }
            })
            .collect();
        if !moving && matches!(from, From::Stack { .. }) {
            let ext = landed.iter().find_map(|m| if let Module::Extensions(e) = m { Some(e.id.clone()) } else { None });
            list = name_copied_extensions(list, stack_id, ext.as_deref());
        }
        let from_deleted = if let From::Deleted { deleted_id } = from { Some(deleted_id.as_str()) } else { None };
        let mut deleted = gone.clone();
        deleted.extend(self.take_from_deleted(from_deleted));
        self.commit(list, Some(deleted));
        match gone.as_slice() {
            [] => None,
            [Deleted::Module { module, .. }] => {
                note(format!("Replaced a {}: it's in Recently deleted", module.layer().label()), true)
            }
            many => note(format!("Replaced {} modules: it's in Recently deleted", many.len()), true),
        }
    }

    /// Whether these modules, on copies of `base`, make a stack.
    pub fn can_start(&self, modules: &[Module], base: &[TemplateModule]) -> Result<(), String> {
        place(&Stack::default(), &on_base(modules, base)).map(drop)
    }

    /// Starts a new stack at `at` with them (copied, or moved), on copies of
    /// `base`.
    pub fn new_stack(
        &mut self,
        modules: &[Module],
        from: &From,
        at: usize,
        moving: bool,
        base: &[TemplateModule],
    ) -> String {
        let made = fresh(&on_base(&landing(modules, from, moving, &self.stacks), base));
        let id = made.id.clone();
        let ext = made.extensions.as_ref().map(|e| e.id.clone());
        // In first, then off its old stack, so `at` still means the gap.
        let mut list = self.stacks.clone();
        list.insert(at.min(list.len()), made);
        if let From::Stack { stack_id } = from
            && moving
        {
            list = without(&list, stack_id, &modules.iter().map(|m| m.id().to_string()).collect::<Vec<_>>());
        }
        if !moving && matches!(from, From::Stack { .. }) {
            list = name_copied_extensions(list, &id, ext.as_deref());
        }
        let from_deleted = if let From::Deleted { deleted_id } = from { Some(deleted_id.as_str()) } else { None };
        let deleted = self.take_from_deleted(from_deleted);
        self.commit(list, Some(deleted));
        id
    }

    /// Deletes one module; a grab of a whole stack asks first (`confirming`).
    pub fn delete_modules(&mut self, modules: &[Module], from: &From) -> Option<Note> {
        let From::Stack { stack_id } = from else { return None };
        let first = modules.first()?;
        let all = self.stack(stack_id).map(|s| s.modules()).unwrap_or_default();
        if all.iter().all(|m| modules.iter().any(|x| x.id() == m.id())) {
            self.confirming = Some(all.iter().map(|m| m.id().to_string()).collect());
            return None;
        }
        let name = self.source_name(Some(stack_id));
        let stacks = without(&self.stacks, stack_id, &[first.id().to_string()]);
        let mut deleted = vec![Deleted::Module { id: uid("d"), module: first.clone(), from: name, at: now_ms() }];
        deleted.extend(self.deleted.clone());
        self.commit(stacks, Some(deleted));
        note(format!("Moved {} to Recently deleted", title(first)), true)
    }

    /// Deletes these modules (by id): the confirmed path.
    pub fn delete_ids(&mut self, ids: &[String]) -> Option<Note> {
        let mut stacks = self.stacks.clone();
        let mut gone: Vec<Deleted> = vec![];
        for st in &self.stacks {
            let mine: Vec<Module> = st.modules().into_iter().filter(|m| ids.iter().any(|i| i == m.id())).collect();
            if mine.is_empty() {
                continue;
            }
            for module in &mine {
                gone.push(Deleted::Module { id: uid("d"), module: module.clone(), from: stack_name(st), at: now_ms() });
            }
            stacks = without(&stacks, &st.id, &mine.iter().map(|m| m.id().to_string()).collect::<Vec<_>>());
        }
        self.confirming = None;
        self.selected.clear();
        if gone.is_empty() {
            return None;
        }
        let whole: Vec<&Stack> =
            self.stacks.iter().filter(|st| st.modules().iter().all(|m| ids.iter().any(|i| i == m.id()))).collect();
        let n = gone.len();
        let one = match gone.as_slice() {
            [Deleted::Module { module, .. }] => title(module),
            _ => format!("{n} modules"),
        };
        let text = match whole.as_slice() {
            [w] if n == w.modules().len() => format!("Deleted {}: its modules are in Recently deleted", stack_name(w)),
            _ => format!("Moved {one} to Recently deleted"),
        };
        let mut deleted = gone;
        deleted.extend(self.deleted.clone());
        self.commit(stacks, Some(deleted));
        note(text, true)
    }

    pub fn toggle_select(&mut self, id: &str) {
        match self.selected.iter().position(|x| x == id) {
            Some(i) => {
                self.selected.remove(i);
            }
            None => self.selected.push(id.to_string()),
        }
    }

    /// Deletes what's selected; asks first if that's a whole stack.
    pub fn delete_selected(&mut self) -> Option<Note> {
        if self.selected.is_empty() {
            return None;
        }
        let sel = self.selected.clone();
        if self.stacks.iter().any(|st| st.modules().iter().all(|m| sel.iter().any(|x| x == m.id()))) {
            self.confirming = Some(sel);
            None
        } else {
            self.delete_ids(&sel)
        }
    }

    /// Whether a submodule can go in a stack's layer.
    pub fn can_place_sub(
        &self,
        sub: &AnySub,
        stack_id: &str,
        layer: Layer,
        from: &SubFrom,
    ) -> Result<(), Option<String>> {
        let parent = sub.parent();
        if layer != parent {
            return Err(Some(
                if parent == Layer::Extensions {
                    "App setups go in an Extensions module"
                } else {
                    "This goes in a Customization module"
                }
                .into(),
            ));
        }
        let stack = self.stack(stack_id).ok_or(None)?;
        let has = match parent {
            Layer::Extensions => stack.extensions.is_some(),
            _ => stack.customization.is_some(),
        };
        if !has {
            return Err(None);
        }
        if matches!(from, SubFrom::Module { stack_id: f, layer: l } if f == stack_id && *l == layer) {
            return Err(Some("Already in this module".into()));
        }
        let name = match sub.key() {
            "vscode" => Some("VS Code"),
            "wadbrowser" => Some("WadBrowser"),
            _ => None,
        };
        if let Some(name) = name
            && !stack.apps().is_empty()
            && !stack.has_app(sub.key())
        {
            return Err(Some(format!("{name} isn't in {}'s template", stack_name(stack))));
        }
        Ok(())
    }

    fn edit_subs(stacks: &mut [Stack], stack_id: &str, layer: Layer, f: impl FnOnce(&mut Vec<AnySub>)) {
        let Some(st) = stacks.iter_mut().find(|s| s.id == stack_id) else { return };
        match layer {
            Layer::Extensions => {
                if let Some(e) = st.extensions.as_mut() {
                    let mut subs: Vec<AnySub> = e.subs.drain(..).map(AnySub::App).collect();
                    f(&mut subs);
                    e.subs =
                        subs.into_iter().filter_map(|x| if let AnySub::App(a) = x { Some(a) } else { None }).collect();
                }
            }
            _ => {
                if let Some(c) = st.customization.as_mut() {
                    let mut subs: Vec<AnySub> = c.subs.drain(..).map(AnySub::Custom).collect();
                    f(&mut subs);
                    c.subs = subs
                        .into_iter()
                        .filter_map(|x| if let AnySub::Custom(a) = x { Some(a) } else { None })
                        .collect();
                }
            }
        }
    }

    /// Puts a submodule in a stack's layer (copied, or moved), replacing
    /// one of its key there (that one waits in Recently deleted).
    pub fn place_sub(&mut self, sub: &AnySub, from: &SubFrom, to_stack: &str, to_layer: Layer, moving: bool) {
        let mut stacks = self.stacks.clone();
        let from_deleted = if let SubFrom::Deleted { deleted_id } = from { Some(deleted_id.as_str()) } else { None };
        let mut deleted = self.take_from_deleted(from_deleted);
        if let SubFrom::Module { stack_id, layer } = from
            && moving
        {
            let id = sub.id().to_string();
            Self::edit_subs(&mut stacks, stack_id, *layer, |subs| subs.retain(|x| x.id() != id));
        }
        let placed = if moving || from_deleted.is_some() { sub.clone() } else { clone_sub(sub) };
        let where_ = stack_name(stacks.iter().find(|s| s.id == to_stack).unwrap_or(&Stack::default()));
        let key = sub.key();
        Self::edit_subs(&mut stacks, to_stack, to_layer, |subs| {
            if let Some(old) = subs.iter().find(|x| x.key() == key) {
                deleted.insert(0, Deleted::Sub { id: uid("d"), sub: old.clone(), from: where_.clone(), at: now_ms() });
            }
            subs.retain(|x| x.key() != key);
            subs.push(placed);
        });
        self.commit(stacks, Some(deleted));
    }

    pub fn delete_sub(&mut self, sub: &AnySub, from: &SubFrom) -> Option<Note> {
        let SubFrom::Module { stack_id, layer } = from else { return None };
        let mut stacks = self.stacks.clone();
        let id = sub.id().to_string();
        Self::edit_subs(&mut stacks, stack_id, *layer, |subs| subs.retain(|x| x.id() != id));
        let mut deleted =
            vec![Deleted::Sub { id: uid("d"), sub: sub.clone(), from: self.source_name(Some(stack_id)), at: now_ms() }];
        deleted.extend(self.deleted.clone());
        self.commit(stacks, Some(deleted));
        note("Submodule moved to Recently deleted", true)
    }

    /// Saves an editor's module: into its stack, or (`fresh`) as a new
    /// module, or (on `new_board`) a new stack.
    pub fn save_module(&mut self, stack_id: &str, fresh_module: bool, new_board: bool, m: Module) -> Option<Note> {
        if fresh_module {
            if is_blank(&m) {
                return note(format!("Nothing was set up, so no {} was added", m.layer().label()), false);
            }
            // A new template with all the apps of templates on the board builds on them.
            let chain = match &m {
                Module::Template(t) if self.stack(stack_id).is_none_or(|s| s.templates.is_empty()) => {
                    chain_under(t, &self.stacks)
                }
                _ => vec![],
            };
            let mut modules: Vec<Module> =
                chain.iter().map(|t| clone_module(&Module::Template(t.clone()), false)).collect();
            modules.push(m.clone());
            let told = (!chain.is_empty()).then(|| {
                format!(
                    "{} has all the apps of {}, so it's stacked on {}",
                    module_name(&m),
                    chain.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(" → "),
                    if chain.len() == 1 { "it" } else { "them" }
                )
            });
            if new_board {
                let mut list = self.stacks.clone();
                list.push(fresh(&modules));
                self.commit(list, None);
                return told.and_then(|t| note(t, true));
            }
            let list = self
                .stacks
                .iter()
                .map(|st| {
                    if st.id == stack_id {
                        place(st, &modules).map(|(s, _)| s).unwrap_or_else(|_| st.clone())
                    } else {
                        st.clone()
                    }
                })
                .collect();
            self.commit(list, None);
            return told.and_then(|t| note(t, true));
        }
        // A shared template renamed (its apps unchanged) is renamed everywhere.
        let was = match &m {
            Module::Template(t) => {
                self.stack(stack_id).and_then(|st| st.templates.iter().find(|x| x.id == t.id)).cloned()
            }
            _ => None,
        };
        let shared = match (&was, &m) {
            (Some(w), Module::Template(t)) if w.name != t.name && same_apps(w, t) => {
                Some(rename_template(&self.stacks, w, &t.name))
            }
            _ => None,
        };
        // A stack's bottom template whose apps changed to take in all of
        // another stack's template builds on it, as a new one does (Dev
        // given Tiled sits on Dev). Its own stack isn't looked in: a
        // template only it has is changed where it is.
        let chain: Vec<TemplateModule> = match (&m, &was) {
            (Module::Template(t), Some(w))
                if !same_apps(w, t)
                    && !t.apps.is_empty()
                    && self.stack(stack_id).and_then(|st| st.templates.first()).is_some_and(|x| x.id == t.id) =>
            {
                let others: Vec<Stack> = self.stacks.iter().filter(|st| st.id != stack_id).cloned().collect();
                chain_under(t, &others)
            }
            _ => vec![],
        };
        let base = shared.as_ref().map(|(s, _)| s.clone()).unwrap_or_else(|| self.stacks.clone());
        let list = base
            .into_iter()
            .map(|mut st| {
                if st.id != stack_id {
                    return st;
                }
                match &m {
                    Module::Extensions(e) => st.extensions = Some(e.clone()),
                    Module::Customization(c) => st.customization = Some(c.clone()),
                    Module::Template(t) => {
                        // Templates above build on this one: an app added here reaches them too.
                        if let Some(at) = st.templates.iter().position(|x| x.id == t.id) {
                            let added: Vec<String> =
                                t.apps.iter().filter(|a| !st.templates[at].apps.contains(a)).cloned().collect();
                            for (i, x) in st.templates.iter_mut().enumerate() {
                                if i == at {
                                    *x = t.clone();
                                } else if i > at {
                                    for a in &added {
                                        if !x.apps.contains(a) {
                                            x.apps.push(a.clone());
                                        }
                                    }
                                }
                            }
                        }
                        if !chain.is_empty() {
                            let mut under: Vec<TemplateModule> = chain
                                .iter()
                                .filter_map(|c| match clone_module(&Module::Template(c.clone()), false) {
                                    Module::Template(c) => Some(c),
                                    _ => None,
                                })
                                .collect();
                            under.append(&mut st.templates);
                            st.templates = under;
                        }
                    }
                }
                st
            })
            .collect();
        self.commit(list, None);
        if !chain.is_empty() {
            let names: Vec<&str> = chain.iter().map(|c| c.name.as_str()).collect();
            return note(
                format!(
                    "{} has all the apps of {}, so it's stacked on {}",
                    module_name(&m),
                    names.join(" → "),
                    if chain.len() == 1 { "it" } else { "them" }
                ),
                true,
            );
        }
        match (shared, was, &m) {
            (Some((_, count)), Some(w), Module::Template(t)) if count > 1 => note(
                format!("Saved, and renamed {} to {} in all {count} wadspaces that share it", w.name, t.name),
                true,
            ),
            _ => note(format!("Saved {}", m.layer().label()), true),
        }
    }

    /// Renames a module where it sits (a customization's name is its
    /// look's): Err(why it can't), else a note for shared ones renamed.
    pub fn rename(&mut self, stack_id: &str, module_id: &str, name: &str) -> Result<Option<Note>, String> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(None);
        }
        let module = self.stack(stack_id).and_then(|st| st.modules().into_iter().find(|m| m.id() == module_id));
        if let Some(clash) = module.as_ref().and_then(|m| name_clash(&self.stacks, stack_id, m, name)) {
            return Err(clash);
        }
        if let Some(t) = self.stack(stack_id).and_then(|st| st.templates.iter().find(|x| x.id == module_id)).cloned() {
            let (stacks, count) = rename_template(&self.stacks, &t, name);
            self.commit(stacks, None);
            return Ok((count > 1).then(|| Note {
                text: format!("Renamed the {} template to {name} in all {count} wadspaces that share it", t.name),
                undo: true,
            }));
        }
        // Extensions shared by stacks (same template, same setups) are renamed together.
        if let Some(owner) = self.stacks.iter().find(|st| st.extensions.as_ref().is_some_and(|e| e.id == module_id)) {
            let sig = ext_sig(owner);
            let sharing = self.stacks.iter().filter(|st| ext_sig(st) == sig).count();
            let stacks = self
                .stacks
                .iter()
                .map(|st| {
                    let mut st = st.clone();
                    if ext_sig(&st) == sig
                        && let Some(e) = st.extensions.as_mut()
                    {
                        e.name = name.to_string();
                    }
                    st
                })
                .collect();
            self.commit(stacks, None);
            return Ok((sharing > 1).then(|| Note {
                text: format!("Renamed the shared Extensions in all {sharing} wadspaces that use them"),
                undo: true,
            }));
        }
        let stacks = self
            .stacks
            .iter()
            .map(|st| {
                let mut st = st.clone();
                if st.id != stack_id {
                    return st;
                }
                if let Some(c) = st.customization.as_mut().filter(|c| c.id == module_id) {
                    if let Some(CustomSub::Look(l)) = c.subs.iter_mut().find(|x| matches!(x, CustomSub::Look(_))) {
                        l.name = name.to_string();
                    } else {
                        c.subs.push(CustomSub::Look(LookSub {
                            id: uid("look"),
                            origin: name.into(),
                            name: name.into(),
                            description: String::new(),
                            wallpaper: blank_wallpaper(),
                            title_bars: false,
                        }));
                    }
                }
                st
            })
            .collect();
        self.commit(stacks, None);
        Ok(None)
    }

    /// Renames a key everywhere the board keeps it (`known`: the machine's,
    /// `{id, name}`): Err(why it can't).
    pub fn rename_key(&mut self, id: &str, name: &str, known: &[Value]) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Give it a name".into());
        }
        let used: Vec<Value> = self
            .stacks
            .iter()
            .filter_map(|st| st.secrets())
            .flat_map(|sub| {
                sub.by_slot
                    .values()
                    .cloned()
                    .chain(sub.groups.iter().flatten().filter_map(|g| g.key.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let other = known.iter().chain(used.iter()).find(|k| {
            k.get("id").and_then(Value::as_str) != Some(id)
                && k.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(name))
        });
        if let Some(o) = other {
            return Err(format!("Another key is already called {}", o["name"].as_str().unwrap_or_default()));
        }
        let swap = |k: &mut Value| {
            if k.get("id").and_then(Value::as_str) == Some(id) {
                k["name"] = name.into();
            }
        };
        let stacks = self
            .stacks
            .iter()
            .map(|st| {
                let mut st = st.clone();
                if let Some(c) = st.customization.as_mut() {
                    for x in &mut c.subs {
                        if let CustomSub::Secrets(sub) = x {
                            sub.by_slot.values_mut().for_each(swap);
                            for g in sub.groups.iter_mut().flatten() {
                                if let Some(k) = g.key.as_mut() {
                                    swap(k);
                                }
                            }
                        }
                    }
                }
                st
            })
            .collect();
        self.commit(stacks, None);
        Ok(())
    }

    /// Applies a fix to a stack's configuration.
    pub fn fix(&mut self, stack_id: &str, f: &Fix) -> Option<Note> {
        let stack = self.stack(stack_id)?.clone();
        let next = match f {
            Fix::AddApp { app_id, .. } => {
                // Onto the top template; a shared one whose apps change gets a name of its own.
                let top = stack.templates.last()?.clone();
                let mut apps = top.apps.clone();
                apps.push(app_id.clone());
                let clash = self.stacks.iter().any(|st| {
                    st.templates.iter().any(|t| {
                        t.id != top.id
                            && t.name == top.name
                            && !(t.apps.len() == apps.len() && t.apps.iter().all(|a| apps.contains(a)))
                    })
                });
                let mut name = top.name.clone();
                if clash {
                    let names: Vec<String> =
                        self.stacks.iter().flat_map(|st| st.templates.iter().map(|t| t.name.clone())).collect();
                    let mut n = 1;
                    while names.contains(&name) {
                        name = format!("{} - {n}", strip_counter(&top.name));
                        n += 1;
                    }
                }
                let mut s = stack.clone();
                if let Some(t) = s.templates.iter_mut().find(|t| t.id == top.id) {
                    t.apps = apps;
                    t.name = name;
                }
                s
            }
            Fix::RemoveKey { slot, .. } => {
                let mut s = stack.clone();
                Self::edit_secrets(&mut s, |sub| {
                    sub.by_slot.remove(slot);
                });
                s
            }
            Fix::RemoveApp { app_id, .. } => {
                let mut s = stack.clone();
                Self::edit_secrets(&mut s, |sub| {
                    sub.by_slot.retain(|k, _| {
                        !(*k == format!("app:{app_id}") || (app_id == "vscode" && k.starts_with("ext:")))
                    });
                    if app_id == "wadbrowser" {
                        sub.groups = None;
                    }
                });
                if let Some(e) = s.extensions.as_mut() {
                    e.subs.retain(|x| x.key() != app_id);
                }
                s
            }
        };
        let list = self.stacks.iter().map(|st| if st.id == stack_id { next.clone() } else { st.clone() }).collect();
        self.commit(list, None);
        note(format!("{}: done", f.label()), true)
    }

    fn edit_secrets(s: &mut Stack, f: impl FnOnce(&mut SecretsSub)) {
        if let Some(c) = s.customization.as_mut()
            && let Some(CustomSub::Secrets(sub)) = c.subs.iter_mut().find(|x| matches!(x, CustomSub::Secrets(_)))
        {
            f(sub);
        }
    }

    /// Adds a stack at the end with a blank template (a free name): its id.
    pub fn create_stack(&mut self) -> String {
        let names: Vec<String> =
            self.stacks.iter().flat_map(|st| st.templates.iter().map(|t| t.name.to_lowercase())).collect();
        let mut name = "New template".to_string();
        let mut n = 1;
        while names.contains(&name.to_lowercase()) {
            name = format!("New template - {n}");
            n += 1;
        }
        let stack = Stack {
            id: uid("stack"),
            templates: vec![TemplateModule::new(uid("template"), name, vec![])],
            ..Default::default()
        };
        let id = stack.id.clone();
        let mut list = self.stacks.clone();
        list.push(stack);
        self.commit(list, None);
        id
    }

    /// The stacks the selected modules are in.
    pub fn selected_stacks(&self) -> Vec<String> {
        self.stacks
            .iter()
            .filter(|st| st.modules().iter().any(|m| self.selected.iter().any(|x| x == m.id())))
            .map(|st| st.id.clone())
            .collect()
    }

    /// Takes in the documents' board (another device's change, or the first
    /// read): the stacks as they are now, undo kept unless asked.
    pub fn load(&mut self, stacks: Vec<Stack>, keep_undo: bool) {
        self.stacks = stacks;
        if !keep_undo {
            self.past.clear();
        }
    }
}

/// The layer image a module is ("template:<id>" / "custom:<id>"), if any.
pub fn image_of(m: &Module) -> Option<String> {
    match m {
        Module::Template(t) => t.doc.as_ref().map(|d| format!("template:{d}")),
        Module::Extensions(e) => e.doc.as_ref().map(|d| format!("custom:{d}")),
        Module::Customization(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t(id: &str, name: &str, apps: &[&str]) -> TemplateModule {
        TemplateModule::new(id, name, apps.iter().map(|a| a.to_string()).collect())
    }

    fn custom(id: &str, name: &str) -> CustomizationModule {
        CustomizationModule {
            layer: CustomizationTag,
            id: id.into(),
            subs: vec![CustomSub::Look(LookSub {
                id: format!("{id}:look"),
                origin: name.into(),
                name: name.into(),
                description: String::new(),
                wallpaper: blank_wallpaper(),
                title_bars: false,
            })],
        }
    }

    #[test]
    fn a_shared_template_given_more_apps_stacks_on_it_and_an_unshared_one_changes_in_place() {
        let mut b = board();
        // A copy of Alpha's stack shares Dev.
        let mut c = b.stacks[0].clone();
        c.id = "c".into();
        c.templates[0].id = "dev@c".into();
        b.stacks.push(c);
        let mut vg = b.stacks[2].templates[0].clone();
        vg.name = "VG Dev".into();
        vg.apps.extend(["tiled".to_string(), "libresprite".to_string()]);
        let n = b.save_module("c", false, false, Module::Template(vg)).unwrap();
        assert_eq!(n.text, "VG Dev has all the apps of Dev, so it's stacked on it");
        let names: Vec<&str> = b.stacks[2].templates.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Dev", "VG Dev"]);
        assert_eq!(b.stacks[0].templates.len(), 1);
        // Writing, only Beta's, given VS Code: still one template.
        let mut w = b.stacks[1].templates[0].clone();
        w.apps.push("vscode".into());
        let n = b.save_module("b", false, false, Module::Template(w)).unwrap();
        assert_eq!(n.text, "Saved Template");
        assert_eq!(b.stacks[1].templates.len(), 1);
        // A rename alone never stacks.
        let mut r = b.stacks[2].templates[1].clone();
        r.name = "Game Dev".into();
        b.save_module("c", false, false, Module::Template(r)).unwrap();
        assert_eq!(b.stacks[2].templates.len(), 2);
    }

    fn board() -> Board {
        let dev = t("dev@a", "Dev", &["wadbrowser", "vscode"]);
        let a = Stack {
            id: "a".into(),
            templates: vec![dev.clone()],
            extensions: None,
            customization: Some(custom("c@a", "Alpha")),
        };
        let b = Stack {
            id: "b".into(),
            templates: vec![t("w@b", "Writing", &["obsidian"])],
            extensions: None,
            customization: Some(custom("c@b", "Beta")),
        };
        Board::new(vec![a, b], vec![])
    }

    #[test]
    fn a_copied_customization_lands_renamed_and_what_it_replaces_waits_in_recently_deleted() {
        let mut b = board();
        let from = From::Stack { stack_id: "a".into() };
        let m = Module::Customization(b.stacks[0].customization.clone().unwrap());
        assert_eq!(b.can_place(std::slice::from_ref(&m), "b", &from), Ok(Some("its Customization".into())));
        let n = b.place_modules(&[m], &from, "b", false).unwrap();
        assert!(n.text.starts_with("Replaced a Customization"));
        assert_eq!(b.stacks[1].look().unwrap().name, "Alpha - 1");
        assert_eq!(b.deleted.len(), 1);
        assert!(b.undo());
        assert_eq!(b.stacks[1].look().unwrap().name, "Beta");
        assert!(b.deleted.is_empty());
    }

    #[test]
    fn deleting_a_whole_stack_asks_first_and_comes_back_from_recently_deleted() {
        let mut b = board();
        let all = b.stacks[0].modules();
        assert!(b.delete_modules(&all, &From::Stack { stack_id: "a".into() }).is_none());
        let ids = b.confirming.clone().unwrap();
        let n = b.delete_ids(&ids).unwrap();
        assert_eq!(n.text, "Deleted Alpha: its modules are in Recently deleted");
        assert_eq!(b.stacks.len(), 1);
        assert_eq!(b.deleted.len(), 2);
        // Dragged back out to a new stack.
        let d = b.deleted[0].clone();
        let Deleted::Module { id, module, .. } = d else { panic!() };
        b.new_stack(&[module], &From::Deleted { deleted_id: id }, 0, false, &[]);
        assert_eq!(b.stacks.len(), 2);
        assert_eq!(b.deleted.len(), 1);
    }

    #[test]
    fn templates_stack_where_they_build_on_what_is_below() {
        let b = board();
        let tiled = t("x", "Game Dev", &["wadbrowser", "vscode", "tiled"]);
        let (s, replaced) = place(&b.stacks[0], &[Module::Template(tiled.clone())]).unwrap();
        assert_eq!(s.templates.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["Dev", "Game Dev"]);
        assert!(replaced.is_empty());
        // One that doesn't fit replaces the stack's templates.
        let (s, replaced) = place(&b.stacks[0], &[Module::Template(t("k", "Art", &["krita"]))]).unwrap();
        assert_eq!(s.templates.len(), 1);
        assert_eq!(replaced.len(), 1);
        assert_eq!(chain_under(&tiled, &b.stacks).len(), 1);
    }

    #[test]
    fn shared_templates_are_renamed_together_and_names_clash_by_kind() {
        let mut b = board();
        let mut c = b.stacks[0].clone();
        c.id = "c".into();
        c.customization = Some(custom("c@c", "Gamma"));
        b.stacks.push(c);
        let n = b.rename("a", "dev@a", "Developer").unwrap().unwrap();
        assert!(n.text.contains("in all 2 wadspaces"));
        assert_eq!(b.stacks[2].templates[0].name, "Developer");
        assert_eq!(b.rename("a", "c@a", "beta"), Err("Beta already uses that name".into()));
    }

    #[test]
    fn fixes_and_keys() {
        let mut b = board();
        let key = json!({ "id": "k1", "name": "Work", "kind": "web" });
        if let Some(c) = b.stacks[1].customization.as_mut() {
            c.subs.push(CustomSub::Secrets(SecretsSub {
                id: "s".into(),
                origin: "".into(),
                by_slot: [("app:vscode".to_string(), key.clone())].into_iter().collect(),
                groups: None,
            }));
        }
        let problems = crate::catalog::problems_of(&b.stacks[1]);
        assert_eq!(problems[0].id, "missing:vscode");
        b.fix("b", &problems[0].fixes[1]);
        assert!(crate::catalog::problems_of(&b.stacks[1]).is_empty());
        assert_eq!(
            b.rename_key("k1", "work", &[json!({ "id": "k2", "name": "Work" })]),
            Err("Another key is already called Work".into())
        );
        let id = b.create_stack();
        assert_eq!(b.stack(&id).unwrap().templates[0].name, "New template");
        assert_eq!(copy_name("Vanua - 2", &["Vanua - 7".into()]), "Vanua - 8");
    }
}
