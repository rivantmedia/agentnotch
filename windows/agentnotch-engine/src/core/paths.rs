//! Paths as strings, with the rules of the OS whose paths they are (AU§1).
//!
//! Every folder the engine keeps is a normalized string: `~` expanded, `.`
//! and `..` resolved lexically (links are NOT resolved, so the result stays
//! recognisable), no trailing separator except at a root. On Windows both
//! separators are accepted and `\` is written, `\\?\` prefixes are dropped,
//! the drive letter is upper-cased, and paths compare case-insensitively:
//! [`Paths::key`] is the lower-cased form used for map keys and hashes,
//! [`Paths::normalize`] keeps the case for display.
//!
//! The module is a pure function of a [`PathStyle`], chosen from
//! `cfg!(windows)` by default and passed explicitly in tests, so both the
//! Windows and the POSIX rules are tested on every OS.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathStyle {
    Windows,
    Posix,
}

impl PathStyle {
    /// The style of the OS this build runs on.
    pub fn native() -> PathStyle {
        if cfg!(windows) {
            PathStyle::Windows
        } else {
            PathStyle::Posix
        }
    }

    /// The separator written.
    pub fn separator(self) -> char {
        match self {
            PathStyle::Windows => '\\',
            PathStyle::Posix => '/',
        }
    }

    /// Separates the folders of `AGENTNOTCH_EXTRA_CONFIG_DIRS` (drive
    /// letters contain `:` on Windows).
    pub fn list_separator(self) -> char {
        match self {
            PathStyle::Windows => ';',
            PathStyle::Posix => ':',
        }
    }

    fn is_separator(self, c: char) -> bool {
        match self {
            PathStyle::Windows => c == '\\' || c == '/',
            PathStyle::Posix => c == '/',
        }
    }

    fn case_insensitive(self) -> bool {
        self == PathStyle::Windows
    }
}

/// A path split into its root (`C:\`, `\\server\share\`, `/`, the
/// drive-relative `C:`, the root-relative `\`, or nothing) and its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Split {
    root: String,
    parts: Vec<String>,
}

impl Split {
    /// A root that parts can't climb above (`..` at it is dropped).
    fn is_absolute(&self) -> bool {
        !self.root.is_empty() && !(self.root.len() == 2 && self.root.ends_with(':'))
    }

    fn join(&self, style: PathStyle) -> String {
        let sep = style.separator().to_string();
        let body = self.parts.join(&sep);
        if self.root.is_empty() {
            return body;
        }
        if body.is_empty() {
            return self.root.clone();
        }
        let mut out = self.root.clone();
        if !out.ends_with(style.separator()) && !(out.len() == 2 && out.ends_with(':')) {
            out.push(style.separator());
        }
        out.push_str(&body);
        out
    }
}

fn split(style: PathStyle, path: &str) -> Split {
    match style {
        PathStyle::Posix => split_posix(path),
        PathStyle::Windows => split_windows(path),
    }
}

fn split_posix(path: &str) -> Split {
    let root = if path.starts_with('/') {
        "/".to_owned()
    } else {
        String::new()
    };
    let raw: Vec<&str> = path.split('/').collect();
    resolve(root, raw)
}

fn split_windows(path: &str) -> Split {
    let mut s: String = path.replace('/', "\\");
    // Verbatim paths: `\\?\C:\x` is `C:\x`, `\\?\UNC\server\share` is `\\server\share`.
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = if rest.len() >= 4
            && rest.is_char_boundary(4)
            && rest[..4].eq_ignore_ascii_case(r"UNC\")
        {
            format!(r"\\{}", &rest[4..])
        } else {
            rest.to_owned()
        };
    }
    let bytes = s.as_bytes();
    if let Some(unc) = s.strip_prefix(r"\\") {
        // UNC (or a device path such as `\\.\pipe\x`): the server and share
        // are the root.
        let mut segments = unc.split('\\').filter(|seg| !seg.is_empty());
        let server = segments.next().unwrap_or("");
        let share = segments.next();
        let root = match share {
            Some(share) => format!(r"\\{server}\{share}\"),
            None => format!(r"\\{server}\"),
        };
        return resolve(root, segments.collect());
    }
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let drive = format!("{}:", (bytes[0] as char).to_ascii_uppercase());
        let rest = &s[2..];
        return match rest.strip_prefix('\\') {
            Some(rest) => resolve(format!("{drive}\\"), rest.split('\\').collect()),
            None => resolve(drive, rest.split('\\').collect()),
        };
    }
    match s.strip_prefix('\\') {
        Some(rest) => resolve("\\".to_owned(), rest.split('\\').collect()),
        None => resolve(String::new(), s.split('\\').collect()),
    }
}

/// `.` dropped, `..` resolved lexically.
fn resolve(root: String, raw: Vec<&str>) -> Split {
    let mut result = Split {
        root,
        parts: Vec::new(),
    };
    for part in raw {
        match part {
            "" | "." => {}
            ".." => {
                if result.parts.last().is_some_and(|last| last != "..") {
                    result.parts.pop();
                } else if !result.is_absolute() {
                    result.parts.push("..".to_owned());
                }
            }
            other => result.parts.push(other.to_owned()),
        }
    }
    result
}

/// Path rules for one style and one home folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    style: PathStyle,
    home: String,
}

