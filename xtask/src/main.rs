//! Repo tasks, run as `cargo xtask <task>`:
//!
//! - `ci`: everything CI checks (Rust fmt, clippy and tests; the wasm
//!   builds and is in budget).
//! - `wasm [--out <dir>]`: build wad-core for the web app (default
//!   `target/wasm-pkg/`; the web app's `npm run wasm` builds it into its
//!   `src/gen/wasm/`): wasm-bindgen (the same version as the crate) and
//!   wasm-opt (binaryen).
//! - `jscheck`: type-check WadBrowser's chrome (plain JS with JSDoc types)
//!   with tsc, fetched by npx.
//! - `layer <layers.json> <dir>`: the build folders wadd would build a
//!   wadspace's layers from (`{template, customization}`, as in
//!   images/examples/), written to `<dir>/template` and
//!   `<dir>/customization`; one line per layer on stdout:
//!   `<key> <image ref> <FROM> <sha>`. images/build.sh --example uses it.
//! - `packages waves [<id>…]`: packages CI builds (the catalog's, by
//!   default), with what they need, as JSON waves: each wave needs only
//!   earlier ones.
//! - `packages needs <id>`: what a package needs under it, one per line.
//! - `packages containerfile <id> [--base <ref>] [--need <id>=<ref>]…
//!   [--token]`: the Containerfile that captures it (wad-core's packages).
//! - `packages index <old index.json> <abi> <base digest> <result.json>…`:
//!   the index with these results in (CI's last step).
//!
//! Its tests check repo files no other crate owns (the base image's start-up
//! banner).

use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn run(dir: &str, cmd: &str, args: &[&str]) -> Result<(), String> {
    let dir = root().join(dir);
    eprintln!("$ (cd {}) {cmd} {}", dir.display(), args.join(" "));
    let status = Command::new(cmd).args(args).current_dir(&dir).status().map_err(|e| format!("{cmd}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{cmd} {} failed ({status})", args.join(" "))) }
}

/// The web app's wasm stays small: raw and gzipped bytes.
const WASM_BUDGET: (u64, u64) = (400 * 1024, 150 * 1024);

/// `out`: relative to where it's run from.
fn wasm(out: Option<&str>) -> Result<(), String> {
    run(
        ".",
        "cargo",
        &["build", "--quiet", "-p", "wad-wasm", "--target", "wasm32-unknown-unknown", "--profile", "wasm"],
    )?;
    let out = match out {
        Some(o) => std::env::current_dir().map_err(|e| e.to_string())?.join(o),
        None => root().join("target/wasm-pkg"),
    };
    let out = out.to_str().ok_or("the output folder's path isn't UTF-8")?;
    run(
        ".",
        "wasm-bindgen",
        &["target/wasm32-unknown-unknown/wasm/wad_wasm.wasm", "--target", "web", "--out-dir", out],
    )?;
    let file = format!("{out}/wad_wasm_bg.wasm");
    run(".", "wasm-opt", &["-Oz", "--all-features", "--strip-debug", &file, "-o", &file])?;
    let raw = std::fs::metadata(&file).map_err(|e| e.to_string())?.len();
    let gz = Command::new("gzip").args(["-9c", &file]).output().map_err(|e| e.to_string())?.stdout.len() as u64;
    eprintln!("wasm: {} KB ({} KB gzipped)", raw / 1024, gz / 1024);
    if raw > WASM_BUDGET.0 || gz > WASM_BUDGET.1 {
        return Err(format!("the wasm is over budget ({} / {} KB)", WASM_BUDGET.0 / 1024, WASM_BUDGET.1 / 1024));
    }
    Ok(())
}

fn ci() -> Result<(), String> {
    run(".", "cargo", &["fmt", "--all", "--check"])?;
    run(".", "cargo", &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])?;
    run(".", "cargo", &["test", "--workspace"])?;
    // wad-core still builds for the web, in budget.
    wasm(None)
}

/// The TypeScript the web app uses (wadspaces-client's package.json).
const TSC: &str = "typescript@7.0.2";

/// WadBrowser's chrome: plain JS with JSDoc types.
fn jscheck() -> Result<(), String> {
    run(".", "npx", &["--yes", "--package", TSC, "tsc", "-p", "apps/wadbrowser/ui"])
}

/// Writes a wadspace's layers' build folders (see the module's docs).
fn layer(docs: &str, dir: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(docs).map_err(|e| format!("{docs}: {e}"))?;
    let docs: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{docs}: {e}"))?;
    let template = &docs["template"];
    let mut layers = vec![("template", wad_core::layers::template_layer(template, &serde_json::Value::Null))];
    if !docs["customization"].is_null() {
        layers.push(("customization", wad_core::layers::extensions_layer(&docs["customization"], template)));
    }
    for (name, l) in layers {
        let out = PathBuf::from(dir).join(name);
        if out.exists() {
            std::fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        }
        for f in l["files"].as_array().into_iter().flatten() {
            let path = out.join(f["path"].as_str().unwrap_or_default());
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("{}: {e}", path.display()))?;
            std::fs::write(&path, f["content"].as_str().unwrap_or_default())
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let s = |k: &str| l[k].as_str().unwrap_or_default().to_string();
        println!("{} {} {} {}", s("key"), s("ref"), s("from"), s("sha"));
    }
    Ok(())
}

