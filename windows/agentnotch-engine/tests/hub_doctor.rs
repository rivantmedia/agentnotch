//! The doctor (design §4.14) and `control status`: the report line by line
//! from fixed facts (a golden), through a hub that was never started over a
//! throwaway home (read-only: no thread, no file, no child), never a token's
//! name, and the control status counts.

mod accounts_support;
mod hub_support;

use accounts_support::{Home, BIIOS, BIIOS_UUID, PARAS, PARAS_UUID};
use agentnotch_engine::hub::doctor::{render, AccountFact, DoctorFacts};
use agentnotch_engine::hub::runtime::RuntimeOptions;
use agentnotch_engine::hub::{Call, DoctorExtras};
use agentnotch_engine::testkit::snapshot_dir;
use agentnotch_proto::ControlStatus;
use hub_support::live::TestHub;
use std::path::PathBuf;
use std::time::Duration;

fn facts() -> DoctorFacts {
    DoctorFacts {
        version: "1.1.0".into(),
        exe: PathBuf::from(r"C:\Users\me\AppData\Local\Agent Notch\agentnotch.exe"),
        sealed: false,
        elevated_app: false,
        elevated_running: None,
        sessions_elevated: 0,
        smart_app_control: "off".into(),
        data: PathBuf::from(r"C:\Users\me\AppData\Roaming\Agent Notch"),
        support: PathBuf::from(r"C:\Users\me\AppData\Local\com.rivantmedia.agentnotch\Claude"),
        support_private: Some(true),
        updates: "updates: off (built from source)".into(),
        pipe_name: r"\\.\pipe\agentnotch-hook-S-1-5-21-1".into(),
        running: None,
        hook_exe: PathBuf::from(r"C:\Users\me\AppData\Local\Agent Notch\agentnotch-hook.exe"),
        hook_exe_present: true,
        accounts: vec![
            AccountFact {
                ring_id: "claude-acct-1a2b3c4d5e6f".into(),
                label: "Claude Work".into(),
                signed_in: true,
                folders: vec![r"~\.claude".into()],
            },
            AccountFact {
                ring_id: "claude-acct-6f5e4d3c2b1a".into(),
                label: "Personal".into(),
                signed_in: false,
                folders: vec![],
            },
        ],
        consent: "unasked".into(),
        hooks_installed: Some(1),
        hook_targets: Some(2),
        form: "exec".into(),
        exec_form_min: Some("2.1.101".into()),
        versions: vec![
            "2.1.282 (binary)".into(),
            "2.1.270 (vscode extension)".into(),
            "unknown (desktop)".into(),
        ],
        status_lines_wrapped: 1,
        status_lines_left_alone: 1,
        left_alone_reason: Some("Status line left alone: its command uses Windows paths".into()),
        claude: Some(PathBuf::from(r"C:\Users\me\.local\bin\claude.exe")),
        desktop_cache: "simple".into(),
        deep_link: r#"registered -> "C:\Users\me\AppData\Local\Agent Notch\agentnotch.exe" "%1""#
            .into(),
        autostart: false,
        shortcut_present: true,
        providers: vec!["codex: ok".into(), "cursor: no data".into()],
    }
}

#[test]
fn the_report_is_the_golden_text() {
    let golden = include_str!("hub_support/doctor-golden.txt");
    assert_eq!(render(&facts()), golden);
}

