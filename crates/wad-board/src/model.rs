//! What the board shares with the rest of the client: WadBrowser's key
//! groups, and new ids.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_are_new_each_time() {
        assert_ne!(uid("x"), uid("x"));
        assert_eq!(radix36(35), "z");
        assert_eq!(radix36(36), "10");
    }

    #[test]
    fn key_groups_without_ids_read_and_write_as_they_are() {
        let v = json!({ "members": ["browser"], "key": { "id": "k" } });
        let g: KeyGroup = serde_json::from_value(v.clone()).unwrap();
        assert_eq!((g.id.as_str(), g.members.len()), ("", 1));
        assert_eq!(serde_json::to_value(&g).unwrap(), v);
    }
}
