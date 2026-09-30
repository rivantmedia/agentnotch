//! An account is a signed-in identity, not a folder (AccountIdentities.swift,
//! AU§5). The folders the registry knows are grouped by who is signed in to
//! them: `oauthAccount.accountUuid`, else the lower-cased email. One group,
//! one ring, one row, one usage reading. Pure.
//!
//! `~/.claude` needs one correction where Claude Parallel Profiles mirrors
//! accounts into it (never on native Windows, AU§0.2, but the rule also
//! covers hand-copied folders): the extension rewrites `oauthAccount`'s
//! email, display name and organization name but keeps its `accountUuid`, so
//! after a switch the file can name one person's email with another
//! person's UUID. When a folder's UUID is known elsewhere under a different
//! email, and its email is known elsewhere under a different UUID, the email
//! wins: the folder joins the identity that email belongs to. The mirrored
//! default joins the email's owner as soon as no other folder pairs its UUID
//! with its email. Choices saved for `~/.claude` go to its owner (the
//! identity its own `accountUuid` names), not to whoever was mirrored in.
//!
//! One login in two organizations (a personal plan and a Team seat) shares
//! one `accountUuid` but has two quotas: when folders the correction didn't
//! touch name one UUID with two `organizationUuid`s, each organization is
//! its own identity (`uuid:<account>/<organization>`).

use super::classify::is_window_dir;
use super::folder::{palette_slot, plan_name, Folder, PALETTE_SIZE};
use super::naming;
use crate::core::paths::Paths;
use crate::model::{Account, AccountId, FolderKind, FolderSource, Identity, IdentityId, RingId};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

pub const UUID_PREFIX: &str = IdentityId::UUID_PREFIX;
pub const EMAIL_PREFIX: &str = IdentityId::EMAIL_PREFIX;
pub const DIR_PREFIX: &str = IdentityId::DIR_PREFIX;
/// Between an account's UUID and its organization in a split key.
pub const ORGANIZATION_SEPARATOR: &str = "/";
/// The ring of `~\.claude` when nobody is signed in anywhere.
pub const DEFAULT_RING_ID: &str = "claude";

/// What the user chose for an identity (its name, colour, tracking, whether
/// its ring is in the notch), kept in `accounts.json` by identity so every
/// folder of it follows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityPrefs {
    pub custom_label: Option<String>,
    pub color_index: i64,
    pub is_hidden: bool,
    /// "Ring in notch" off (a Windows addition; the Mac keeps it in
    /// upstream's ring list).
    pub ring_hidden: bool,
}

/// One Claude account: everything signed in as one identity (the Mac's
/// `ClaudeIdentityAccount`).
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityAccount {
    /// `uuid:<accountUuid>[/<organizationUuid>]`, `email:<address>`, or
    /// `dir:<folder>` for a folder added by hand that nobody signed in to.
    pub id: IdentityId,
    /// Stable whatever folders come and go.
    pub ring_id: RingId,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub organization_name: Option<String>,
    pub organization_uuid: Option<String>,
    pub account_uuid: Option<String>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    /// Folders Claude Code runs in as this identity: `~\.claude` first, then
    /// VS Code windows, then other folders.
    pub run_dirs: Vec<Folder>,
    /// Claude Parallel Profiles stores holding it (never written).
    pub store_dirs: Vec<Folder>,
    pub custom_label: Option<String>,
    pub color_index: i64,
    pub is_hidden: bool,
    pub ring_hidden: bool,
    /// `~\.claude` (used without `CLAUDE_CONFIG_DIR`) runs as it now.
    pub includes_default: bool,
    /// The organization this identity is, when one login has several.
    pub organization_scope: Option<String>,
    /// Its run folders that are VS Code windows' working copies.
    pub window_dir_ids: Vec<AccountId>,
    /// Told apart from every other identity's (`naming`).
    pub default_label: Option<String>,
    pub default_monogram: Option<String>,
}

