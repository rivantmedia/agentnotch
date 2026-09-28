//! `agentnotch://` links arriving on the command line (DESIGN-WIN §2.4 `deeplink.rs`, §4.10,
//! §4.11): the website sign-in's callback and the notifications' actions.
//!
//! Windows starts `agentnotch.exe "<url>"` for a link; a second launch hands its argv to the
//! running app through the single-instance plugin (seam WSI), which joins it with `|` and splits
//! it again. So a link is accepted only as exactly one argument after the exe that starts with
//! the scheme: a URL that itself held `|` arrives as several arguments and is ignored, never
//! half-read. The URL is never logged: a sign-in callback carries the authorisation code.

const SCHEME: &str = "agentnotch:";
const LINK_PREFIX: &str = "agentnotch://";

/// The link in `args` (argv, the exe first), if that is all `args` holds.
pub(super) fn from_args(args: &[String]) -> Option<&str> {
    match args {
        [_exe, url] if starts_with_ignore_case(url, LINK_PREFIX) => Some(url.as_str()),
        _ => None,
    }
}

/// Whether an argument names the scheme at all (such an argument is never a subcommand).
pub(super) fn names_scheme(arg: &str) -> bool {
    starts_with_ignore_case(arg.trim_start_matches(['"', '\'']), SCHEME)
}

/// Hands a link to the hub, off the caller's thread: a sign-in callback exchanges its code with
/// the website, which must not hold up the single-instance plugin or the setup.
pub(super) fn handle(url: String) {
    std::thread::spawn(move || {
        let Some(hub) = super::hub() else {
            super::log("deep link ignored (Claude Code control isn't running)");
            return;
        };
        let outcome = hub.handle_deep_link(&url);
        super::log(&format!("deep link {}", describe(&outcome)));
    });
}

fn describe(outcome: &agentnotch_engine::hub::DeepLinkOutcome) -> String {
    use agentnotch_engine::hub::DeepLinkOutcome::*;
    match outcome {
        SignInCompleted => "completed the sign-in".into(),
        SignInIgnored(why) | Ignored(why) => format!("ignored ({why})"),
        Opened => "opened".into(),
        Reviewed => "marked a session reviewed".into(),
    }
}

/// URL schemes are case-insensitive (RFC 3986 §3.1), and Windows passes the link as it was
/// written.
fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

#[cfg(test)]
mod tests {
    use super::{from_args, names_scheme};

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn a_link_is_exactly_one_argument_after_the_exe() {
        let link = "agentnotch://auth-callback?code=abc&state=xyz";
        assert_eq!(from_args(&argv(&["agentnotch.exe", link])), Some(link));
        assert_eq!(
            from_args(&argv(&["agentnotch.exe", "AgentNotch://open?session=1"])),
            Some("AgentNotch://open?session=1")
        );
    }

    #[test]
    fn anything_else_is_not_a_link() {
        assert_eq!(from_args(&argv(&["agentnotch.exe"])), None);
        assert_eq!(from_args(&argv(&["agentnotch.exe", "doctor"])), None);
        assert_eq!(
            from_args(&argv(&["agentnotch.exe", "agentnotch:open"])),
            None
        );
        assert_eq!(
            from_args(&argv(&["agentnotch.exe", "https://example.com"])),
            None
        );
        // The plugin split a URL holding `|` into several arguments: ignored whole.
        assert_eq!(
            from_args(&argv(&[
                "agentnotch.exe",
                "agentnotch://auth-callback?code=a",
                "b"
            ])),
            None
        );
        assert_eq!(
            from_args(&argv(&["agentnotch.exe", "--silent", "agentnotch://open"])),
            None
        );
    }

    #[test]
    fn the_scheme_is_recognised_in_any_case_and_quoting() {
        assert!(names_scheme("agentnotch:x"));
        assert!(names_scheme("AGENTNOTCH://open"));
        assert!(names_scheme("\"agentnotch://open\""));
        assert!(!names_scheme("doctor"));
        assert!(!names_scheme("agent"));
    }
}
