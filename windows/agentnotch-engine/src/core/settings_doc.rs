//! settings.json edited by splicing (SettingsDocument.swift, OrderedJSON.swift;
//! HS§3.4, DESIGN-WIN §4.3).
//!
//! [`Json`] is a JSON value that remembers what serde_json forgets: the order
//! of an object's keys and how each number was spelled. [`SettingsDocument`]
//! holds a settings.json as its bytes plus its parsed top-level object, and a
//! change to one top-level key rewrites only the bytes of that member, in the
//! file's own layout. Every other key keeps its position, spacing, number
//! spelling and escapes, so a file Claude Code wrote (`JSON.stringify` with
//! two spaces) comes back byte for byte after an install and an uninstall.
//!
//! Windows editors add two things the Mac never saw: CRLF line endings and a
//! UTF-8 byte order mark. Both are kept: the bytes outside the object (the
//! BOM among them) are never touched, and whatever the splice writes uses
//! the file's own newline (the majority style when a file mixes them).
//!
//! A document refuses (`None`) anything that isn't a JSON object at the top
//! level, which the installer must then never overwrite. A missing or blank
//! file is the empty object.

use std::collections::{BTreeMap, BTreeSet};

/// A JSON value with its object keys in order and its numbers as written.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Object(Vec<Member>),
    Array(Vec<Json>),
    String(String),
    /// The number exactly as written in the source (or by us).
    Number(String),
    Bool(bool),
    Null,
}

/// One member of an object.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub key: String,
    pub value: Json,
}

impl Member {
    pub fn new(key: impl Into<String>, value: Json) -> Member {
        Member {
            key: key.into(),
            value,
        }
    }
}

impl Json {
    pub fn int(value: i64) -> Json {
        Json::Number(value.to_string())
    }

    pub fn string(value: impl Into<String>) -> Json {
        Json::String(value.into())
    }