impl Paths {
    pub fn new(style: PathStyle, home: &str) -> Paths {
        Paths {
            style,
            home: split(style, home).join(style),
        }
    }

    /// The native style, for the real home.
    pub fn native(home: &Path) -> Paths {
        Paths::new(PathStyle::native(), &home.to_string_lossy())
    }

    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// The home folder, normalized.
    pub fn home(&self) -> &str {
        &self.home
    }

    /// `~`, `~\x` and `~/x` against the home folder; anything else as is.
    fn expand_tilde(&self, path: &str) -> String {
        if path == "~" {
            return self.home.clone();
        }
        let mut chars = path.chars();
        if chars.next() == Some('~') && chars.next().is_some_and(|c| self.style.is_separator(c)) {
            return format!("{}{}{}", self.home, self.style.separator(), &path[2..]);
        }
        path.to_owned()
    }

    /// The normalized path, in display case.
    pub fn normalize(&self, path: &str) -> String {
        split(self.style, &self.expand_tilde(path)).join(self.style)
    }

    /// The form two spellings of one path share: map keys and hashes
    /// (lower-cased on Windows, where paths compare case-insensitively).
    pub fn key(&self, path: &str) -> String {
        let normalized = self.normalize(path);
        if self.style.case_insensitive() {
            normalized.to_lowercase()
        } else {
            normalized
        }
    }

    /// Two paths name the same folder, spelled alike (links not resolved).
    pub fn same(&self, a: &str, b: &str) -> bool {
        self.key(a) == self.key(b)
    }

    /// What Settings shows: the path in display case, `~` for the home
    /// folder (`~\.claude-work`).
    pub fn abbreviate(&self, path: &str) -> String {
        let normalized = self.normalize(path);
        if self.same(&normalized, &self.home) {
            return "~".to_owned();
        }
        match self.strip_prefix(&self.home, &normalized) {
            Some(rest) => format!("~{}{}", self.style.separator(), rest),
            None => normalized,
        }
    }

    /// The part of `path` below `ancestor`, without a leading separator, when
    /// `path` is strictly inside it.
    pub fn strip_prefix(&self, ancestor: &str, path: &str) -> Option<String> {
        let ancestor = split(self.style, &self.expand_tilde(ancestor));
        let inner = split(self.style, &self.expand_tilde(path));
        let fold = |s: &str| {
            if self.style.case_insensitive() {
                s.to_lowercase()
            } else {
                s.to_owned()
            }
        };
        if fold(&ancestor.root) != fold(&inner.root) || inner.parts.len() <= ancestor.parts.len() {
            return None;
        }
        let matches = ancestor
            .parts
            .iter()
            .zip(&inner.parts)
            .all(|(a, b)| fold(a) == fold(b));
        matches
            .then(|| inner.parts[ancestor.parts.len()..].join(&self.style.separator().to_string()))
    }

    /// `path` is `ancestor` or inside it.
    pub fn is_within(&self, ancestor: &str, path: &str) -> bool {
        self.same(ancestor, path) || self.strip_prefix(ancestor, path).is_some()
    }

    /// A drive, share or `/` root.
    pub fn is_root(&self, path: &str) -> bool {
        let split = split(self.style, &self.expand_tilde(path));
        split.parts.is_empty() && split.is_absolute()
    }

    /// `child` inside `base`, normalized.
    pub fn join(&self, base: &str, child: &str) -> String {
        self.normalize(&format!(
            "{}{}{}",
            self.expand_tilde(base),
            self.style.separator(),
            child
        ))
    }

    /// The root first (when there is one), then each part: NSString's
    /// `pathComponents`.
    pub fn components(&self, path: &str) -> Vec<String> {
        let split = split(self.style, &self.expand_tilde(path));
        let mut components = Vec::with_capacity(split.parts.len() + 1);
        if !split.root.is_empty() {
            components.push(split.root);
        }
        components.extend(split.parts);
        components
    }

    fn join_components(&self, components: &[String]) -> String {
        let Some(first) = components.first() else {
            return String::new();
        };
        let is_root =
            first.ends_with(self.style.separator()) || (first.len() == 2 && first.ends_with(':'));
        let split = if is_root {
            Split {
                root: first.clone(),
                parts: components[1..].to_vec(),
            }
        } else {
            Split {
                root: String::new(),
                parts: components.to_vec(),
            }
        };
        split.join(self.style)
    }