impl IdentityAccount {
    pub fn folders(&self) -> impl Iterator<Item = &Folder> {
        self.run_dirs.iter().chain(&self.store_dirs)
    }

    pub fn folder_ids(&self) -> Vec<AccountId> {
        self.folders().map(|f| f.id.clone()).collect()
    }

    pub fn is_signed_in(&self) -> bool {
        self.email.is_some() || self.account_uuid.is_some()
    }

    /// A folder added by hand that nobody signed in to: its ring says how to
    /// sign in.
    pub fn is_standalone_unsigned(&self) -> bool {
        self.id.as_str().starts_with(DIR_PREFIX)
    }

    /// The folder it is shown and launched from: a run folder when it has
    /// one (the default first), else its first store.
    pub fn primary_dir(&self) -> Option<&Folder> {
        self.run_dirs.first().or_else(|| self.store_dirs.first())
    }

    pub fn last_seen_at(&self) -> Option<SystemTime> {
        self.folders().filter_map(|f| f.last_seen_at).max()
    }

    /// The name shown everywhere: the custom name, else the default one.
    pub fn label(&self, paths: &Paths) -> String {
        match self.custom_label.as_deref().filter(|l| !l.is_empty()) {
            Some(custom) => custom.to_owned(),
            None => self
                .default_label
                .clone()
                .unwrap_or_else(|| self.representative(paths).label(paths)),
        }
    }

    /// The name without the user's own.
    pub fn default_name(&self, paths: &Paths) -> String {
        self.default_label
            .clone()
            .unwrap_or_else(|| naming::base_label(&self.representative(paths), paths))
    }

    pub fn monogram(&self, paths: &Paths) -> String {
        self.default_monogram
            .clone()
            .unwrap_or_else(|| self.representative(paths).monogram(paths))
    }

    pub fn plan_name(&self) -> Option<String> {
        plan_name(
            self.rate_limit_tier.as_deref(),
            self.subscription_type.as_deref(),
        )
    }

    /// How to start Claude Code as this identity from a terminal, when there
    /// is a way: `claude` while `~\.claude` runs as it, else a standalone run
    /// folder's line. `None` when it runs only in VS Code windows or only a
    /// store holds it: a window's working copy belongs to that window, and
    /// Claude Code is never run in a store.
    pub fn terminal_launch_command(&self, paths: &Paths) -> Option<String> {
        if self.includes_default {
            return Some("claude".to_owned());
        }
        self.run_dirs
            .iter()
            .find(|folder| {
                folder.kind == FolderKind::Run
                    && folder
                        .config_dir_env
                        .as_deref()
                        .is_some_and(|env| !env.is_empty())
                    && !self.window_dir_ids.contains(&folder.id)
            })
            .map(|folder| folder.launch_command(paths))
    }

    /// `terminal_launch_command`, else plain `claude`.
    pub fn launch_command(&self, paths: &Paths) -> String {
        self.terminal_launch_command(paths)
            .unwrap_or_else(|| "claude".to_owned())
    }

    /// Any but the one `~\.claude` runs as on its own can be forgotten
    /// (forgetting that would forget every terminal session); one the
    /// extension keeps in a store or a window can, whoever `~\.claude` holds.
    pub fn can_be_forgotten(&self) -> bool {
        !self.includes_default || !self.store_dirs.is_empty() || !self.window_dir_ids.is_empty()
    }

    /// A folder-shaped stand-in (the primary folder with this identity's
    /// fields), for rules written for folders: names, plans.
    pub fn representative(&self, paths: &Paths) -> Folder {
        let mut folder = self
            .primary_dir()
            .cloned()
            .unwrap_or_else(|| Folder::new(paths, &paths.default_config_dir()));
        let identity = Identity {
            account_uuid: self.account_uuid.clone(),
            email: self.email.clone(),
            display_name: self.display_name.clone(),
            organization_name: self.organization_name.clone(),
            organization_uuid: self.organization_uuid.clone(),
            rate_limit_tier: self.rate_limit_tier.clone(),
            ..Identity::default()
        };
        folder.identity = (identity != Identity::default()).then_some(identity);
        folder.subscription_type = self.subscription_type.clone();
        folder.custom_label = self.custom_label.clone();
        folder.color_index = self.color_index;
        folder.is_hidden = self.is_hidden;
        folder.default_label = None;
        folder.default_monogram = None;
        folder
    }