#[test]
fn a_running_instance_and_the_other_states_change_their_lines() {
    let mut facts = facts();
    facts.running = Some((3, 2));
    facts.elevated_running = Some(true);
    facts.sessions_elevated = 1;
    facts.smart_app_control = "on".into();
    facts.support_private = None;
    facts.hook_exe_present = false;
    facts.updates =
        "updates: on feed=https://x/latest.json key=B5A5638361FBD019 signed-version=required"
            .into();
    facts.claude = None;
    facts.versions.clear();
    facts.form = "none".into();
    facts.exec_form_min = None;
    facts.shortcut_present = false;
    facts.autostart = true;
    facts.accounts[0].label = "Two\nlines".into();
    let text = render(&facts);
    let line = |prefix: &str| {
        text.lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix} line in {text}"))
            .to_owned()
    };
    assert!(line("pipe:").ends_with("running (sessions 3, accounts 2)"));
    assert_eq!(
        line("elevated:"),
        "elevated: app=no running=yes sessions-elevated=1"
    );
    assert!(line("smart-app-control:").starts_with("smart-app-control: on"));
    assert!(line("support:").ends_with("(private: not made yet)"));
    assert!(line("hook exe:").ends_with("(missing)"));
    assert!(line("updates:").contains("signed-version=required"));
    assert_eq!(line("claude:"), "claude: not found");
    assert!(line("claude-versions:").contains("none known"));
    assert_eq!(
        line("hooks:"),
        "hooks: consent=unasked installed=1/2 form=none exec-form-min=unset"
    );
    assert_eq!(line("autostart:"), "autostart: on");
    assert_eq!(line("notifications:"), "notifications: shortcut missing");
    // A label can't break a line.
    assert!(line("account: claude-acct-1a2b3c4d5e6f").contains("\"Two lines\""));
}

// ---- through a hub ----

fn base_of(home: &Home) -> PathBuf {
    home.roots
        .home
        .parent()
        .expect("the test's root")
        .to_path_buf()
}

fn two_accounts(home: &Home) {
    home.write(".claude/sessions/1.json", "{}");
    home.write_json(".claude.json", &home.login(PARAS_UUID, PARAS, None));
    home.write(".claude-work/sessions/1.json", "{}");
    home.write_json(
        ".claude-work/.claude.json",
        &home.login(BIIOS_UUID, BIIOS, None),
    );
    home.write(".local/bin/claude.exe", "");
}

fn extras() -> DoctorExtras {
    DoctorExtras {
        exe: PathBuf::from("agentnotch.exe"),
        updates: "updates: off (built from source)".into(),
        deep_link: "registered".into(),
        autostart: false,
        shortcut_present: true,
        providers: vec!["codex: ok".into()],
        running: None,
    }
}

fn hub_over(home: &Home) -> TestHub {
    TestHub::over(&base_of(home), RuntimeOptions::default(), |_| {}, |_| {})
}

