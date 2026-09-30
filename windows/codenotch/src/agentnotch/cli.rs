//! `agentnotch.exe <command>`: the fork's command line, handled before Tauri starts (seam WCLI,
//! DESIGN-WIN §4.14).
//!
//! The exe is a GUI-subsystem program: PowerShell neither waits for it nor captures its output.
//! So every command attaches to the parent console to print (upstream's `attach_console`) and
//! also writes its text to `<data>\<command>.log`, where scripts (the smoke test, the
//! uninstaller) read it after `Start-Process -Wait -PassThru`.
//!
//! The fork claims upstream's command names too (`doctor`, `install-hooks`, `uninstall-hooks`,
//! `autostart`): upstream's arms still sit in `main`, unreachable, and upstream's doctor would
//! read Claude's credentials. An argument naming the `agentnotch:` scheme is never a command,
//! whatever else argv holds: such an argv is either an app launch (`None`) or, when it also names
//! one of these commands, refused without running anything.

use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use agentnotch_engine::hub::{DoctorExtras, Hub};

use super::{deeplink, setup, DISPLAY_NAME};

/// Every command this module answers for.
const COMMANDS: [&str; 6] = [
    "doctor",
    "inspect-accounts",
    "install-hooks",
    "uninstall-hooks",
    "control",
    "autostart",
];

/// The exit code of a command that was refused with nothing run: a command line naming a command
/// and a link at once, `install-hooks` without the consent.
const REFUSED: i32 = 2;
/// The scheme the installer registers for the app's links.
const LINK_SCHEME: &str = "agentnotch";

/// `Some(exit code)` for a command the fork owns (it has run, or was refused), `None` to let the
/// app start.
pub fn run(args: &[String]) -> Option<i32> {
    let command = args.get(1)?.as_str();
    if args.iter().skip(1).any(|a| deeplink::names_scheme(a)) {
        // A link is never a command. Letting such an argv start the app is right unless it also
        // names a command: upstream's arms for those names sit in `main` right after this seam,
        // so `None` would run them (its doctor reads Claude's credentials; its installer writes
        // hooks without consent). Windows never launches the app that way (a link arrives as the
        // only argument), so it is refused, with nothing run, printed or written.
        return COMMANDS.contains(&command).then_some(REFUSED);
    }
    if !COMMANDS.contains(&command) {
        return None;
    }
    crate::attach_console();
    let rest: Vec<&str> = args.iter().skip(2).map(String::as_str).collect();
    let quiet = command == "uninstall-hooks" && rest.contains(&"--quiet");
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| match command {
        "doctor" => doctor(&rest),
        "inspect-accounts" => inspect_accounts(),
        "install-hooks" => install_hooks(),
        "uninstall-hooks" => uninstall_hooks(quiet),
        "control" => control(&rest),
        // "autostart", the last of COMMANDS.
        _ => autostart(&rest),
    }));
    // A command that panics still ends with its exit code and a line in its log, never with a
    // bare 101: the uninstaller's `uninstall-hooks --quiet` in particular must never fail.
    let (code, text) = outcome.unwrap_or_else(|_| {
        let code = if quiet || command == "doctor" { 0 } else { 1 };
        (code, format!("{command}: failed unexpectedly"))
    });
    if !quiet {
        print(&text);
    }
    write_log(command, &text);
    Some(code)
}

