//! Newtype ids. Each serialises as a plain string.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                $name(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                $name(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                $name(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(
    /// A Claude config folder: its normalized path in display case
    /// (`core::paths::Paths::normalize`, AU§1.2). Two ids name the same folder
    /// when their `core::paths::Paths::key` is equal (Windows paths compare
    /// case-insensitively).
    AccountId
);

string_id!(
    /// A signed-in identity: `uuid:<accountUuid>[/<organizationUuid>]`,
    /// `email:<address>`, or `dir:<folder id>` for a folder added by hand
    /// that nobody signed in to yet (AU§5.1).
    IdentityId
);

string_id!(
    /// A ring: `claude-acct-<12 hex>` for an identity, `claude`,
    /// `claude-<slug>` or `claude-dir-<8 hex>` for a `dir:` identity (AU§5.4).
    RingId
);

string_id!(
    /// Claude Code's session id.
    SessionId
);

impl IdentityId {
    pub const UUID_PREFIX: &'static str = "uuid:";
    pub const EMAIL_PREFIX: &'static str = "email:";
    pub const DIR_PREFIX: &'static str = "dir:";

    /// Someone is signed in (`uuid:` or `email:`), as opposed to a `dir:`
    /// folder nobody signed in to.
    pub fn is_login(&self) -> bool {
        self.0.starts_with(Self::UUID_PREFIX) || self.0.starts_with(Self::EMAIL_PREFIX)
    }

    /// The account UUID of a `uuid:` id, without its organization.
    pub fn account_uuid(&self) -> Option<&str> {
        let rest = self.0.strip_prefix(Self::UUID_PREFIX)?;
        rest.split('/').next().filter(|uuid| !uuid.is_empty())
    }

    /// The organization of a split `uuid:<acct>/<org>` id.
    pub fn organization(&self) -> Option<&str> {
        let rest = self.0.strip_prefix(Self::UUID_PREFIX)?;
        rest.split_once('/')
            .map(|(_, org)| org)
            .filter(|org| !org.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_plain_strings_in_json() {
        assert_eq!(
            serde_json::to_string(&SessionId::from("abc")).unwrap(),
            "\"abc\""
        );
        let ring: RingId = serde_json::from_str("\"claude-acct-1a2b3c4d5e6f\"").unwrap();
        assert_eq!(ring.as_str(), "claude-acct-1a2b3c4d5e6f");
    }

    #[test]
    fn identity_parts() {
        let split = IdentityId::from("uuid:acct-1/org-9");
        assert_eq!(split.account_uuid(), Some("acct-1"));
        assert_eq!(split.organization(), Some("org-9"));
        assert!(split.is_login());
        let plain = IdentityId::from("uuid:acct-1");
        assert_eq!(plain.organization(), None);
        let email = IdentityId::from("email:me@work.com");
        assert_eq!(email.account_uuid(), None);
        assert!(email.is_login());
        assert!(!IdentityId::from("dir:c:\\users\\me\\.claude-x").is_login());
    }
}
