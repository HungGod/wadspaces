//! WadSpaces' core logic, shared by wadd (native) and the UI (WebAssembly, via
//! wad-wasm): the Builder's layers (templates, extensions, wadspaces) and
//! their images, the catalog, projects, saved sign-ins, and the units wadd
//! writes.
//!
//! fixtures/core/goldens.json holds what it returns for a wide corpus, which
//! the tests here and the UI's check alike. Values are JSON, as in the
//! TypeScript wrappers, so objects keep their keys' order and absent fields
//! stay absent.
//!
//! [`call`] is the one entry point by name (the WebAssembly boundary uses it).

pub mod catalog;
pub mod generator;
pub mod js;
pub mod layers;
pub mod packages;
pub mod projects;
pub mod signins;
pub mod spec;
pub mod tar;
mod url;
pub mod v4;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

/// `{"$bytes": base64}` → bytes.
fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    v.get("$bytes").and_then(Value::as_str).and_then(|s| B64.decode(s).ok())
}

fn bytes_json(b: &[u8]) -> Value {
    json!({ "$bytes": B64.encode(b) })
}

/// Calls a core function by its TypeScript name with JSON arguments. Errors
/// are what the TypeScript threw (its message), or an unknown name.
pub fn call(name: &str, args: &[Value]) -> Result<Value, String> {
    let a = |i: usize| args.get(i).unwrap_or(&Value::Null);
    let s = |i: usize| args.get(i).and_then(Value::as_str).unwrap_or("");
    let opt_s = |i: usize| args.get(i).and_then(Value::as_str);
    let list = |i: usize| args.get(i).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    Ok(match name {
        // spec
        "slugify" => spec::slugify(s(0)).into(),
        "features" => json!(spec::FEATURES),
        // generator
        "wadbrowserConf" => generator::wadbrowser_conf(a(0)).map(Value::from).unwrap_or(Value::Null),
        "quadlet" => generator::quadlet(
            a(0),
            opt_s(1).unwrap_or(generator::STATE_DIR),
            args.get(2).and_then(Value::as_u64).map(|u| u as u32),
        )
        .into(),
        // catalog
        // projects
        "validateProject" => json!(projects::validate_project(a(0))),
        "cleanDraft" => projects::clean_draft(a(0)),
        "cleanSource" => projects::clean_source(a(0)),
        "sourceLabel" => projects::source_label(a(0)),
        "sourcePath" => projects::source_path(a(0)),
        "repoName" => projects::repo_name(s(0)).into(),
        "githubUrl" => projects::github_url(s(0)).map(Value::from).unwrap_or(Value::Null),
        "repoFullName" => projects::repo_full_name(s(0)).into(),
        "sameRepo" => projects::same_repo(s(0), s(1)).into(),
        "toMountName" => projects::to_mount_name(s(0)).into(),
        "freeMountName" => projects::free_mount_name(list(0), s(1)).into(),
        "baseName" => projects::base_name(s(0)).into(),
        "repoToDraft" => projects::repo_to_draft(a(0)),
        "toProjectDoc" => projects::to_project_doc(s(0), a(1)),
        "reposToProjects" => Value::Array(projects::repos_to_projects(list(0))),
        "projectForRepo" => projects::project_for_repo(list(0), a(1)).cloned().unwrap_or(Value::Null),
        "findProject" => projects::find_project(list(0), a(1)).cloned().unwrap_or(Value::Null),
        // the patterns firestore.rules and the UI's inputs use
        "isGithubUrl" => projects::is_github_url(s(0)).into(),
        "isMount" => projects::is_mount(s(0)).into(),
        "isProjectId" => projects::is_project_id(s(0)).into(),
        "isDriveUuid" => projects::is_drive_uuid(s(0)).into(),
        "isRepoName" => projects::is_repo_name(s(0)).into(),
        "isId" => spec::is_id(s(0)).into(),
        // saved sign-ins
        "signInPaths" => signins::paths_for(s(0)).map(|p| json!(p)).unwrap_or(Value::Null),
        "isSignInPath" => signins::is_path(s(0)).into(),
        // the Builder's layers (layers.rs) and catalog (catalog.rs)
        "catalog" => catalog::catalog().clone(),
        "starters" => catalog::starters().clone(),
        "signInKinds" => json!(signins::SLOT_KINDS),
        "emptySetup" => layers::empty_setup(s(0)),
        "setupOf" => layers::setup_of(a(0)),
        "sameSetup" => layers::same_setup(a(0), a(1)).into(),
        "webappId" => layers::webapp_id(s(0)).into(),
        "webappIcon" => layers::webapp_icon(s(0)).into(),
        "imageKey" => layers::image_key(a(0)).into(),
        "imageRef" => layers::image_ref(s(0)).into(),
        "resolve" => layers::resolve(a(0), a(1)),
        "slotsFor" => Value::Array(layers::slots_for(a(0), a(1))),
        "browserSlots" => Value::Array(layers::browser_slots(a(0))),
        "newSecretName" => layers::new_secret_name(s(0), a(1)).into(),
        "defaultBrowserSecretName" => layers::default_browser_secret_name(s(0), a(1)).into(),
        "fits" => layers::fits(a(0), a(1)).into(),
        "kindOfSlotKey" => layers::kind_of_slot_key(s(0)).map(Value::from).unwrap_or(Value::Null),
        "secretUse" => layers::secret_use(list(0), list(1)),
        "opensFolders" => layers::opens_folders(a(0)).into(),
        "desktopFor" => Value::Array(layers::desktop_for(a(0), a(1), list(2))),
        "startMenuFor" => Value::Array(layers::start_menu_for(a(0), a(1))),
        "bootables" => Value::Array(layers::bootables(a(0), a(1))),
        "layoutJsonFor" => layers::layout_json_for(a(0), a(1)).into(),
        "isExtensionId" => layers::is_extension_id(s(0)).into(),
        "templateDockerfile" => layers::template_dockerfile(a(0), a(1)).into(),
        "extensionsDockerfile" => layers::extensions_dockerfile(a(0), a(1)).into(),
        "templateLayer" => layers::template_layer(a(0), a(1)),
        "extensionsLayer" => layers::extensions_layer(a(0), a(1)),
        "chainOf" => Value::Array(layers::chain_of(a(0), s(1))),
        "chainOk" => layers::chain_ok(list(0)).map(Value::from).unwrap_or(Value::Null),
        "resolveStack" => layers::resolve_stack(a(0), a(1)),
        "stackImageKey" => layers::stack_image_key(a(0)).into(),
        "keyItems" => json!(layers::key_items(a(0), a(1))),
        "keyGroupsOf" => Value::Array(layers::key_groups_of(a(0), a(1), a(2))),
        "migrateV2" => layers::migrate_v2(a(0), a(1), list(2)),
        "wallpaperExt" => layers::wallpaper_ext(a(0)).into(),
        "runSummary" => json!(layers::run_summary(a(0), list(1), list(2))),
        "workspaceFor" => layers::workspace_for(a(0), a(1), a(2), a(3)),
        "knownWebapps" => Value::Array(layers::known_webapps(list(0), list(1))),
        "named" => layers::named(s(0), list(1), opt_s(2)).into(),
        "stacks" => Value::Array(layers::stacks(a(0))),
        "nameTaken" => layers::name_taken(s(0), a(1), opt_s(2)).into(),
        "importable" => layers::importable(a(0), a(1)),
        "importInto" => {
            let parts: Vec<&str> = list(3).iter().filter_map(Value::as_str).collect();
            layers::import_into(a(0), a(1), a(2), &parts)
        }
        // version 4 wadspaces (design B's Builder: v4.rs)
        "textures" => json!(v4::TEXTURES),
        "v4Check" => match v4::check(a(0)) {
            Ok(()) => Value::Null,
            Err(e) => e.into(),
        },
        "v4Apps" => Value::Array(v4::apps_of(a(0))),
        "v4Packages" => json!(v4::packages_of(a(0))),
        "v4ImageKey" => v4::image_key(a(0)).into(),
        "v4TemplateSetup" => {
            let (t, s) = v4::as_template_setup(a(0));
            json!([t, s])
        }
        "v4WorkspaceFor" => v4::workspace_for(a(0), a(1)),
        "v4State" => v4::state(a(0), list(1)).into(),
        "v4View" => v4::view(a(0)),
        // tar
        "tar" => {
            let owned: Vec<(String, Vec<u8>)> = list(0)
                .iter()
                .map(|e| {
                    let path = e.get("path").and_then(Value::as_str).unwrap_or("").to_string();
                    let content = match e.get("content") {
                        Some(Value::String(t)) => t.as_bytes().to_vec(),
                        Some(b) => bytes_of(b).unwrap_or_default(),
                        None => vec![],
                    };
                    (path, content)
                })
                .collect();
            let entries: Vec<tar::Entry> = owned.iter().map(|(p, c)| tar::Entry { path: p, content: c }).collect();
            bytes_json(&tar::tar(&entries)?)
        }
        _ => return Err(format!("no core function {name}")),
    })
}

#[cfg(test)]
mod goldens;
