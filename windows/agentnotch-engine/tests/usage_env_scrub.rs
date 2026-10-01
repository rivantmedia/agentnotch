//! The environment a usage check (and a session summary) runs `claude` with
//! (§4.6, D 1811-1813). The scrub is what keeps a check from spending, and
//! answering for, another account: `CLAUDE_CONFIG_DIR`,
//! `CLAUDE_SECURESTORAGE_CONFIG_DIR` and the `ANTHROPIC_*` names would each
//! make the child run as another login.

use agentnotch_engine::core::paths::PathStyle;
use agentnotch_engine::usage::env::{is_scrubbed, scrubbed_env_in};
use std::ffi::{OsStr, OsString};
use std::path::Path;

fn pairs(list: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    list.iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

/// Every removed name, in mixed case (Windows names are case-insensitive),
/// around the two that must survive.
fn windows_base() -> Vec<(OsString, OsString)> {
    pairs(&[
        ("ClaudeCode", "1"),
        ("Path", r"C:\Windows\system32;C:\Windows"),
        ("claude_pid", "123"),
        ("Claude_Effort", "high"),
        ("ai_agent", "claude"),
        ("Claude_Config_Dir", r"C:\Users\me\.claude-other"),
        (
            "claude_SecureStorage_config_dir",
            r"C:\Users\me\.claude-other",
        ),
        ("Claude_Code_Session_Id", "s"),
        ("SystemRoot", r"C:\Windows"),
        ("claude_agent_sdk_Version", "1"),
        ("anthropic_api_key", "sk-ant-secret"),
        ("Anthropic_Auth_Token", "token"),
    ])
}

const RAW_DIR: &str = r"C:\Users\me\.claude-work\";
const BINARY_DIR: &str = r"C:\Users\me\.local\bin";

#[test]
fn windows_vector_keeps_only_path_systemroot_and_the_folder() {
    let env = scrubbed_env_in(
        PathStyle::Windows,
        &windows_base(),
        Some(RAW_DIR),
        Path::new(BINARY_DIR),
    );
    assert_eq!(
        env,
        pairs(&[
            // The variable keeps its own spelling; the binary's folder comes first.
            (
                "Path",
                r"C:\Users\me\.local\bin;C:\Windows\system32;C:\Windows"
            ),
            ("SystemRoot", r"C:\Windows"),
            // The raw value, exactly as the folder's own sessions see it.
            ("CLAUDE_CONFIG_DIR", RAW_DIR),
        ])
    );
}

#[test]
fn the_default_folder_runs_with_claude_config_dir_unset() {
    for raw in [None, Some("")] {
        let env = scrubbed_env_in(
            PathStyle::Windows,
            &windows_base(),
            raw,
            Path::new(BINARY_DIR),
        );
        assert_eq!(
            env,
            pairs(&[
                (
                    "Path",
                    r"C:\Users\me\.local\bin;C:\Windows\system32;C:\Windows"
                ),
                ("SystemRoot", r"C:\Windows"),
            ]),
            "config dir {raw:?}"
        );
    }
}

#[test]
fn posix_style_joins_with_a_colon() {
    let base = pairs(&[
        ("PATH", "/usr/bin:/bin"),
        ("HOME", "/Users/me"),
        ("CLAUDE_CONFIG_DIR", "/Users/me/.claude-other"),
        ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/Users/me/.claude-other"),
        ("ANTHROPIC_API_KEY", "sk-ant-secret"),
        ("CLAUDE_CODE_ENTRYPOINT", "cli"),
    ]);
    let env = scrubbed_env_in(
        PathStyle::Posix,
        &base,
        Some("/Users/me/.claude-work/"),
        Path::new("/Users/me/.local/bin"),
    );
    assert_eq!(
        env,
        pairs(&[
            ("PATH", "/Users/me/.local/bin:/usr/bin:/bin"),
            ("HOME", "/Users/me"),
            ("CLAUDE_CONFIG_DIR", "/Users/me/.claude-work/"),
        ])
    );
}

/// UsageProbeParsingTests.environmentIsScrubbed. The Mac's `environment`
/// leaves `PATH` alone; here that is an empty binary folder.
#[test]
fn environment_is_scrubbed() {
    let base = pairs(&[
        ("PATH", "/usr/bin"),
        ("HOME", "/Users/me"),
        ("CLAUDECODE", "1"),
        ("CLAUDE_PID", "123"),
        ("CLAUDE_EFFORT", "high"),
        ("CLAUDE_CODE_SESSION_ID", "s"),
        ("CLAUDE_CODE_MESSAGING_TOKEN", "secret"),
        ("CLAUDE_CODE_ENTRYPOINT", "cli"),
        ("CLAUDE_AGENT_SDK_VERSION", "1"),
        ("AI_AGENT", "claude"),
        ("CLAUDE_CONFIG_DIR", "/Users/me/.claude-other"),
    ]);
    let kept = pairs(&[("PATH", "/usr/bin"), ("HOME", "/Users/me")]);
    assert_eq!(
        scrubbed_env_in(PathStyle::Posix, &base, None, Path::new("")),
        kept
    );
    let mut with_folder = kept.clone();
    with_folder.push(("CLAUDE_CONFIG_DIR".into(), "/Users/me/.claude-work/".into()));
    assert_eq!(
        scrubbed_env_in(
            PathStyle::Posix,
            &base,
            Some("/Users/me/.claude-work/"),
            Path::new("")
        ),
        with_folder
    );
}

#[test]
fn a_missing_path_is_the_binarys_folder_alone() {
    let env = scrubbed_env_in(
        PathStyle::Windows,
        &pairs(&[("SystemRoot", r"C:\Windows")]),
        None,
        Path::new(BINARY_DIR),
    );
    assert_eq!(
        env,
        pairs(&[("SystemRoot", r"C:\Windows"), ("PATH", BINARY_DIR)])
    );
}

#[test]
fn only_the_listed_names_and_prefixes_are_scrubbed() {
    for name in [
        "CLAUDECODE",
        "claude_pid",
        "Claude_Effort",
        "AI_AGENT",
        "CLAUDE_CONFIG_DIR",
        "Claude_SecureStorage_Config_Dir",
        "CLAUDE_CODE_",
        "claude_code_use_bedrock",
        "CLAUDE_AGENT_SDK_X",
        "ANTHROPIC_API_KEY",
        "anthropic_auth_token",
    ] {
        assert!(is_scrubbed(OsStr::new(name)), "{name}");
    }
    for name in [
        "PATH",
        "SystemRoot",
        "USERPROFILE",
        "APPDATA",
        "CLAUDE",
        "CLAUDE_CODE",
        "ANTHROPIC_BASE_URL",
        "XCLAUDECODE",
    ] {
        assert!(!is_scrubbed(OsStr::new(name)), "{name}");
    }
}
