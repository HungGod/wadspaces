//! Packages: an app's, a tool's or a VS Code extension's files on their own.
//!
//! Each is built once on the base by `wadspaces-pkg capture` (images/base),
//! FROM scratch: only the files the install added, plus fragments for what
//! installs share (dpkg's status, users). CI publishes them as
//! ghcr.io/hunggod/wadspaces-pkg-<id>; wadd pulls them (or builds one here)
//! and tags each by its image id. A layer's image is then the base with its
//! packages copied on and `wadspaces-pkg assemble`: nothing is installed
//! when a wadspace's image is built.
//!
//! The copying goes through chain images: one per prefix of an image's
//! packages (the base, then each package copied on, one build each), tagged
//! by what's in it. An image starts FROM the longest chain the machine has
//! and copies only the rest, so taking a package out of the middle copies
//! the ones above it again and adding one copies just that one; podman's own
//! cache would read every package again to check it (measured with
//! apps/wadd/dev/try-packages.sh: removing one in the middle 14 s vs 60 s,
//! adding an extension 7 s). Plain COPY, not --link: podman's --link (buildah
//! 1.43) reuses nothing after a base change and copies slower.
//!
//! A package's id is its feature's name (vscode, rust, electron-deps…),
//! `apt-<app>` for an app from Debian, or `ext-<marketplace id>` (lowercase)
//! for a VS Code extension.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::catalog::{self, Recipe};
use crate::layers::is_extension_id;
use crate::spec::{FEATURE_NEEDS, FEATURES};

/// Where CI publishes packages: `<REGISTRY>/wadspaces-pkg-<id>:<abi>`.
pub const REGISTRY: &str = "ghcr.io/hunggod";
/// A package image's name, before its id (local_ref, remote_ref).
pub const PREFIX: &str = "wadspaces-pkg-";
/// Chain images: `<CHAIN>:<sha12 of the base and packages>`.
pub const CHAIN: &str = "localhost/wadspaces-chain";

/// What a package installs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A `wadspaces-feature` script.
    Feature(&'static str),
    /// A catalog app's Debian packages (`wadspaces-apt`).
    Apt { app: String, packages: Vec<&'static str>, desktop: Option<&'static str> },
    /// A VS Code extension, by its marketplace id.
    Extension(String),
}

/// A catalog app's package; none for an app the base has (WadBrowser).
pub fn of_app(app: &str) -> Option<String> {
    match catalog::catalog_recipe(app)? {
        Recipe::Feature { feature, .. } => Some(feature.into()),
        Recipe::Apt { .. } => Some(format!("apt-{app}")),
        Recipe::Builtin { .. } => None,
    }
}

/// A command-line tool's package (tools are features).
pub fn of_tool(tool: &str) -> Option<String> {
    FEATURES.contains(&tool).then(|| tool.to_string())
}

/// A VS Code extension's package.
pub fn of_extension(ext: &str) -> Option<String> {
    is_extension_id(ext).then(|| format!("ext-{}", ext.to_lowercase()))
}

/// What package `id` installs, if it's one WadSpaces knows how to build.
pub fn source(id: &str) -> Option<Source> {
    if let Some(f) = FEATURES.iter().find(|f| **f == id) {
        return Some(Source::Feature(f));
    }
    if let Some(app) = id.strip_prefix("apt-") {
        return match catalog::catalog_recipe(app)? {
            Recipe::Apt { packages, desktop } => {
                Some(Source::Apt { app: app.into(), packages: packages.to_vec(), desktop })
            }
            _ => None,
        };
    }
    let ext = id.strip_prefix("ext-")?;
    is_extension_id(ext).then(|| Source::Extension(ext.into()))
}

/// What a package is called: its app's, tool's or VS Code extension's name
/// in the catalog, else its id.
pub fn name(id: &str) -> String {
    let c = catalog::catalog();
    let named = |k: &str, want: &dyn Fn(&str) -> bool| -> Option<String> {
        c[k].as_array()?.iter().find(|x| x["id"].as_str().is_some_and(want))?["name"].as_str().map(String::from)
    };
    if let Some(ext) = id.strip_prefix("ext-") {
        return named("extensions", &|x| x.to_lowercase() == ext).unwrap_or_else(|| ext.to_string());
    }
    named("apps", &|x| of_app(x).as_deref() == Some(id)).or_else(|| named("tools", &|x| x == id)).unwrap_or_else(|| {
        match id {
            "electron-deps" => "Electron's libraries".into(),
            "git" => "Git".into(),
            _ => id.to_string(),
        }
    })
}