/// What the smoke test's phase 3 greps for.
#[test]
fn an_unstarted_hub_reports_every_line_the_smoke_test_checks_and_changes_nothing() {
    let home = Home::new();
    two_accounts(&home);
    // A credentials file whose names the report must never carry (spelled in
    // pieces: no source file of the fork spells them out).
    let (file, key, field) = (
        [".cred", "entials.json"].concat(),
        ["claude", "AiOauth"].concat(),
        ["access", "Token"].concat(),
    );
    home.write(
        &format!(".claude/{file}"),
        &format!(r#"{{"{key}":{{"{field}":"sk-ant-oat01-secret"}}}}"#),
    );
    let before = snapshot_dir(&home.roots.home).unwrap();
    let hub = hub_over(&home);
    let report = hub.hub.doctor_report(&extras());

    for prefix in [
        "Agent Notch doctor v1.1.0 (com.rivantmedia.agentnotch)",
        "exe: ",
        "sealed: no",
        "elevated: app=no running=none sessions-elevated=0",
        "smart-app-control: unknown",
        "data: ",
        "support: ",
        "updates: off (built from source)",
        "pipe: \\\\.\\pipe\\agentnotch-hook-test no instance running",
        "hook exe: ",
        "accounts: 2",
        "hooks: consent=unasked installed=0/2 form=none exec-form-min=",
        "claude-versions: ",
        "status-line: wrapped=0 left-alone=0",
        "claude: ",
        "desktop-cache: absent",
        "deep-link: registered",
        "autostart: off",
        "notifications: shortcut present (AUMID com.rivantmedia.agentnotch)",
        "providers: codex: ok",
    ] {
        assert!(
            report.lines().any(|line| line.starts_with(prefix)),
            "no `{prefix}` line in\n{report}"
        );
    }
    assert_eq!(
        report
            .lines()
            .filter(|l| l.starts_with("account: "))
            .count(),
        2
    );
    assert!(
        report
            .lines()
            .any(|l| l.starts_with("claude: ") && l.contains("claude.exe")),
        "{report}"
    );
    for name in [file.as_str(), key.as_str(), field.as_str(), "sk-ant"] {
        assert!(!report.contains(name), "{name} in\n{report}");
    }

    // Read-only: nothing written in the home or the support folder, nothing
    // run, no thread (a never-started hub has no events).
    assert_eq!(snapshot_dir(&home.roots.home).unwrap(), before);
    assert!(std::fs::metadata(&home.roots.support).is_err());
    assert!(hub.handles.runner.spawned().is_empty());
    assert!(hub.files.writes_of("control-settings.json").is_empty());
}

#[test]
fn a_running_instance_is_reported_on_the_pipe_line() {
    let home = Home::new();
    two_accounts(&home);
    let hub = hub_over(&home);
    let mut extra = extras();
    extra.running = Some(ControlStatus {
        sessions: 3,
        accounts: 2,
        elevated: false,
        ..ControlStatus::default()
    });
    let report = hub.hub.doctor_report(&extra);
    assert!(
        report.contains("agentnotch-hook-test running (sessions 3, accounts 2)"),
        "{report}"
    );
    assert!(report.contains("elevated: app=no running=no"), "{report}");
}

#[test]
fn consent_and_the_installed_hooks_show_in_the_report() {
    let home = Home::new();
    two_accounts(&home);
    let install = home.roots.install_dir.clone().expect("an install folder");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("agentnotch-hook.exe"), b"MZ not really an exe").unwrap();
    let hub = hub_over(&home);
    hub.hub
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    // As `install-hooks` does on a hub that never started.
    hub.hub
        .call(Call::HooksReinstall { account_id: None })
        .expect("a pass");
    let report = hub.hub.doctor_report(&extras());
    let hooks = report
        .lines()
        .find(|l| l.starts_with("hooks: "))
        .expect("a hooks line");
    assert!(
        hooks.starts_with("hooks: consent=granted installed=2/2 form="),
        "{hooks}"
    );
    assert!(!hooks.contains("form=none"), "{hooks}");
    assert!(
        report.contains("hook exe: ") && report.contains("(present)"),
        "{report}"
    );
}

#[test]
fn a_sealed_hub_reports_through_the_same_lines() {
    let sealed = agentnotch_engine::hub::Hub::sealed(
        hub_support::live::config(&Home::new().roots),
        std::sync::Arc::new(agentnotch_engine::testkit::FakeClock::at_ms(
            1_800_000_000_000,
        )),
    );
    let report = sealed.doctor_report(&extras());
    assert!(report.contains("\nsealed: yes\n"), "{report}");
    assert!(report.contains("sealed (no server)"), "{report}");
    assert!(
        report.contains("claude: not looked for (sealed)"),
        "{report}"
    );
    assert!(report.contains("hooks: consent="), "{report}");
    assert!(!report.contains(["access", "Token"].concat().as_str()));
}

// ---- control status ----

#[test]
fn control_status_counts_what_the_pages_are_shown() {
    let home = Home::new();
    two_accounts(&home);
    let hub = hub_over(&home);
    hub.hub.start().expect("the hub starts");
    let status = |hub: &TestHub| {
        hub.handles.clock.advance(Duration::from_millis(100));
        hub.sync();
        hub.sync();
        hub.hub.control_status()
    };
    let first = status(&hub);
    assert_eq!(first.version, "1.1.0");
    assert!(!first.sealed);
    assert_eq!((first.accounts, first.rings), (2, 2));
    assert_eq!((first.readings, first.sessions, first.held), (0, 0, 0));
    assert_eq!(first.hook_consent, "unasked");
    assert_eq!(first.transport, "listening");
    assert_eq!(first.cloud, "signed_out");
    assert!(!first.sync);

    hub.hub
        .call(Call::HookConsent { grant: false })
        .expect("not now");
    assert_eq!(status(&hub).hook_consent, "declined");
    hub.hub
        .call(Call::HookConsent { grant: true })
        .expect("turn on");
    assert_eq!(status(&hub).hook_consent, "granted");
}