    /// An object from `(key, value)` pairs, in that order.
    pub fn object<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Json)>) -> Json {
        Json::Object(
            pairs
                .into_iter()
                .map(|(key, value)| Member::new(key, value))
                .collect(),
        )
    }

    pub fn members(&self) -> Option<&[Member]> {
        match self {
            Json::Object(members) => Some(members),
            _ => None,
        }
    }

    pub fn items(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Json::Object(_))
    }

    /// The value for `key` in an object: the last one, as JavaScript reads a
    /// repeated key. `None` for a missing key or a value that isn't an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.members()?
            .iter()
            .rev()
            .find(|member| member.key == key)
            .map(|member| &member.value)
    }

    /// Sets `key` in an object: in place where it already is (repeats of it
    /// dropped), appended where it isn't, removed for `None`. A no-op on
    /// anything but an object.
    pub fn set(&mut self, key: &str, value: Option<Json>) {
        let Json::Object(members) = self else {
            return;
        };
        match value {
            Some(value) => {
                if let Some(index) = members.iter().rposition(|m| m.key == key) {
                    members[index].value = value;
                    let mut position = 0;
                    members.retain(|member| {
                        let keep = position == index || member.key != key;
                        position += 1;
                        keep
                    });
                } else {
                    members.push(Member::new(key, value));
                }
            }
            None => members.retain(|member| member.key != key),
        }
    }

    /// Equal as JSON: objects regardless of key order (the last of a
    /// repeated key counts), numbers by value.
    pub fn is_equivalent(&self, other: &Json) -> bool {
        match (self, other) {
            (Json::Object(lhs), Json::Object(rhs)) => {
                let left = last_wins(lhs);
                let right = last_wins(rhs);
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right
                            .get(key)
                            .is_some_and(|other| value.is_equivalent(other))
                    })
            }
            (Json::Array(lhs), Json::Array(rhs)) => {
                lhs.len() == rhs.len() && lhs.iter().zip(rhs).all(|(a, b)| a.is_equivalent(b))
            }
            (Json::String(lhs), Json::String(rhs)) => lhs == rhs,
            (Json::Number(lhs), Json::Number(rhs)) => {
                lhs == rhs
                    || matches!(
                        (lhs.parse::<f64>(), rhs.parse::<f64>()),
                        (Ok(a), Ok(b)) if a == b
                    )
            }
            (Json::Bool(lhs), Json::Bool(rhs)) => lhs == rhs,
            (Json::Null, Json::Null) => true,
            _ => false,
        }
    }

    /// Both absent, or both present and equivalent.
    pub fn equivalent(lhs: Option<&Json>, rhs: Option<&Json>) -> bool {
        match (lhs, rhs) {
            (None, None) => true,
            (Some(a), Some(b)) => a.is_equivalent(b),
            _ => false,
        }
    }

    /// JSON text the way JavaScript's `JSON.stringify(value, null, 2)` writes
    /// it (Claude Code's own format), with `\n` line breaks.
    pub fn serialized(&self) -> String {
        self.serialized_with("", "  ", "\n")
    }

    /// Like [`Json::serialized`], each line after the first starting with
    /// `indent`, one level of indentation being `unit`, lines ending with
    /// `newline`. An empty `unit` writes it on one line with no spaces.
    pub fn serialized_with(&self, indent: &str, unit: &str, newline: &str) -> String {
        let mut out = String::new();
        self.write(&mut out, indent, unit, newline);
        out
    }

    fn write(&self, out: &mut String, indent: &str, unit: &str, newline: &str) {
        match self {
            Json::Object(members) => {
                if members.is_empty() {
                    out.push_str("{}");
                    return;
                }
                let inner = format!("{indent}{unit}");
                out.push('{');
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    if !unit.is_empty() {
                        out.push_str(newline);
                        out.push_str(&inner);
                    }
                    out.push_str(&quoted(&member.key));
                    out.push_str(if unit.is_empty() { ":" } else { ": " });
                    member.value.write(out, &inner, unit, newline);
                }
                if !unit.is_empty() {
                    out.push_str(newline);
                    out.push_str(indent);
                }
                out.push('}');
            }
            Json::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                let inner = format!("{indent}{unit}");
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    if !unit.is_empty() {
                        out.push_str(newline);
                        out.push_str(&inner);
                    }
                    item.write(out, &inner, unit, newline);
                }
                if !unit.is_empty() {
                    out.push_str(newline);
                    out.push_str(indent);
                }
                out.push(']');
            }
            Json::String(text) => out.push_str(&quoted(text)),
            Json::Number(text) => out.push_str(text),
            Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Json::Null => out.push_str("null"),
        }
    }

    /// Parses a whole JSON document (UTF-8, optional BOM). Tolerates
    /// trailing commas, as Foundation's parser does on the Mac; rejects
    /// everything else that isn't JSON.
    pub fn parse(bytes: &[u8]) -> Result<Json, ParseError> {
        Parser::new(bytes).document().map(|document| document.value)
    }
}

fn last_wins(members: &[Member]) -> BTreeMap<&str, &Json> {
    members
        .iter()
        .map(|member| (member.key.as_str(), &member.value))
        .collect()
}

/// A JSON string literal, escaped as `JSON.stringify` escapes it.
pub fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---- Parsing ----

/// Where a parse failed, as a byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not JSON at byte {}", self.offset)
    }
}

impl std::error::Error for ParseError {}

/// Where one member of the top-level object sits in the source bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberSpan {
    pub key: String,
    /// The key's opening quote.
    pub key_start: usize,
    /// The first byte of the value.
    pub value_start: usize,
    /// One past the value's last byte.
    pub value_end: usize,
}

/// The top-level object of a document, with the byte range of each member.
#[derive(Debug, Clone, PartialEq)]
pub struct TopLevel {
    pub value: Json,
    /// The `{`.
    pub object_start: usize,
    /// One past the `}`.
    pub object_end: usize,
    pub spans: Vec<MemberSpan>,
    /// A trailing comma was accepted somewhere (strict JSON parsers refuse
    /// the file, so they can't be asked to agree with it).
    pub trailing_comma: bool,
}