fn doctor(rest: &[&str]) -> (i32, String) {
    if rest.first() == Some(&"deep") {
        // Upstream's diagnostics: window, monitor and provider facts; it reads no Claude
        // credential.
        return (0, crate::diag::run());
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let extras = DoctorExtras {
        exe: exe.clone(),
        updates: super::update::doctor_line(),
        // What Windows would run for an `agentnotch:` link (HKCU\Software\Classes\agentnotch),
        // as the installer registered it.
        deep_link: deep_link_line(agentnotch_win::shell::scheme_command(LINK_SCHEME)),
        autostart: crate::autostart::is_enabled(),
        // The Start-menu shortcut Windows needs before it shows this app's notifications.
        shortcut_present: agentnotch_win::shell::start_menu_shortcut(DISPLAY_NAME).is_some(),
        providers: Vec::new(),
        // Asking a running copy (`control status`) needs the pipe client (WP1).
        running: None,
    };
    match offline_hub(&exe) {
        Ok(hub) => (0, hub.doctor_report(&extras)),
        Err(e) => (
            0,
            format!("Agent Notch doctor v{}\nerror: {e}", super::app_version()),
        ),
    }
}

/// The doctor's `deep-link:` value (§4.14).
fn deep_link_line(command: Option<String>) -> String {
    match command {
        Some(command) => format!("registered -> {command}"),
        None => "not registered".into(),
    }
}

/// One install pass now, through a hub that is never started (nothing listens, nothing is
/// watched): the pass itself is the engine's.
fn install_hooks() -> (i32, String) {
    let exe = std::env::current_exe().unwrap_or_default();
    match offline_hub(&exe) {
        // The consent is read from the hub, never assumed: it is what the user clicked.
        Ok(hub) => install_hooks_with(hub.snapshot().setup.hook_consent, || {
            super::calls::engine_call(&hub, "hooks_reinstall", serde_json::json!({}))
                .map(|_| ())
                .map_err(|e| e.message)
        }),
        Err(e) => (1, format!("install-hooks: {e}")),
    }
}

/// `install-hooks` given the consent on record: without it nothing is written (exit 2);
/// with it, `pass` runs once (0, or 1 with the engine's reason).
fn install_hooks_with(
    hook_consent: Option<bool>,
    pass: impl FnOnce() -> Result<(), String>,
) -> (i32, String) {
    if let Err(refusal) = super::hooks_change_allowed(true, hook_consent) {
        return (REFUSED, refusal);
    }
    match pass() {
        Ok(()) => (0, "Claude Code hooks are installed.".into()),
        Err(e) => (1, format!("install-hooks: {e}")),
    }
}

fn inspect_accounts() -> (i32, String) {
    match setup::roots_and_platform() {
        Ok((roots, platform)) => (0, Hub::inspect_accounts(&roots, &platform)),
        Err(e) => (1, format!("inspect-accounts: {e}")),
    }
}

fn uninstall_hooks(quiet: bool) -> (i32, String) {
    let result = setup::roots_and_platform()
        .and_then(|(roots, platform)| Hub::uninstall_hooks(&roots, &platform));
    match result {
        Ok(summary) => (0, summary),
        // `--quiet` is the uninstaller's: it must never fail an uninstall over the hooks.
        Err(e) => (if quiet { 0 } else { 1 }, format!("uninstall-hooks: {e}")),
    }
}

/// `control status|quit` asks a running copy over the hook pipe. The pipe's client side isn't
/// in this build (it comes with the bridge, WP1), so the command says so rather than guess
/// whether a copy runs; §4.14's exit codes (0, or 3 with no copy running) come with it.
fn control(rest: &[&str]) -> (i32, String) {
    match rest {
        ["status"] | ["quit"] => (
            1,
            "control: talking to a running copy isn't available in this build".into(),
        ),
        _ => (1, "usage: agentnotch.exe control status|quit".into()),
    }
}

fn autostart(rest: &[&str]) -> (i32, String) {
    let result = match rest {
        ["on"] => crate::autostart::enable(),
        ["off"] => crate::autostart::disable(),
        _ => Err("usage: agentnotch.exe autostart on|off".into()),
    };
    match result {
        Ok(message) => (0, format!("OK: {message}")),
        Err(e) => (1, format!("FAILED: {e}")),
    }
}

/// A hub that is never started: the doctor reads what it needs without running anything.
fn offline_hub(exe: &Path) -> Result<Hub, String> {
    let config = setup::hub_config(super::app_version().to_string(), exe)?;
    if super::sealed() {
        return Ok(Hub::sealed(
            config,
            std::sync::Arc::new(agentnotch_win::SystemClock),
        ));
    }
    let platform = agentnotch_win::platform(&config.roots, &config.hook_exe);
    Ok(Hub::new(config, platform))
}

fn print(text: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

/// `<data>\<command>.log`, beside upstream's config (the sealed data folder when sealed).
fn write_log(command: &str, text: &str) {
    let path: PathBuf = crate::config::config_path().with_file_name(format!("{command}.log"));
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let _ = std::fs::write(path, format!("{text}\n"));
}

#[cfg(test)]
mod tests {
    use super::COMMANDS;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn upstreams_command_names_are_claimed() {
        // Upstream's arms for these stay in main, unreachable: its doctor reads Claude's
        // credentials and its installer writes upstream's hooks. Read from main itself, so a
        // command an upstream merge adds fails here until the fork claims it.
        for name in crate::CONSOLE_CMDS {
            assert!(COMMANDS.contains(&name), "{name}");
        }
        for name in ["control", "inspect-accounts"] {
            assert!(COMMANDS.contains(&name), "{name}");
        }
    }

    #[test]
    fn upstreams_arms_never_run_beside_a_link() {
        // `None` would fall through to upstream's arm for the same name in main.
        for name in crate::CONSOLE_CMDS {
            for link in ["agentnotch://open", "AGENTNOTCH:x", "\"agentnotch://open\""] {
                assert_eq!(
                    super::run(&argv(&["agentnotch.exe", name, link])),
                    Some(super::REFUSED),
                    "{name} {link}"
                );
                assert_eq!(
                    super::run(&argv(&["agentnotch.exe", name, "--quiet", link])),
                    Some(super::REFUSED),
                    "{name} --quiet {link}"
                );
            }
        }
    }

    #[test]
    fn install_hooks_refuses_without_the_consent() {
        for consent in [None, Some(false)] {
            let (code, text) = super::install_hooks_with(consent, || {
                panic!("nothing may be written without the consent")
            });
            assert_eq!(code, super::REFUSED, "{consent:?}");
            assert_eq!(code, 2);
            assert_eq!(text, super::super::TURN_ON_FIRST);
        }
    }

    #[test]
    fn install_hooks_runs_one_pass_with_the_consent() {
        let mut passes = 0;
        let (code, _) = super::install_hooks_with(Some(true), || {
            passes += 1;
            Ok(())
        });
        assert_eq!((code, passes), (0, 1));
        let (code, text) =
            super::install_hooks_with(Some(true), || Err("settings.json doesn't parse".into()));
        assert_eq!(code, 1);
        assert_eq!(text, "install-hooks: settings.json doesn't parse");
    }

    #[test]
    fn control_is_claimed_and_says_it_cannot_ask_yet() {
        // Never 0: a script must not read "no answer" as "nothing is running".
        for rest in [&["status"][..], &["quit"], &[], &["status", "now"]] {
            assert_eq!(super::control(rest).0, 1, "{rest:?}");
        }
    }

    #[test]
    fn the_doctor_says_what_a_link_would_run() {
        assert_eq!(super::deep_link_line(None), "not registered");
        assert_eq!(
            super::deep_link_line(Some(r#""C:\Apps\agentnotch.exe" "%1""#.into())),
            r#"registered -> "C:\Apps\agentnotch.exe" "%1""#
        );
    }

    #[test]
    fn the_app_starts_for_anything_else() {
        assert_eq!(super::run(&argv(&["agentnotch.exe"])), None);
        assert_eq!(super::run(&argv(&["agentnotch.exe", "--silent"])), None);
        assert_eq!(super::run(&argv(&["agentnotch.exe", "settings"])), None);
    }

    #[test]
    fn a_deep_link_is_never_a_command() {
        // A link (and whatever follows it) starts the app; the single-instance plugin hands it
        // to a running copy.
        assert_eq!(
            super::run(&argv(&[
                "agentnotch.exe",
                "agentnotch://auth-callback?code=x"
            ])),
            None
        );
        assert_eq!(
            super::run(&argv(&["agentnotch.exe", "agentnotch://open", "doctor"])),
            None
        );
        // Beside a command name, nothing runs at all.
        assert_eq!(
            super::run(&argv(&["agentnotch.exe", "doctor", "agentnotch://open"])),
            Some(super::REFUSED)
        );
        assert_eq!(
            super::run(&argv(&[
                "agentnotch.exe",
                "control",
                "status",
                "agentnotch:x"
            ])),
            Some(super::REFUSED)
        );
        // Not a command: the app starts, and the link is ignored (not the only argument).
        assert_eq!(
            super::run(&argv(&["agentnotch.exe", "--silent", "agentnotch://open"])),
            None
        );
    }
}
