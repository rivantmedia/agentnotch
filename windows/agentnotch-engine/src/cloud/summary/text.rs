//! The text half of the session summarizer (CL§9.5-9.7; the Mac's
//! `SessionSummarizer.swift`): what a summary is written from (the
//! excerpt), what is taken out of it (`redact`), and what a summary may
//! carry when it leaves this PC (`scrub`: secrets redacted, whitespace
//! folded, absolute paths cut to their last component, at most 2,000
//! characters).
//!
//! The scrub is the last privacy gate before text leaves the PC, so where a
//! rule is ambiguous it shortens more. It knows the Mac's and Linux's paths
//! (WSL and remote sessions write `/home/…`) and the Windows forms: drive
//! paths with either separator, UNC, `\\?\`, `%USERPROFILE%`-style,
//! `~\`, MSYS `/c/Users/…`, WSL's `/mnt/c/…`, `\\wsl$\…` and
//! `\\wsl.localhost\…`. Pure apart from the transcript read and the one
//! folder listing in [`LocalNames`].

use crate::cloud::contract::{clamp_utf16, limit, seconds_between, trim_spaces};
use crate::cloud::ledger::{SessionOwners, Stretch};
use crate::cloud::scanner::{
    for_each_line, is_copied, is_safe_transcript_path, CHUNK_SIZE as SCAN_CHUNK_SIZE,
};
use crate::core::paths::PathStyle;
use crate::core::time::parse_iso8601;
use crate::platform::SecureFiles;
use fancy_regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};
use unicode_segmentation::UnicodeSegmentation;

/// The most characters of a session's text a summary is written from.
pub const MAX_EXCERPT_CHARACTERS: usize = 24_000;
/// Of a long session, this much of its start is kept; the rest of the
/// allowance goes to its end.
pub const HEAD_SHARE: usize = 8_000;
/// No one message takes more than this.
pub const MAX_SEGMENT: usize = 4_000;
/// Between a session's start and its end, when the middle was dropped.
pub const GAP_MARKER: &str = "\n\n[…]\n\n";

const REDACTED: &str = "[redacted]";

// ---- Whitespace and the scrub ----

/// Every run of whitespace (newlines, tabs, no-break and other wide spaces)
/// as one plain space, none at either end. Pure.
pub fn folding_whitespace(text: &str) -> String {
    text.split(char::is_whitespace)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `folding_whitespace`, then at most the contract's 2,000 UTF-16 units.
pub fn one_paragraph(text: &str) -> String {
    clamp_utf16(&folding_whitespace(text), limit::SUMMARY_TEXT)
}

/// A summary as it may leave this PC: likely secrets redacted ([`redact`]),
/// whitespace folded, absolute paths cut to their last component
/// ([`shorten_paths`]), and at most the contract's 2,000 characters.
/// Whitespace is folded before paths are shortened: a name split by a
/// newline, a double or a no-break space is still one name to them. Pure
/// given `known_names`; scrubbing twice changes nothing.
pub fn scrub(text: &str, known_names: &[String]) -> String {
    one_paragraph(&shorten_paths(
        &folding_whitespace(&redact(text)),
        known_names,
    ))
}

/// How a result was exited: the last non-empty line of what the process
/// wrote to stderr, in words. Pure.
pub fn exit_description(status: i32, stderr: &str) -> String {
    let last_line = stderr
        .split([
            '\n', '\r', '\u{0B}', '\u{0C}', '\u{85}', '\u{2028}', '\u{2029}',
        ])
        .map(trim_spaces)
        .rfind(|line| !line.is_empty());
    match last_line {
        Some(line) => {
            let lower = line.to_lowercase();
            if lower.contains("unknown option") || lower.contains("unknown argument") {
                format!(
                    "This Claude Code is too old for session summaries ({})",
                    prefix(line, 120)
                )
            } else {
                format!("Claude Code exited ({status}): {}", prefix(line, 160))
            }
        }
        None => format!("Claude Code exited with status {status}"),
    }
}

/// The first `count` characters (grapheme clusters).
fn prefix(text: &str, count: usize) -> String {
    text.graphemes(true).take(count).collect()
}

/// The number of characters (grapheme clusters), the way the Mac counts.
fn glen(text: &str) -> usize {
    text.graphemes(true).count()
}

// ---- Shortening paths ----

/// A path's kind: `Posix` uses `/` only, `Windows` both separators and
/// allows parentheses inside a component (`Program Files (x86)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Posix,
    Windows,
}

impl Mode {
    fn is_sep(self, c: char) -> bool {
        c == '/' || (self == Mode::Windows && c == '\\')
    }

    /// A character a path component may hold: no whitespace, none of
    /// `"'`<>[]{}` (and, in a Posix path, none of `()`).
    fn is_path_char(self, c: char) -> bool {
        !c.is_whitespace()
            && !"\"'`<>[]{}".contains(c)
            && (self == Mode::Windows || !"()".contains(c))
    }

    /// A character a word inside a path may hold: no separator, and no
    /// punctuation that ends a clause.
    fn is_word_char(self, c: char) -> bool {
        self.is_path_char(c) && !self.is_sep(c) && !",;!?".contains(c)
    }

