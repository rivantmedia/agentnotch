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
//! whatever else argv holds.

use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use agentnotch_engine::hub::{DoctorExtras, Hub};

use super::{deeplink, setup};

/// Every command this module answers for.
const COMMANDS: [&str; 6] = [
    "doctor",
    "inspect-accounts",
    "install-hooks",
    "uninstall-hooks",
    "control",
    "autostart",
];

/// `Some(exit code)` for a command the fork owns (it has run), `None` to let the app start.
pub fn run(args: &[String]) -> Option<i32> {
    if args.iter().skip(1).any(|a| deeplink::names_scheme(a)) {
        return None;
    }
    let command = args.get(1)?.as_str();
    if !COMMANDS.contains(&command) {
        return None;
    }
    crate::attach_console();
    let rest: Vec<&str> = args.iter().skip(2).map(String::as_str).collect();
    let quiet = command == "uninstall-hooks" && rest.contains(&"--quiet");
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| match command {
        "doctor" => doctor(&rest),
        "inspect-accounts" => inspect_accounts(),
        "install-hooks" => (
            1,
            "Installing hooks from the command line isn't available in this build. \
             Turn on Claude Code control in Settings › Claude Code."
                .to_string(),
        ),
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
        // The registration check (HKCU\Software\Classes\agentnotch) and the Start-menu shortcut
        // check come with the app's Windows plumbing (WP9); until then they say "unknown".
        deep_link: "unknown".into(),
        autostart: crate::autostart::is_enabled(),
        shortcut_present: false,
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
        // credentials and its installer writes upstream's hooks.
        for name in [
            "doctor",
            "install-hooks",
            "uninstall-hooks",
            "autostart",
            "control",
            "inspect-accounts",
        ] {
            assert!(COMMANDS.contains(&name), "{name}");
        }
    }

    #[test]
    fn the_app_starts_for_anything_else() {
        assert_eq!(super::run(&argv(&["agentnotch.exe"])), None);
        assert_eq!(super::run(&argv(&["agentnotch.exe", "--silent"])), None);
        assert_eq!(super::run(&argv(&["agentnotch.exe", "settings"])), None);
    }

    #[test]
    fn a_deep_link_is_never_a_command() {
        assert_eq!(
            super::run(&argv(&[
                "agentnotch.exe",
                "agentnotch://auth-callback?code=x"
            ])),
            None
        );
        assert_eq!(
            super::run(&argv(&["agentnotch.exe", "doctor", "agentnotch://open"])),
            None
        );
        assert_eq!(
            super::run(&argv(&[
                "agentnotch.exe",
                "uninstall-hooks",
                "AGENTNOTCH:x"
            ])),
            None
        );
    }
}
