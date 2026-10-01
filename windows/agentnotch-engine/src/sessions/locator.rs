//! Finds session and subagent transcript files (TranscriptLocator.swift).
//! The hook's `transcript_path` is authoritative; everything here is the
//! fallback for sessions discovered without one (registry, status line,
//! older hook scripts).
//!
//! Claude Code stores a session at `<config dir>\projects\<slug>\<id>.jsonl`;
//! the slug is the working folder with every UTF-16 unit outside
//! `[A-Za-z0-9]` replaced by `-` (`C:\Users\me\proj` is `C--Users-me-proj`),
//! and a slug over 200 characters is truncated and suffixed with a hash the
//! engine cannot reproduce: the search by session id finds those.
//!
//! Existence is asked of [`SecureFiles::identity`] and links are resolved by
//! [`SecureFiles::canonical`], so tests can stand a fake in for links that a
//! Windows runner may not create.

use crate::core::paths::Paths;
use crate::platform::SecureFiles;
use std::path::Path;

pub struct TranscriptLocator<'a> {
    pub paths: &'a Paths,
    pub files: &'a dyn SecureFiles,
}

impl<'a> TranscriptLocator<'a> {
    pub fn new(paths: &'a Paths, files: &'a dyn SecureFiles) -> Self {
        TranscriptLocator { paths, files }
    }

    fn exists(&self, path: &str) -> bool {
        self.files.identity(Path::new(path)).is_ok()
    }

    /// `<config dir>\projects\<slug>\<id>.jsonl` as Claude Code would name
    /// it, without checking that it exists. A long slug can't be reproduced
    /// exactly, so prefer [`Self::transcript_path`].
    pub fn expected_transcript_path(
        &self,
        session_id: &str,
        cwd: &str,
        config_dir: &str,
    ) -> String {
        self.paths
            .expected_transcript_path(config_dir, cwd, session_id)
    }

    /// An existing transcript: the hook-provided path if it exists, else the
    /// computed slug path, else a search of `<config dir>\projects\*\<id>.jsonl`.
    pub fn transcript_path(
        &self,
        session_id: &str,
        cwd: Option<&str>,
        config_dir: &str,
        hint: Option<&str>,
    ) -> Option<String> {
        if let Some(hint) = hint.filter(|hint| !hint.is_empty()) {
            if self.exists(hint) {
                return Some(hint.to_owned());
            }
        }
        if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) {
            let expected = self.expected_transcript_path(session_id, cwd, config_dir);
            if self.exists(&expected) {
                return Some(expected);
            }
        }
        self.search_transcript(session_id, config_dir)
    }

    /// A transcript in any of `config_dirs` (the session's own first),
    /// reading each physical `projects` folder once however many config
    /// folders link to it. The path is spelt through the first config folder
    /// that has it, never the link target: the config folder is read back
    /// from it (`Paths::config_dir_from_transcript`).
    pub fn transcript_path_in(
        &self,
        session_id: &str,
        cwd: Option<&str>,
        config_dirs: &[String],
        hint: Option<&str>,
    ) -> Option<String> {
        let mut seen_projects: Vec<String> = Vec::new();
        for config_dir in config_dirs {
            let projects = self.real_path(&self.paths.join(config_dir, "projects"));
            let key = self.paths.key(&projects);
            if seen_projects.contains(&key) {
                continue;
            }
            seen_projects.push(key);
            // The hint names one file: it is tried with the first folder only.
            let hint = if seen_projects.len() == 1 { hint } else { None };
            if let Some(found) = self.transcript_path(session_id, cwd, config_dir, hint) {
                return Some(found);
            }
        }
        None
    }

    /// Scans every project folder of the account for `<id>.jsonl`.
    pub fn search_transcript(&self, session_id: &str, config_dir: &str) -> Option<String> {
        if !is_safe_file_name(session_id) {
            return None;
        }
        let projects = self.paths.join(config_dir, "projects");
        let mut folders: Vec<String> = std::fs::read_dir(&projects)
            .ok()?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        folders.sort();
        let file_name = format!("{session_id}.jsonl");
        folders
            .iter()
            .map(|folder| {
                self.paths
                    .join(&self.paths.join(&projects, folder), &file_name)
            })
            .find(|candidate| self.exists(candidate))
    }

    /// The file a path names, every link resolved: two config folders that
    /// share their history (Claude Parallel Profiles links `projects\` to
    /// `~\.claude-shared`) name one transcript two ways. A path that can't be
    /// resolved (it does not exist) stays as normalized.
    pub fn real_path(&self, path: &str) -> String {
        match self.files.canonical(Path::new(&self.paths.normalize(path))) {
            Ok(resolved) => self.paths.normalize(&resolved.to_string_lossy()),
            Err(_) => self.paths.normalize(path),
        }
    }

    /// Whether two paths name the same file (spelt alike, or through links).
    pub fn is_same_file(&self, a: &str, b: &str) -> bool {
        self.paths.same(a, b) || self.paths.same(&self.real_path(a), &self.real_path(b))
    }

    /// The transcript of a subagent of the session whose main transcript is
    /// `transcript_path`. Current Claude Code nests them under the session:
    /// `<project>\<id>\subagents\agent-<agent>.jsonl`; workflow agents one
    /// level deeper (`subagents\workflows\<wid>\agent-<agent>.jsonl`); older
    /// versions stored them flat: `<project>\agent-<agent>.jsonl`. The nested
    /// path when nothing exists yet (the file may still be created).
    pub fn subagent_transcript_path(&self, transcript_path: &str, agent_id: &str) -> String {
        let project_dir = self.paths.parent(transcript_path).unwrap_or_default();
        let file = self.paths.file_name(transcript_path).unwrap_or_default();
        let session_id = file.strip_suffix(".jsonl").unwrap_or(&file);
        let file_name = format!("agent-{agent_id}.jsonl");
        let subagents_dir = self
            .paths
            .join(&self.paths.join(&project_dir, session_id), "subagents");
        let nested = self.paths.join(&subagents_dir, &file_name);
        if self.exists(&nested) {
            return nested;
        }
        let flat = self.paths.join(&project_dir, &file_name);
        if self.exists(&flat) {
            return flat;
        }
        if is_safe_file_name(agent_id) {
            if let Some(found) = find_file(Path::new(&subagents_dir), &file_name, 6) {
                return self.paths.normalize(&found);
            }
        }
        nested
    }
}

/// The first file called `name` under `dir` (sorted, links not followed).
fn find_file(dir: &Path, name: &str, depth: usize) -> Option<String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in &entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_file() && entry.file_name() == name {
            return Some(entry.path().to_string_lossy().into_owned());
        }
    }
    if depth == 0 {
        return None;
    }
    for entry in &entries {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if let Some(found) = find_file(&entry.path(), name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// An agent id that may name a transcript, `agent-<id>.jsonl`: letters,
/// digits, `-` and `_` (as Claude Code writes them), at most 128. The id
/// comes from a transcript and becomes part of a path, where on Windows a
/// `..` collapses before any folder is looked at and a `:` names a stream
/// of another file.
pub fn is_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Session and agent ids come from other processes; never let one escape a
/// folder.
fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', '\0']) && name != "." && name != ".."
}
