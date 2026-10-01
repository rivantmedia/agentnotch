//! One Claude Code config folder as the registry keeps it (the Mac's
//! `ClaudeAccount`, AU§2.1): where it is, how sessions spell
//! `CLAUDE_CONFIG_DIR` for it, who is signed in there, and what the user
//! chose for it. An *account* is the identity signed in to one or more of
//! these (`IdentityAccount`).

use crate::core::paths::{PathStyle, Paths};
use crate::model::{AccountId, FolderKind, FolderSource, Identity, RunFolder};
use std::path::PathBuf;
use std::time::SystemTime;

/// Colours in the account palette; colour indices cycle through these.
pub const PALETTE_SIZE: i64 = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Folder {
    /// The normalized folder in display case; also its path.
    pub id: AccountId,
    /// `CLAUDE_CONFIG_DIR` as sessions of this folder run with it, kept
    /// verbatim; `None` for the default folder used without it.
    pub config_dir_env: Option<String>,
    pub custom_label: Option<String>,
    /// Every `CLAUDE_CONFIG_DIR` spelling sessions of this folder were seen
    /// with, oldest first; `""` stands for "unset". Spellings of one folder
    /// are one login on Windows (the login is a file inside the folder, found
    /// through a case-insensitive path), so they are kept once per
    /// `Paths::key`: only the default folder can hold two (unset, and set to
    /// itself), whose identity files differ.
    pub seen_config_dir_envs: Vec<String>,
    /// From the folder's identity file (`oauthAccount`); `None` when nobody
    /// is signed in there.
    pub identity: Option<Identity>,
    /// `max`, `pro`, `team`, `enterprise`: from the organization type, or
    /// from `get_usage` when the file carries none.
    pub subscription_type: Option<String>,
    /// Index into the palette; assigned once and kept stable (it may exceed
    /// the palette, which cycles).
    pub color_index: i64,
    pub source: FolderSource,
    /// Last time a hook event or status line came from this folder.
    pub last_seen_at: Option<SystemTime>,
    /// "Track sessions and hooks" off. For a folder someone is signed in
    /// to, its identity's choice (the registry copies it in).
    pub is_hidden: bool,
    /// What the folder is for, from the latest classification.
    pub kind: FolderKind,
    /// Its name and badge among every known folder (`naming::assign`).
    pub default_label: Option<String>,
    pub default_monogram: Option<String>,
}

impl Folder {
    /// A folder at `dir` (normalized here) with nothing known about it yet.
    pub fn new(paths: &Paths, dir: &str) -> Folder {
        Folder {
            id: AccountId::new(paths.normalize(dir)),
            config_dir_env: None,
            custom_label: None,
            seen_config_dir_envs: Vec::new(),
            identity: None,
            subscription_type: None,
            color_index: 0,
            source: FolderSource::Discovered,
            last_seen_at: None,
            is_hidden: false,
            kind: FolderKind::Run,
            default_label: None,
            default_monogram: None,
        }
    }

    /// The folder's path (its id).
    pub fn dir(&self) -> &str {
        self.id.as_str()
    }