    fn components(self, path: &[char]) -> Vec<String> {
        let mut parts = vec![String::new()];
        for &c in path {
            if self.is_sep(c) {
                parts.push(String::new());
            } else if let Some(last) = parts.last_mut() {
                last.push(c);
            }
        }
        parts
    }
}

/// Stands for a space inside a known name while the path is shortened (a
/// private-use character: never a path's own).
const NAME_JOINER: char = '\u{E000}';

/// Stands for an apostrophe inside a name while the path is shortened (see
/// `joining_apostrophes`): a Windows profile folder may be named
/// `Jane O'Neil`, and a quote would otherwise end the path inside the name
/// and leave `'Neil` behind. A quote around a path has a space or the
/// text's end on one side, so it still ends one.
const APOSTROPHE_JOINER: char = '\u{E001}';

/// Windows XP's profile root, still a link to `Users`: a name after it is a
/// user's, as after `Users`.
const LEGACY_PROFILES: &str = "Documents and Settings";

/// The folders a path may start at (the Mac's, and the Linux ones a WSL or
/// remote session writes).
const ROOTS: [&str; 15] = [
    "Users", "home", "private", "Volumes", "System", "opt", "var", "Library", "etc", "tmp", "mnt",
    "root", "media", "srv", "usr",
];

/// Words that join a path to what follows it in a sentence, never the rest
/// of a user's or volume's name.
const JOINING_WORDS: [&str; 34] = [
    "a", "an", "and", "as", "at", "but", "by", "for", "from", "in", "inside", "into", "is", "of",
    "on", "onto", "or", "over", "so", "than", "that", "the", "then", "to", "under", "via", "vs",
    "was", "were", "which", "while", "with", "within", "without",
];

/// Words that stand between the parts of a Windows folder's name
/// (`OneDrive - Company`, `Fish & Chips`).
const CONNECTORS: [&str; 5] = ["-", "–", "—", "&", "+"];

/// A character `\w` would take: a letter, digit, mark or `_`.
fn is_word_like(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || ('\u{0300}'..='\u{036F}').contains(&c)
}

/// What may not come right before a path's start.
fn blocks_start(c: char) -> bool {
    is_word_like(c) || matches!(c, '.' | '~' | '-')
}

/// `/Users/jane/work/acme/.env` → `.env`: absolute paths under /Users,
/// /home, /private, /Volumes, /System, /opt, /var, /Library, /etc, /tmp,
/// /mnt and /root, `~/…`, and the Windows forms (see the module's
/// description), cut to their last component (trailing punctuation kept
/// outside).
///
/// No user's or volume's name survives, spaces and all, and the words after
/// a path stay words:
/// - A component may contain spaces (`/Volumes/Macintosh HD/…`,
///   `/Users/Jane Doe/…`, `C:\Program Files\…`): the words after a space (up
///   to four, the last one followed by a separator) stay in the path when
///   they look like the rest of a folder's name. That is when each starts
///   with a capital letter or a digit (in a Windows path also a lone `-`,
///   `&` or `+` and what follows it, and a `(x86)`), and, for a folder
///   deeper than a user's or volume's name, the component before the space
///   isn't a file's name (`api.ts to handle client/server` stays prose). A
///   user's or volume's name may also go on in lowercase, by one word that
///   isn't a joining one (`and`, `for`, `to`…), when the path clearly does
///   after the separator (`/home/jane doe/tmp/x.log`: another separator, a
///   file's name, or nothing; `/Users/jane and src/a.ts` stays prose). Never
///   across a `.`, `,`, `;`, `:`, `!` or `?`, and never into a new path.
/// - A path that ends at a home folder (`/Users/jane`, `/home/jane`,
///   `C:\Users\jane`, `/mnt/c/Users/jane`, `\\wsl$\Ubuntu\home\jane`)
///   becomes `~`; one that ends at a volume (`/Volumes/Backup`), a drive's
///   root (`C:\`), a UNC server or share (`\\server\share`) or a WSL
///   distribution (`\\wsl$\Ubuntu`) becomes `…`: their last component is the
///   name. A name ending the path takes along the capitalised words (or
///   numbers) right after it, up to three (`/Volumes/My Passport` → `…`,
///   `/Users/Jane Doe` → `~`).
/// - `known_names` (this PC's users, see [`LocalNames`]) are recognised whole
///   after `/Volumes/`, `/Users/` or `/home/` (either separator), whatever
///   their case or spaces (`/Volumes/my backup` → `…`).
///
/// Spaces are plain single spaces here (`scrub` folds the rest first). Pure
/// given `known_names`; shortening twice changes nothing.
pub fn shorten_paths(text: &str, known_names: &[String]) -> String {
    let text = joining_apostrophes(&joining_legacy_profiles(&joining_known_names(
        text,
        known_names,
    )));
    let t: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    while cursor < t.len() {
        let Some((start, head_end, mode)) = find_start(&t, cursor) else {
            break;
        };
        result.extend(&t[cursor..start]);
        let mut end = head_end;
        while let Some(more) = continuation(&t, start, end, mode) {
            end = more;
        }
        let mut path: Vec<char> = t[start..end].to_vec();
        let mut trailing: Vec<char> = Vec::new();
        while path.len() > 1 && path.last().is_some_and(|c| ".,;:!?)]}'\"`".contains(*c)) {
            trailing.extend(path.pop());
        }
        trailing.reverse();
        let mut ends_in_slash = false;
        while path.len() > 1 && path.last().is_some_and(|c| mode.is_sep(*c)) {
            path.pop();
            ends_in_slash = true;
        }
        let mut next = end;
        match named_folder(&path, mode) {
            Some(named) => {
                // The name may go on past the space the path stopped at.
                if trailing.is_empty() && !ends_in_slash {
                    next = end_of_name(&t, end);
                }
                result.push(if named == Named::Home { '~' } else { '…' });
            }
            None => {
                let component = last_component(&path, mode);
                if component.is_empty() || component == "~" || is_root_like(&path, mode) {
                    result.push('…');
                } else {
                    result.push_str(&component);
                }
            }
        }
        result.extend(&trailing);
        cursor = next;
    }
    result.extend(&t[cursor..]);
    result
        .replace(NAME_JOINER, " ")
        .replace(APOSTROPHE_JOINER, "'")
}

