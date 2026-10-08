//! The engine's files in `<support>`, each through its own data-transfer
//! types with the Mac's names and date encodings (§3): the model types
//! never derive a file format. A Mac-written fixture of each file in
//! `tests/fixtures/mac-files/` round-trips through these types
//! (`tests/persist_mac_files.rs`).
//!
//! Files are written pretty-printed with sorted keys (as the Mac's
//! `JSONEncoder` writes them) through `SecureFiles::write_atomic` with
//! `WriteMode::Private`.

pub mod accounts;
pub mod hook_install;
pub mod limits;
pub mod review;
pub mod settings;
pub mod usage;

use serde::Serialize;
use serde_json::Value;

/// Pretty JSON with every object's keys sorted, and a final newline.
pub fn encode_pretty_sorted<T: Serialize>(value: &T) -> Vec<u8> {
    // Through a Value: its maps are sorted, whatever the struct's field order.
    let value = serde_json::to_value(value).unwrap_or(Value::Null);
    let mut bytes = serde_json::to_vec_pretty(&value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// Compact JSON with sorted keys (`review-state.json`'s encoding).
pub fn encode_compact_sorted<T: Serialize>(value: &T) -> Vec<u8> {
    let value = serde_json::to_value(value).unwrap_or(Value::Null);
    serde_json::to_vec(&value).unwrap_or_default()
}

/// Two JSON documents say the same: equal key sets, equal values, numbers
/// compared by value (`18000` and `18000.0` are one number).
pub fn json_equivalent(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_equivalent(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_equivalent(v, w)))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn equivalence() {
        assert!(json_equivalent(
            &json!({"a": 18000, "b": [1.5]}),
            &json!({"b": [1.5], "a": 18000.0})
        ));
        assert!(!json_equivalent(
            &json!({"a": 1}),
            &json!({"a": 1, "b": null})
        ));
        assert!(!json_equivalent(&json!({"a": "1"}), &json!({"a": 1})));
    }

    #[test]
    fn sorted_pretty() {
        #[derive(Serialize)]
        struct S {
            z: u8,
            a: &'static str,
        }
        let text = String::from_utf8(encode_pretty_sorted(&S { z: 1, a: "x/y" })).unwrap();
        assert_eq!(text, "{\n  \"a\": \"x/y\",\n  \"z\": 1\n}\n");
    }
}
