//! The Builder's board for the native WadSpaces Client: stacks of modules
//! (templates, Extensions, a customization), read from the documents and
//! written back ([`sync`]), what the catalog says about them ([`catalog`]),
//! and the board's edits ([`board`]). A port of the web app's
//! components/builder (model.ts, sync.ts, catalog.ts, the store's pure
//! parts); fixtures/board/goldens.json (what those returned) keeps them the
//! same.

pub mod b;
pub mod board;
pub mod catalog;
pub mod model;
pub mod sync;

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub use board::Board;
pub use model::*;
pub use sync::{Docs, Plan, plan, plan_new, to_board, with_refs};

fn arg<T: DeserializeOwned>(args: &[Value], i: usize) -> Result<T, String> {
    serde_json::from_value(args.get(i).cloned().unwrap_or(Value::Null)).map_err(|e| format!("argument {i}: {e}"))
}

fn to<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn fits(r: Result<(), String>) -> Value {
    match r {
        Ok(()) => json!({ "ok": true }),
        Err(why) => json!({ "ok": false, "why": why }),
    }
}

/// A board function by its web app name, on JSON arguments (the goldens'
/// cases; the web app could call these through wasm too).
pub fn call(name: &str, args: &[Value]) -> Result<Value, String> {
    Ok(match name {
        "toBoard" => to(sync::to_board(&arg(args, 0)?)),
        "plan" => {
            let stacks: Vec<Stack> = arg(args, 0)?;
            let docs: Docs = arg(args, 1)?;
            let owner: String = arg(args, 2)?;
            let now: String = arg(args, 3)?;
            to(sync::plan(&stacks, &docs, &owner, &now))
        }
        "planNew" => {
            let stack: Stack = arg(args, 0)?;
            let docs: Docs = arg(args, 1)?;
            let owner: String = arg(args, 2)?;
            let now: String = arg(args, 3)?;
            to(sync::plan_new(&stack, &docs, &owner, &now))
        }
        "withRefs" => {
            let stacks: Vec<Stack> = arg(args, 0)?;
            let refs: HashMap<String, String> = arg(args, 1)?;
            to(sync::with_refs(&stacks, &refs))
        }
        "setupOfModule" => sync::setup_of_module(&arg(args, 0)?),
        "customizationOf" => sync::customization_of(&arg(args, 0)?),
        "buildsOn" => fits(builds_on(&arg(args, 0)?, &arg(args, 1)?)),
        "fitTemplates" => {
            match fit_templates(&arg::<Vec<TemplateModule>>(args, 0)?, &arg::<Vec<TemplateModule>>(args, 1)?) {
                Ok(at) => json!({ "ok": true, "at": at }),
                Err(why) => json!({ "ok": false, "why": why }),
            }
        }
        "isBlank" => is_blank(&arg(args, 0)?).into(),
        "moduleName" => module_name(&arg(args, 0)?).into(),
        "strayApps" => to(stray_apps(&arg(args, 0)?)),
        "missingFor" => to(missing_for(&arg(args, 0)?)),
        "statusOf" => status_of(&arg(args, 0)?).into(),
        "stackName" => stack_name(&arg(args, 0)?).into(),
        "browserItems" => to(catalog::browser_items(&arg(args, 0)?)),
        "keyGroupsOf" => {
            let s: Stack = arg(args, 0)?;
            to(catalog::key_groups_of(&s, s.secrets()))
        }
        "keyMap" => {
            let s: Stack = arg(args, 0)?;
            to(catalog::key_map(&s, s.secrets()))
        }
        "keyCount" => {
            let (all, set) = catalog::key_count(&arg(args, 0)?);
            json!({ "all": all, "set": set })
        }
        "webKey" => catalog::web_key(&arg(args, 0)?, &arg::<String>(args, 1)?).unwrap_or(Value::Null),
        "problemsOf" => to(catalog::problems_of(&arg(args, 0)?)),
        "secretSlots" => to(catalog::secret_slots(&arg(args, 0)?)),
        "desktopOf" => to(catalog::desktop_of(&arg(args, 0)?)),
        "setupsFor" => to(catalog::setups_for(&arg::<Vec<String>>(args, 0)?)),
        "subFits" => fits(catalog::sub_fits(&arg::<String>(args, 0)?, arg::<Option<Vec<String>>>(args, 1)?.as_deref())),
        "extSig" => to(board::ext_sig(&arg(args, 0)?)),
        "nameClash" => {
            let stacks: Vec<Stack> = arg(args, 0)?;
            to(board::name_clash(&stacks, &arg::<String>(args, 1)?, &arg(args, 2)?, &arg::<String>(args, 3)?))
        }
        "grab" => to(board::grab(&arg(args, 0)?, &arg::<String>(args, 1)?, arg(args, 2)?)),
        "keysOfKind" => to(board::keys_of_kind(
            &arg::<Vec<Value>>(args, 0)?,
            &arg::<Vec<Stack>>(args, 1)?,
            &arg::<String>(args, 2)?,
        )),
        "copyName" => board::copy_name(&arg::<String>(args, 0)?, &arg::<Vec<String>>(args, 1)?).into(),
        "chainUnder" => to(board::chain_under(&arg(args, 0)?, &arg::<Vec<Stack>>(args, 1)?)),
        "sameApps" => board::same_apps(&arg(args, 0)?, &arg(args, 1)?).into(),
        "place" => match board::place(&arg(args, 0)?, &arg::<Vec<Module>>(args, 1)?) {
            Ok((stack, replaced)) => json!({ "ok": true, "stack": to(stack), "replaced": to(replaced) }),
            Err(why) => json!({ "ok": false, "why": why }),
        },
        // Design B's board (b.rs): edits in turn, and what it derives.
        "bRun" => b::run(args.first().unwrap_or(&Value::Null))?,
        "bGroups" => {
            let board: b::Board = arg(args, 0)?;
            to(board.groups_of(&arg::<String>(args, 1)?))
        }
        "bSlots" => {
            let board: b::Board = arg(args, 0)?;
            to(board.slots_of(&arg::<String>(args, 1)?))
        }
        "bNewKey" => {
            let board: b::Board = arg(args, 0)?;
            board.new_key(&arg::<String>(args, 1)?, &arg::<String>(args, 2)?)
        }
        other => return Err(format!("no board function {other}")),
    })
}

#[cfg(test)]
mod goldens;
#[cfg(test)]
mod phone;