    /// The model's view of it.
    pub fn to_account(&self, paths: &Paths) -> Account {
        Account {
            identity_id: self.id.clone(),
            ring_id: self.ring_id.clone(),
            label: self.label(paths),
            // The name the account has without any nickname the hub adds: the
            // user's own name, else the engine's default (GUX-4, CS-4).
            own_label: Some(self.label(paths)),
            monogram: self.monogram(paths),
            color_index: palette_slot(self.color_index),
            email: self.email.clone(),
            plan_name: self.plan_name(),
            organization_uuid: self.organization_uuid.clone(),
            run_dirs: self.run_dirs.iter().map(|f| f.id.clone()).collect(),
            store_dirs: self.store_dirs.iter().map(|f| f.id.clone()).collect(),
            includes_default: self.includes_default,
            is_tracked: !self.is_hidden,
            // An untracked account has no ring, whatever was chosen for it
            // (the choice is kept for when it is tracked again).
            ring_shown: !self.is_hidden && !self.ring_hidden,
            is_signed_in: self.is_signed_in(),
            launch_command: self.terminal_launch_command(paths),
            can_forget: self.can_be_forgotten(),
        }
    }
}

/// A space or a tab, of any width: Foundation's `.whitespaces`, which has no
/// line breaks in it.
pub(crate) fn is_inline_space(c: char) -> bool {
    c.is_whitespace()
        && !matches!(
            c,
            '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        )
}

/// Spaces and tabs trimmed off both ends.
pub(crate) fn trim_spaces(text: &str) -> &str {
    text.trim_matches(is_inline_space)
}

fn clean(value: Option<&str>) -> Option<String> {
    let value = trim_spaces(value?).to_lowercase();
    (!value.is_empty()).then_some(value)
}

/// A folder's own key, before any correction: its UUID, else its email.
pub fn base_key(account_uuid: Option<&str>, email: Option<&str>) -> Option<String> {
    if let Some(uuid) = clean(account_uuid) {
        return Some(format!("{UUID_PREFIX}{uuid}"));
    }
    clean(email).map(|email| format!("{EMAIL_PREFIX}{email}"))
}

/// How a folder's identity key was settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub key: String,
    /// The folder's own UUID belongs to someone else (a mirrored
    /// `.claude.json`); its other fields may be stale too.
    pub corrected: bool,
}

