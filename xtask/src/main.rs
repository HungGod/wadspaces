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

/// The pictures the phone's desktop preview draws, 96 px PNGs in `dir`
/// (Android decodes no SVG): each catalog app's (app-<id>.png), the
/// project folder (folder.png) and the extensions with their own
/// (ext-<id>.png). The phone's scripts/sync-wadspaces.sh runs it.
fn phone_icons(dir: Option<&str>) -> Result<(), String> {
    let dir = std::path::PathBuf::from(dir.ok_or("usage: cargo xtask phone-icons <dir>")?);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let public = root().join("assets/public");
    let c = wad_core::catalog::catalog();
    let mut want: Vec<(String, String)> = c["apps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some((format!("app-{}", a["id"].as_str()?), a["icon"].as_str()?.to_string())))
        .collect();
    want.push(("folder".into(), "/folder.svg".into()));
    want.push(("ext-anthropic.claude-code".into(), "/icons/claude.png".into()));
    for (name, src) in &want {
        let path = public.join(src.trim_start_matches('/'));
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let img = wad_icons::decode::decode(&bytes).ok_or_else(|| format!("{}: not a picture", path.display()))?;
        let img = image::imageops::resize(&img, 96, 96, image::imageops::FilterType::Lanczos3);
        let out = dir.join(format!("{name}.png"));
        img.save(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    println!("{} icons in {}", want.len(), dir.display());
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
        "packages" => packages(&std::env::args().skip(2).collect::<Vec<_>>()),
        "phone-icons" => phone_icons(std::env::args().nth(2).as_deref()),
        _ => Err("usage: cargo xtask <ci|wasm|jscheck|packages|phone-icons>".into()),
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
