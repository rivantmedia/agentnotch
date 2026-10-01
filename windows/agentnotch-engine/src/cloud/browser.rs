//! What the Windows browser service decides before it touches the OS
//! (DESIGN-WIN §4.11): whether a URL may be opened at all, and whether a dev
//! run asked for sign-in URLs to be written to a file instead of opened (the
//! smoke test signs in without a browser). Pure, so the rule is tested on
//! every system and stays the same as `DevFlags::dev_browser_log`.

use crate::cloud::website;
use crate::core::sealed;
use std::path::PathBuf;

/// The file a dev run's sign-in URLs go to instead of the browser:
/// `AGENTNOTCH_DEV_BROWSER_LOG`, honoured only with `AGENTNOTCH_DEV` exactly
/// `1` and never in a sealed run (the same rule as
/// `DevFlags::dev_browser_log`, so the hub and the browser agree).
pub fn browser_log_target(get_env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if sealed::is_sealed(&get_env) || get_env("AGENTNOTCH_DEV").as_deref() != Some("1") {
        return None;
    }
    get_env("AGENTNOTCH_DEV_BROWSER_LOG")
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Whether the browser may be handed this URL: only what the website rule
/// keeps (https, or http to this PC), so a file path, a `javascript:` URL or
/// another scheme's handler is never launched through the shell.
pub fn may_open(url: &str) -> bool {
    website::validated_link(Some(url)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::flags::DevFlags;
    use crate::core::paths::{PathStyle, Paths};

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn the_log_needs_dev_exactly_one_and_a_file() {
        let log = "C:\\Temp\\browser.log";
        assert_eq!(
            browser_log_target(env(&[
                ("AGENTNOTCH_DEV", "1"),
                ("AGENTNOTCH_DEV_BROWSER_LOG", log)
            ])),
            Some(PathBuf::from(log))
        );
        assert_eq!(
            browser_log_target(env(&[
                ("AGENTNOTCH_DEV", "1"),
                ("AGENTNOTCH_DEV_BROWSER_LOG", "  /tmp/b.log ")
            ])),
            Some(PathBuf::from("/tmp/b.log"))
        );
        for dev in ["true", "yes", " 1", "0", ""] {
            assert_eq!(
                browser_log_target(env(&[
                    ("AGENTNOTCH_DEV", dev),
                    ("AGENTNOTCH_DEV_BROWSER_LOG", log)
                ])),
                None,
                "{dev:?}"
            );
        }
        assert_eq!(
            browser_log_target(env(&[("AGENTNOTCH_DEV_BROWSER_LOG", log)])),
            None
        );
        for empty in ["", "   "] {
            assert_eq!(
                browser_log_target(env(&[
                    ("AGENTNOTCH_DEV", "1"),
                    ("AGENTNOTCH_DEV_BROWSER_LOG", empty)
                ])),
                None
            );
        }
        assert_eq!(browser_log_target(env(&[("AGENTNOTCH_DEV", "1")])), None);
    }

    #[test]
    fn a_sealed_run_never_logs() {
        for seal in [
            ("AGENTNOTCH_SAFE_MODE", "1"),
            ("AGENTNOTCH_SAFE_MODE", "tru"),
            ("CODENOTCH_DEMO", "1"),
        ] {
            assert_eq!(
                browser_log_target(env(&[
                    seal,
                    ("AGENTNOTCH_DEV", "1"),
                    ("AGENTNOTCH_DEV_BROWSER_LOG", "C:\\b.log")
                ])),
                None,
                "{seal:?}"
            );
        }
    }

    #[test]
    fn the_rule_matches_dev_flags() {
        let cases: [&[(&str, &str)]; 5] = [
            &[
                ("AGENTNOTCH_DEV", "1"),
                ("AGENTNOTCH_DEV_BROWSER_LOG", "b.log"),
            ],
            &[
                ("AGENTNOTCH_DEV", "true"),
                ("AGENTNOTCH_DEV_BROWSER_LOG", "b.log"),
            ],
            &[("AGENTNOTCH_DEV", "1"), ("AGENTNOTCH_DEV_BROWSER_LOG", " ")],
            &[
                ("AGENTNOTCH_SAFE_MODE", "1"),
                ("AGENTNOTCH_DEV", "1"),
                ("AGENTNOTCH_DEV_BROWSER_LOG", "b.log"),
            ],
            &[("AGENTNOTCH_DEV_BROWSER_LOG", "b.log")],
        ];
        let paths = Paths::new(PathStyle::Windows, "C:\\Users\\me");
        for pairs in cases {
            let flags = DevFlags::from_env(env(pairs), &[], &paths);
            assert_eq!(
                browser_log_target(env(pairs)),
                flags.dev_browser_log.map(PathBuf::from),
                "{pairs:?}"
            );
        }
    }

    #[test]
    fn only_https_or_local_http_may_open() {
        for ok in [
            "https://example.supabase.co/auth/v1/authorize?provider=google&code_challenge=x",
            "https://agentnotch.example.com/dashboard",
            "http://localhost:3000/api/app/v1/config",
            "http://127.0.0.1:54321/auth/v1/authorize",
            "http://[::1]:3000/",
        ] {
            assert!(may_open(ok), "{ok}");
        }
        for refused in [
            "",
            "   ",
            "http://example.com/",
            "file:///C:/Windows/System32/calc.exe",
            "C:\\Windows\\System32\\calc.exe",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "agentnotch://auth-callback?code=x",
            "notaurl",
        ] {
            assert!(!may_open(refused), "{refused}");
        }
    }
}
