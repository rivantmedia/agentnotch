//! The pipe's name and whose it is. Both ends compute the name from the
//! user's SID, so nothing is templated into hook commands and every user has
//! their own pipe. Pipe names are global, though: another user can create
//! the name first. So the server creates the pipe with a security descriptor
//! only it could have given it ([`pipe_sddl`]), and every client reads that
//! descriptor back and checks it ([`PipeSecurity::is_ours`]) before it writes
//! a byte: the Windows form of the Mac script's `st_uid` check on the socket.

/// Every pipe name the fork uses starts with this (case-insensitively, as
/// Windows compares them).
pub const PIPE_NAME_PREFIX: &str = r"\\.\pipe\";

/// `\\.\pipe\agentnotch-hook-<sid>`, where `sid` is the string SID of the
/// process token's user (`S-1-5-21-…`).
pub fn pipe_name(user_sid: &str) -> String {
    format!(r"{PIPE_NAME_PREFIX}agentnotch-hook-{user_sid}")
}

/// Local System, the one other account the pipe's DACL names.
pub const SYSTEM_SID: &str = "S-1-5-18";

/// The pipe's security descriptor in SDDL: owned by the user, with a
/// protected DACL (no inherited entries) that allows the user and SYSTEM and
/// nobody else.
///
/// The owner is set explicitly: an elevated app's objects would otherwise be
/// owned by Administrators, which the client's check refuses.
pub fn pipe_sddl(user_sid: &str) -> String {
    format!("O:{user_sid}D:P(A;;GA;;;{user_sid})(A;;GA;;;SY)")
}

/// One entry of a pipe's DACL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeAce {
    /// An access-allowed entry; anything else (deny, audit, object and
    /// callback entries) is `false`.
    pub allow: bool,
    /// Who it names, as a string SID; empty when it could not be read.
    pub sid: String,
}

/// What a client read from the pipe it opened (`GetSecurityInfo` on its own
/// handle, so it works whatever the server's integrity level is).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipeSecurity {
    /// The owner's string SID.
    pub owner: Option<String>,
    /// `SE_DACL_PROTECTED`: the DACL takes nothing from a parent.
    pub dacl_protected: bool,
    /// `None`: the pipe has no DACL at all (everyone may do anything).
    pub dacl: Option<Vec<PipeAce>>,
}

impl PipeSecurity {
    /// Whether the pipe is the app of `user_sid` and no one else's: owned by
    /// that user, with a protected DACL holding only allow entries for the
    /// user and SYSTEM.
    ///
    /// Another local user who created the name first cannot make our SID the
    /// owner of their pipe, and a pipe of ours that anyone else may open is
    /// not the one the app made. Either way the client must not write: what
    /// it sends are tool inputs, and what it would read back approves tools.
    pub fn is_ours(&self, user_sid: &str) -> bool {
        let same = |sid: &str| sid.eq_ignore_ascii_case(user_sid);
        if user_sid.is_empty() || !self.owner.as_deref().is_some_and(same) {
            return false;
        }
        let Some(dacl) = &self.dacl else {
            return false;
        };
        self.dacl_protected
            && dacl.iter().any(|ace| ace.allow && same(&ace.sid))
            && dacl.iter().all(|ace| {
                ace.allow && (same(&ace.sid) || ace.sid.eq_ignore_ascii_case(SYSTEM_SID))
            })
    }
}

