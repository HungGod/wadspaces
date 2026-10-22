//! The Builder's board ([`b`]): version 5 wadspaces, each on one machine,
//! one card each (its customization, keys, add-ons and apps), and the edits
//! made to them; what the catalog says about apps, add-ons and keys
//! ([`catalog`]). [`b4`] is the templates' board the client still uses.
//! fixtures/board/b.json (the board's scenarios) keeps the WadSpaces Client
//! and the phone's copy the same.

pub mod b;
pub mod b4;
pub mod catalog;
pub mod model;

use serde::de::DeserializeOwned;
use serde_json::Value;

pub use model::*;

fn arg<T: DeserializeOwned>(args: &[Value], i: usize) -> Result<T, String> {
    serde_json::from_value(args.get(i).cloned().unwrap_or(Value::Null)).map_err(|e| format!("argument {i}: {e}"))
}

fn to<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// A board function by its name, on JSON arguments (the goldens' cases).
pub fn call(name: &str, args: &[Value]) -> Result<Value, String> {
    Ok(match name {
        // The board (b/): edits in turn, and what it derives.
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
            let known: Vec<Value> = if args.len() > 3 { arg(args, 3)? } else { vec![] };
            board.new_key(&arg::<String>(args, 1)?, &arg::<String>(args, 2)?, &known)
        }
        "bKeyMap" => {
            let board: b::Board = arg(args, 0)?;
            to(board.key_map(&arg::<String>(args, 1)?))
        }
        "bSecrets" => {
            let board: b::Board = arg(args, 0)?;
            to(board.secrets_by_key(&arg::<String>(args, 1)?))
        }
        other => return Err(format!("no board function {other}")),
    })
}