/// Parses a document whose top level must be an object, keeping where its
/// members are. `Ok(None)` when the top level is something else.
pub fn parse_top_level_object(bytes: &[u8]) -> Result<Option<TopLevel>, ParseError> {
    let document = Parser::new(bytes).document()?;
    Ok(
        match (document.value.is_object(), document.start, document.end) {
            (true, Some(start), Some(end)) => Some(TopLevel {
                value: document.value,
                object_start: start,
                object_end: end,
                spans: document.spans,
                trailing_comma: document.trailing_comma,
            }),
            _ => None,
        },
    )
}

/// Deepest nesting the parser follows: anything deeper (no settings.json
/// comes close) is refused rather than recursed into on a small stack.
pub const MAX_DEPTH: usize = 256;

struct Document {
    value: Json,
    spans: Vec<MemberSpan>,
    start: Option<usize>,
    end: Option<usize>,
    trailing_comma: bool,
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
    depth: usize,
    trailing_comma: bool,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Parser<'a> {
        let index = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
        Parser {
            bytes,
            index,
            depth: 0,
            trailing_comma: false,
        }
    }

    fn error(&self) -> ParseError {
        ParseError { offset: self.index }
    }

    fn document(mut self) -> Result<Document, ParseError> {
        self.skip_whitespace();
        let start = self.index;
        let mut spans = Vec::new();
        let value = if self.peek() == Some(b'{') {
            self.object(Some(&mut spans))?
        } else {
            self.value()?
        };
        let end = self.index;
        self.skip_whitespace();
        if self.index != self.bytes.len() {
            return Err(self.error());
        }
        let is_object = value.is_object();
        Ok(Document {
            value,
            spans: if is_object { spans } else { Vec::new() },
            start: is_object.then_some(start),
            end: is_object.then_some(end),
            trailing_comma: self.trailing_comma,
        })
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.index += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.peek() != Some(byte) {
            return Err(self.error());
        }
        self.index += 1;
        Ok(())
    }

    fn value(&mut self) -> Result<Json, ParseError> {
        self.skip_whitespace();
        match self.peek().ok_or_else(|| self.error())? {
            b'{' => self.object(None),
            b'[' => self.array(),
            b'"' => Ok(Json::String(self.string()?)),
            b't' => self.literal(b"true").map(|_| Json::Bool(true)),
            b'f' => self.literal(b"false").map(|_| Json::Bool(false)),
            b'n' => self.literal(b"null").map(|_| Json::Null),
            _ => self.number().map(Json::Number),
        }
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), ParseError> {
        for &byte in word {
            self.expect(byte)?;
        }
        Ok(())
    }

    fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error());
        }
        Ok(())
    }

    fn object(&mut self, spans: Option<&mut Vec<MemberSpan>>) -> Result<Json, ParseError> {
        self.expect(b'{')?;
        self.enter()?;
        let result = self.object_body(spans);
        self.depth -= 1;
        result
    }

    fn object_body(&mut self, mut spans: Option<&mut Vec<MemberSpan>>) -> Result<Json, ParseError> {
        let mut members = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.index += 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.skip_whitespace();
            if self.peek() == Some(b'}') && !members.is_empty() {
                self.trailing_comma = true;
                self.index += 1;
                return Ok(Json::Object(members));
            }
            let key_start = self.index;
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            let value_start = self.index;
            let value = self.value()?;
            if let Some(spans) = spans.as_deref_mut() {
                spans.push(MemberSpan {
                    key: key.clone(),
                    key_start,
                    value_start,
                    value_end: self.index,
                });
            }
            members.push(Member::new(key, value));
            self.skip_whitespace();
            let byte = self.peek().ok_or_else(|| self.error())?;
            self.index += 1;
            match byte {
                b'}' => return Ok(Json::Object(members)),
                b',' => {}
                _ => {
                    return Err(ParseError {
                        offset: self.index - 1,
                    })
                }
            }
        }
    }

    fn array(&mut self) -> Result<Json, ParseError> {
        self.expect(b'[')?;
        self.enter()?;
        let result = self.array_body();
        self.depth -= 1;
        result
    }

    fn array_body(&mut self) -> Result<Json, ParseError> {
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.index += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_whitespace();
            if self.peek() == Some(b']') && !items.is_empty() {
                self.trailing_comma = true;
                self.index += 1;
                return Ok(Json::Array(items));
            }
            items.push(self.value()?);
            self.skip_whitespace();
            let byte = self.peek().ok_or_else(|| self.error())?;
            self.index += 1;
            match byte {
                b']' => return Ok(Json::Array(items)),
                b',' => {}
                _ => {
                    return Err(ParseError {
                        offset: self.index - 1,
                    })
                }
            }
        }
    }

    fn number(&mut self) -> Result<String, ParseError> {
        let start = self.index;
        let digit = |b: Option<u8>| b.is_some_and(|b| b.is_ascii_digit());
        if self.peek() == Some(b'-') {
            self.index += 1;
        }
        if !digit(self.peek()) {
            return Err(self.error());
        }
        if self.peek() == Some(b'0') {
            self.index += 1;
        } else {
            while digit(self.peek()) {
                self.index += 1;
            }
        }
        if self.peek() == Some(b'.') {
            self.index += 1;
            if !digit(self.peek()) {
                return Err(self.error());
            }
            while digit(self.peek()) {
                self.index += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.index += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.index += 1;
            }
            if !digit(self.peek()) {
                return Err(self.error());
            }
            while digit(self.peek()) {
                self.index += 1;
            }
        }
        // Only ASCII digits, signs, `.` and `e` were consumed.
        Ok(String::from_utf8_lossy(&self.bytes[start..self.index]).into_owned())
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = String::new();
        let mut run_start = self.index;
        loop {
            let byte = self.peek().ok_or_else(|| self.error())?;
            match byte {
                b'"' => {
                    self.flush_run(&mut out, run_start, self.index)?;
                    self.index += 1;
                    return Ok(out);
                }
                0x00..=0x1F => return Err(self.error()),
                b'\\' => {
                    self.flush_run(&mut out, run_start, self.index)?;
                    self.index += 1;
                    let escape = self.peek().ok_or_else(|| self.error())?;
                    self.index += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => {
                            return Err(ParseError {
                                offset: self.index - 1,
                            })
                        }
                    }
                    run_start = self.index;
                }
                _ => self.index += 1,
            }
        }
    }

    /// `\uXXXX` (the `\u` already read), a surrogate pair joined.
    fn unicode_escape(&mut self) -> Result<char, ParseError> {
        let mut code = self.hex4()?;
        if (0xD800..=0xDBFF).contains(&code)
            && self.peek() == Some(b'\\')
            && self.bytes.get(self.index + 1) == Some(&b'u')
        {
            let save = self.index;
            self.index += 2;
            let low = self.hex4()?;
            if (0xDC00..=0xDFFF).contains(&low) {
                code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
            } else {
                self.index = save;
            }
        }
        // A lone surrogate can't be held in a Rust string; JavaScript would
        // keep it, so refuse rather than alter it.
        char::from_u32(code).ok_or_else(|| self.error())
    }

    fn flush_run(&self, out: &mut String, start: usize, end: usize) -> Result<(), ParseError> {
        if end > start {
            let text = std::str::from_utf8(&self.bytes[start..end])
                .map_err(|_| ParseError { offset: start })?;
            out.push_str(text);
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        if self.index + 4 > self.bytes.len() {
            return Err(self.error());
        }
        let mut value = 0u32;
        for _ in 0..4 {
            let digit = (self.bytes[self.index] as char)
                .to_digit(16)
                .ok_or_else(|| self.error())?;
            value = value * 16 + digit;
            self.index += 1;
        }
        Ok(value)
    }
}

