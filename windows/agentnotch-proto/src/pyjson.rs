//! Python's `json.dumps` output, and JSON parsed with its keys kept in
//! document order.
//!
//! The Mac hook script prints its permission decision with `json.dumps`:
//! `", "` and `": "` separators, every non-ASCII character escaped, and
//! object keys in insertion order (the original `tool_input`'s keys first,
//! then the app's `updated_input` additions). Claude Code only parses the
//! JSON, but printing the very same bytes keeps the Mac's test vectors
//! valid on Windows. serde_json's `Value` sorts keys (the workspace can't
//! turn on `preserve_order`: feature unification would change upstream's
//! crates), so this module carries its own small ordered value.

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Number;
use std::fmt;

/// A JSON value whose objects keep their keys in document order. A key that
/// appears twice keeps its first position and its last value, like a Python
/// dict built by `json.loads`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ordered {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

impl Ordered {
    pub(crate) fn get(&self, key: &str) -> Option<&Ordered> {
        match self {
            Ordered::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Sets `key`, keeping its position when it exists (Python's `dict[k] = v`).
    pub(crate) fn insert(entries: &mut Vec<(String, Ordered)>, key: String, value: Ordered) {
        match entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => entries.push((key, value)),
        }
    }

    pub(crate) fn from_value(value: &serde_json::Value) -> Ordered {
        match value {
            serde_json::Value::Null => Ordered::Null,
            serde_json::Value::Bool(b) => Ordered::Bool(*b),
            serde_json::Value::Number(n) => Ordered::Number(n.clone()),
            serde_json::Value::String(s) => Ordered::String(s.clone()),
            serde_json::Value::Array(items) => {
                Ordered::Array(items.iter().map(Ordered::from_value).collect())
            }
            serde_json::Value::Object(map) => Ordered::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), Ordered::from_value(v)))
                    .collect(),
            ),
        }
    }

    pub(crate) fn from_slice(bytes: &[u8]) -> Option<Ordered> {
        serde_json::from_slice(bytes).ok()
    }
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedVisitor;

        impl<'de> Visitor<'de> for OrderedVisitor {
            type Value = Ordered;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_unit<E>(self) -> Result<Ordered, E> {
                Ok(Ordered::Null)
            }
            fn visit_none<E>(self) -> Result<Ordered, E> {
                Ok(Ordered::Null)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Ordered, E> {
                Ok(Ordered::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Ordered, E> {
                Ok(Ordered::Number(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Ordered, E> {
                Ok(Ordered::Number(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Ordered, E> {
                Number::from_f64(v)
                    .map(Ordered::Number)
                    .ok_or_else(|| E::custom("not a JSON number"))
            }
            fn visit_str<E>(self, v: &str) -> Result<Ordered, E> {
                Ok(Ordered::String(v.to_owned()))
            }
            fn visit_string<E>(self, v: String) -> Result<Ordered, E> {
                Ok(Ordered::String(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Ordered, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Ordered::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut entries = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Ordered>()? {
                    Ordered::insert(&mut entries, key, value);
                }
                Ok(Ordered::Object(entries))
            }
        }

        deserializer.deserialize_any(OrderedVisitor)
    }
}

/// `json.dumps(value)` with Python's defaults.
pub(crate) fn dumps(value: &Ordered) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

/// `json.dumps` of a serde_json value (its keys come out sorted).
pub(crate) fn dumps_value(value: &serde_json::Value) -> String {
    dumps(&Ordered::from_value(value))
}

fn write_value(value: &Ordered, out: &mut String) {
    match value {
        Ordered::Null => out.push_str("null"),
        Ordered::Bool(true) => out.push_str("true"),
        Ordered::Bool(false) => out.push_str("false"),
        Ordered::Number(n) => out.push_str(&n.to_string()),
        Ordered::String(s) => write_string(s, out),
        Ordered::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Ordered::Object(entries) => {
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_string(key, out);
                out.push_str(": ");
                write_value(item, out);
            }
            out.push('}');
        }
    }
}

/// Python's `ensure_ascii` escaping: everything outside space..tilde is
/// escaped, non-BMP characters as a surrogate pair.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ' '..='~' => out.push(ch),
            _ => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_python_separators_and_escapes() {
        let value = Ordered::from_slice(
            br#"{"b": [1, 2.5, true, null], "a": "\u00e9\ud83d\ude00\"\\\n\u0001\u007f~"}"#,
        )
        .unwrap();
        // Keys stay in document order; escapes are Python's.
        assert_eq!(
            dumps(&value),
            r#"{"b": [1, 2.5, true, null], "a": "\u00e9\ud83d\ude00\"\\\n\u0001\u007f~"}"#
        );
    }

    #[test]
    fn empty_containers() {
        assert_eq!(
            dumps(&Ordered::from_slice(b"{\"a\": {}, \"b\": []}").unwrap()),
            r#"{"a": {}, "b": []}"#
        );
    }

    #[test]
    fn duplicate_keys_keep_the_first_place_and_the_last_value() {
        let value = Ordered::from_slice(br#"{"k": 1, "z": 0, "k": 2}"#).unwrap();
        assert_eq!(dumps(&value), r#"{"k": 2, "z": 0}"#);
    }

    #[test]
    fn serde_values_come_out_sorted() {
        let value = serde_json::json!({"z": 1, "a": {"y": 2, "b": 3}});
        assert_eq!(dumps_value(&value), r#"{"a": {"b": 3, "y": 2}, "z": 1}"#);
    }
}
