//! The commands the installer writes and how they are read back: command
//! forms, the exec-form gate, the recogniser, the status line takeover rule
//! and the loop guard. Ports of the Mac's `HookCommandsTests`,
//! `HookInstallerTests` (versionGating, effectiveVersionIsTheLowestKnown,
//! scriptPathOfOurCommands, thirdPartyHooksMentioningAScriptAreNotRemoved) and
//! `StatusLineScriptTests.neverChainsToAWrapper`, with Windows' spellings.
//!
//! Every path here is a string and every recogniser is built with
//! `PathStyle::Windows`, so the vectors run the same on any OS.

use agentnotch_engine::core::paths::PathStyle;
use agentnotch_engine::core::settings_doc::Json;
use agentnotch_engine::hooks::commands::{
    carries_unquoted, display_command, effective_version, exec_form, exec_form_allowed, form_name,
    git_bash_candidates, hook_copy_path, is_upstream_hook, not_hookable_reason, string_command,
    string_command_for, takeover, Recognised, Recogniser, Subcommand, Takeover, HOOK_EXE_NAME,
    STATUS_LINE_NOT_AVAILABLE,
};
use agentnotch_engine::hooks::events::hook_events;
use agentnotch_engine::hooks::facts::ClaudeCodeFacts;
use agentnotch_engine::hooks::shell_words;
use agentnotch_engine::hooks::version::ClaudeCodeVersion;
use agentnotch_engine::runtime_types::{CommandForm, VersionSighting, VersionSource};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn json(text: &str) -> Json {
    Json::parse(text.as_bytes()).expect("valid JSON")
}

fn windows() -> Recogniser<'static> {
    Recogniser::new(PathStyle::Windows)
}

fn version(major: u32, minor: u32, patch: u32) -> ClaudeCodeVersion {
    ClaudeCodeVersion::new(major, minor, patch)
}

// ---- Command forms ----

const HOOK: &str = "C:/Users/me/.claude/hooks/agentnotch-hook.exe";

/// `theCommandWeWrite`: the path unquoted with forward slashes, no guard.
#[test]
fn the_string_command_is_the_unquoted_path() {
    let no_short = |_: &Path| -> Option<PathBuf> { panic!("no short path is needed") };
    assert_eq!(
        string_command_for(Path::new(HOOK), Subcommand::Hook, &no_short).as_deref(),
        Some("C:/Users/me/.claude/hooks/agentnotch-hook.exe hook")
    );
    assert_eq!(
        string_command_for(Path::new(HOOK), Subcommand::StatusLine, &no_short).as_deref(),
        Some("C:/Users/me/.claude/hooks/agentnotch-hook.exe statusline")
    );
    // Backslashes become forward slashes.
    assert_eq!(
        string_command(
            r"C:\Users\me\.claude\hooks\agentnotch-hook.exe",
            Subcommand::Hook
        )
        .as_deref(),
        Some("C:/Users/me/.claude/hooks/agentnotch-hook.exe hook")
    );
    // Whatever it writes, the recogniser reads back (`runs(command(...))`).
    let command = string_command(HOOK, Subcommand::Hook).unwrap();
    assert!(windows().command(&command).is_some());
}

#[test]
fn hook_copy_lives_in_hooks_of_the_config_folder() {
    let copy = hook_copy_path(Path::new("C:/Users/me/.claude"));
    assert_eq!(copy.file_name().unwrap(), HOOK_EXE_NAME);
    assert_eq!(copy.parent().unwrap().file_name().unwrap(), "hooks");
    assert_eq!(HOOK_EXE_NAME, "agentnotch-hook.exe");
}