/// The packages `id` needs under it.
pub fn needs(id: &str) -> Vec<&'static str> {
    if id.starts_with("ext-") {
        return vec!["vscode"];
    }
    FEATURE_NEEDS.iter().find(|(f, _)| *f == id).map(|(_, n)| n.to_vec()).unwrap_or_default()
}

/// Where a package goes in WadSpaces' order: features in install order
/// (what they need first, the big, common ones low), then Debian apps, then
/// extensions, each by id.
fn rank(id: &str) -> (u8, usize, &str) {
    match FEATURES.iter().position(|f| *f == id) {
        Some(i) => (0, i, id),
        None if id.starts_with("apt-") => (1, 0, id),
        None if id.starts_with("ext-") => (2, 0, id),
        None => (3, 0, id),
    }
}

/// `ids` in WadSpaces' order, once each.
pub fn canonical<S: AsRef<str>>(ids: &[S]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if !out.iter().any(|o| o == id.as_ref()) {
            out.push(id.as_ref().to_string());
        }
    }
    out.sort_by(|a, b| rank(a).cmp(&rank(b)));
    out
}

/// `ids` and everything they need, in WadSpaces' order.
pub fn closure<S: AsRef<str>>(ids: &[S]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut todo: Vec<String> = ids.iter().map(|s| s.as_ref().to_string()).collect();
    while let Some(id) = todo.pop() {
        if out.contains(&id) {
            continue;
        }
        todo.extend(needs(&id).into_iter().map(String::from));
        out.push(id);
    }
    canonical(&out)
}

/// The order an image stacks `wanted` in: the longest chain the machine
/// already stores (`stored`: its images' packages, bottom first, all on the
/// current base) made only of wanted packages, then the rest in WadSpaces'
/// order. COPY --link layers don't depend on each other, so any order gives
/// the same files; this one copies the least.
pub fn stack_order(wanted: &[String], stored: &[Vec<String>]) -> Vec<String> {
    let mut best: &[String] = &[];
    for chain in stored {
        let n = chain.iter().take_while(|id| wanted.contains(id)).count();
        if n > best.len() {
            best = &chain[..n];
        }
    }
    let rest: Vec<&String> = wanted.iter().filter(|id| !best.contains(id)).collect();
    best.iter().cloned().chain(canonical(&rest)).collect()
}

/// A package's image on this machine, by its image id (sha256 hex): what an
/// assembly copies from. Ids survive the stick's re-compression; registry
/// digests don't.
pub fn local_ref(id: &str, image_id: &str) -> String {
    let short: String = image_id.trim_start_matches("sha256:").chars().take(12).collect();
    format!("localhost/{PREFIX}{id}:{short}")
}

/// A package as CI publishes it, for the base's ABI.
pub fn remote_ref(id: &str, abi: &str) -> String {
    format!("{REGISTRY}/{PREFIX}{id}:{abi}")
}

/// The command that installs it, as wadspaces-pkg runs it.
fn install_command(src: &Source) -> Vec<String> {
    match src {
        Source::Feature(f) => vec!["wadspaces-feature".into(), f.to_string()],
        Source::Apt { app, packages, desktop } => {
            let mut apt = vec!["wadspaces-apt".to_string(), app.clone()];
            if let Some(d) = desktop {
                apt.extend(["--desktop".into(), d.to_string()]);
            }
            apt.extend(packages.iter().map(|p| p.to_string()));
            vec!["sh".into(), "-c".into(), format!("{} && wadspaces-apt --clean", apt.join(" "))]
        }
        Source::Extension(e) => vec!["wadspaces-vscode-extensions".into(), e.clone()],
    }
}

