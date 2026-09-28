//! The pipe's name. Both ends compute it from the user's SID, so nothing is
//! templated into hook commands and every user has their own pipe.

/// Every pipe name the fork uses starts with this (case-insensitively, as
/// Windows compares them).
pub const PIPE_NAME_PREFIX: &str = r"\\.\pipe\";

/// `\\.\pipe\agentnotch-hook-<sid>`, where `sid` is the string SID of the
/// process token's user (`S-1-5-21-…`).
pub fn pipe_name(user_sid: &str) -> String {
    format!(r"{PIPE_NAME_PREFIX}agentnotch-hook-{user_sid}")
}

/// The development override `AGENTNOTCH_SOCKET`, when it applies.
///
/// The hook exe honours it only together with `AGENTNOTCH_DEV=1`, so a
/// leftover export in a shell can never redirect a real session's hooks;
/// the app always honours it (`honour_without_dev`). Only a pipe path is
/// accepted: anything else (empty, a file path, a Unix socket path left over
/// from the Mac) is ignored rather than opened.
pub fn dev_pipe_override(get_env: impl Fn(&str) -> Option<String>, honour_without_dev: bool) -> Option<String> {
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
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
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
        assert_eq!(dev_pipe_override(env(&[("AGENTNOTCH_SOCKET", pipe)]), false), None);
        assert_eq!(
            dev_pipe_override(env(&[("AGENTNOTCH_SOCKET", pipe), ("AGENTNOTCH_DEV", "1")]), false),
            Some(pipe.to_string())
        );
        assert_eq!(dev_pipe_override(env(&[("AGENTNOTCH_SOCKET", pipe), ("AGENTNOTCH_DEV", "true")]), false), None);
    }

    #[test]
    fn the_app_honours_it_always() {
        let pipe = r"\\.\PIPE\agentnotch-test-2";
        assert_eq!(dev_pipe_override(env(&[("AGENTNOTCH_SOCKET", pipe)]), true), Some(pipe.to_string()));
        assert_eq!(dev_pipe_override(env(&[]), true), None);
    }

    #[test]
    fn only_pipe_paths_count() {
        for bad in ["", "/tmp/agentnotch/hook.sock", r"\\.\pipe\", r"C:\pipe\x", r"\\.\pipe\a\b", r"\\.\pipe\a/b"] {
            assert_eq!(dev_pipe_override(env(&[("AGENTNOTCH_SOCKET", bad)]), true), None, "{bad:?}");
        }
    }
}
