//! The digests that name accounts and projects on the website (CL§1.6; the
//! Mac's `CloudKeys`), window ids as the website takes them, and the paths a
//! project key is made from.
//!
//! - `accountKey` = SHA-256 of the lowercased `<accountUuid>/<organizationUuid>`
//!   (the account UUID alone when no organization is known). It never
//!   depends on how this PC groups identities, so the same Claude account in
//!   the same organization has the same key for every user and computer,
//!   which is what pooling joins on.
//! - A project key is an HMAC-SHA256 keyed with this install's secret
//!   (`cloud-install-secret`) of `<accountKey>:<project path>`, so a guessed
//!   path can't be checked against it. The Windows install's keys differ
//!   from the Mac's for the same folder by design: the secret is per install.

use crate::core::paths::{PathStyle, Paths};
use crate::model::IdentityId;
use crate::platform::SecureFiles;
use crate::runtime_types::CloudAccount;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::path::Path;

use super::contract::trim_spaces;

pub const SESSION_WINDOW: &str = "session";
pub const WEEKLY_WINDOW: &str = "weekly_all";
pub const EXTRA_USAGE_WINDOW: &str = "extra_usage";
pub const SCOPED_PREFIX: &str = "weekly_";
/// `AccountIdentityGrouping.organizationSeparator`.
pub const ORGANIZATION_SEPARATOR: &str = "/";

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    hex(&Sha256::digest(bytes.as_ref()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 64 lowercase hex digits, as the website checks keys.
pub fn is_key(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A UUID in any case, written 8-4-4-4-12 (Claude Code's session ids; the
/// device id). The braced, urn and hyphenless forms a parser would accept
/// are not session ids.
pub fn is_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, b)| match index {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

fn is_lower_alphanumeric(b: u8) -> bool {
    b.is_ascii_digit() || b.is_ascii_lowercase()
}

/// `session`, `weekly_all`, `extra_usage` or `weekly_<model>` as the website
/// accepts it: a lowercase letter or digit, then up to 56 of lowercase
/// letters, digits, `_`, `.` and `-` (the website refuses a whole batch over
/// one bad window id).
pub fn is_window_id(id: &str) -> bool {
    if id == SESSION_WINDOW || id == WEEKLY_WINDOW || id == EXTRA_USAGE_WINDOW {
        return true;
    }
    let Some(rest) = id.strip_prefix(SCOPED_PREFIX) else {
        return false;
    };
    let rest = rest.as_bytes();
    let Some(first) = rest.first() else {
        return false;
    };
    rest.len() <= 57
        && is_lower_alphanumeric(*first)
        && rest[1..]
            .iter()
            .all(|b| is_lower_alphanumeric(*b) || matches!(b, b'_' | b'.' | b'-'))
}

/// A window id as the website takes it: as is, or a `weekly_<model>`
/// shortened to fit. `None` for anything else.
pub fn contract_window_id(id: &str) -> Option<String> {
    let id = id.to_lowercase();
    if is_window_id(&id) {
        return Some(id);
    }
    let rest = id.strip_prefix(SCOPED_PREFIX)?;
    let shortened = format!(
        "{SCOPED_PREFIX}{}",
        rest.chars().take(57).collect::<String>()
    );
    is_window_id(&shortened).then_some(shortened)
}

/// `weekly_<slug>` for a model family: lowercased, anything but ASCII
/// letters and digits folded to `_` ("Sonnet 4.5" → `weekly_sonnet_4_5`;
/// UsageRingWindows.scopedID).
pub fn scoped_id(model: &str) -> String {
    let mut slug = String::new();
    let mut last_was_separator = false;
    for c in model.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            last_was_separator = false;
        } else if !last_was_separator && !slug.is_empty() {
            slug.push('_');
            last_was_separator = true;
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }
    format!(
        "{SCOPED_PREFIX}{}",
        if slug.is_empty() { "scoped" } else { &slug }
    )
}

/// SHA-256 of the lowercased `<accountUuid>/<organizationUuid>` (both from
/// `oauthAccount`), or of the lowercased account UUID alone when the
/// organization is unknown.
pub fn account_key(account_uuid: &str, organization_uuid: Option<&str>) -> String {
    let account = trim_spaces(account_uuid);
    let base = match organization_uuid.map(trim_spaces).filter(|o| !o.is_empty()) {
        Some(organization) => format!("{account}{ORGANIZATION_SEPARATOR}{organization}"),
        None => account.to_owned(),
    };
    sha256_hex(base.to_lowercase())
}

fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(trim_spaces)
        .map(str::to_lowercase)
        .filter(|v| !v.is_empty())
}

/// The organization an identity is signed in to: the one it was split by
/// (`uuid:<account>/<organization>`), else the one its own folders'
/// `oauthAccount` names (they name at most one, or the identity would have
/// been split). A mirrored (corrected) folder's is never used: it is stale.
/// `None` when nothing says.
pub fn organization_of(account: &CloudAccount) -> Option<String> {
    if let Some(scope) = clean(account.identity_id.organization()) {
        return Some(scope);
    }
    account
        .folders
        .iter()
        .filter(|folder| !folder.corrected)
        .find_map(|folder| clean(folder.organization_uuid.as_deref()))
}

/// The key of an engine identity: its account UUID and its own
/// organization. `None` for `email:` and `dir:` identities, which have no
/// account UUID to share across computers.
pub fn account_key_of(account: &CloudAccount) -> Option<String> {
    account_key_for(&account.identity_id, organization_of(account).as_deref())
}

/// [`account_key`] of a `uuid:` identity id.
pub fn account_key_for(identity: &IdentityId, organization: Option<&str>) -> Option<String> {
    let uuid = identity.account_uuid().filter(|u| !u.is_empty())?;
    Some(account_key(uuid, organization))
}

/// HMAC-SHA256, keyed with this install's secret, of
/// `<accountKey>:<project path>`: stable per install, and the path can't be
/// guessed back from it.
pub fn project_key(account_key: &str, path: &str, secret: &[u8]) -> String {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.update(format!("{account_key}:{path}").as_bytes());
    hex(&mac.finalize().into_bytes())
}

/// A session's working directory as the project key uses it (pure): `~`,
/// `~\…` and `~/…` expanded against `home`, then the folder's canonical path
/// from `canonical` (links and junctions resolved, the case as it is on
/// disk, no `\\?\`), else, when the folder is gone, the path normalized
/// lexically (separators, drive letter, `.` and `..`, no trailing
/// separator). Never lower-cased: the canonical case already unifies
/// `c:\code` and `C:\Code` for a folder that exists.
pub fn project_path_in(
    style: PathStyle,
    cwd: &str,
    home: &str,
    canonical: &dyn Fn(&str) -> Option<String>,
) -> String {
    let paths = Paths::new(style, home);
    let expanded = paths.normalize(cwd);
    match canonical(&expanded) {
        Some(real) => paths.normalize(&real),
        None => expanded,
    }
}

/// [`project_path_in`] on this OS, through the platform's canonical paths.
pub fn project_path(cwd: &str, home: &Path, files: &dyn SecureFiles) -> String {
    let home = home.to_string_lossy();
    project_path_in(PathStyle::native(), cwd, &home, &|path| {
        files
            .canonical(Path::new(path))
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    })
}

/// The working directory's last component (what the website shows): split
/// on `\` and `/`, `C:` for a drive root, the path itself when there is no
/// component.
pub fn project_name(cwd: &str) -> String {
    let is_separator = |c: char| c == '/' || c == '\\';
    let mut path = cwd;
    while path.chars().count() > 1 && path.ends_with(is_separator) {
        path = &path[..path.len() - 1];
    }
    match path.rsplit(is_separator).next() {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_ids() {
        assert!(
            is_window_id("session") && is_window_id("weekly_all") && is_window_id("extra_usage")
        );
        assert!(is_window_id("weekly_sonnet_4_5") && is_window_id("weekly_opus-4.1"));
        assert!(!is_window_id("weekly_") && !is_window_id("weekly__x") && !is_window_id("monthly"));
        let long = format!("weekly_{}", "a".repeat(90));
        assert_eq!(contract_window_id(&long).map(|id| id.len()), Some(7 + 57));
        assert_eq!(contract_window_id("nonsense"), None);
        assert_eq!(
            contract_window_id("weekly_Opus").as_deref(),
            Some("weekly_opus")
        );
        assert!(!is_window_id("weekly_Opus"));
        assert_eq!(scoped_id("Sonnet 4.5"), "weekly_sonnet_4_5");
        assert_eq!(scoped_id("¿?"), "weekly_scoped");
    }

    #[test]
    fn uuids() {
        assert!(is_uuid("a1b2c3d4-e5f6-4789-8abc-def012345678"));
        assert!(is_uuid("A1B2C3D4-E5F6-4789-8ABC-DEF012345678"));
        assert!(!is_uuid("a1b2c3d4e5f647898abcdef012345678"));
        assert!(!is_uuid("{a1b2c3d4-e5f6-4789-8abc-def01234567}"));
        assert!(!is_uuid("not-a-uuid"));
    }

    #[test]
    fn project_names_on_every_separator() {
        assert_eq!(project_name("/Users/me/code/agentnotch/"), "agentnotch");
        assert_eq!(project_name(r"C:\Users\me\code\app\"), "app");
        assert_eq!(project_name("C:/Users/me/work/billing"), "billing");
        assert_eq!(project_name(r"C:\"), "C:");
        assert_eq!(project_name("/"), "/");
    }

    #[test]
    fn windows_project_paths() {
        let none = |_: &str| None;
        let style = PathStyle::Windows;
        let home = r"C:\Users\Me";
        assert_eq!(
            project_path_in(style, r"c:\code\app\", home, &none),
            r"C:\code\app"
        );
        assert_eq!(
            project_path_in(style, r"\\?\C:\code\.\x\..\app", home, &none),
            r"C:\code\app"
        );
        assert_eq!(
            project_path_in(style, r"~\code\app", home, &none),
            r"C:\Users\Me\code\app"
        );
        assert_eq!(
            project_path_in(style, "~/code/app", home, &none),
            r"C:\Users\Me\code\app"
        );
        assert_eq!(project_path_in(style, r"C:\", home, &none), r"C:\");
        // The case on disk decides, not how the cwd was typed.
        let on_disk =
            |p: &str| (p.to_lowercase() == r"c:\code\app").then(|| r"C:\Code\App".to_owned());
        assert_eq!(
            project_path_in(style, r"c:\CODE\app", home, &on_disk),
            r"C:\Code\App"
        );
        // A junction resolves to its target.
        let junction = |p: &str| (p == r"C:\link\app").then(|| r"\\?\D:\real\app".to_owned());
        assert_eq!(
            project_path_in(style, r"C:\link\app", home, &junction),
            r"D:\real\app"
        );
    }
}