// ---- The document ----

/// The UTF-8 byte order mark Windows editors put at the start of a file.
pub const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// A settings.json as bytes plus its parsed top-level object, edited by
/// splicing.
#[derive(Debug, Clone)]
pub struct SettingsDocument {
    /// The top-level object as parsed (or as edited).
    value: Json,
    /// The source bytes; `None` for a missing or blank file.
    source: Option<Vec<u8>>,
    top_level: Option<TopLevel>,
    /// Keys changed with `set`, and their new values (`None` = removed).
    changes: Vec<(String, Option<Json>)>,
    /// A blank file's BOM and newline, kept when it is first written.
    blank_bom: bool,
    blank_newline: &'static str,
}

impl SettingsDocument {
    /// Parses `data` (`None` = no file). `None` when it can't be read back as
    /// a JSON object, which the installer must never overwrite.
    pub fn new(data: Option<&[u8]>) -> Option<SettingsDocument> {
        let Some(data) = data.filter(|data| !is_blank(data)) else {
            let blank = data.unwrap_or_default();
            return Some(SettingsDocument {
                value: Json::Object(Vec::new()),
                source: None,
                top_level: None,
                changes: Vec::new(),
                blank_bom: blank.starts_with(BOM),
                blank_newline: newline_style(blank),
            });
        };
        let top_level = parse_top_level_object(data).ok()??;
        // Two independent parsers must agree it is an object: serde_json and
        // ours (which keeps order). serde_json refuses trailing commas, which
        // ours (like Foundation on the Mac) accepts, so it can't be asked
        // about such a file.
        if !top_level.trailing_comma {
            let body = data.strip_prefix(BOM).unwrap_or(data);
            if !serde_json::from_slice::<serde_json::Value>(body).is_ok_and(|v| v.is_object()) {
                return None;
            }
        }
        Some(SettingsDocument {
            value: top_level.value.clone(),
            source: Some(data.to_vec()),
            top_level: Some(top_level),
            changes: Vec::new(),
            blank_bom: false,
            blank_newline: "\n",
        })
    }