/// How to tell the installed version, for the package's label.
fn probe(id: &str, src: &Source) -> Option<String> {
    Some(match src {
        Source::Feature(f) => match *f {
            "vscode" => "code --version --no-sandbox --user-data-dir /tmp/wadspaces-probe".into(),
            "rust" => "rustc --version".into(),
            "cpp" => "cmake --version".into(),
            "nodejs" => "node --version".into(),
            "python" => "python3 --version".into(),
            "go" => "go version".into(),
            "android" => "cat /usr/local/android-sdk/wadspaces-versions".into(),
            "claude-code" => "claude --version".into(),
            "firebase" => "firebase --version".into(),
            "git" => "git --version".into(),
            _ => return None,
        },
        Source::Apt { packages, .. } => format!("dpkg-query -W -f='${{Version}}' {}", packages.first()?),
        Source::Extension(_) => {
            format!(
                "python3 -c 'import json; print(json.load(open(\"/usr/share/wadspaces/vscode-extensions.d/{}.json\"))[\"version\"])'",
                &id[4..]
            )
        }
    })
}

/// JSON exec form, so nothing in it is read by a shell.
fn exec(words: &[String]) -> String {
    serde_json::to_string(words).unwrap_or_default()
}

/// A package's Containerfile: the base, the packages under it (`needs`:
/// (id, image ref) for everything it needs, theirs too, in WadSpaces'
/// order), and the capture. `token`: the build gets the github_token secret
/// (GitHub's API, unthrottled).
pub fn package_containerfile(id: &str, base: &str, needs: &[(String, String)], token: bool) -> Result<String, String> {
    let src = source(id).ok_or_else(|| format!("WadSpaces doesn't know how to package {id}"))?;
    let want = closure(&self::needs(id));
    let missing: Vec<&str> = want.iter().filter(|n| !needs.iter().any(|(i, _)| i == *n)).map(String::as_str).collect();
    if !missing.is_empty() {
        return Err(format!("{id} needs {} first", missing.join(", ")));
    }
    let what = match &src {
        Source::Feature(f) => format!("wadspaces-feature {f}"),
        Source::Apt { app, .. } => format!("{app} from Debian"),
        Source::Extension(e) => format!("the VS Code extension {e}"),
    };
    let mut lines = vec![
        format!("# The {id} package: what {what} adds to the base. Generated by WadSpaces."),
        format!("ARG BASE_IMAGE={base}"),
        "FROM ${BASE_IMAGE} AS build".into(),
    ];
    for (_, r) in needs {
        lines.push(format!("COPY --from={r} / /"));
    }
    if !needs.is_empty() {
        lines.push("RUN wadspaces-pkg assemble".into());
    }
    let mut capture = vec!["wadspaces-pkg".to_string(), "capture".into(), id.into(), "--out".into(), "/out".into()];
    let names: Vec<&str> = needs.iter().map(|(i, _)| i.as_str()).collect();
    if !names.is_empty() {
        capture.extend(["--needs".into(), names.join(",")]);
    }
    if let Some(p) = probe(id, &src) {
        capture.extend(["--probe".into(), p]);
    }
    capture.push("--".into());
    capture.extend(install_command(&src));
    let mount = if token { "--mount=type=secret,id=github_token " } else { "" };
    lines.push(format!("RUN {mount}{}", exec(&capture)));
    lines.extend([
        String::new(),
        "FROM scratch".into(),
        "COPY --from=build /out/ /".into(),
        format!("LABEL io.wadspaces.pkg={} io.wadspaces.pkg.needs={}", quote(id), quote(&names.join(","))),
    ]);
    Ok(lines.join("\n") + "\n")
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// The chain image for the base (its image id) with `pkgs` ((id, image
/// ref), bottom first) copied on: the same packages on the same base are
/// the same chain.
pub fn chain_ref(base_id: &str, pkgs: &[(String, String)]) -> String {
    let mut h = Sha256::new();
    h.update(base_id.trim_start_matches("sha256:").as_bytes());
    for (_, r) in pkgs {
        h.update(b"|");
        h.update(r.as_bytes());
    }
    let hex: String = h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("{CHAIN}:{hex}")
}

/// One step of a chain: `from` (the base or the chain under it) with a
/// package copied on. Labelled with the chain so far, so wadd can tell what
/// chains it stores (`io.wadspaces.chain`, as `pkgs_label`) and on what base
/// (`io.wadspaces.from`, set by the build).
pub fn chain_step(from: &str, pkg_ref: &str, chain: &[(String, String)]) -> String {
    format!(
        "# A chain image. Generated by WadSpaces.\nFROM {from}\nCOPY --from={pkg_ref} / /\nLABEL io.wadspaces.chain={}\n",
        quote(&pkgs_label(chain))
    )
}

/// The start of a layer's Containerfile: its chain (the base with its
/// packages) assembled, or the base when it has no packages. What the layer
/// adds of its own (labels, web apps, root/) follows.
pub fn assembled(from: &str, has_packages: bool) -> Vec<String> {
    let mut lines = vec![format!("ARG BASE_IMAGE={from}"), "FROM ${BASE_IMAGE}".into(), String::new()];
    if has_packages {
        lines.push("RUN wadspaces-pkg assemble".into());
    }
    lines
}

/// The `io.wadspaces.pkgs` label: `id@image-id-12,…` in stack order.
pub fn pkgs_label(pkgs: &[(String, String)]) -> String {
    pkgs.iter()
        .map(|(id, r)| format!("{id}@{}", r.rsplit_once(':').map(|(_, t)| t).unwrap_or("")))
        .collect::<Vec<_>>()
        .join(",")
}

/// Reads an `io.wadspaces.pkgs` label back: package ids, bottom first.
pub fn pkgs_of_label(label: &str) -> Vec<String> {
    label.split(',').filter_map(|p| p.split('@').next()).filter(|p| !p.is_empty()).map(String::from).collect()
}

/// Every package the catalog can ask for (its apps, tools and extensions,
/// and what they need), in WadSpaces' order: what CI builds.
pub fn all() -> Vec<String> {
    let c = catalog::catalog();
    let ids =
        |k: &str| -> Vec<&str> { c[k].as_array().into_iter().flatten().filter_map(|v| v["id"].as_str()).collect() };
    let mut want: Vec<String> = ids("apps").into_iter().filter_map(of_app).collect();
    want.extend(ids("tools").into_iter().filter_map(of_tool));
    want.extend(ids("extensions").into_iter().filter_map(of_extension));
    closure(&want)
}

/// `ids` in waves for CI: each wave's packages need only earlier waves'.
pub fn waves<S: AsRef<str>>(ids: &[S]) -> Vec<Vec<String>> {
    let all = closure(ids);
    let mut done: Vec<String> = Vec::new();
    let mut out = Vec::new();
    while done.len() < all.len() {
        let wave: Vec<String> = all
            .iter()
            .filter(|id| !done.contains(id) && needs(id).iter().all(|n| done.iter().any(|d| d == n)))
            .cloned()
            .collect();
        if wave.is_empty() {
            break;
        }
        done.extend(wave.iter().cloned());
        out.push(wave);
    }
    out
}

/// The index CI publishes (the `packages` release's index.json), with this
/// run's packages in: `{version: 1, registry, abis: {<abi>: {base,
/// packages: {<id>: {digest, imageId, version, size, needs, content,
/// built}}}}}`. `results`: one object per package CI pushed, those fields
/// and its `id`. Other ABIs' entries stay, so machines on an older base
/// still find theirs.
pub fn merge_index(old: &Value, abi: &str, base: &str, results: &[Value]) -> Value {
    let mut index = if old.get("version").and_then(Value::as_u64) == Some(1) { old.clone() } else { Value::Null };
    if index.is_null() {
        index = json!({ "version": 1, "registry": REGISTRY, "abis": {} });
    }
    let entry = &mut index["abis"][abi];
    if !entry.is_object() {
        *entry = json!({ "base": base, "packages": {} });
    }
    entry["base"] = base.into();
    for r in results {
        let Some(id) = r["id"].as_str().filter(|id| source(id).is_some()) else { continue };
        let mut p = r.clone();
        if let Some(o) = p.as_object_mut() {
            o.remove("id");
        }
        entry["packages"][id] = p;
    }
    index
}

/// What the index says of package `id` for `abi`, if CI published it.
pub fn in_index<'a>(index: &'a Value, abi: &str, id: &str) -> Option<&'a Value> {
    index["abis"][abi]["packages"].get(id).filter(|p| p.is_object())
}

#[cfg(test)]
mod tests;
