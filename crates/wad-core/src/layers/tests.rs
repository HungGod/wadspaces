//! What a wadspace is made of: extensions' ids and a mount as wadd takes it.
//! (Slots, keys, the desktop and the Start menu are the goldens' and v4's
//! tests.)

use serde_json::json;

use super::*;

#[test]
fn extension_ids_are_plain_marketplace_ids() {
    for ok in ["anthropic.claude-code", "rust-lang.rust-analyzer", "ms-vscode.cpptools", "a.b.c"] {
        assert!(is_extension_id(ok), "{ok}");
    }
    for bad in ["", "noperiod", ".x", "x.", "a b.c", "a.$(x)", "a;b.c"] {
        assert!(!is_extension_id(bad), "{bad}");
    }
}

#[test]
fn a_mount_goes_to_wadd_with_where_it_is() {
    let custom = json!({ "projects": [
        { "id": "mount-a1", "kind": "mount", "source": { "machineId": "m1", "paths": ["/var/home/wad/Thesis"] }, "as": "Thesis" },
        { "id": "x", "kind": "later", "as": "X" },
    ] });
    assert_eq!(
        super::workspace_projects(&custom),
        [json!({ "id": "mount-a1", "kind": "mount", "mount": "Thesis",
                 "from": { "machineId": "m1", "paths": ["/var/home/wad/Thesis"] } })]
    );
}