    /// The top-level object as parsed or edited.
    pub fn value(&self) -> &Json {
        &self.value
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        self.value.get(key)
    }

    /// Replaces (or with `None`, removes) one top-level key.
    pub fn set(&mut self, key: &str, value: Option<Json>) {
        self.value.set(key, value.clone());
        self.changes.retain(|(changed, _)| changed != key);
        self.changes.push((key.to_owned(), value));
    }

    /// The document's bytes with every `set` applied.
    pub fn data(&self) -> Vec<u8> {
        let (Some(source), Some(top)) = (&self.source, &self.top_level) else {
            let newline = self.blank_newline;
            let mut out = if self.blank_bom {
                BOM.to_vec()
            } else {
                Vec::new()
            };
            out.extend_from_slice(self.value.serialized_with("", "  ", newline).as_bytes());
            out.extend_from_slice(newline.as_bytes());
            return out;
        };
        if self.changes.is_empty() {
            return source.clone();
        }
        let style = Style::of(source, top);
        let spans = &top.spans;

        // What becomes of each original member: kept as is, a new value, or
        // gone. A repeated key keeps only its last occurrence (the one JSON
        // readers use).
        let mut replacements: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
        let mut removed = BTreeSet::new();
        let mut inserted: Vec<Member> = Vec::new();
        for (key, change) in &self.changes {
            let indices: Vec<usize> = (0..spans.len()).filter(|&i| spans[i].key == *key).collect();
            match change {
                Some(value) => {
                    if let Some((&last, earlier)) = indices.split_last() {
                        replacements.insert(
                            last,
                            value
                                .serialized_with(&style.member_indent, &style.unit, style.newline)
                                .into_bytes(),
                        );
                        removed.extend(earlier.iter().copied());
                    } else {
                        inserted.push(Member::new(key.clone(), value.clone()));
                    }
                }
                None => removed.extend(indices),
            }
        }

        let kept: Vec<usize> = (0..spans.len()).filter(|i| !removed.contains(i)).collect();
        if kept.is_empty() {
            // Every original member went: write the object afresh.
            let object = self.value.serialized_with("", &style.unit, style.newline);
            let mut text = source[..top.object_start].to_vec();
            text.extend_from_slice(object.as_bytes());
            text.extend_from_slice(&source[top.object_end..]);
            return text;
        }

        // `{` + what preceded the first member, then each kept member with the
        // separator that preceded it in the file, then additions, then what
        // followed the last member, and `}`.
        let mut body: Vec<u8> = source[top.object_start + 1..spans[0].key_start].to_vec();
        for (position, &index) in kept.iter().enumerate() {
            if position > 0 {
                body.extend_from_slice(&source[spans[index - 1].value_end..spans[index].key_start]);
            }
            body.extend_from_slice(&source[spans[index].key_start..spans[index].value_start]);
            match replacements.get(&index) {
                Some(bytes) => body.extend_from_slice(bytes),
                None => body
                    .extend_from_slice(&source[spans[index].value_start..spans[index].value_end]),
            }
        }
        for member in &inserted {
            let mut addition = if style.unit.is_empty() {
                ",".to_owned()
            } else {
                format!(",{}{}", style.newline, style.member_indent)
            };
            addition.push_str(&quoted(&member.key));
            addition.push_str(if style.unit.is_empty() { ":" } else { ": " });
            addition.push_str(&member.value.serialized_with(
                &style.member_indent,
                &style.unit,
                style.newline,
            ));
            body.extend_from_slice(addition.as_bytes());
        }
        body.extend_from_slice(&source[spans[spans.len() - 1].value_end..top.object_end - 1]);

        let mut text = source[..=top.object_start].to_vec();
        text.extend_from_slice(&body);
        text.extend_from_slice(&source[top.object_end - 1..]);
        text
    }
}