/// The identity key of every signed-in folder, by folder id (see the module
/// comment for the mirror correction and organizations). Folders nobody is
/// signed in to have none. `mirrored_default` is `~\.claude`'s id when
/// Claude Parallel Profiles mirrors accounts into it.
pub fn identity_keys(
    folders: &[Folder],
    mirrored_default: Option<&str>,
) -> BTreeMap<String, Resolution> {
    struct Login {
        folder: String,
        uuid: Option<String>,
        email: Option<String>,
        organization: Option<String>,
    }
    let logins: Vec<Login> = folders
        .iter()
        .filter_map(|folder| {
            let uuid = clean(folder.account_uuid());
            let email = clean(folder.email());
            (uuid.is_some() || email.is_some()).then(|| Login {
                folder: folder.dir().to_owned(),
                uuid,
                email,
                organization: clean(folder.organization_uuid()),
            })
        })
        .collect();
    let mut result: BTreeMap<String, Resolution> = BTreeMap::new();
    for login in &logins {
        let others: Vec<&Login> = logins.iter().filter(|o| o.folder != login.folder).collect();
        // UUIDs the same email has in other folders, most common first.
        let uuids_for_email = |email: &str| -> Vec<String> {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for other in others.iter().filter(|o| o.email.as_deref() == Some(email)) {
                if let Some(uuid) = other.uuid.as_deref() {
                    *counts.entry(uuid).or_default() += 1;
                }
            }
            let mut sorted: Vec<(&str, usize)> = counts.into_iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            sorted
                .into_iter()
                .map(|(uuid, _)| uuid.to_owned())
                .collect()
        };
        let resolution = match (&login.uuid, &login.email) {
            (Some(uuid), Some(email)) => {
                let uuid_elsewhere_as_others = others.iter().any(|o| {
                    o.uuid.as_ref() == Some(uuid) && o.email.as_ref().is_some_and(|e| e != email)
                });
                let email_elsewhere: Vec<String> = uuids_for_email(email)
                    .into_iter()
                    .filter(|other| other != uuid)
                    .collect();
                // The folder the extension mirrors into: its UUID is stale as
                // soon as nothing else pairs it with this email.
                let stale_mirror = mirrored_default == Some(login.folder.as_str())
                    && !others
                        .iter()
                        .any(|o| o.uuid.as_ref() == Some(uuid) && o.email.as_ref() == Some(email));
                match email_elsewhere.first() {
                    Some(owner) if uuid_elsewhere_as_others || stale_mirror => Resolution {
                        key: format!("{UUID_PREFIX}{owner}"),
                        corrected: true,
                    },
                    _ => Resolution {
                        key: format!("{UUID_PREFIX}{uuid}"),
                        corrected: false,
                    },
                }
            }
            (Some(uuid), None) => Resolution {
                key: format!("{UUID_PREFIX}{uuid}"),
                corrected: false,
            },
            // An email-only login joins the UUID that email has elsewhere.
            (None, Some(email)) => match uuids_for_email(email).first() {
                Some(owner) => Resolution {
                    key: format!("{UUID_PREFIX}{owner}"),
                    corrected: false,
                },
                None => Resolution {
                    key: format!("{EMAIL_PREFIX}{email}"),
                    corrected: false,
                },
            },
            (None, None) => continue,
        };
        result.insert(login.folder.clone(), resolution);
    }

    // One UUID in several organizations: one identity per organization. Only
    // folders the correction left alone count (a mirrored file keeps a stale
    // organization too); the rest join the most common one.
    let organizations: HashMap<&str, Option<&str>> = logins
        .iter()
        .map(|l| (l.folder.as_str(), l.organization.as_deref()))
        .collect();
    let mut by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (folder, resolution) in &result {
        by_key
            .entry(resolution.key.clone())
            .or_default()
            .push(folder.clone());
    }
    for (key, members) in by_key {
        if !key.starts_with(UUID_PREFIX) {
            continue;
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for member in &members {
            if result.get(member).is_some_and(|r| !r.corrected) {
                if let Some(Some(org)) = organizations.get(member.as_str()) {
                    *counts.entry(org).or_default() += 1;
                }
            }
        }
        if counts.len() <= 1 {
            continue;
        }
        let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let common = ranked[0].0.to_owned();
        for member in &members {
            let Some(resolution) = result.get(member).cloned() else {
                continue;
            };
            let own = organizations.get(member.as_str()).copied().flatten();
            let organization = if resolution.corrected {
                common.clone()
            } else {
                own.map(str::to_owned).unwrap_or_else(|| common.clone())
            };
            result.insert(
                member.clone(),
                Resolution {
                    key: format!("{key}{ORGANIZATION_SEPARATOR}{organization}"),
                    corrected: resolution.corrected,
                },
            );
        }
    }
    result
}

/// The account UUID of a `uuid:` key (without any organization).
pub fn account_uuid_of_key(key: &str) -> Option<String> {
    let value = key.strip_prefix(UUID_PREFIX)?;
    Some(
        value
            .split_once(ORGANIZATION_SEPARATOR)
            .map_or(value, |(account, _)| account)
            .to_owned(),
    )
}

/// The organization of a split `uuid:<account>/<organization>` key.
pub fn organization_of_key(key: &str) -> Option<String> {
    let value = key.strip_prefix(UUID_PREFIX)?;
    value
        .split_once(ORGANIZATION_SEPARATOR)
        .map(|(_, org)| org.to_owned())
}

/// Who `~\.claude` belongs to, whoever the extension mirrored into it: the
/// identity its own `accountUuid` names (the extension never rewrites
/// that), when that identity exists.
pub fn default_owner(default_folder: Option<&Folder>, keys: &[String]) -> Option<String> {
    let folder = default_folder?;
    let email = if folder
        .identity
        .as_ref()
        .and_then(|i| i.account_uuid.as_ref())
        .is_none()
    {
        folder.email()
    } else {
        None
    };
    let own = base_key(folder.account_uuid(), email)?;
    if keys.contains(&own) {
        return Some(own);
    }
    let prefix = format!("{own}{ORGANIZATION_SEPARATOR}");
    let split: Vec<&String> = keys.iter().filter(|k| k.starts_with(&prefix)).collect();
    if split.len() == 1 {
        return Some(split[0].clone());
    }
    let organization = folder.organization_uuid()?.to_lowercase();
    let wanted = format!("{prefix}{organization}");
    split.into_iter().find(|k| **k == wanted).cloned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `claude-acct-` and the first 12 hex digits of the SHA-256 of an account's
/// UUID (or email, when it has none), lower-cased: it never depends on which
/// folders exist.
pub fn ring_id_for_account_key(account_key: &str) -> RingId {
    let digest = Sha256::digest(account_key.to_lowercase().as_bytes());
    RingId::new(format!("claude-acct-{}", hex(&digest[..6])))
}

/// A folder's own ring id, which a `dir:` identity keeps:
/// `~\.claude` → `claude`, `~\.claude-<slug>` → `claude-<slug>` (upstream's
/// ids), anything else `claude-dir-<first 8 hex of sha256(key)>`. Worked out
/// on `Paths::key`, so every spelling of a Windows folder gets one ring.
pub fn ring_id_for_config_dir(paths: &Paths, config_dir: &str) -> RingId {
    let key = paths.key(config_dir);
    let home = paths.key(paths.home());
    if let (Some(parent), Some(name)) = (paths.parent(&key), paths.file_name(&key)) {
        if paths.key(&parent) == home {
            if name == ".claude" {
                return RingId::new(DEFAULT_RING_ID);
            }
            if let Some(slug) = name.strip_prefix(".claude-").filter(|s| !s.is_empty()) {
                return RingId::new(format!("claude-{slug}"));
            }
        }
    }
    let digest = Sha256::digest(key.as_bytes());
    RingId::new(format!("claude-dir-{}", hex(&digest[..4])))
}

/// An identity's ring id: from its UUID (or email) for a login, its folder's
/// for a `dir:` identity.
pub fn ring_id_for_identity(paths: &Paths, key: &str) -> RingId {
    if let Some(dir) = key.strip_prefix(DIR_PREFIX) {
        return ring_id_for_config_dir(paths, dir);
    }
    let value = key
        .strip_prefix(UUID_PREFIX)
        .or_else(|| key.strip_prefix(EMAIL_PREFIX))
        .unwrap_or(key);
    ring_id_for_account_key(value)
}

/// Whether `id` names a Claude ring (upstream's test: `claude` or a
/// `claude-` prefix).
pub fn is_claude_ring(id: &str) -> bool {
    id == DEFAULT_RING_ID || id.starts_with("claude-")
}

/// What grouping makes of the registry's folders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Grouping {
    /// One per identity, by label.
    pub identities: Vec<IdentityAccount>,
    /// Folder id → identity id, for every grouped folder.
    pub identity_of_folder: BTreeMap<String, String>,
    /// Run folders nobody is signed in to that have no ring.
    pub unsigned_folders: Vec<Folder>,
    /// Folders of identities the user forgot.
    pub forgotten_folders: Vec<Folder>,
    /// Prefs for every identity, including those derived just now.
    pub prefs: BTreeMap<String, IdentityPrefs>,
    /// Folders whose key was corrected.
    pub corrected_folders: BTreeSet<String>,
    /// Folder id → identity key for every signed-in folder, forgotten
    /// identities' included.
    pub folder_keys: BTreeMap<String, String>,
    /// The identity `~\.claude` belongs to by its own `accountUuid`.
    pub default_owner: Option<String>,
}

/// `~\.claude` first, then VS Code windows, then other run folders, then
/// stores; by path within each.
pub fn ordered_folders(folders: &[Folder], paths: &Paths) -> Vec<Folder> {
    let rank = |folder: &Folder| -> u8 {
        if folder.kind == FolderKind::Store {
            3
        } else if folder.is_default(paths) {
            0
        } else if is_window_dir(paths, folder.dir()) {
            1
        } else {
            2
        }
    };
    let mut sorted = folders.to_vec();
    sorted.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.id.cmp(&b.id)));
    sorted
}

