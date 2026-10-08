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
//!
//! Sealed (`AGENTNOTCH_SAFE_MODE`), the commands that would read or write the real Claude
//! folders (`inspect-accounts`, `install-hooks`, `uninstall-hooks`) do neither and say so; the
//! doctor asks the sealed hub.

use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use std::time::Duration;

use agentnotch_engine::hub::{DoctorExtras, Hub};
use agentnotch_win::pipe_server::client::{self, ClientError};
use agentnotch_win::proto::{ControlOp, ControlResponse, ControlStatus};

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
/// and a link at once, `install-hooks` without the consent, a Claude folder's command sealed.
const REFUSED: i32 = 2;
/// `control status|quit` (and nothing else) when no copy of the app is running.
const NO_INSTANCE: i32 = 3;
/// How long the doctor waits for a running copy's status: it must not hang a support request.
const STATUS_WAIT: Duration = Duration::from_secs(2);
/// How long `control` waits: `quit` answers before the app stops, so this is only the pipe.
const CONTROL_WAIT: Duration = Duration::from_secs(5);
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
    let sealed = super::sealed();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| match command {
        _ if sealed && touches_claude_folders(command) => sealed_answer(command),
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

/// The commands that read or write the real Claude folders (through the real platform, which
/// a sealed run never builds: DESIGN-WIN §4.13).
fn touches_claude_folders(command: &str) -> bool {
    matches!(
        command,
        "inspect-accounts" | "install-hooks" | "uninstall-hooks"
    )
}

/// What one of those answers in a sealed run, with nothing read or written. A sealed run never
/// installs hooks, so there are none of its own to remove: `uninstall-hooks` is done (0, which
/// the uninstaller's `--quiet` needs anyway); the other two are refused.
fn sealed_answer(command: &str) -> (i32, String) {
    match command {
        "uninstall-hooks" => (
            0,
            "Sealed: nothing was removed (a sealed run touches no Claude folder).".into(),
        ),
        "install-hooks" => (
            REFUSED,
            "Sealed: a sealed run installs no hooks (it touches no Claude folder).".into(),
        ),
        _ => (
            REFUSED,
            format!("Sealed: {command} reads the Claude folders, which a sealed run never does."),
        ),
    }
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
        // A short `control status` call; a sealed run never talks to a real copy.
        running: if super::sealed() {
            None
        } else {
            let pipe = pipe_name_for(&exe);
            running_status(|op| client::control(&pipe, op, STATUS_WAIT))
        },
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

/// A running copy's status for the doctor's `pipe:` line: only what a good answer carries, `None`
/// for anything else (no copy, a stranger's pipe, a timeout, an answer that isn't a status).
fn running_status(
    ask: impl FnOnce(ControlOp) -> Result<ControlResponse, ClientError>,
) -> Option<ControlStatus> {
    ask(ControlOp::Status).ok().and_then(|answer| answer.status)
}

/// The hook pipe this process talks to (empty when the user's SID can't be read: the client then
/// refuses the name).
fn pipe_name_for(exe: &Path) -> String {
    setup::hub_config(super::app_version().to_string(), exe)
        .map(|config| config.pipe_name)
        .unwrap_or_default()
}

/// `control status|quit` asks a running copy over the hook pipe: 0 and the answer, 3 when no copy
/// runs, 1 for anything else (a usage slip, a stranger's pipe, a timeout, a refusal).
fn control(rest: &[&str]) -> (i32, String) {
    let op = match rest {
        ["status"] => ControlOp::Status,
        ["quit"] => ControlOp::Quit,
        _ => return (1, "usage: agentnotch.exe control status|quit".into()),
    };
    let exe = std::env::current_exe().unwrap_or_default();
    let pipe = pipe_name_for(&exe);
    control_with(op, |op| client::control(&pipe, op, CONTROL_WAIT))
}

/// [`control`] over whatever asks the pipe, so the exit codes are tested with a fake client.
fn control_with(
    op: ControlOp,
    ask: impl FnOnce(ControlOp) -> Result<ControlResponse, ClientError>,
) -> (i32, String) {
    match ask(op) {
        Ok(answer) if !answer.ok => (
            1,
            format!(
                "control: {}",
                answer.error.unwrap_or_else(|| "refused".into())
            ),
        ),
        Ok(answer) => match (op, answer.status) {
            (ControlOp::Quit, _) => (0, "Agent Notch is quitting.".into()),
            (ControlOp::Status, Some(status)) => (0, status_text(&status)),
            (ControlOp::Status, None) => (1, "control: the answer carried no status".into()),
        },
        Err(ClientError::NotRunning) => (NO_INSTANCE, "no instance running".into()),
        Err(e) => (1, format!("control: {e}")),
    }
}

/// `control status`, one `name: value` line each: counts and states only (the smoke test reads
/// `transport:`, `accounts:`, `readings:`, `hook_consent:` and `cloud:`).
fn status_text(status: &ControlStatus) -> String {
    [
        format!("version: {}", status.version),
        format!("sealed: {}", status.sealed),
        format!("elevated: {}", status.elevated),
        format!("accounts: {}", status.accounts),
        format!("rings: {}", status.rings),
        format!("readings: {}", status.readings),
        format!("sessions: {}", status.sessions),
        format!("held: {}", status.held),
        format!("hook_consent: {}", status.hook_consent),
        format!("transport: {}", status.transport),
        format!("cloud: {}", status.cloud),
        format!("sync: {}", status.sync),
    ]
    .join("\n")
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
    use agentnotch_win::pipe_server::client::ClientError;
    use agentnotch_win::proto::{ControlOp, ControlResponse, ControlStatus};

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

    // DESIGN-WIN §4.13: a sealed run reads and writes no Claude folder, the command line included.
    #[test]
    fn a_sealed_run_never_reaches_the_claude_folders_from_the_command_line() {
        for command in ["inspect-accounts", "install-hooks", "uninstall-hooks"] {
            assert!(super::touches_claude_folders(command), "{command}");
            let (_, text) = super::sealed_answer(command);
            assert!(text.starts_with("Sealed: "), "{text}");
        }
        // The doctor asks the sealed hub; control and autostart are no Claude folder's.
        for command in ["doctor", "control", "autostart"] {
            assert!(!super::touches_claude_folders(command), "{command}");
        }
        for command in COMMANDS {
            assert!(
                super::touches_claude_folders(command)
                    || ["doctor", "control", "autostart"].contains(&command),
                "{command} must say whether it reaches the Claude folders"
            );
        }
        // Nothing installed, nothing to remove: done, as the uninstaller's --quiet needs.
        assert_eq!(super::sealed_answer("uninstall-hooks").0, 0);
        // Never "installed" or an account list made up: refused, with nothing done.
        assert_eq!(super::sealed_answer("install-hooks").0, super::REFUSED);
        assert_eq!(super::sealed_answer("inspect-accounts").0, super::REFUSED);
    }

    fn status() -> ControlStatus {
        ControlStatus {
            version: "1.1.0".into(),
            accounts: 2,
            rings: 2,
            readings: 2,
            hook_consent: "unasked".into(),
            transport: "listening".into(),
            cloud: "signed_out".into(),
            ..ControlStatus::default()
        }
    }

    #[test]
    fn control_status_prints_the_answer_and_exits_0() {
        let (code, text) = super::control_with(ControlOp::Status, |op| {
            assert_eq!(op, ControlOp::Status);
            Ok(ControlResponse::status(status()))
        });
        assert_eq!(code, 0);
        for line in [
            "transport: listening",
            "accounts: 2",
            "readings: 2",
            "hook_consent: unasked",
            "cloud: signed_out",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} in\n{text}");
        }
    }

    #[test]
    fn control_quit_exits_0_once_the_app_took_it() {
        let (code, _) = super::control_with(ControlOp::Quit, |op| {
            assert_eq!(op, ControlOp::Quit);
            Ok(ControlResponse::ok())
        });
        assert_eq!(code, 0);
    }

    #[test]
    fn control_exits_3_when_no_instance_runs() {
        for op in [ControlOp::Status, ControlOp::Quit] {
            let (code, text) = super::control_with(op, |_| Err(ClientError::NotRunning));
            assert_eq!((code, text.as_str()), (3, "no instance running"));
        }
    }

    #[test]
    fn control_never_reads_another_failure_as_not_running() {
        // A script must not take a stranger's pipe, a busy or silent app, or a refusal for
        // "nothing is running".
        for failure in [
            ClientError::NotOurs,
            ClientError::Busy,
            ClientError::Timeout,
            ClientError::NotAvailable,
            ClientError::Other("closed".into()),
        ] {
            let (code, _) = super::control_with(ControlOp::Status, |_| Err(failure.clone()));
            assert_eq!(code, 1, "{failure:?}");
        }
        let (code, text) = super::control_with(ControlOp::Status, |_| {
            Ok(ControlResponse::error("the hub is stopping"))
        });
        assert_eq!((code, text.as_str()), (1, "control: the hub is stopping"));
        let (code, _) = super::control_with(ControlOp::Status, |_| Ok(ControlResponse::ok()));
        assert_eq!(code, 1);
    }

    #[test]
    fn control_wants_status_or_quit() {
        for rest in [&[][..], &["now"], &["status", "now"]] {
            assert_eq!(super::control(rest).0, 1, "{rest:?}");
        }
    }

    #[test]
    fn the_doctors_running_line_comes_from_a_good_status_only() {
        let running = super::running_status(|_| Ok(ControlResponse::status(status())));
        assert_eq!(running.map(|s| s.accounts), Some(2));
        for answer in [
            Err(ClientError::NotRunning),
            Err(ClientError::Timeout),
            Ok(ControlResponse::ok()),
            Ok(ControlResponse::error("no")),
        ] {
            assert_eq!(super::running_status(|_| answer), None);
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