/// `O'Neil` → `O<joiner>Neil`: an apostrophe after a letter or digit and
/// before three or more of them, so a name holding one stays one path
/// component. A contraction or possessive (`'s`, `'t`, `'re`, `'ll`…) is
/// left alone: `/Users/jane's files` still ends the path at the quote. Pure.
fn joining_apostrophes(text: &str) -> String {
    if !text.contains('\'') {
        return text.to_owned();
    }
    let mut t: Vec<char> = text.chars().collect();
    for i in 1..t.len() {
        let follows = t[i + 1..]
            .iter()
            .take(3)
            .take_while(|c| c.is_alphanumeric())
            .count();
        if t[i] == '\'' && t[i - 1].is_alphanumeric() && follows == 3 {
            t[i] = APOSTROPHE_JOINER;
        }
    }
    t.into_iter().collect()
}

/// `C:\Documents and Settings\…` → `C:\Documents<joiner>and<joiner>Settings\…`
/// (any case, either separator, right after a drive's root), so the matcher
/// takes the folder whole and knows the name after it is a user's. Pure.
fn joining_legacy_profiles(text: &str) -> String {
    if !text.contains(' ') || !(text.contains('\\') || text.contains('/')) {
        return text.to_owned();
    }
    let phrase: Vec<char> = LEGACY_PROFILES.chars().collect();
    let mut t: Vec<char> = text.chars().collect();
    let mut i = 3;
    while i + phrase.len() <= t.len() {
        let after_drive_root = matches!(t[i - 1], '\\' | '/')
            && t[i - 2] == ':'
            && t[i - 3].is_ascii_alphabetic()
            && (i == 3 || !is_word_like(t[i - 4]));
        let found = after_drive_root
            && same_ignoring_case(&t[i..i + phrase.len()], &phrase)
            && t.get(i + phrase.len())
                .is_none_or(|c| c.is_whitespace() || "/\\\"'`<>()[]{}.,;:!?".contains(*c));
        if found {
            for c in &mut t[i..i + phrase.len()] {
                if *c == ' ' {
                    *c = NAME_JOINER;
                }
            }
            i += phrase.len();
        } else {
            i += 1;
        }
    }
    t.into_iter().collect()
}

/// The first path in `t` at or after `from`: where it starts, where its
/// first run of components ends, and what kind it is.
fn find_start(t: &[char], from: usize) -> Option<(usize, usize, Mode)> {
    (from..t.len()).find_map(|i| {
        let mode = start_at(t, i)?;
        let mut end = i;
        while end < t.len() && mode.is_path_char(t[end]) {
            end += 1;
        }
        Some((i, end, mode))
    })
}

/// Whether a path starts at `i`, and of what kind.
fn start_at(t: &[char], i: usize) -> Option<Mode> {
    let before = i.checked_sub(1).map(|b| t[b]);
    if before.is_some_and(blocks_start) {
        return None;
    }
    let at = |k: usize| t.get(i + k).copied();
    // At least one more path character after the head.
    let more = |k: usize, mode: Mode| at(k).is_some_and(|c| mode.is_path_char(c));
    match t[i] {
        '~' => match at(1) {
            Some('/') if more(2, Mode::Posix) => Some(Mode::Posix),
            Some('\\') if more(2, Mode::Windows) => Some(Mode::Windows),
            _ => None,
        },
        '/' => {
            let word: String = t[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            let after = i + 1 + word.len();
            let slash_then_more = t.get(after) == Some(&'/') && more(after - i + 1, Mode::Posix);
            if ROOTS.contains(&word.as_str()) && slash_then_more {
                return Some(Mode::Posix);
            }
            // MSYS and Git Bash: `/c/Users/…` (never inside `//host/…`).
            (word.len() == 1 && slash_then_more && before != Some('/')).then_some(Mode::Posix)
        }
        c if c.is_ascii_alphabetic() => {
            (at(1) == Some(':') && matches!(at(2), Some('\\' | '/'))).then_some(Mode::Windows)
        }
        '\\' => {
            let run = t[i..].iter().take_while(|c| **c == '\\').count();
            (run >= 2 && more(run, Mode::Windows)).then_some(Mode::Windows)
        }
        '%' => {
            let name = t[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '(' | ')'))
                .count();
            (name > 0 && at(name + 1) == Some('%') && matches!(at(name + 2), Some('\\' | '/')))
                .then_some(Mode::Windows)
        }
        _ => None,
    }
}