fn trust_rank(folder: &Folder, keys: &BTreeMap<String, Resolution>, paths: &Paths) -> u8 {
    if keys.get(folder.dir()).is_some_and(|r| r.corrected) {
        9
    } else if folder.kind == FolderKind::Store {
        0
    } else if is_window_dir(paths, folder.dir()) {
        1
    } else if folder.is_default(paths) {
        3
    } else {
        2
    }
}

/// The first palette slot nobody uses; once all are taken, the least used
/// one (lowest on ties), so colours stay as distinct as possible.
pub fn next_color_index(used: &[i64]) -> i64 {
    let mut counts = [0usize; PALETTE_SIZE as usize];
    for index in used {
        counts[palette_slot(*index) as usize] += 1;
    }
    let minimum = counts.iter().copied().min().unwrap_or(0);
    counts.iter().position(|c| *c == minimum).unwrap_or(0) as i64
}

/// An identity's prefs from its folders' old per-folder choices: the first
/// folder's (default first); its colour unless another identity has it,
/// else the least used one.
pub fn derived_prefs(ordered: &[&Folder], taken: &[i64]) -> IdentityPrefs {
    let first = ordered.first();
    let label = ordered
        .iter()
        .filter_map(|f| f.custom_label.clone())
        .find(|l| !l.is_empty());
    let mut color = first.map_or_else(|| next_color_index(taken), |f| f.color_index);
    if taken
        .iter()
        .any(|t| palette_slot(*t) == palette_slot(color))
    {
        color = next_color_index(taken);
    }
    IdentityPrefs {
        custom_label: label,
        color_index: color,
        is_hidden: first.is_some_and(|f| f.is_hidden),
        ring_hidden: false,
    }
}