    fn identity_field(&self, pick: impl Fn(&Identity) -> Option<&String>) -> Option<&str> {
        self.identity
            .as_ref()
            .and_then(pick)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    pub fn email(&self) -> Option<&str> {
        self.identity_field(|i| i.email.as_ref())
    }

    pub fn account_uuid(&self) -> Option<&str> {
        self.identity_field(|i| i.account_uuid.as_ref())
    }

    pub fn display_name(&self) -> Option<&str> {
        self.identity_field(|i| i.display_name.as_ref())
    }

    pub fn organization_name(&self) -> Option<&str> {
        self.identity_field(|i| i.organization_name.as_ref())
    }

    pub fn organization_uuid(&self) -> Option<&str> {
        self.identity_field(|i| i.organization_uuid.as_ref())
    }

    pub fn rate_limit_tier(&self) -> Option<&str> {
        self.identity_field(|i| i.rate_limit_tier.as_ref())
    }

    /// Signed in to claude.ai, per its identity file.
    pub fn is_signed_in(&self) -> bool {
        self.email().is_some() || self.account_uuid().is_some()
    }

    /// `~/.claude` used without `CLAUDE_CONFIG_DIR`.
    pub fn is_default(&self, paths: &Paths) -> bool {
        self.config_dir_env.as_deref().unwrap_or("").is_empty()
            && paths.is_default_config_dir(self.dir())
    }

    /// Where its identity file is, as Claude Code resolves it.
    pub fn global_config_file(&self, paths: &Paths) -> String {
        paths.global_config_file(self.dir(), self.config_dir_env.as_deref())
    }

    /// The name shown everywhere: the custom name, else the default one.
    pub fn label(&self, paths: &Paths) -> String {
        match self.custom_label.as_deref().filter(|l| !l.is_empty()) {
            Some(custom) => custom.to_owned(),
            None => self
                .default_label
                .clone()
                .unwrap_or_else(|| super::naming::base_label(self, paths)),
        }
    }

    /// Two letters for compact badges, told apart from every other folder's
    /// by the registry; outside it, the first choice.
    pub fn monogram(&self, paths: &Paths) -> String {
        self.default_monogram.clone().unwrap_or_else(|| {
            super::naming::monogram_candidates(self, paths)
                .into_iter()
                .next()
                .unwrap_or_else(|| "CC".to_owned())
        })
    }

    /// "Max 20x", "Pro", …
    pub fn plan_name(&self) -> Option<String> {
        plan_name(self.rate_limit_tier(), self.subscription_type.as_deref())
    }

    /// The line that starts Claude Code in this folder (§4.2): `claude` for
    /// the default folder, else one that sets `CLAUDE_CONFIG_DIR` first.
    pub fn launch_command(&self, paths: &Paths) -> String {
        if self.is_default(paths) {
            return "claude".to_owned();
        }
        let dir = self
            .config_dir_env
            .as_deref()
            .filter(|env| !env.is_empty())
            .unwrap_or(self.dir());
        launch_command_for(paths.style(), dir)
    }

    /// Applies what its identity file says (`None` = signed out): every
    /// identity field follows the file; the plan too, unless the file names
    /// none while someone is signed in (then `get_usage`'s stays).
    pub fn apply_identity(&mut self, identity: Option<Identity>) {
        match identity.as_ref().and_then(Identity::subscription_type) {
            Some(plan) => self.subscription_type = Some(plan),
            None if identity.is_none() => self.subscription_type = None,
            None => {}
        }
        self.identity = identity;
    }

    /// The palette slot its colour index falls in.
    pub fn palette_index(&self) -> u8 {
        palette_slot(self.color_index)
    }

    /// The model's view of it.
    pub fn to_run_folder(&self) -> RunFolder {
        RunFolder {
            id: self.id.clone(),
            config_dir: PathBuf::from(self.dir()),
            config_dir_env: self.config_dir_env.clone(),
            custom_label: self.custom_label.clone(),
            seen_config_dir_envs: self.seen_config_dir_envs.clone(),
            identity: self.identity.clone(),
            subscription_type: self.subscription_type.clone(),
            color_index: self.palette_index(),
            source: self.source,
            last_seen_at: self.last_seen_at,
            is_hidden: self.is_hidden,
            kind: self.kind,
        }
    }
}

/// A colour index in the palette's range.
pub fn palette_slot(color_index: i64) -> u8 {
    // rem_euclid of 8 is always 0..8.
    color_index.rem_euclid(PALETTE_SIZE) as u8
}

/// "Max 20x", "Max 5x" from the rate-limit tier, else the plan capitalised.
pub fn plan_name(rate_limit_tier: Option<&str>, subscription_type: Option<&str>) -> Option<String> {
    if let Some(tier) = rate_limit_tier.map(str::to_lowercase) {
        if tier.contains("max_20x") {
            return Some("Max 20x".to_owned());
        }
        if tier.contains("max_5x") {
            return Some("Max 5x".to_owned());
        }
    }
    match subscription_type.map(str::to_lowercase).as_deref() {
        Some("max") => Some("Max".to_owned()),
        Some("pro") => Some("Pro".to_owned()),
        Some("team") => Some("Team".to_owned()),
        Some("enterprise") => Some("Enterprise".to_owned()),
        Some(other) if !other.is_empty() => Some(capitalized(other)),
        _ => None,
    }
}

/// Each word's first letter upper-cased and the rest lower-cased (Swift's
/// `capitalized`).
fn capitalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_word_start = true;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if at_word_start {
                out.extend(ch.to_uppercase());
            } else {
                out.extend(ch.to_lowercase());
            }
            at_word_start = false;
        } else {
            out.push(ch);
            at_word_start = true;
        }
    }
    out
}

/// The line that starts Claude Code with `CLAUDE_CONFIG_DIR` set to `dir`,
/// for the shell the user pastes it into: PowerShell on Windows
/// (`$env:CLAUDE_CONFIG_DIR='C:\Users\me\.claude-work'; claude`), a POSIX
/// shell elsewhere (`CLAUDE_CONFIG_DIR='/Users/me/.claude-work' claude`).
/// The folder is single-quoted, so nothing in its name is expanded.
pub fn launch_command_for(style: PathStyle, dir: &str) -> String {
    match style {
        PathStyle::Windows => format!("$env:CLAUDE_CONFIG_DIR={}; claude", powershell_quote(dir)),
        PathStyle::Posix => format!("CLAUDE_CONFIG_DIR={} claude", posix_quote(dir)),
    }
}

/// A PowerShell single-quoted string. PowerShell takes the typographic
/// single quotes (‘ ’ ‚ ‛) for `'` too, so each of them is doubled as well:
/// a folder named `Paul’s` must not end the string early.
pub fn powershell_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for ch in text.chars() {
        if matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(ch);
        }
        out.push(ch);
    }
    out.push('\'');
    out
}

/// A POSIX single-quoted word: `'` becomes `'\''`.
pub fn posix_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