/// Where `t[start..end]` (a path that ends just before a space) goes on
/// past that space: the end of the components after the next separator.
/// `None` when the path ends there (see `shorten_paths`). Pure.
fn continuation(t: &[char], start: usize, end: usize, mode: Mode) -> Option<usize> {
    let space = end;
    if t.get(space) != Some(&' ') || end == start || ".,;:!?".contains(t[end - 1]) {
        return None;
    }
    let path = &t[start..end];
    let is_name = named_folder(path, mode).is_some();
    let component = mode.components(path).pop().unwrap_or_default();
    if !is_name && is_file_name(&component.chars().collect::<Vec<_>>()) {
        return None;
    }
    // Up to four words, the last one followed by a separator.
    let mut words: Vec<String> = Vec::new();
    let mut index = space + 1;
    loop {
        if index >= t.len() || !mode.is_word_char(t[index]) || t[index] == '~' {
            return None;
        }
        let word_start = index;
        while index < t.len() && mode.is_word_char(t[index]) {
            index += 1;
        }
        words.push(t[word_start..index].iter().collect());
        let &next = t.get(index)?;
        if mode.is_sep(next) {
            break;
        }
        let last = t[index - 1];
        if next != ' ' || words.len() >= 4 || ".:".contains(last) {
            return None;
        }
        index += 1;
    }
    let sep = index;
    let mut stop = sep + 1;
    while stop < t.len() && mode.is_path_char(t[stop]) {
        stop += 1;
    }
    let capitalised = words_look_like_a_name(&words, mode);
    // A user's or volume's name in lowercase: one more word, never a joining
    // one (`/home/jane doe/…`, but `/Users/jane and src/…`).
    let lowercase_name = is_name
        && words.len() == 1
        && !JOINING_WORDS.contains(&words[0].to_lowercase().as_str())
        && goes_on_as_a_path(&t[sep + 1..stop], mode);
    (capitalised || lowercase_name).then_some(stop)
}

/// Each word starts with a capital letter or a digit; in a Windows path
/// also a lone connector (`-`, `&`), the word after one, and a `(x86)`.
fn words_look_like_a_name(words: &[String], mode: Mode) -> bool {
    let mut after_connector = false;
    for word in words {
        let starts_like_a_name = word
            .chars()
            .next()
            .is_some_and(|c| c.is_uppercase() || c.is_numeric());
        let accepted = starts_like_a_name
            || (mode == Mode::Windows
                && (CONNECTORS.contains(&word.as_str())
                    || after_connector
                    || (word.len() > 2 && word.starts_with('(') && word.ends_with(')'))));
        if !accepted {
            return false;
        }
        after_connector = CONNECTORS.contains(&word.as_str());
    }
    true
}

/// `api.ts`, `com.acme.plist`, `v1.2`: a name, a dot and a short extension
/// of letters and digits (not a dot folder like `.config`). Pure.
fn is_file_name(component: &[char]) -> bool {
    let Some(dot) = component.iter().rposition(|c| *c == '.') else {
        return false;
    };
    let extension = &component[dot + 1..];
    dot > 0 && (1..=10).contains(&extension.len()) && extension.iter().all(|c| c.is_alphanumeric())
}