/// Groups `folders` (infrastructure left out) into identities. `prefs` holds
/// what the user chose per identity; an identity without an entry takes its
/// first folder's (the default folder first, then by path), the way
/// `accounts.json` from before identities recorded them per folder.
/// `saved` are the folders whose choices were saved (asked first when an
/// identity's prefs are derived); `mirrors_default` says Claude Parallel
/// Profiles mirrors accounts into `~\.claude`.
pub fn group(
    folders: &[Folder],
    prefs: &BTreeMap<String, IdentityPrefs>,
    forgotten: &BTreeSet<String>,
    saved: &BTreeSet<String>,
    mirrors_default: bool,
    paths: &Paths,
) -> Grouping {
    let usable: Vec<&Folder> = folders
        .iter()
        .filter(|f| f.kind != FolderKind::Infrastructure)
        .collect();
    let usable_owned: Vec<Folder> = usable.iter().map(|f| (*f).clone()).collect();
    let default_folder = usable.iter().copied().find(|f| f.is_default(paths));
    let keys = identity_keys(
        &usable_owned,
        if mirrors_default {
            default_folder.map(Folder::dir)
        } else {
            None
        },
    );
    let signed_in_exists = keys.values().any(|r| !forgotten.contains(&r.key));

    let mut buckets: BTreeMap<String, Vec<&Folder>> = BTreeMap::new();
    let mut unsigned: Vec<Folder> = Vec::new();
    let mut forgotten_folders: Vec<Folder> = Vec::new();
    for folder in &usable {
        if let Some(resolution) = keys.get(folder.dir()) {
            if forgotten.contains(&resolution.key) {
                forgotten_folders.push((*folder).clone());
            } else {
                buckets
                    .entry(resolution.key.clone())
                    .or_default()
                    .push(folder);
            }
            continue;
        }
        // Nobody signed in. Only a run folder can have a ring then: one
        // added by hand ("Run … then /login"), and `~\.claude` while no
        // identity exists anywhere.
        if folder.kind != FolderKind::Run {
            continue;
        }
        let is_window = is_window_dir(paths, folder.dir());
        let is_default = folder.is_default(paths);
        let by_hand = folder.source == FolderSource::Manual && !is_window && !is_default;
        let lonely_default = is_default && !signed_in_exists;
        let key = format!("{DIR_PREFIX}{}", folder.dir());
        if (by_hand || lonely_default) && !forgotten.contains(&key) {
            buckets.entry(key).or_default().push(folder);
        } else {
            unsigned.push((*folder).clone());
        }
    }

    // `~\.claude`'s own choices (saved per folder, before identities) are its
    // owner's, not those of whoever was mirrored in.
    let bucket_keys: Vec<String> = buckets.keys().cloned().collect();
    let owner = default_owner(default_folder, &bucket_keys);
    let default_is_mirrored =
        default_folder.is_some_and(|d| keys.get(d.dir()).is_some_and(|r| r.corrected));
    let prefs_sources = |key: &str, ordered: &[Folder]| -> Vec<Folder> {
        match default_folder {
            Some(default) if default_is_mirrored => {
                let mut sources: Vec<Folder> = ordered
                    .iter()
                    .filter(|f| f.id != default.id)
                    .cloned()
                    .collect();
                if owner.as_deref() == Some(key) {
                    sources.insert(0, default.clone());
                }
                sources
            }
            _ => ordered.to_vec(),
        }
    };

    let mut all_prefs = prefs.clone();
    let mut identities: Vec<IdentityAccount> = Vec::new();
    let mut identity_of_folder: BTreeMap<String, String> = BTreeMap::new();
    // Missing prefs are derived in a stable order so colours don't depend on
    // map order: identities holding the default first, then by key.
    let mut ordered_keys = bucket_keys.clone();
    let holds_default = |key: &String| {
        buckets
            .get(key)
            .is_some_and(|m| m.iter().any(|f| f.is_default(paths)))
    };
    ordered_keys.sort_by(|a, b| holds_default(b).cmp(&holds_default(a)).then(a.cmp(b)));
    for key in &ordered_keys {
        let Some(members) = buckets.get(key) else {
            continue;
        };
        let member_folders: Vec<Folder> = members.iter().map(|f| (*f).clone()).collect();
        let ordered = ordered_folders(&member_folders, paths);
        if !all_prefs.contains_key(key) {
            let sources = prefs_sources(key, &ordered);
            let (saved_sources, unsaved): (Vec<&Folder>, Vec<&Folder>) =
                sources.iter().partition(|f| saved.contains(f.dir()));
            let chain: Vec<&Folder> = saved_sources.into_iter().chain(unsaved).collect();
            // Colours of the identities here now (not of ones long gone).
            let taken: Vec<i64> = bucket_keys
                .iter()
                .filter_map(|k| all_prefs.get(k).map(|p| p.color_index))
                .collect();
            all_prefs.insert(key.clone(), derived_prefs(&chain, &taken));
        }
        let chosen = all_prefs.get(key).cloned().unwrap_or_default();
        // Identity fields from the most trustworthy folder that has each:
        // stores (the account's own copy), windows, standalone folders, then
        // `~\.claude`, a corrected (mirrored) folder last.
        let mut trusted: Vec<&Folder> = members.to_vec();
        trusted.sort_by_key(|f| trust_rank(f, &keys, paths));
        let field = |pick: &dyn Fn(&Folder) -> Option<String>| -> Option<String> {
            trusted
                .iter()
                .find_map(|f| pick(f).filter(|v| !v.is_empty()))
        };
        let scope = organization_of_key(key);
        let organization_uuid = match &scope {
            Some(scope) => Some(
                trusted
                    .iter()
                    .filter_map(|f| f.organization_uuid())
                    .find(|org| org.to_lowercase() == *scope)
                    .map(str::to_owned)
                    .unwrap_or_else(|| scope.clone()),
            ),
            None => field(&|f| f.organization_uuid().map(str::to_owned)),
        };
        let run_dirs: Vec<Folder> = ordered
            .iter()
            .filter(|f| f.kind == FolderKind::Run)
            .cloned()
            .collect();
        let store_dirs: Vec<Folder> = ordered
            .iter()
            .filter(|f| f.kind == FolderKind::Store)
            .cloned()
            .collect();
        let identity = IdentityAccount {
            id: IdentityId::new(key.clone()),
            ring_id: ring_id_for_identity(paths, key),
            email: field(&|f| f.email().map(str::to_owned)),
            display_name: field(&|f| f.display_name().map(str::to_owned)),
            organization_name: field(&|f| f.organization_name().map(str::to_owned)),
            organization_uuid,
            account_uuid: account_uuid_of_key(key)
                .or_else(|| field(&|f| f.account_uuid().map(str::to_owned))),
            subscription_type: field(&|f| f.subscription_type.clone()),
            rate_limit_tier: field(&|f| f.rate_limit_tier().map(str::to_owned)),
            includes_default: run_dirs.iter().any(|f| f.is_default(paths)),
            window_dir_ids: run_dirs
                .iter()
                .filter(|f| is_window_dir(paths, f.dir()))
                .map(|f| f.id.clone())
                .collect(),
            run_dirs,
            store_dirs,
            custom_label: chosen.custom_label.clone(),
            color_index: chosen.color_index,
            is_hidden: chosen.is_hidden,
            ring_hidden: chosen.ring_hidden,
            organization_scope: scope,
            default_label: None,
            default_monogram: None,
        };
        for folder in members {
            identity_of_folder.insert(folder.dir().to_owned(), key.clone());
        }
        identities.push(identity);
    }

    // Names told apart among tracked identities (an untracked one is named
    // as if it joined them), the same rule folders have.
    let representatives: Vec<Folder> = identities.iter().map(|i| i.representative(paths)).collect();
    let mut by_representative: HashMap<String, usize> = HashMap::new();
    for (index, representative) in representatives.iter().enumerate() {
        by_representative
            .entry(representative.dir().to_owned())
            .or_insert(index);
    }
    for named in naming::named(&representatives, paths) {
        if let Some(index) = by_representative.get(named.dir()) {
            identities[*index].default_label = named.default_label.clone();
            identities[*index].default_monogram = named.default_monogram.clone();
        }
    }
    // By name, not by which one `~\.claude` runs as: the extension mirrors
    // the focused window's account into it, and the rings must not swap
    // places every time another window is focused.
    identities.sort_by(|a, b| {
        naming::compare_labels(&a.label(paths), &b.label(paths)).then_with(|| a.id.cmp(&b.id))
    });
    Grouping {
        identities,
        identity_of_folder,
        unsigned_folders: ordered_folders(&unsigned, paths),
        forgotten_folders,
        prefs: all_prefs,
        corrected_folders: keys
            .iter()
            .filter(|(_, r)| r.corrected)
            .map(|(folder, _)| folder.clone())
            .collect(),
        folder_keys: keys
            .into_iter()
            .map(|(folder, r)| (folder, r.key))
            .collect(),
        default_owner: owner,
    }
}
