//! Every run-time switch, parsed in one place with one rule for "on" (`1`,
//! `true` or `yes`, any case, spaces trimmed) (§4.13, Appendix C). The glue
//! reads the environment and argv once and hands the result to the hub.

use crate::core::paths::Paths;
use crate::core::sealed;

/// What shapes this run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevFlags {
    /// `AGENTNOTCH_SAFE_MODE` (fails closed, `core::sealed`).
    pub sealed: bool,
    /// `--no-install` / `AGENTNOTCH_NO_INSTALL`: never write a settings.json
    /// or hooks folder; probes and summaries off.
    pub no_install: bool,
    /// `AGENTNOTCH_NO_NOTIFICATIONS`: no toasts, no permission queries.
    pub no_notifications: bool,
    /// `AGENTNOTCH_USAGE_PROBE`: scheduled probes even with `--no-install`.
    pub usage_probe_on_dev_run: bool,
    /// `AGENTNOTCH_EXTRA_CONFIG_DIRS`, `;`-separated, normalized.
    pub extra_config_dirs: Vec<String>,
    /// `--dump-state` / `AGENTNOTCH_DUMP_STATE`: a line per session change
    /// into `run.log`.
    pub dump_state: bool,
    /// `AGENTNOTCH_WEB_URL`, raw (the cloud validates it: https, or http to
    /// localhost); never when sealed.
    pub web_url_override: Option<String>,
    /// `AGENTNOTCH_SOCKET`: the pipe name (`\\.\pipe\…`), which the app
    /// always honours.
    pub pipe_override: Option<String>,
    /// `AGENTNOTCH_DEV=1`.
    pub dev: bool,
    /// `AGENTNOTCH_DEV_BROWSER_LOG`: only with `AGENTNOTCH_DEV=1`, never sealed.
    pub dev_browser_log: Option<String>,
    /// Sealed only: `AGENTNOTCH_OPEN_PANEL_ON_LAUNCH` (`sessions` |
    /// `session:<id>` | `settings`).
    pub open_panel_on_launch: Option<String>,
    /// Sealed only: `AGENTNOTCH_PANEL_SELF_TEST`.
    pub panel_self_test: bool,
    /// Sealed only: `AGENTNOTCH_SELF_TEST_OUT`.
    pub self_test_out: Option<String>,
    /// Sealed only: `AGENTNOTCH_SNAPSHOT_CLAUDE`.
    pub snapshot_claude: Option<String>,
}

impl DevFlags {
    /// The rule for a boolean switch.
    pub fn truthy(value: Option<&str>) -> bool {
        value
            .map(|v| v.trim().to_lowercase())
            .is_some_and(|v| v == "1" || v == "true" || v == "yes")
    }