/// The package tasks (see the module's docs).
fn packages(args: &[String]) -> Result<(), String> {
    use wad_core::packages as p;
    const USAGE: &str = "usage: cargo xtask packages waves [<id>…] | needs <id> | graph | \
                         containerfile <id> [--base <ref>] [--need <id>=<ref>]… [--token] | \
                         index <old index.json> <abi> <base digest> <result.json>…";
    match args {
        // What the phone needs to name a wadspace's set as wadd does, and
        // to name templates as the board does: each catalog app's package
        // (null: the base has it) and name, the catalog's tools, the
        // features in install order, and what each needs.
        [cmd] if cmd == "graph" => {
            let c = wad_core::catalog::catalog();
            let apps: serde_json::Map<String, serde_json::Value> = c["apps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a["id"].as_str())
                .map(|id| (id.to_string(), p::of_app(id).map(serde_json::Value::from).unwrap_or_default()))
                .collect();
            let needs: serde_json::Map<String, serde_json::Value> =
                wad_core::spec::FEATURE_NEEDS.iter().map(|(f, n)| (f.to_string(), serde_json::json!(n))).collect();
            let names: serde_json::Map<String, serde_json::Value> = c["apps"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| (a["id"].as_str().unwrap_or_default().to_string(), a["name"].clone()))
                .collect();
            let tools: Vec<&str> =
                c["tools"].as_array().into_iter().flatten().filter_map(|t| t["id"].as_str()).collect();
            let graph = serde_json::json!({
                "apps": apps, "names": names, "tools": tools, "features": wad_core::spec::FEATURES, "needs": needs,
            });
            println!("{}", serde_json::to_string_pretty(&graph).map_err(|e| e.to_string())?);
            Ok(())
        }
        [cmd, ids @ ..] if cmd == "waves" => {
            let ids = if ids.is_empty() { p::all() } else { ids.to_vec() };
            println!("{}", serde_json::to_string(&p::waves(&ids)).map_err(|e| e.to_string())?);
            Ok(())
        }
        [cmd, id] if cmd == "needs" => {
            p::closure(&p::needs(id)).iter().for_each(|n| println!("{n}"));
            Ok(())
        }
        [cmd, id, rest @ ..] if cmd == "containerfile" => {
            let (mut base, mut needs, mut token) = (wad_core::spec::DEFAULT_BASE_IMAGE.to_string(), Vec::new(), false);
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--base" => base = it.next().ok_or(USAGE)?.clone(),
                    "--need" => {
                        let (n, r) = it.next().and_then(|v| v.split_once('=')).ok_or(USAGE)?;
                        needs.push((n.to_string(), r.to_string()));
                    }
                    "--token" => token = true,
                    _ => return Err(USAGE.into()),
                }
            }
            print!("{}", p::package_containerfile(id, &base, &needs, token)?);
            Ok(())
        }
        [cmd, old, abi, base, results @ ..] if cmd == "index" => {
            let read = |f: &str| -> Result<serde_json::Value, String> {
                let text = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
                serde_json::from_str(&text).map_err(|e| format!("{f}: {e}"))
            };
            // No index yet (the first run): start from nothing.
            let old = read(old).unwrap_or(serde_json::Value::Null);
            let results = results.iter().map(|f| read(f)).collect::<Result<Vec<_>, _>>()?;
            let index = p::merge_index(&old, abi, base, &results);
            println!("{}", serde_json::to_string_pretty(&index).map_err(|e| e.to_string())?);
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "ci" => ci(),
        "wasm" => match std::env::args().skip(2).collect::<Vec<_>>().as_slice() {
            [] => wasm(None),
            [flag, dir] if flag == "--out" => wasm(Some(dir)),
            _ => Err("usage: cargo xtask wasm [--out <dir>]".into()),
        },
        "jscheck" => jscheck(),
        "layer" => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            match args.as_slice() {
                [docs, dir] => layer(docs, dir),
                _ => Err("usage: cargo xtask layer <layers.json> <dir>".into()),
            }
        }
        "packages" => packages(&std::env::args().skip(2).collect::<Vec<_>>()),
        _ => Err("usage: cargo xtask <ci|wasm|jscheck|layer|packages>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::root;

    /// The base image's start-up banner (linuxserver's init-adduser prints it
    /// with `cat`): it shows the same in any terminal and in podman logs or
    /// journalctl, so plain printable ASCII, LF line ends, no trailing spaces,
    /// narrower than 80 columns, ending in a newline.
    #[test]
    fn the_branding_is_terminal_safe() {
        let path = root().join("images/base/root/etc/s6-overlay/s6-rc.d/init-adduser/branding");
        let text = std::fs::read(&path).expect("the branding file");
        assert!(text.ends_with(b"\n"), "ends in a newline");
        for (n, line) in text.split(|&b| b == b'\n').enumerate() {
            let n = n + 1;
            assert!(
                line.iter().all(|&b| (b' '..=b'~').contains(&b)),
                "line {n}: printable ASCII only (no tabs, CR or UTF-8)"
            );
            assert!(!line.ends_with(b" "), "line {n}: trailing spaces");
            assert!(line.len() <= 80, "line {n}: wider than 80 columns");
        }
    }
}