/// What follows a separator is clearly more of a path: another separator, a
/// file's name, or nothing (the separator ended it). Pure.
fn goes_on_as_a_path(rest: &[char], mode: Mode) -> bool {
    let mut rest = rest;
    while let Some((last, before)) = rest.split_last() {
        if ".,;:!?".contains(*last) {
            rest = before;
        } else {
            break;
        }
    }
    rest.is_empty() || rest.iter().any(|c| mode.is_sep(*c)) || is_file_name(rest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    Home,
    Volume,
}

/// Whether the path ends at a user's home folder (`…/Users/<name>`,
/// `…/home/<name>`) or a volume (`…/Volumes/<name>`, `/mnt/<drive>`). Pure.
fn named_folder(path: &[char], mode: Mode) -> Option<Named> {
    let mut parts = mode.components(path);
    if mode == Mode::Windows {
        parts.retain(|part| !part.is_empty());
    }
    if parts.len() < 3 || parts.last().is_none_or(|name| name.is_empty()) {
        return None;
    }
    let parent = parts[parts.len() - 2].to_lowercase();
    if mode == Mode::Windows
        && parent
            == LEGACY_PROFILES
                .to_lowercase()
                .replace(' ', &NAME_JOINER.to_string())
    {
        return Some(Named::Home);
    }
    match parent.as_str() {
        "users" | "home" => Some(Named::Home),
        "volumes" => Some(Named::Volume),
        // WSL's `/mnt/<drive>`, never a folder that happens to be called mnt.
        "mnt" if parts.len() == 3 && parts[0].is_empty() => Some(Named::Volume),
        _ => None,
    }
}

/// The last component of a path (a trailing separator already taken off).
fn last_component(path: &[char], mode: Mode) -> String {
    mode.components(path).pop().unwrap_or_default()
}

/// A path that names nothing but a drive, a UNC server or share, a WSL
/// distribution or an environment variable: no component of it is one to
/// keep.
fn is_root_like(path: &[char], mode: Mode) -> bool {
    if mode != Mode::Windows {
        return false;
    }
    let unc = path.starts_with(&['\\', '\\']);
    let mut parts: Vec<String> = mode
        .components(path)
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    if unc && matches!(parts.first().map(String::as_str), Some("?" | ".")) {
        parts.remove(0);
        if parts.first().is_some_and(|p| p.eq_ignore_ascii_case("UNC")) {
            parts.remove(0);
        }
    }
    let is_drive = |part: &str| {
        part.len() == 2
            && part.starts_with(|c: char| c.is_ascii_alphabetic())
            && part.ends_with(':')
    };
    let is_variable = |part: &str| part.len() > 2 && part.starts_with('%') && part.ends_with('%');
    match parts.as_slice() {
        [only] => is_drive(only) || is_variable(only) || unc,
        [drive, ..] if unc && is_drive(drive) => false,
        _ if unc => parts.len() <= 2,
        _ => false,
    }
}

/// Where a user's or volume's name that ends a path ends: past up to three
/// more words that start with a capital letter or a digit (`My Passport`,
/// `Macintosh HD`, `Untitled 2`, `Jane Doe`), stopping at punctuation. Pure.
fn end_of_name(t: &[char], start: usize) -> usize {
    let mut end = start;
    for _ in 0..3 {
        let mut index = end;
        if t.get(index) != Some(&' ') {
            break;
        }
        index += 1;
        if !t
            .get(index)
            .is_some_and(|c| c.is_uppercase() || c.is_numeric())
        {
            break;
        }
        let mut word_end = index;
        while word_end < t.len()
            && !t[word_end].is_whitespace()
            && !"\"'`<>()[]{}/\\.,;:!?".contains(t[word_end])
        {
            word_end += 1;
        }
        end = word_end;
        if word_end < t.len() && t[word_end] != ' ' {
            break;
        }
    }
    end
}

/// `/Volumes/My Passport` → `/Volumes/My<joiner>Passport` for every known
/// name with a space in it, after `/Volumes/`, `/Users/` or `/home/` (either
/// separator) and before the end, a separator, whitespace or punctuation, so
/// the matcher takes the name whole. Longest names first, whatever the case.
/// Pure.
fn joining_known_names(text: &str, names: &[String]) -> String {
    let spaced: BTreeSet<Vec<char>> = names
        .iter()
        .map(|name| trim_spaces(name))
        .filter(|name| name.contains(' '))
        .map(|name| name.chars().collect())
        .collect();
    if spaced.is_empty() || !(text.contains('/') || text.contains('\\')) {
        return text.to_owned();
    }
    let mut ordered: Vec<&Vec<char>> = spaced.iter().collect();
    ordered.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    let mut t: Vec<char> = text.chars().collect();
    for name in ordered {
        let mut i = 0;
        while i + name.len() <= t.len() {
            let found = same_ignoring_case(&t[i..i + name.len()], name)
                && follows_a_name_folder(&t[..i])
                && t.get(i + name.len())
                    .is_none_or(|c| c.is_whitespace() || "/\\\"'`<>()[]{}.,;:!?".contains(*c));
            if found {
                for c in &mut t[i..i + name.len()] {
                    if *c == ' ' {
                        *c = NAME_JOINER;
                    }
                }
                i += name.len();
            } else {
                i += 1;
            }
        }
    }
    t.into_iter().collect()
}

/// Whether `before` ends with `/Volumes/`, `/Users/` or `/home/` (either
/// separator, any case).
fn follows_a_name_folder(before: &[char]) -> bool {
    ["Volumes", "Users", "home"].iter().any(|folder| {
        let folder: Vec<char> = folder.chars().collect();
        let length = folder.len() + 2;
        before.len() >= length
            && matches!(before[before.len() - length], '/' | '\\')
            && matches!(before[before.len() - 1], '/' | '\\')
            && same_ignoring_case(
                &before[before.len() - length + 1..before.len() - 1],
                &folder,
            )
    })
}

fn same_ignoring_case(a: &[char], b: &[char]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.to_lowercase().eq(y.to_lowercase()))
}

// ---- This PC's names ----

/// The names a summary's paths may carry on this PC, so the scrub knows them
/// whole (spaces and all): its users' profile folders (`%SystemDrive%\Users`,
/// without the system's own) and the home folder's own name. Folder
/// listings only; nothing in them is opened. None when sealed. Listed again
/// at most once a minute.
pub struct LocalNames {
    users: Option<PathBuf>,
    home: String,
    sealed: bool,
    cache: Mutex<Option<(SystemTime, Vec<String>)>>,
}

impl LocalNames {
    pub const LIFETIME: Duration = Duration::from_secs(60);

