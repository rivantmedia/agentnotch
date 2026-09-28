//! Picks a few top-level values out of a JSON object without parsing the
//! rest (JSONFieldScanner.swift). `.claude.json` can be megabytes of project
//! history, MCP servers and feature flags; the engine needs two of its keys.
//! The scanner walks the top-level object byte by byte, skipping every other
//! value (strings with their escapes, nested objects and arrays) without
//! building it, and hands back the raw bytes of the wanted values, which are
//! then parsed on their own. Nothing else in the file becomes a value.

use serde_json::Value;
use std::collections::HashMap;

/// The raw bytes of the top-level values named in `keys`. `None` when
/// `data` isn't a JSON object, or is cut off before its end (caught
/// mid-write). A key that appears twice keeps its last value, as JSON
/// parsers do.
pub fn values<'a>(data: &'a [u8], keys: &[&str]) -> Option<HashMap<String, &'a [u8]>> {
    Scanner {
        bytes: data,
        index: 0,
    }
    .top_level(keys)
}

/// The wanted top-level values, each parsed on its own (scalars too); a
/// value that doesn't parse is left out.
pub fn objects(data: &[u8], keys: &[&str]) -> Option<HashMap<String, Value>> {
    let raw = values(data, keys)?;
    Some(
        raw.into_iter()
            .filter_map(|(key, bytes)| serde_json::from_slice(bytes).ok().map(|v| (key, v)))
            .collect(),
    )
}

struct Scanner<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Scanner<'a> {
    fn top_level(mut self, keys: &[&str]) -> Option<HashMap<String, &'a [u8]>> {
        self.skip_byte_order_mark();
        self.skip_space();
        if !self.take(b'{') {
            return None;
        }
        let mut result = HashMap::new();
        self.skip_space();
        if self.take(b'}') {
            return Some(result);
        }
        while self.index < self.bytes.len() {
            self.skip_space();
            if self.peek() != Some(b'"') {
                return None;
            }
            let key_start = self.index;
            if !self.skip_string() {
                return None;
            }
            let key = self.decode_key(key_start, self.index);
            self.skip_space();
            if !self.take(b':') {
                return None;
            }
            self.skip_space();
            let value_start = self.index;
            if !self.skip_value() {
                return None;
            }
            if let Some(key) = key.filter(|k| keys.contains(&k.as_str())) {
                result.insert(key, &self.bytes[value_start..self.index]);
            }
            self.skip_space();
            if self.take(b',') {
                continue;
            }
            if self.take(b'}') {
                return Some(result);
            }
            return None;
        }
        None
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn skip_byte_order_mark(&mut self) {
        if self.bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            self.index = 3;
        }
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.index += 1;
        }
    }

    fn take(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    /// Past a string starting at its opening quote.
    fn skip_string(&mut self) -> bool {
        if !self.take(b'"') {
            return false;
        }
        while let Some(byte) = self.peek() {
            if byte == b'\\' {
                self.index += 2;
                continue;
            }
            self.index += 1;
            if byte == b'"' {
                return true;
            }
        }
        false
    }

    /// Past one value of any kind, without building it.
    fn skip_value(&mut self) -> bool {
        match self.peek() {
            None => false,
            Some(b'"') => self.skip_string(),
            Some(b'{' | b'[') => {
                let mut depth = 0usize;
                while let Some(byte) = self.peek() {
                    match byte {
                        b'"' => {
                            if !self.skip_string() {
                                return false;
                            }
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                self.index += 1;
                                return true;
                            }
                        }
                        _ => {}
                    }
                    self.index += 1;
                }
                false
            }
            Some(_) => {
                // A number, true, false or null: up to the next delimiter.
                let start = self.index;
                while let Some(byte) = self.peek() {
                    if matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                        break;
                    }
                    self.index += 1;
                }
                self.index > start
            }
        }
    }

    /// A key's text; keys with escapes go through the JSON parser.
    fn decode_key(&self, start: usize, end: usize) -> Option<String> {
        let inner = &self.bytes[start + 1..end - 1];
        if !inner.contains(&b'\\') {
            return Some(String::from_utf8_lossy(inner).into_owned());
        }
        serde_json::from_slice::<String>(&self.bytes[start..end]).ok()
    }
}