    /// The last part (`.claude-work`); `None` at a root or for `""`.
    pub fn file_name(&self, path: &str) -> Option<String> {
        split(self.style, &self.expand_tilde(path))
            .parts
            .last()
            .cloned()
    }

    /// The folder holding `path`; `None` at a root.
    pub fn parent(&self, path: &str) -> Option<String> {
        let mut split = split(self.style, &self.expand_tilde(path));
        split.parts.pop()?;
        Some(split.join(self.style))
    }

    pub fn to_path_buf(&self, path: &str) -> PathBuf {
        PathBuf::from(self.normalize(path))
    }

    // ---- Claude Code's layout ----

    /// `~\.claude`, where Claude Code runs when `CLAUDE_CONFIG_DIR` is unset.
    pub fn default_config_dir(&self) -> String {
        self.join(&self.home, ".claude")
    }

    pub fn is_default_config_dir(&self, dir: &str) -> bool {
        self.same(dir, &self.default_config_dir())
    }

    /// `~\.claude.json`: the default folder's identity file.
    pub fn default_identity_file(&self) -> String {
        self.join(&self.home, ".claude.json")
    }

    /// The config folder of a transcript path: the part before
    /// `projects\<slug>\<file>.jsonl` (subagent transcripts sit deeper). The
    /// search runs from the end, so a folder that itself contains a
    /// `projects` component still resolves; `projects` compares
    /// case-insensitively on Windows. `None` for any other shape.
    pub fn config_dir_from_transcript(&self, transcript_path: &str) -> Option<String> {
        let components = self.components(transcript_path);
        if components.len() < 4 {
            return None;
        }
        let is_projects = |c: &str| {
            if self.style.case_insensitive() {
                c.eq_ignore_ascii_case("projects")
            } else {
                c == "projects"
            }
        };
        (1..=components.len() - 3)
            .rev()
            .find(|&index| is_projects(&components[index]))
            .map(|index| self.join_components(&components[..index]))
    }

    /// The account's identity file, as Claude Code resolves it:
    /// `join(CLAUDE_CONFIG_DIR || homedir, ".claude.json")`. Set to anything,
    /// even `~\.claude`, the folder's own file is read; unset, the default
    /// folder's is `~\.claude.json`.
    pub fn global_config_file(&self, config_dir: &str, config_dir_env: Option<&str>) -> String {
        if let Some(env) = config_dir_env.filter(|env| !env.is_empty()) {
            return self.join(env, ".claude.json");
        }
        if self.is_default_config_dir(config_dir) {
            return self.default_identity_file();
        }
        self.join(config_dir, ".claude.json")
    }

    /// A session's config folder: its transcript path decides (unless that
    /// runs through shared-history infrastructure), then the raw
    /// `CLAUDE_CONFIG_DIR`, then `~\.claude` (SessionFilter.configDir).
    pub fn session_config_dir(
        &self,
        transcript_path: Option<&str>,
        config_dir_env: Option<&str>,
        is_infrastructure: impl Fn(&str) -> bool,
    ) -> String {
        if let Some(dir) = transcript_path.and_then(|t| self.config_dir_from_transcript(t)) {
            if !is_infrastructure(&dir) {
                return dir;
            }
        }
        if let Some(env) = config_dir_env.filter(|env| !env.is_empty()) {
            return self.normalize(env);
        }
        self.default_config_dir()
    }

    /// A list of folders (`AGENTNOTCH_EXTRA_CONFIG_DIRS`): split on `;`
    /// (Windows) or `:` (POSIX), trimmed, `~` expanded, normalized, empties
    /// dropped.
    pub fn split_list(&self, value: &str) -> Vec<String> {
        value
            .split(self.style.list_separator())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| self.normalize(item))
            .filter(|item| !item.is_empty())
            .collect()
    }

    /// Where Claude Code keeps a session's transcript, by its rule (the
    /// hook's `transcript_path` is authoritative; a slug over 200 characters
    /// carries a hash suffix this can't reproduce).
    pub fn expected_transcript_path(
        &self,
        config_dir: &str,
        cwd: &str,
        session_id: &str,
    ) -> String {
        let projects = self.join(config_dir, "projects");
        self.join(
            &self.join(&projects, &project_slug(cwd)),
            &format!("{session_id}.jsonl"),
        )
    }
}

/// Claude Code's project folder name for a working folder: every UTF-16
/// unit outside `[A-Za-z0-9]` becomes `-` (JavaScript's replace), so
/// `C:\Users\me\proj` is `C--Users-me-proj`.
pub fn project_slug(cwd: &str) -> String {
    let mut slug = String::with_capacity(cwd.len());
    for ch in cwd.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else {
            for _ in 0..ch.len_utf16() {
                slug.push('-');
            }
        }
    }
    slug
}