/// The development override `AGENTNOTCH_SOCKET`, when it applies.
///
/// The hook exe honours it only together with `AGENTNOTCH_DEV=1`, so a
/// leftover export in a shell can never redirect a real session's hooks;
/// the app always honours it (`honour_without_dev`). Only a pipe path is
/// accepted: anything else (empty, a file path, a Unix socket path left over
/// from the Mac) is ignored rather than opened.
pub fn dev_pipe_override(
    get_env: impl Fn(&str) -> Option<String>,
    honour_without_dev: bool,
) -> Option<String> {
    if !honour_without_dev && get_env("AGENTNOTCH_DEV").as_deref() != Some("1") {
        return None;
    }
    let value = get_env("AGENTNOTCH_SOCKET")?;
    let prefix_len = PIPE_NAME_PREFIX.len();
    let has_prefix = value.len() > prefix_len
        && value.is_char_boundary(prefix_len)
        && value[..prefix_len].eq_ignore_ascii_case(PIPE_NAME_PREFIX);
    // A name part with another separator would open some other device.
    let name_ok = has_prefix && !value[prefix_len..].contains(['\\', '/']);
    name_ok.then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn the_name_carries_the_sid() {
        assert_eq!(
            pipe_name("S-1-5-21-1004336348-1177238915-682003330-512"),
            r"\\.\pipe\agentnotch-hook-S-1-5-21-1004336348-1177238915-682003330-512"
        );
    }

    #[test]
    fn the_hook_needs_the_dev_switch() {
        let pipe = r"\\.\pipe\agentnotch-test-1";
        assert_eq!(
            dev_pipe_override(env(&[("AGENTNOTCH_SOCKET", pipe)]), false),
            None
        );
        assert_eq!(
            dev_pipe_override(
                env(&[("AGENTNOTCH_SOCKET", pipe), ("AGENTNOTCH_DEV", "1")]),
                false
            ),
            Some(pipe.to_string())
        );
        assert_eq!(
            dev_pipe_override(
                env(&[("AGENTNOTCH_SOCKET", pipe), ("AGENTNOTCH_DEV", "true")]),
                false
            ),
            None
        );
    }

    #[test]
    fn the_app_honours_it_always() {
        let pipe = r"\\.\PIPE\agentnotch-test-2";
        assert_eq!(
            dev_pipe_override(env(&[("AGENTNOTCH_SOCKET", pipe)]), true),
            Some(pipe.to_string())
        );
        assert_eq!(dev_pipe_override(env(&[]), true), None);
    }

    const ME: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";
    const OTHER: &str = "S-1-5-21-1004336348-1177238915-682003330-1002";

    fn allow(sid: &str) -> PipeAce {
        PipeAce {
            allow: true,
            sid: sid.into(),
        }
    }

    fn the_apps_pipe() -> PipeSecurity {
        PipeSecurity {
            owner: Some(ME.into()),
            dacl_protected: true,
            dacl: Some(vec![allow(ME), allow(SYSTEM_SID)]),
        }
    }

    #[test]
    fn the_descriptor_names_the_user_as_owner() {
        assert_eq!(
            pipe_sddl("S-1-5-21-1-2-3-1001"),
            "O:S-1-5-21-1-2-3-1001D:P(A;;GA;;;S-1-5-21-1-2-3-1001)(A;;GA;;;SY)"
        );
    }

    #[test]
    fn the_apps_own_pipe_is_trusted() {
        assert!(the_apps_pipe().is_ours(ME));
        // SID text compares without case, and the order of entries is free.
        let mut reordered = the_apps_pipe();
        reordered.dacl = Some(vec![allow(SYSTEM_SID), allow(&ME.to_lowercase())]);
        assert!(reordered.is_ours(ME));
        // SYSTEM may be left out.
        let mut alone = the_apps_pipe();
        alone.dacl = Some(vec![allow(ME)]);
        assert!(alone.is_ours(ME));
    }

    #[test]
    fn a_squatters_pipe_is_refused() {
        // Another user created the name first, open to everyone.
        let squatter = PipeSecurity {
            owner: Some(OTHER.into()),
            dacl_protected: false,
            dacl: Some(vec![allow("S-1-1-0")]),
        };
        assert!(!squatter.is_ours(ME));
        // They can copy our DACL, but never become us as the owner.
        let mut copied = the_apps_pipe();
        copied.owner = Some(OTHER.into());
        assert!(!copied.is_ours(ME));
        // Administrators own an elevated process's objects by default.
        let mut elevated_default = the_apps_pipe();
        elevated_default.owner = Some("S-1-5-32-544".into());
        assert!(!elevated_default.is_ours(ME));
    }

    #[test]
    fn a_pipe_others_can_reach_is_refused() {
        let mut open = the_apps_pipe();
        open.dacl = Some(vec![allow(ME), allow(SYSTEM_SID), allow("S-1-1-0")]);
        assert!(!open.is_ours(ME));

        let mut other_user = the_apps_pipe();
        other_user.dacl = Some(vec![allow(ME), allow(OTHER)]);
        assert!(!other_user.is_ours(ME));

        let mut inherits = the_apps_pipe();
        inherits.dacl_protected = false;
        assert!(!inherits.is_ours(ME));

        let mut no_dacl = the_apps_pipe();
        no_dacl.dacl = None;
        assert!(!no_dacl.is_ours(ME));

        // An entry of another kind (deny, audit, an unreadable one).
        let mut odd = the_apps_pipe();
        odd.dacl = Some(vec![
            allow(ME),
            PipeAce {
                allow: false,
                sid: ME.into(),
            },
        ]);
        assert!(!odd.is_ours(ME));

        // A DACL that does not name us at all isn't the app's either.
        let mut system_only = the_apps_pipe();
        system_only.dacl = Some(vec![allow(SYSTEM_SID)]);
        assert!(!system_only.is_ours(ME));
        let mut empty = the_apps_pipe();
        empty.dacl = Some(Vec::new());
        assert!(!empty.is_ours(ME));
    }

    #[test]
    fn an_unknown_owner_or_user_is_refused() {
        let mut ownerless = the_apps_pipe();
        ownerless.owner = None;
        assert!(!ownerless.is_ours(ME));
        assert!(!the_apps_pipe().is_ours(""));
        assert!(!PipeSecurity::default().is_ours(ME));
    }

    #[test]
    fn only_pipe_paths_count() {
        for bad in [
            "",
            "/tmp/agentnotch/hook.sock",
            r"\\.\pipe\",
            r"C:\pipe\x",
            r"\\.\pipe\a\b",
            r"\\.\pipe\a/b",
        ] {
            assert_eq!(
                dev_pipe_override(env(&[("AGENTNOTCH_SOCKET", bad)]), true),
                None,
                "{bad:?}"
            );
        }
    }
}