/// True for an empty or whitespace-only file (a lone BOM included), which is
/// safe to treat as `{}`. Bytes that aren't valid UTF-8 are not blank: they
/// fall through to the parse, and the parse refuses.
pub fn is_blank(data: &[u8]) -> bool {
    let body = data.strip_prefix(BOM).unwrap_or(data);
    std::str::from_utf8(body).is_ok_and(|text| text.chars().all(char::is_whitespace))
}

/// `\r\n` when most of the file's line breaks are CRLF, else `\n`.
pub fn newline_style(bytes: &[u8]) -> &'static str {
    let mut crlf = 0usize;
    let mut lf = 0usize;
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            if index > 0 && bytes[index - 1] == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
    }
    if crlf > lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// How a file lays out its top-level object.
struct Style {
    /// Whitespace before each top-level key; empty when they share a line.
    member_indent: String,
    /// One level of indentation; empty for a one-line file.
    unit: String,
    newline: &'static str,
}

impl Style {
    fn of(source: &[u8], top: &TopLevel) -> Style {
        let newline = newline_style(source);
        let Some(first) = top.spans.first() else {
            return Style {
                member_indent: "  ".into(),
                unit: "  ".into(),
                newline,
            };
        };
        // The whitespace between the line start and the first key. A CRLF
        // line break ends in `\n` too, so this reads both styles.
        let mut start = first.key_start;
        while start > 0 && matches!(source[start - 1], b' ' | b'\t') {
            start -= 1;
        }
        if start > 0 && source[start - 1] == b'\n' {
            let indent = String::from_utf8_lossy(&source[start..first.key_start]).into_owned();
            let unit = if indent.is_empty() {
                "  ".to_owned()
            } else {
                indent.clone()
            };
            Style {
                member_indent: indent,
                unit,
                newline,
            }
        } else {
            Style {
                member_indent: String::new(),
                unit: String::new(),
                newline,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_on_a_non_object_is_a_no_op() {
        let mut value = Json::int(3);
        value.set("a", Some(Json::Null));
        assert_eq!(value, Json::int(3));
    }

    #[test]
    fn newline_majority() {
        assert_eq!(newline_style(b"{\r\n\"a\": 1\r\n}\n"), "\r\n");
        assert_eq!(newline_style(b"{\n\"a\": 1\r\n}\n"), "\n");
        assert_eq!(newline_style(b"{}"), "\n");
    }

    #[test]
    fn blank_includes_a_lone_bom() {
        assert!(is_blank(b""));
        assert!(is_blank(b" \r\n\t"));
        assert!(is_blank(b"\xEF\xBB\xBF\r\n"));
        assert!(!is_blank(b"\xFF"));
        assert!(!is_blank(b"{}"));
    }
}