    /// Folders of `%SystemDrive%\Users` that are the system's, not a person's.
    const SYSTEM_ENTRIES: [&'static str; 5] = [
        "Public",
        "Default",
        "Default User",
        "All Users",
        "desktop.ini",
    ];

    pub fn new(users: Option<PathBuf>, home: &str, sealed: bool) -> Self {
        LocalNames {
            users,
            home: home.to_owned(),
            sealed,
            cache: Mutex::new(None),
        }
    }

    /// The names now (`[]` when sealed), listed again when the last listing
    /// is a minute old or more.
    pub fn current(&self, now: SystemTime) -> Vec<String> {
        if self.sealed {
            return Vec::new();
        }
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, names)) = cache.as_ref() {
            if seconds_between(now, *at).abs() < Self::LIFETIME.as_secs_f64() {
                return names.clone();
            }
        }
        let names = Self::list_windows(self.users.as_deref(), &self.home);
        *cache = Some((now, names.clone()));
        names
    }

    /// `text` scrubbed with this PC's names.
    pub fn scrub(&self, text: &str, now: SystemTime) -> String {
        scrub(text, &self.current(now))
    }

    /// The entries of the users folder (the system's own left out) and the
    /// home folder's own name. Pure apart from the listing.
    pub fn list_windows(users: Option<&Path>, home: &str) -> Vec<String> {
        let mut names = BTreeSet::new();
        if let Some(users) = users {
            for entry in std::fs::read_dir(users).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !Self::SYSTEM_ENTRIES
                    .iter()
                    .any(|system| system.eq_ignore_ascii_case(&name))
                {
                    names.insert(name);
                }
            }
        }
        names.extend(home_name(home));
        names.into_iter().collect()
    }

    /// The Mac's listing: the names in `volumes` and `users` (hidden ones and
    /// `Shared` left out) and the home folder's own. Kept for the vectors
    /// the Mac's tests give it. Pure apart from the listings.
    pub fn list(volumes: &Path, users: &Path, home: &str) -> Vec<String> {
        let mut names = BTreeSet::new();
        for folder in [volumes, users] {
            for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with('.') && name != "Shared" {
                    names.insert(name);
                }
            }
        }
        names.extend(home_name(home));
        names.into_iter().collect()
    }
}

/// The last component of the home folder's path, whichever separator it uses.
fn home_name(home: &str) -> Option<String> {
    home.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .map(str::to_owned)
}

// ---- The excerpt ----

/// The excerpt of the transcript at `path` (a session's own file, read in
/// full), at most `limit` characters: the typed prompts and Claude's replies,
/// text only. With `stretches` (one account's part of a session more than
/// one account ran), only lines dated within them. `None` when it can't be
/// read, isn't a transcript under a `projects` folder (a link out of one is
/// never opened) or holds no conversation.
pub fn build_excerpt(
    files: &dyn SecureFiles,
    style: PathStyle,
    path: &str,
    session_id: &str,
    stretches: Option<&[Stretch]>,
    limit: usize,
) -> Option<String> {
    if !is_safe_transcript_path(path, style) {
        return None;
    }
    let real = files.canonical(Path::new(path)).ok()?;
    if !is_safe_transcript_path(&real.to_string_lossy(), style) {
        return None;
    }
    let mut excerpt = Accumulator::new(limit);
    let mut offset = 0u64;
    for_each_line(&real, &mut offset, SCAN_CHUNK_SIZE, |line| {
        let Ok(json @ Value::Object(_)) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if let Some(stretches) = stretches {
            let stamp = json
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_iso8601);
            if !SessionOwners::contains(stretches, stamp) {
                return;
            }
        }
        if let Some(segment) = segment(&json, session_id) {
            excerpt.add(&segment);
        }
    })?;
    let text = excerpt.text();
    (!text.is_empty()).then_some(text)
}

/// One line as excerpt text: "User: …" for a typed prompt, "Claude: …" for a
/// reply's text. `None` for everything else (tool calls and results,
/// thinking, meta lines, subagents, lines of another session or copied from
/// one). Pure.
pub fn segment(json: &Value, session_id: &str) -> Option<String> {
    if json.get("isSidechain").and_then(Value::as_bool) == Some(true)
        || json.get("isMeta").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    if is_copied(json, session_id) {
        return None;
    }
    let content = json.get("message").and_then(|m| m.get("content"));
    let (speaker, text) = match json.get("type").and_then(Value::as_str) {
        Some("user") => {
            if !is_human_prompt(json) {
                return None;
            }
            let text = match content.and_then(Value::as_str) {
                Some(text) => text.to_owned(),
                None => text_blocks(content),
            };
            ("User", text)
        }
        Some("assistant") => ("Claude", text_blocks(content)),
        _ => return None,
    };
    let cleaned = redact(text.trim());
    if cleaned.is_empty() {
        return None;
    }
    Some(format!("{speaker}: {}", clipped(&cleaned, MAX_SEGMENT)))
}

/// The content as an array of objects (the Mac's `[[String: Any]]` cast).
fn blocks(content: Option<&Value>) -> Option<Vec<&Map<String, Value>>> {
    content?.as_array()?.iter().map(Value::as_object).collect()
}