/// A space in the user name: the 8.3 short name of the config folder
/// carries it, through the closure (`GetShortPathNameW` on Windows).
#[test]
fn a_path_with_a_space_uses_its_8_3_short_name() {
    let asked = RefCell::new(Vec::new());
    let short = |path: &Path| -> Option<PathBuf> {
        asked.borrow_mut().push(path.to_path_buf());
        Some(PathBuf::from(r"C:\Users\JOHNSM~1\.claude"))
    };
    let exe = Path::new("C:/Users/John Smith/.claude/hooks/agentnotch-hook.exe");
    assert_eq!(
        string_command_for(exe, Subcommand::Hook, &short).as_deref(),
        Some("C:/Users/JOHNSM~1/.claude/hooks/agentnotch-hook.exe hook")
    );
    assert_eq!(
        string_command_for(exe, Subcommand::StatusLine, &short).as_deref(),
        Some("C:/Users/JOHNSM~1/.claude/hooks/agentnotch-hook.exe statusline")
    );
    // Asked about the config folder, which exists before anything is installed.
    assert_eq!(asked.borrow()[0], Path::new("C:/Users/John Smith/.claude"));

    // A trailing separator on the short name makes no double slash.
    let trailing = |_: &Path| Some(PathBuf::from(r"D:\CLAUDE~1\"));
    assert_eq!(
        string_command_for(
            Path::new("D:/Claude Data/hooks/agentnotch-hook.exe"),
            Subcommand::Hook,
            &trailing
        )
        .as_deref(),
        Some("D:/CLAUDE~1/hooks/agentnotch-hook.exe hook")
    );
}

/// 8.3 names may be disabled on the volume: then there is no string form.
#[test]
fn no_short_name_means_no_string_command() {
    let exe = Path::new("C:/Users/John Smith/.claude/hooks/agentnotch-hook.exe");
    let none = |_: &Path| -> Option<PathBuf> { None };
    assert_eq!(string_command_for(exe, Subcommand::Hook, &none), None);
    assert_eq!(string_command_for(exe, Subcommand::StatusLine, &none), None);
    // A short name that still carries a space (or worse) does not pass the test.
    let still_spaced = |_: &Path| Some(PathBuf::from(r"C:\Users\John Smith\.claude"));
    assert_eq!(
        string_command_for(exe, Subcommand::Hook, &still_spaced),
        None
    );
    let shell_meaning = |_: &Path| Some(PathBuf::from(r"C:\Users\A&B\.claude"));
    assert_eq!(
        string_command_for(exe, Subcommand::Hook, &shell_meaning),
        None
    );
}

#[test]
fn the_exact_texts_shown_when_a_folder_cannot_be_hooked() {
    assert_eq!(
        STATUS_LINE_NOT_AVAILABLE,
        "Live status line isn't available for this folder"
    );
    // Exec form not established: the characters are the only reason.
    assert_eq!(
        not_hookable_reason(&ClaudeCodeFacts::default()),
        "Can't be hooked here: its path has characters a hook command can't carry"
    );
    let facts = ClaudeCodeFacts {
        exec_form_min: Some("2.1.150".into()),
        versions: Vec::new(),
    };
    assert_eq!(
        not_hookable_reason(&facts),
        "Can't be hooked here: its path needs Claude Code 2.1.150 or later everywhere on this PC"
    );
}

/// `quotesAnything`, Windows' way: a command is never quoted, so any path
/// that would need quotes is refused instead.
#[test]
fn only_paths_no_shell_gives_a_meaning_to_go_unquoted() {
    for ok in [
        "C:/Users/me/.claude/hooks/agentnotch-hook.exe",
        "C:/Users/my_name-2/.claude-work/x",
        "/c/Users/me/x",
        // Non-ASCII letters and digits of any script are plain in both shells.
        "C:/Users/José/.claude/x",
        "C:/Users/佐藤/.claude/x",
        "C:/Users/Åsa/x",
        "C:/Users/١٢٣/x",
        // `~` mid-word is literal in both shells (8.3 names have one).
        "C:/Users/JOHNSM~1/.claude/x",
        "a~",
        "x~y",
    ] {
        assert!(carries_unquoted(ok), "{ok}");
    }
    for bad in [
        "",
        "C:/Users/John Smith/x",
        "C:/Users/it's/x",
        r#"C:/Users/a"b/x"#,
        "C:/Users/$HOME/x",
        "C:/Users/`x`/x",
        "C:/Users/(x)/x",
        "C:/Users/a&b/x",
        "C:/Users/a;b/x",
        "C:/Users/a|b/x",
        "C:/Users/a#b/x",
        "C:/Users/a%b%/x",
        "C:/Users/a!b/x",
        "C:/Users/a,b/x",
        "C:/Users/a[b]/x",
        "C:/Users/a{b}/x",
        "C:/Users/a*b/x",
        "C:/Users/a=b/x",
        "C:/Users/tab\tx",
        "C:/Users/new\nline",
        // `~` first is a home directory in bash and a path in PowerShell.
        "~/x",
        "~",
    ] {
        assert!(!carries_unquoted(bad), "{bad:?}");
    }
    for value in [
        "plain",
        "with space",
        "it's",
        "$HOME `x` $(y) \"z\"",
        "new\nline",
        "",
    ] {
        if !carries_unquoted(value) {
            assert_eq!(string_command(value, Subcommand::Hook), None, "{value:?}");
        }
    }
    assert_eq!(string_command("C:/Users/a b/x.exe", Subcommand::Hook), None);
}

#[test]
fn a_form_is_shown_and_named() {
    let text = CommandForm::Text("C:/x/agentnotch-hook.exe hook".into());
    assert_eq!(
        display_command(&text).as_deref(),
        Some("C:/x/agentnotch-hook.exe hook")
    );
    assert_eq!(form_name(&text), Some("string"));
    let exec = exec_form(Path::new(r"C:\x\agentnotch-hook.exe"));
    assert_eq!(
        display_command(&exec).as_deref(),
        Some(r"C:\x\agentnotch-hook.exe hook --exec")
    );
    assert_eq!(form_name(&exec), Some("exec"));
    let no = CommandForm::NotPossible("why".into());
    assert_eq!((display_command(&no), form_name(&no)), (None, None));
    assert_eq!(Subcommand::Hook.arg(), "hook");
    assert_eq!(Subcommand::StatusLine.arg(), "statusline");
}

// ---- Versions and the exec-form gate ----

fn sighting(source: VersionSource, version: Option<&str>) -> VersionSighting {
    VersionSighting {
        source,
        path: None,
        version: version.map(str::to_owned),
    }
}

fn facts_with_min(min: Option<&str>) -> ClaudeCodeFacts {
    ClaudeCodeFacts {
        exec_form_min: min.map(str::to_owned),
        versions: Vec::new(),
    }
}

/// The exec form is only written once every Claude Code on the PC is known
/// to run it.
#[test]
fn exec_form_needs_every_sighting_known_and_new_enough() {
    let facts = facts_with_min(Some("2.1.150"));
    let allowed = |sightings: &[VersionSighting]| exec_form_allowed(sightings, &facts);
    let binary = |v: Option<&str>| sighting(VersionSource::Binary, v);

    // All at or above the minimum.
    assert!(allowed(&[binary(Some("2.1.150"))]));
    assert!(allowed(&[
        binary(Some("2.1.280")),
        sighting(VersionSource::StatusLine, Some("2.2.0")),
        sighting(VersionSource::BundledVsCode, Some("3.0.1")),
    ]));
    // Text around the number is fine (`claude --version` prints one).
    assert!(allowed(&[binary(Some("2.1.200 (Claude Code)"))]));
    // A bundled copy whose version can't be told keeps everyone on string form.
    assert!(!allowed(&[
        binary(Some("2.1.280")),
        sighting(VersionSource::BundledDesktop, None),
    ]));
    assert!(!allowed(&[binary(None)]));
    assert!(!allowed(&[binary(Some("no version here"))]));
    // One below the minimum.
    assert!(!allowed(&[
        binary(Some("2.1.280")),
        binary(Some("2.1.149"))
    ]));
    assert!(!allowed(&[sighting(
        VersionSource::Registry,
        Some("2.0.50")
    )]));
    // Nothing seen: nothing established.
    assert!(!allowed(&[]));
}

/// While the facts file establishes no minimum, exec form is never written.
#[test]
fn exec_form_is_never_written_without_a_minimum() {
    let facts = facts_with_min(None);
    assert!(!exec_form_allowed(
        &[sighting(VersionSource::Binary, Some("9.9.9"))],
        &facts
    ));
    assert!(!exec_form_allowed(&[], &facts));
    assert!(!exec_form_allowed(
        &[sighting(VersionSource::Binary, Some("9.9.9"))],
        &ClaudeCodeFacts::compiled_in()
    ));
}

/// `effectiveVersionIsTheLowestKnown`
#[test]
fn the_effective_version_is_the_lowest_known() {
    let new = "2.1.280";
    let old = "2.0.50";
    let middle = "2.1.90";
    let of = |items: &[Option<&str>]| {
        let sightings: Vec<_> = items
            .iter()
            .map(|v| sighting(VersionSource::Binary, *v))
            .collect();
        effective_version(&sightings)
    };
    assert_eq!(of(&[Some(new)]), Some(version(2, 1, 280)));
    assert_eq!(of(&[Some(new), Some(old)]), Some(version(2, 0, 50)));
    assert_eq!(of(&[Some(new), Some(middle)]), Some(version(2, 1, 90)));
    assert_eq!(of(&[Some(new), None, Some(old)]), Some(version(2, 0, 50)));
    // Unknown ones don't count towards the events (the baseline is written
    // when nothing is known).
    assert_eq!(of(&[None]), None);
    assert_eq!(of(&[]), None);
    assert_eq!(
        effective_version(&[
            sighting(
                VersionSource::BundledVsCode,
                Some("anthropic.claude-code-2.1.88-win32-x64")
            ),
            sighting(VersionSource::StatusLine, Some("2.1.90")),
        ]),
        Some(version(2, 1, 88))
    );
}

/// `versionGating`, through the effective version.
#[test]
fn events_follow_the_effective_version() {
    let events_for =
        |v: Option<&str>| hook_events(effective_version(&[sighting(VersionSource::Binary, v)]));
    assert_eq!(events_for(None).len(), 10);
    assert!(!events_for(Some("2.1.32")).contains(&"TaskCompleted"));
    assert!(events_for(Some("2.1.33")).contains(&"TaskCompleted"));
    assert!(!events_for(Some("2.1.88")).contains(&"PermissionDenied"));
    assert!(events_for(Some("2.1.89")).contains(&"PermissionDenied"));
    assert_eq!(events_for(Some("2.1.280")).len(), 17);
}

// ---- The recogniser ----

fn recognised(command: &str) -> Option<Recognised> {
    windows().command(command)
}

fn subcommand(command: &str) -> Option<Subcommand> {
    recognised(command).map(|found| found.subcommand)
}

#[test]
fn recognises_string_commands_in_every_spelling() {
    for command in [
        "C:/Users/me/.claude/hooks/agentnotch-hook.exe hook",
        r"C:\Users\me\.claude\hooks\agentnotch-hook.exe hook",
        r#""C:\Users\John Smith\.claude\hooks\agentnotch-hook.exe" hook"#,
        r#""C:/Users/John Smith/.claude/hooks/agentnotch-hook.exe" hook"#,
        "'C:/Users/John Smith/.claude/hooks/agentnotch-hook.exe' hook",
        "/c/Users/me/.claude/hooks/agentnotch-hook.exe hook",
        "/C/Users/me/.claude/hooks/agentnotch-hook.exe hook",
        // Case: Windows file names have none.
        "C:/Users/me/.claude/hooks/AgentNotch-Hook.EXE hook",
        "c:\\users\\me\\.claude\\hooks\\AGENTNOTCH-HOOK.EXE hook",
        // The exe alone, or reached through a relative path.
        "agentnotch-hook.exe hook",
        "./hooks/agentnotch-hook.exe hook",
        // After other commands, through `exec`, `env` and assignments,
        // and PowerShell's call operator.
        "cd /tmp && C:/x/agentnotch-hook.exe hook",
        "echo hi; C:/x/agentnotch-hook.exe hook",
        "exec C:/x/agentnotch-hook.exe hook",
        "FOO=1 C:/x/agentnotch-hook.exe hook",
        "env -i FOO=1 C:/x/agentnotch-hook.exe hook",
        r#"& "C:\Users\John Smith\.claude\hooks\agentnotch-hook.exe" hook"#,
    ] {
        let found = recognised(command).unwrap_or_else(|| panic!("not recognised: {command}"));
        assert_eq!(found.subcommand, Subcommand::Hook, "{command}");
        assert!(!found.exec_form, "{command}");
        assert!(!found.exec_marker, "{command}");
    }
    assert_eq!(
        subcommand("C:/Users/me/.claude/hooks/agentnotch-hook.exe statusline"),
        Some(Subcommand::StatusLine)
    );
    assert_eq!(
        subcommand(r#""C:\a b\agentnotch-hook.exe" statusline"#),
        Some(Subcommand::StatusLine)
    );
}

#[test]
fn a_command_names_the_exe_it_runs() {
    let found =
        recognised(r#""C:\Users\John Smith\.claude\hooks\agentnotch-hook.exe" hook"#).unwrap();
    assert_eq!(
        found.exe,
        r"C:\Users\John Smith\.claude\hooks\agentnotch-hook.exe"
    );
    assert_eq!(
        windows().exe_path(&found),
        PathBuf::from(r"C:\Users\John Smith\.claude\hooks\agentnotch-hook.exe")
    );
    // Git Bash's drive spelling and forward slashes are what the file system
    // takes as backslashes.
    let bash = recognised("/c/Users/me/.claude/hooks/agentnotch-hook.exe hook").unwrap();
    assert_eq!(
        windows().exe_path(&bash),
        PathBuf::from(r"C:\Users\me\.claude\hooks\agentnotch-hook.exe")
    );
    let forward = recognised("D:/x/agentnotch-hook.exe hook").unwrap();
    assert_eq!(
        windows().exe_path(&forward),
        PathBuf::from(r"D:\x\agentnotch-hook.exe")
    );
    // `/c` alone is a drive; `/cx/…` is not.
    assert_eq!(windows().native_path("/c"), PathBuf::from(r"C:"));
    assert_eq!(windows().native_path("/cx/y"), PathBuf::from(r"\cx\y"));
}

#[test]
fn the_exec_marker_is_read_and_nothing_else_after_the_argument() {
    let with_marker = recognised("C:/x/agentnotch-hook.exe hook --exec").unwrap();
    assert!(with_marker.exec_marker);
    assert!(!with_marker.exec_form);
    let status = recognised("C:/x/agentnotch-hook.exe statusline --exec").unwrap();
    assert_eq!(status.subcommand, Subcommand::StatusLine);
    assert!(status.exec_marker);

    // Trailing junk is somebody else's command.
    for command in [
        "C:/x/agentnotch-hook.exe hook extra",
        "C:/x/agentnotch-hook.exe hook --exec extra",
        "C:/x/agentnotch-hook.exe hook --other",
        "C:/x/agentnotch-hook.exe hook --exec --exec",
        "C:/x/agentnotch-hook.exe statusline --skip",
        "C:/x/agentnotch-hook.exe --exec hook",
        "C:/x/agentnotch-hook.exe run",
        "C:/x/agentnotch-hook.exe Hook",
        "C:/x/agentnotch-hook.exe",
        "C:/x/agentnotch-hook.exe --exec",
        "",
        "   ",
        ";",
    ] {
        assert!(recognised(command).is_none(), "{command:?}");
    }
}

/// `scriptPathOfOurCommands` (the last two lines) and
/// `thirdPartyHooksMentioningAScriptAreNotRemoved`: naming the exe is not
/// running it.
#[test]
fn a_third_party_command_that_only_mentions_the_exe_is_not_ours() {
    for command in [
        "notify.cmd --skip agentnotch-hook.exe",
        "notify.cmd --skip C:/x/hooks/agentnotch-hook.exe hook",
        "~/bin/notify.sh --skip /x/hooks/agentnotch-hook.exe",
        "echo C:/x/hooks/agentnotch-hook.exe hook",
        "echo /x/hooks/agentnotch-hook.exe",
        "C:/x/agentnotch-hook.exe hook; echo done",
        "C:/x/agentnotch-hook.exe hook | tee log",
        "type C:/x/agentnotch-hook.exe",
        "C:/x/other.exe hook",
        "C:/x/agentnotch-hook.exe.bak hook",
        "C:/x/my-agentnotch-hook.exe hook",
        "C:/x/agentnotch-hook hook",
        "C:/x/agentnotch-hook.exe/other.exe hook",
        // A script that mentions the name in its own name.
        "C:/x/hooks/agentnotch-hook.exe-wrapper.cmd hook",
        "python3 /x/hooks/other.py",
    ] {
        assert!(recognised(command).is_none(), "{command}");
    }
    let entries = json(
        r#"{"hooks":[
            {"type":"command","command":"notify.cmd --skip agentnotch-hook.exe"},
            {"type":"command","command":"C:/x/agentnotch-hook.exe hook"}
        ]}"#,
    );
    let ours: Vec<bool> = entries
        .get("hooks")
        .and_then(Json::items)
        .unwrap()
        .iter()
        .map(|entry| windows().is_our_hook(entry))
        .collect();
    assert_eq!(ours, [false, true]);
}

/// An 8.3 name is compared after the long-path lookup.
#[test]
fn a_short_name_is_recognised_through_the_long_path_lookup() {
    let command = "C:/Users/JOHNSM~1/.claude/hooks/AGENTN~1.EXE hook";
    let asked = RefCell::new(Vec::new());
    let long = |path: &Path| -> Option<PathBuf> {
        asked.borrow_mut().push(path.to_path_buf());
        Some(PathBuf::from(
            r"C:\Users\John Smith\.claude\hooks\agentnotch-hook.exe",
        ))
    };
    let with_lookup = Recogniser::with_long_paths(PathStyle::Windows, &long);
    let found = with_lookup.command(command).expect("recognised");
    assert_eq!(found.subcommand, Subcommand::Hook);
    assert_eq!(found.exe, "C:/Users/JOHNSM~1/.claude/hooks/AGENTN~1.EXE");
    assert_eq!(
        asked.borrow().as_slice(),
        [PathBuf::from(
            r"C:\Users\JOHNSM~1\.claude\hooks\AGENTN~1.EXE"
        )]
    );
    // Quoted, backslashes, and Git Bash's drive spelling.
    assert!(with_lookup
        .command(r#""C:\Users\JOHNSM~1\.claude\hooks\AGENTN~1.EXE" statusline"#)
        .is_some());
    assert!(with_lookup
        .command("/c/Users/JOHNSM~1/.claude/hooks/AGENTN~1.EXE hook")
        .is_some());
    assert!(with_lookup.is_our_hook(&json(&format!(
        r#"{{"type":"command","command":"{command}"}}"#
    ))));

    // Without a lookup only the exe's own name is known.
    assert!(windows().command(command).is_none());

    // A short name that spells out to something else is not ours.
    let other = |_: &Path| Some(PathBuf::from(r"C:\Tools\notify.exe"));
    assert!(Recogniser::with_long_paths(PathStyle::Windows, &other)
        .command("C:/Users/JOHNSM~1/tools/NOTIFY~1.EXE hook")
        .is_none());
    // No such file, no answer.
    let nothing = |_: &Path| -> Option<PathBuf> { None };
    assert!(Recogniser::with_long_paths(PathStyle::Windows, &nothing)
        .command(command)
        .is_none());

    // The lookup is only asked about names that carry a `~`.
    let asked = RefCell::new(0);
    let counting = |_: &Path| -> Option<PathBuf> {
        *asked.borrow_mut() += 1;
        None
    };
    let counting = Recogniser::with_long_paths(PathStyle::Windows, &counting);
    assert!(counting.command("C:/x/agentnotch-hook.exe hook").is_some());
    assert!(counting.command("C:/x/other.exe hook").is_none());
    assert_eq!(*asked.borrow(), 0);
}

#[test]
fn recognises_exec_form_entries() {
    let entry = json(
        r#"{"type":"command","command":"C:\\Users\\me\\.claude\\hooks\\agentnotch-hook.exe","args":["hook","--exec"]}"#,
    );
    let found = windows().entry(&entry).expect("recognised");
    assert_eq!(found.subcommand, Subcommand::Hook);
    assert!(found.exec_form);
    assert!(found.exec_marker);
    assert_eq!(found.exe, r"C:\Users\me\.claude\hooks\agentnotch-hook.exe");
    assert!(windows().is_our_hook(&entry));
    assert!(!windows().is_our_status_line(Some(&entry)));

    // Case, a `/` path, the timeout the PermissionRequest entry carries.
    assert!(windows().is_our_hook(&json(
        r#"{"type":"command","command":"C:/x/AGENTNOTCH-HOOK.EXE","args":["hook","--exec"],"timeout":86400}"#
    )));
    // Without the marker (an entry written by hand).
    let bare = json(r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":["hook"]}"#);
    let found = windows().entry(&bare).unwrap();
    assert!(found.exec_form && !found.exec_marker);
    // A status line in exec form.
    let status =
        json(r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":["statusline"]}"#);
    assert!(windows().is_our_status_line(Some(&status)));
    assert!(!windows().is_our_hook(&status));

    for not_ours in [
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":["hook","--exec","x"]}"#,
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":["--exec","hook"]}"#,
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":["run"]}"#,
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":[]}"#,
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":["hook",1]}"#,
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":"hook"}"#,
        r#"{"type":"command","command":"C:/x/other.exe","args":["hook","--exec"]}"#,
        r#"{"type":"command","command":"notify.cmd","args":["agentnotch-hook.exe"]}"#,
        // In exec form `command` is the exe alone, never a command line.
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe hook","args":["hook"]}"#,
        r#"{"type":"command","args":["hook"]}"#,
        r#"{"type":"command","command":7}"#,
        r#"{"type":"command"}"#,
        "[]",
        r#""agentnotch-hook.exe hook""#,
    ] {
        assert!(windows().entry(&json(not_ours)).is_none(), "{not_ours}");
    }

    // An exec form written with an 8.3 exe is compared after the lookup.
    let long = |_: &Path| {
        Some(PathBuf::from(
            r"C:\Users\John Smith\.claude\hooks\agentnotch-hook.exe",
        ))
    };
    assert!(Recogniser::with_long_paths(PathStyle::Windows, &long).is_our_hook(&json(
        r#"{"type":"command","command":"C:\\Users\\JOHNSM~1\\.claude\\hooks\\AGENTN~1.EXE","args":["hook","--exec"]}"#
    )));
}

#[test]
fn a_string_entry_is_recognised_by_its_command() {
    let hook =
        json(r#"{"type":"command","command":"C:/x/agentnotch-hook.exe hook","timeout":86400}"#);
    assert!(windows().is_our_hook(&hook));
    assert!(!windows().is_our_status_line(Some(&hook)));
    let status =
        json(r#"{"type":"command","command":"C:/x/agentnotch-hook.exe statusline","padding":1}"#);
    assert!(windows().is_our_status_line(Some(&status)));
    assert!(!windows().is_our_hook(&status));
    assert!(!windows().is_our_status_line(None));
    assert!(!windows().is_our_status_line(Some(&json("{}"))));
}

/// The official app's entries are only reported, never taken for ours.
#[test]
fn the_official_apps_hook_is_told_apart() {
    for entry in [
        r#"{"type":"command","command":"C:\\Users\\me\\AppData\\Local\\Codenotch\\codenotch-hook.exe hook"}"#,
        r#"{"type":"command","command":"C:/Users/me/.codenotch/codenotch-hook.exe"}"#,
        r#"{"type":"command","command":"\"C:\\Program Files\\Codenotch\\CODENOTCH-HOOK.EXE\" hook"}"#,
        r#"{"type":"command","command":"/c/x/codenotch-hook hook"}"#,
        r#"{"type":"command","command":"C:\\x\\codenotch-hook.exe","args":["hook"]}"#,
        r#"{"type":"command","command":"C:/x/eatbean-hook.exe hook"}"#,
        r#"{"type":"command","command":"C:/x/pacman-hook.exe hook"}"#,
        r#"{"type":"command","command":"echo hi && C:/x/codenotch-hook.exe hook"}"#,
    ] {
        let entry = json(entry);
        assert!(is_upstream_hook(&entry), "{entry:?}");
        assert!(windows().entry(&entry).is_none());
        assert!(!windows().is_our_hook(&entry));
    }
    for entry in [
        // Merely mentioning the name is not running it.
        r#"{"type":"command","command":"notify.cmd --skip codenotch-hook.exe"}"#,
        r#"{"type":"command","command":"echo codenotch-hook"}"#,
        r#"{"type":"command","command":"C:/x/codenotch-hook.exe hook; echo done"}"#,
        r#"{"type":"command","command":"C:/x/codenotch-hook.exe.bak hook"}"#,
        r#"{"type":"command","command":"C:/x/notcodenotch-hook.exe hook"}"#,
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe hook"}"#,
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":["hook","--exec"]}"#,
        r#"{"type":"command","command":"C:/x/codenotch-hook.exe hook","args":["hook"]}"#,
        r#"{"type":"command","command":""}"#,
        r#"{"type":"command"}"#,
        r#"{"type":"command","command":5}"#,
        "[]",
    ] {
        assert!(!is_upstream_hook(&json(entry)), "{entry}");
    }
}

// ---- The takeover rule ----

fn takeover_of(status_line: Option<&str>, git_bash: bool) -> Takeover {
    let value = status_line.map(json);
    takeover(value.as_ref(), &windows(), git_bash)
}

fn left_alone(why: &str) -> Takeover {
    Takeover::LeaveAlone(format!("Status line left alone: {why}"))
}

#[test]
fn no_status_line_installs_ours_and_ours_is_updated() {
    assert_eq!(takeover_of(None, false), Takeover::Install);
    assert_eq!(takeover_of(None, true), Takeover::Install);
    for ours in [
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe statusline","padding":0}"#,
        r#"{"type":"command","command":"C:/x/agentnotch-hook.exe","args":["statusline"]}"#,
        r#"{"type":"command","command":"\"C:\\x y\\agentnotch-hook.exe\" statusline"}"#,
    ] {
        assert_eq!(takeover_of(Some(ours), false), Takeover::Update, "{ours}");
        assert_eq!(takeover_of(Some(ours), true), Takeover::Update, "{ours}");
    }
}

#[test]
fn someone_elses_plain_command_is_wrapped_when_git_bash_is_there() {
    for command in [
        r#"{"type":"command","command":"bash ~/.claude/statusline.sh"}"#,
        r#"{"type":"command","command":"npx -y ccstatusline@latest","padding":0}"#,
        r#"{"type":"command","command":"/c/Users/me/bin/sl.sh --fast"}"#,
        r#"{"type":"command","command":"C:/tools/sl.exe"}"#,
    ] {
        assert_eq!(
            takeover_of(Some(command), true),
            Takeover::Wrap,
            "{command}"
        );
        assert_eq!(
            takeover_of(Some(command), false),
            left_alone("Git Bash isn't installed"),
            "{command}"
        );
    }
}

#[test]
fn a_backslash_leaves_the_status_line_alone() {
    assert_eq!(
        takeover_of(
            Some(r#"{"type":"command","command":"C:\\tools\\sl.exe --fast"}"#),
            true
        ),
        left_alone("its command uses Windows paths")
    );
    assert_eq!(
        takeover_of(Some(r#"{"type":"command","command":"a\\b"}"#), true),
        left_alone("its command uses Windows paths")
    );
}

#[test]
fn anything_powershell_leaves_the_status_line_alone() {
    let powershell = left_alone("its command needs PowerShell");
    for command in [
        "pwsh -File x.ps1",
        "sl.ps1",
        "echo $env:USERNAME",
        "echo $ENV:USERNAME",
        "powershell -NoProfile -Command x",
        "PowerShell.exe -c x",
        "POWERSHELL -c x",
        "PWSH -c x",
        "/c/Program Files/PowerShell/7/pwsh.exe x",
        "run SCRIPT.PS1",
    ] {
        let entry = format!(r#"{{"type":"command","command":{}}}"#, quote(command));
        assert_eq!(takeover_of(Some(&entry), true), powershell, "{command}");
        assert_eq!(takeover_of(Some(&entry), false), powershell, "{command}");
    }
}

#[test]
fn cmd_leaves_the_status_line_alone() {
    let cmd = left_alone("its command needs cmd.exe");
    for command in [
        "cmd.exe /c x",
        "CMD.EXE /C x",
        "cmd /c x",
        "CMD /C echo hi",
        "x & cmd /c y",
    ] {
        let entry = format!(r#"{{"type":"command","command":{}}}"#, quote(command));
        assert_eq!(takeover_of(Some(&entry), true), cmd, "{command}");
    }
    // `cmd` as part of another word, or without `/c`, is only a name.
    for command in ["mycmd x", "cmdline --x", "cmd"] {
        let entry = format!(r#"{{"type":"command","command":{}}}"#, quote(command));
        assert_eq!(takeover_of(Some(&entry), true), Takeover::Wrap, "{command}");
    }
}

#[test]
fn a_shell_key_leaves_the_status_line_alone() {
    assert_eq!(
        takeover_of(
            Some(r#"{"type":"command","command":"x","shell":"powershell"}"#),
            true
        ),
        left_alone("its command needs PowerShell")
    );
    assert_eq!(
        takeover_of(
            Some(r#"{"type":"command","command":"x","shell":"pwsh"}"#),
            true
        ),
        left_alone("its command needs PowerShell")
    );
    assert_eq!(
        takeover_of(
            Some(r#"{"type":"command","command":"x","shell":"bash"}"#),
            true
        ),
        left_alone("it names its own shell")
    );
    // Even one that isn't a string.
    assert_eq!(
        takeover_of(Some(r#"{"type":"command","command":"x","shell":7}"#), true),
        left_alone("it names its own shell")
    );
}

#[test]
fn only_a_plain_command_object_is_wrapped() {
    let plain = left_alone("it isn't a plain command");
    for status_line in [
        r#"{"type":"static","text":"hi"}"#,
        r#"{"command":"x"}"#,
        r#"{"type":"command"}"#,
        r#"{"type":"command","command":""}"#,
        r#"{"type":"command","command":"   "}"#,
        r#"{"type":"command","command":5}"#,
        // A program and its arguments, run without a shell.
        r#"{"type":"command","command":"node","args":["sl.js"]}"#,
        r#"{"type":"command","command":"sl.sh","args":[]}"#,
        "{}",
        "[]",
        r#""bash x.sh""#,
        "7",
        "null",
        "true",
    ] {
        assert_eq!(takeover_of(Some(status_line), true), plain, "{status_line}");
    }
}

/// The takeover never wraps something that would run the wrapper again.
#[test]
fn a_status_line_running_our_own_wrapper_is_left_alone() {
    let own = left_alone("it runs this app's own wrapper");
    for command in [
        "python3 '/x/hooks/agentnotch-statusline.py'",
        "python3 /x/hooks/superpowered-codenotch-statusline.py",
        "python3 /x/hooks/superpowered-notch-statusline.py",
        "bash -c 'C:/x/agentnotch-hook.exe statusline'",
    ] {
        let entry = format!(r#"{{"type":"command","command":{}}}"#, quote(command));
        assert_eq!(takeover_of(Some(&entry), true), own, "{command}");
    }
}

/// The quoted, JSON-escaped form of a command for a test entry.
fn quote(text: &str) -> String {
    agentnotch_engine::core::settings_doc::quoted(text)
}

// ---- The loop guard ----

/// `neverChainsToAWrapper`: a previous command that would run the wrapper
/// again is refused in every spelling.
#[test]
fn the_loop_guard_refuses_our_wrapper_in_every_spelling() {
    let guard = windows();
    for name in [
        "agentnotch-statusline.py",
        "superpowered-codenotch-statusline.py",
        "superpowered-notch-statusline.py",
    ] {
        assert!(
            guard.trips_loop_guard(&format!("python3 '/x/hooks/{name}'")),
            "{name}"
        );
        assert!(guard.trips_loop_guard(&format!("python3 '/x/hooks/{}'", name.to_uppercase())));
        assert!(guard.trips_loop_guard(&format!("bash C:/Users/me/.claude/hooks/{name}")));
    }
    for command in [
        "C:/x/agentnotch-hook.exe statusline",
        r"C:\x\agentnotch-hook.exe statusline",
        r#""C:\x y\AgentNotch-Hook.exe" STATUSLINE"#,
        "/c/x/agentnotch-hook.exe statusline --exec",
        "bash -c 'C:/x/agentnotch-hook.exe statusline'",
        "echo start && agentnotch-hook.exe statusline",
        // Any spelling of the name, with the argument anywhere.
        "agentnotch-hook statusline",
        "AGENTNOTCH-HOOK.EXE StatusLine",
        "statusline C:/x/agentnotch-hook.exe",
    ] {
        assert!(guard.trips_loop_guard(command), "{command}");
    }
}

#[test]
fn the_loop_guard_leaves_other_status_lines_alone() {
    let guard = windows();
    for command in [
        "echo hi",
        "",
        "ccusage statusline",
        "npx -y ccstatusline@latest",
        "bash ~/.claude/statusline.sh",
        // Our hook exe without `statusline` is the hook, not the wrapper.
        "C:/x/agentnotch-hook.exe hook",
        "agentnotch-hook",
        "agentnotch statusline",
        "notify-statusline.sh",
    ] {
        assert!(!guard.trips_loop_guard(command), "{command:?}");
    }
}

/// An 8.3 spelling of the exe has no `agentnotch-hook` in it: it is found
/// through the long-path lookup.
#[test]
fn the_loop_guard_sees_through_an_8_3_name() {
    let command = "C:/Users/JOHNSM~1/.claude/hooks/AGENTN~1.EXE statusline";
    assert!(!windows().trips_loop_guard(command));
    let long = |_: &Path| {
        Some(PathBuf::from(
            r"C:\Users\John Smith\.claude\hooks\agentnotch-hook.exe",
        ))
    };
    let with_lookup = Recogniser::with_long_paths(PathStyle::Windows, &long);
    assert!(with_lookup.trips_loop_guard(command));
    assert!(with_lookup
        .trips_loop_guard(r#""C:\Users\JOHNSM~1\.claude\hooks\AGENTN~1.EXE" statusline"#));
    // Without `statusline` it is only the hook.
    assert!(!with_lookup.trips_loop_guard("C:/Users/JOHNSM~1/.claude/hooks/AGENTN~1.EXE hook"));
}

// ---- Git Bash ----

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    move |name| map.get(name).cloned()
}

fn paths(items: &[&str]) -> Vec<PathBuf> {
    items.iter().map(PathBuf::from).collect()
}

#[test]
fn git_bash_is_looked_for_in_claude_codes_order() {
    // The user's own choice first, then Program Files (64-bit), then x86.
    assert_eq!(
        git_bash_candidates(env(&[
            ("CLAUDE_CODE_GIT_BASH_PATH", r"D:\Tools\bash.exe"),
            ("ProgramFiles", r"E:\Programs"),
            ("ProgramFiles(x86)", r"E:\Programs (x86)"),
        ])),
        paths(&[
            r"D:\Tools\bash.exe",
            r"E:\Programs\Git\bin\bash.exe",
            r"E:\Programs (x86)\Git\bin\bash.exe",
        ])
    );
    // Nothing set: the standard folders.
    assert_eq!(
        git_bash_candidates(env(&[])),
        paths(&[
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ])
    );
    // Blank values count as unset; a trailing backslash makes no double one.
    assert_eq!(
        git_bash_candidates(env(&[
            ("CLAUDE_CODE_GIT_BASH_PATH", "   "),
            ("ProgramFiles", ""),
            ("ProgramFiles(x86)", r"D:\x86\"),
        ])),
        paths(&[
            r"C:\Program Files\Git\bin\bash.exe",
            r"D:\x86\Git\bin\bash.exe",
        ])
    );
    // The same place twice is listed once.
    assert_eq!(
        git_bash_candidates(env(&[
            (
                "CLAUDE_CODE_GIT_BASH_PATH",
                r"C:\Program Files\Git\bin\bash.exe"
            ),
            ("ProgramFiles", r"C:\Program Files"),
            ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
        ])),
        paths(&[
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ])
    );
}

// ---- Words ----

/// `splitsLikeTheShell` and `lastSimpleCommandAndInterpreter`, through the
/// public module (the unit tests hold the finer vectors).
#[test]
fn shell_words_read_both_shells_commands() {
    assert_eq!(
        shell_words::words(r#""C:\Program Files\Tool\tool.exe" run"#),
        [r"C:\Program Files\Tool\tool.exe", "run"]
    );
    assert_eq!(
        shell_words::words("python3 '/a b/c.py'")
            .last()
            .map(String::as_str),
        Some("/a b/c.py")
    );
    assert_eq!(
        shell_words::last_simple_command("a; b c && d e"),
        Some(vec!["d".to_owned(), "e".to_owned()])
    );
    assert_eq!(shell_words::file_name(r"C:\a\b.exe"), "b.exe");
}