    /// From the environment and argv (`paths` expands and normalizes the
    /// extra folders).
    pub fn from_env(
        get_env: impl Fn(&str) -> Option<String>,
        args: &[String],
        paths: &Paths,
    ) -> DevFlags {
        let sealed = sealed::is_sealed(&get_env);
        let flag = |arg: &str, env: &str| {
            args.iter().any(|a| a == arg) || Self::truthy(get_env(env).as_deref())
        };
        let value = |env: &str| {
            get_env(env)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let dev = get_env("AGENTNOTCH_DEV").as_deref() == Some("1");
        DevFlags {
            sealed,
            no_install: flag("--no-install", "AGENTNOTCH_NO_INSTALL"),
            no_notifications: Self::truthy(get_env("AGENTNOTCH_NO_NOTIFICATIONS").as_deref()),
            usage_probe_on_dev_run: Self::truthy(get_env("AGENTNOTCH_USAGE_PROBE").as_deref()),
            extra_config_dirs: value("AGENTNOTCH_EXTRA_CONFIG_DIRS")
                .map(|v| paths.split_list(&v))
                .unwrap_or_default(),
            dump_state: flag("--dump-state", "AGENTNOTCH_DUMP_STATE"),
            web_url_override: if sealed {
                None
            } else {
                value("AGENTNOTCH_WEB_URL")
            },
            pipe_override: agentnotch_proto::dev_pipe_override(&get_env, true),
            dev,
            dev_browser_log: if dev && !sealed {
                value("AGENTNOTCH_DEV_BROWSER_LOG")
            } else {
                None
            },
            open_panel_on_launch: if sealed {
                value("AGENTNOTCH_OPEN_PANEL_ON_LAUNCH")
            } else {
                None
            },
            panel_self_test: sealed
                && Self::truthy(get_env("AGENTNOTCH_PANEL_SELF_TEST").as_deref()),
            self_test_out: if sealed {
                value("AGENTNOTCH_SELF_TEST_OUT")
            } else {
                None
            },
            snapshot_claude: if sealed {
                value("AGENTNOTCH_SNAPSHOT_CLAUDE")
            } else {
                None
            },
        }
    }

    /// settings.json and hooks folders may be written (consent aside).
    pub fn installs_allowed(&self) -> bool {
        !self.sealed && !self.no_install
    }

    pub fn notifications_allowed(&self) -> bool {
        !self.sealed && !self.no_notifications
    }

    /// Scheduled usage probes may run.
    pub fn probes_allowed(&self) -> bool {
        !self.sealed && (!self.no_install || self.usage_probe_on_dev_run)
    }

    /// Session summaries never run in a `--no-install` run.
    pub fn summaries_allowed(&self) -> bool {
        !self.sealed && !self.no_install
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::PathStyle;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn paths() -> Paths {
        Paths::new(PathStyle::Windows, r"C:\Users\me")
    }

    #[test]
    fn truthy_rule() {
        for on in ["1", "true", "TRUE", " yes "] {
            assert!(DevFlags::truthy(Some(on)), "{on}");
        }
        for off in ["", "0", "on", "no", "y"] {
            assert!(!DevFlags::truthy(Some(off)), "{off}");
        }
        assert!(!DevFlags::truthy(None));
    }

    #[test]
    fn live_run() {
        let flags = DevFlags::from_env(
            env(&[
                ("AGENTNOTCH_NO_INSTALL", "1"),
                (
                    "AGENTNOTCH_EXTRA_CONFIG_DIRS",
                    r"~\.claude-a; D:/work/.claude-b ;",
                ),
                ("AGENTNOTCH_WEB_URL", "http://127.0.0.1:3000"),
                ("AGENTNOTCH_SOCKET", r"\\.\pipe\agentnotch-dev"),
                ("AGENTNOTCH_PANEL_SELF_TEST", "1"),
                ("AGENTNOTCH_DEV_BROWSER_LOG", r"C:\tmp\browser.log"),
            ]),
            &["--dump-state".into()],
            &paths(),
        );
        assert!(!flags.sealed);
        assert!(flags.no_install && flags.dump_state);
        assert!(!flags.installs_allowed() && !flags.probes_allowed() && !flags.summaries_allowed());
        assert_eq!(
            flags.extra_config_dirs,
            [r"C:\Users\me\.claude-a", r"D:\work\.claude-b"]
        );
        assert_eq!(
            flags.web_url_override.as_deref(),
            Some("http://127.0.0.1:3000")
        );
        assert_eq!(
            flags.pipe_override.as_deref(),
            Some(r"\\.\pipe\agentnotch-dev")
        );
        // Sealed-only and dev-only switches don't apply.
        assert!(!flags.panel_self_test);
        assert_eq!(flags.dev_browser_log, None);
    }

    #[test]
    fn usage_probe_on_a_dev_run() {
        let flags = DevFlags::from_env(
            env(&[("AGENTNOTCH_USAGE_PROBE", "yes")]),
            &["--no-install".into()],
            &paths(),
        );
        assert!(flags.probes_allowed());
        assert!(!flags.installs_allowed());
    }

    #[test]
    fn sealed_run() {
        let flags = DevFlags::from_env(
            env(&[
                ("AGENTNOTCH_SAFE_MODE", "1"),
                ("AGENTNOTCH_WEB_URL", "https://example.test"),
                ("AGENTNOTCH_PANEL_SELF_TEST", "1"),
                ("AGENTNOTCH_SNAPSHOT_CLAUDE", r"C:\snap"),
                ("AGENTNOTCH_DEV", "1"),
                ("AGENTNOTCH_DEV_BROWSER_LOG", r"C:\tmp\browser.log"),
            ]),
            &[],
            &paths(),
        );
        assert!(flags.sealed);
        assert_eq!(flags.web_url_override, None);
        assert_eq!(flags.dev_browser_log, None);
        assert!(flags.panel_self_test);
        assert_eq!(flags.snapshot_claude.as_deref(), Some(r"C:\snap"));
        assert!(
            !flags.installs_allowed() && !flags.notifications_allowed() && !flags.probes_allowed()
        );
    }
}