fn text_blocks(content: Option<&Value>) -> String {
    let Some(blocks) = blocks(content) else {
        return String::new();
    };
    blocks
        .into_iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a user line carries that a person typed: not a meta line, a tool
/// result, a compact summary, a prompt another source injected (a finished
/// background task, a slash command's echo, a caveat), nor anything whose
/// origin isn't a human. Pure.
pub fn is_human_prompt(json: &Value) -> bool {
    if json.get("type").and_then(Value::as_str) != Some("user")
        || json.get("isMeta").and_then(Value::as_bool) == Some(true)
        || json.get("toolUseResult").is_some()
        || json.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
    {
        return false;
    }
    if let Some(origin) = json.get("origin").and_then(Value::as_object) {
        if origin.get("kind").and_then(Value::as_str) != Some("human") {
            return false;
        }
    }
    let content = json.get("message").and_then(|m| m.get("content"));
    let text = match content.and_then(Value::as_str) {
        Some(text) => Some(text),
        None => blocks(content).and_then(|blocks| {
            blocks
                .into_iter()
                .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .and_then(|b| b.get("text"))
                .and_then(Value::as_str)
        }),
    };
    !is_injected_prompt(text)
}

const INJECTED_PREFIXES: [&str; 7] = [
    "<task-notification>",
    "<command-name>",
    "<command-message>",
    "<local-command",
    "<bash-input>",
    "<bash-stdout>",
    "Caveat:",
];

fn is_injected_prompt(prompt: Option<&str>) -> bool {
    let Some(prompt) = prompt else {
        return false;
    };
    let prompt = prompt.trim_start();
    INJECTED_PREFIXES
        .iter()
        .any(|prefix| prompt.starts_with(prefix))
}

/// The start and end of `text` with a marker between, `limit` characters at
/// most.
pub fn clipped(text: &str, limit: usize) -> String {
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    if graphemes.len() <= limit {
        return text.to_owned();
    }
    let marker = " […] ";
    let keep = limit.saturating_sub(glen(marker));
    let head = keep / 2;
    let tail = keep - head;
    let mut out: String = graphemes[..head].concat();
    out.push_str(marker);
    out.push_str(&graphemes[graphemes.len() - tail..].concat());
    out
}

/// Keeps the first `HEAD_SHARE` characters and the latest rest, never
/// holding more than the limit (a transcript can be hundreds of MB).
struct Accumulator {
    limit: usize,
    head: String,
    head_len: usize,
    head_full: bool,
    tail: VecDeque<(String, usize)>,
    tail_len: usize,
    dropped: bool,
}

impl Accumulator {
    fn new(limit: usize) -> Self {
        Accumulator {
            limit,
            head: String::new(),
            head_len: 0,
            head_full: false,
            tail: VecDeque::new(),
            tail_len: 0,
            dropped: false,
        }
    }

    fn head_limit(&self) -> usize {
        HEAD_SHARE.min(self.limit)
    }

    fn tail_limit(&self) -> usize {
        self.limit
            .saturating_sub(self.head_limit() + glen(GAP_MARKER))
    }

    fn add(&mut self, segment: &str) {
        let piece = format!("{segment}\n\n");
        let length = glen(&piece);
        if !self.head_full {
            if self.head_len + length <= self.head_limit() {
                self.head.push_str(&piece);
                self.head_len += length;
                return;
            }
            self.head_full = true;
        }
        self.tail.push_back((piece, length));
        self.tail_len += length;
        while self.tail_len > self.tail_limit() {
            let Some((_, dropped)) = self.tail.pop_front() else {
                break;
            };
            self.tail_len -= dropped;
            self.dropped = true;
        }
    }

    fn text(&self) -> String {
        let end: String = self.tail.iter().map(|(piece, _)| piece.as_str()).collect();
        let whole =
            if self.dropped || (!end.is_empty() && self.head_len + self.tail_len > self.limit) {
                format!("{}{GAP_MARKER}{end}", self.head)
            } else {
                format!("{}{end}", self.head)
            };
        prefix(whole.trim(), self.limit)
    }
}

// ---- Redaction ----

/// Likely secrets replaced by `[redacted]`: private key blocks, API keys and
/// access tokens in their usual formats (`sk-…`, `sk_live_…`, `rk_…`,
/// `ghp_…`/`gho_…`/`github_pat_…`, `xoxb-…`/`xoxp-…`, `AKIA…`, `AIza…`,
/// `sb_secret_…`, `npm_…`, `ya29.…`, JWTs), `Bearer <token>`, the password in
/// `scheme://user:password@host`, the value of a
/// `…password…`/`…secret…`/`…token…`/`…key…` setting, the value of any
/// `NAME=value` or `NAME: value` whose name ends in KEY, TOKEN, SECRET,
/// PASSWORD or PASS (`STRIPE_KEY`, `db_pass`, `apiToken`; a plain word like
/// "key" or "monkey" in a sentence isn't a name), and any other run of 32 or
/// more letters and digits that looks random. A summary never needs them,
/// and both the excerpt and the summary leave this PC. When a pattern gives
/// up on a pathological text (its backtracking limit), the whole text is
/// redacted. Pure.
pub fn redact(text: &str) -> String {
    let mut result = text.to_owned();
    for (pattern, template) in secret_patterns() {
        match replace_all(pattern, &result, template) {
            Some(replaced) => result = replaced,
            None => return REDACTED.to_owned(),
        }
    }
    redact_random_runs(&result)
}

/// `Regex::replace_all`, but a matching error is a `None`, not a panic.
fn replace_all(pattern: &Regex, text: &str, template: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for captures in pattern.captures_iter(text) {
        let captures = captures.ok()?;
        let found = captures.get(0)?;
        out.push_str(&text[last..found.start()]);
        captures.expand(template, &mut out);
        last = found.end();
    }
    out.push_str(&text[last..]);
    Some(out)
}

fn secret_patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        SECRET_PATTERNS
            .iter()
            .map(|(pattern, template)| {
                (
                    Regex::new(pattern)
                        .unwrap_or_else(|e| panic!("redaction pattern {pattern}: {e}")),
                    *template,
                )
            })
            .collect()
    })
}

const SECRET_PATTERNS: [(&str, &str); 16] = [
    // A private key block, or its first line when the end is cut off.
    (
        r"-----BEGIN [A-Z0-9 ]*KEY-----(?:[\s\S]*?-----END [A-Z0-9 ]*KEY-----|[\s\S]*)",
        "[redacted]",
    ),
    (r"\bsk-[A-Za-z0-9_\-]{20,}", "[redacted]"),
    (
        r"\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{10,}",
        "[redacted]",
    ),
    (r"\bgh[pousr]_[A-Za-z0-9]{20,}", "[redacted]"),
    (r"\bgithub_pat_[A-Za-z0-9_]{20,}", "[redacted]"),
    (r"\bAKIA[0-9A-Z]{16}\b", "[redacted]"),
    (r"\bxox[abprs]-[A-Za-z0-9\-]{10,}", "[redacted]"),
    (r"\bAIza[0-9A-Za-z_\-]{30,}", "[redacted]"),
    (r"\bsb_secret_[A-Za-z0-9_\-]{10,}", "[redacted]"),
    (r"\bnpm_[A-Za-z0-9]{20,}", "[redacted]"),
    (r"\bya29\.[A-Za-z0-9_\-]{20,}", "[redacted]"),
    (
        r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
        "[redacted]",
    ),
    (
        r"(?i)\b(bearer)\s+[A-Za-z0-9._~+/=\-]{12,}",
        "${1} [redacted]",
    ),
    (
        r"(?i)\b([a-z][a-z0-9+.\-]*://[^\s:/@]+):[^\s@/]+@",
        "${1}:[redacted]@",
    ),
    (
        r##"(?i)((?<![A-Za-z0-9])[A-Za-z0-9_.\-]*(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credential)[A-Za-z0-9_.\-]*["']?\s*[:=]\s*)["']?(?!\[redacted\])[^\s"']{6,}["']?"##,
        "${1}[redacted]",
    ),
    // Any setting whose name ends in KEY, TOKEN, SECRET, PASSWORD or PASS,
    // whatever the value's length: an upper-case name, or one whose ending
    // follows a `_`, `.` or `-` or starts a camel-case word. (A value
    // already redacted is left as it is, so scrubbing twice changes
    // nothing.)
    (
        r##"((?<![A-Za-z0-9_.\-])(?:[A-Z0-9_.\-]*(?:KEY|TOKEN|SECRET|PASSWORD|PASS)|[A-Za-z0-9_.\-]*(?:[_.\-](?i:key|token|secret|password|pass)|[A-Za-z0-9](?:Key|Token|Secret|Password|Pass)))["']?\s*[:=]\s*)["']?(?!\[redacted\])[^\s"']+["']?"##,
        "${1}[redacted]",
    ),
];

/// Runs of 32 or more key-like characters that mix letters and digits and
/// look random (at least 3 bits of entropy per character): tokens the named
/// patterns don't know. Words, and hex hashes of a few kinds, go too.
fn redact_random_runs(text: &str) -> String {
    static RUN: OnceLock<regex::Regex> = OnceLock::new();
    let run = RUN.get_or_init(|| {
        regex::Regex::new(r"[A-Za-z0-9+/=_\-]{32,}").expect("the random-run pattern compiles")
    });
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for found in run.find_iter(text) {
        let candidate = found.as_str();
        if candidate.bytes().any(|b| b.is_ascii_digit())
            && candidate.bytes().any(|b| b.is_ascii_alphabetic())
            && entropy(candidate) >= 3.0
        {
            out.push_str(&text[last..found.start()]);
            out.push_str(REDACTED);
            last = found.end();
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Shannon entropy in bits per character. Pure.
pub fn entropy(text: &str) -> f64 {
    let mut counts: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    let mut total = 0usize;
    for c in text.chars() {
        *counts.entry(c).or_default() += 1;
        total += 1;
    }
    if total == 0 {
        return 0.0;
    }
    let total = total as f64;
    counts
        .values()
        .map(|count| {
            let p = *count as f64 / total;
            -p * p.log2()
        })
        .sum()
}
