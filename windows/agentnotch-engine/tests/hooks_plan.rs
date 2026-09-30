//! What a settings.json becomes: ports of the Mac's `HookInstallerPlanTests`
//! and `SettingsFidelityTests` (HookInstallerTests.swift), with Windows'
//! command spellings, CRLF and BOM variants, the status line reasons only
//! Windows has, and the removal of the official app's hooks.
//!
//! Nothing here touches a file: a plan is bytes in, bytes out. Every path is a
//! string and every recogniser is built with `PathStyle::Windows`, so the
//! vectors run the same on any OS.

use agentnotch_engine::core::paths::PathStyle;
use agentnotch_engine::core::settings_doc::{Json, Member, BOM};
use agentnotch_engine::hooks::commands::{is_upstream_hook, Recogniser};
use agentnotch_engine::hooks::events::hook_events;
use agentnotch_engine::hooks::plan::{
    chain_target, first_hook_entry, hook_entries, is_a_wrapper, plan_install, plan_uninstall,
    plan_upstream_removal, removing_hooks, restored, PreviousStatusLineChange, Refusal, Saved,
    SettingsPlan, SettingsWrite, StatusLineWish, CHAINS_NOTHING,
};
use agentnotch_engine::hooks::version::ClaudeCodeVersion;
use agentnotch_engine::runtime_types::{CommandForm, StatusLineIntent};
use std::cell::Cell;
use std::path::{Path, PathBuf};

/// The hook command of the folder under test.
const COMMAND: &str = "C:/Users/me/.claude-work/hooks/agentnotch-hook.exe hook";
/// Its status line command.
const STATUS: &str = "C:/Users/me/.claude-work/hooks/agentnotch-hook.exe statusline";

/// A settings.json like a real one: other tools' hooks on the same events
/// (the official app's among them), a status line, permissions and env, and
/// one stale entry of ours on an event no longer registered.
const REALISTIC: &str = r#"{
  "$schema": "https://json.schemastore.org/claude-code-settings.json",
  "model": "opus",
  "env": {"BASH_DEFAULT_TIMEOUT_MS": "300000"},
  "permissions": {"allow": ["Bash(npm test:*)", "Read(~/notes/**)"], "deny": [], "defaultMode": "default"},
  "enabledPlugins": {"superpowers@claude-plugins-official": true},
  "hooks": {
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "~/bin/guard-bash.sh", "timeout": 30}]},
      {"matcher": "*", "hooks": [{"type": "command", "command": "C:/Users/me/.claude/hooks/codenotch-hook.exe"}]}
    ],
    "PermissionRequest": [
      {"matcher": "*", "hooks": [
        {"type": "command", "command": "node C:/tools/bridge.js --source claude", "timeout": 86400},
        {"type": "command", "command": "C:/Users/me/.claude/hooks/codenotch-hook.exe", "timeout": 86400}
      ]}
    ],
    "Stop": [{"hooks": [{"type": "command", "command": "powershell -NoProfile -Command [console]::beep()"}]}],
    "TeammateIdle": [{"hooks": [{"type": "command", "command": "C:/old/path/hooks/agentnotch-hook.exe hook"}]}]
  },
  "statusLine": {"type": "command", "command": "~/.claude/statusline.sh", "padding": 2, "refreshInterval": 5}
}"#;

const OFFICIAL: &str = "C:/Users/me/.claude/hooks/codenotch-hook.exe";

// ---- Helpers ----

fn json(text: &str) -> Json {
    let value = Json::parse(text.as_bytes()).expect("valid JSON");
    assert!(value.is_object(), "expected an object: {text}");
    value
}

fn object(bytes: &[u8]) -> Json {
    let value = Json::parse(bytes).expect("valid JSON");
    assert!(value.is_object());
    value
}

fn windows() -> Recogniser<'static> {
    Recogniser::new(PathStyle::Windows)
}

fn latest() -> Option<ClaudeCodeVersion> {
    Some(ClaudeCodeVersion::new(2, 1, 280))
}

fn events(version: Option<ClaudeCodeVersion>) -> Vec<String> {
    hook_events(version)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn text_form() -> CommandForm {
    CommandForm::Text(COMMAND.to_owned())
}

const WRAP: StatusLineWish<'static> = StatusLineWish::Wrap {
    command: STATUS,
    git_bash: true,
};

/// One install plan, with everything a test doesn't say left at its default:
/// the string form, the baseline events, the status line left alone, nothing
/// saved and no backups.
struct Install<'a> {
    form: CommandForm,
    version: Option<ClaudeCodeVersion>,
    wish: StatusLineWish<'a>,
    saved: Option<Json>,
    backup: Option<Json>,
}

impl Default for Install<'_> {
    fn default() -> Self {
        Install {
            form: text_form(),
            version: None,
            wish: StatusLineWish::Leave,
            saved: None,
            backup: None,
        }
    }
}

impl Install<'_> {
    fn plan(&self, data: Option<&[u8]>) -> SettingsPlan {
        let backup = || self.backup.clone();
        let saved = Saved {
            previous_status_line: self.saved.as_ref(),
            backup_status_line: &backup,
        };
        plan_install(
            data,
            &self.form,
            &events(self.version),
            self.wish,
            &saved,
            &windows(),
        )
    }
}

fn install(data: Option<&[u8]>) -> SettingsPlan {
    Install::default().plan(data)
}

fn wrapping(version: Option<ClaudeCodeVersion>) -> Install<'static> {
    Install {
        version,
        wish: WRAP,
        ..Install::default()
    }
}

fn uninstall(data: Option<&[u8]>, saved: Option<&Json>, backup: Option<&Json>) -> SettingsPlan {
    let backup = || backup.cloned();
    plan_uninstall(
        data,
        &Saved {
            previous_status_line: saved,
            backup_status_line: &backup,
        },
        &windows(),
    )
}

fn written_bytes(plan: &SettingsPlan) -> Vec<u8> {
    match &plan.settings {
        SettingsWrite::Write(bytes) => bytes.clone(),
        other => panic!("expected a write, got {other:?}"),
    }
}

fn written(plan: &SettingsPlan) -> Json {
    object(&written_bytes(plan))
}

fn saved_bytes(plan: &SettingsPlan) -> Vec<u8> {
    match &plan.previous_status_line {
        Some(PreviousStatusLineChange::Save(bytes)) => bytes.clone(),
        other => panic!("expected the previous status line to be saved, got {other:?}"),
    }
}

/// The `command` of every hook entry of `event`, in file order.
fn commands(settings: &Json, event: &str) -> Vec<String> {
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(Json::items)
        .unwrap_or_default()
        .iter()
        .flat_map(|group| group.get("hooks").and_then(Json::items).unwrap_or_default())
        .filter_map(|entry| entry.get("command").and_then(Json::as_str))
        .map(str::to_owned)
        .collect()
}

fn keys(value: &Json) -> Vec<&str> {
    value
        .members()
        .unwrap_or_default()
        .iter()
        .map(|member| member.key.as_str())
        .collect()
}

/// `settings` with our hook entries taken out, written independently of
/// `removing_hooks`.
fn without_our_hooks(settings: &Json) -> Json {
    let recogniser = windows();
    let mut cleaned = Vec::new();
    for member in settings
        .get("hooks")
        .and_then(Json::members)
        .unwrap_or_default()
    {
        let mut groups = Vec::new();
        for group in member.value.items().unwrap_or_default() {
            let entries: Vec<Json> = group
                .get("hooks")
                .and_then(Json::items)
                .unwrap_or_default()
                .iter()
                .filter(|entry| !recogniser.is_our_hook(entry))
                .cloned()
                .collect();
            if entries.is_empty() {
                continue;
            }
            let mut updated = group.clone();
            updated.set("hooks", Some(Json::Array(entries)));
            groups.push(updated);
        }
        if !groups.is_empty() {
            cleaned.push(Member::new(member.key.clone(), Json::Array(groups)));
        }
    }
    let mut copy = settings.clone();
    copy.set("hooks", Some(Json::Object(cleaned)));
    copy
}

// ---- Hooks ----

#[test]
fn adds_our_hooks_and_leaves_everything_else() {
    let original = json(REALISTIC);
    let plan = install_with_version(REALISTIC.as_bytes(), latest());
    let settings = written(&plan);
    assert_eq!(plan.previous_status_line, None);
    assert_eq!(plan.status_line, StatusLineIntent::Nothing);

    for event in [
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "Notification",
        "Stop",
        "SubagentStop",
        "SessionStart",
        "SessionEnd",
        "PreCompact",
        "PostToolUseFailure",
        "SubagentStart",
        "PostCompact",
        "StopFailure",
        "PermissionDenied",
        "TaskCreated",
        "TaskCompleted",
    ] {
        assert!(
            commands(&settings, event).contains(&COMMAND.to_owned()),
            "missing {event}"
        );
    }
    // Other tools' entries, the official app's included, are untouched, and
    // ours come after them.
    assert_eq!(
        commands(&settings, "PreToolUse"),
        ["~/bin/guard-bash.sh", OFFICIAL, COMMAND]
    );
    assert_eq!(
        commands(&settings, "PermissionRequest"),
        ["node C:/tools/bridge.js --source claude", OFFICIAL, COMMAND]
    );
    assert_eq!(
        commands(&settings, "Stop"),
        ["powershell -NoProfile -Command [console]::beep()", COMMAND]
    );

    // Our stale entry on an event no longer registered is gone.
    assert!(settings.get("hooks").unwrap().get("TeammateIdle").is_none());

    // Every non-hook key is exactly as it was, in the same order.
    assert_eq!(keys(&settings), keys(&original));
    for member in original.members().unwrap() {
        if member.key != "hooks" {
            assert!(
                Json::equivalent(settings.get(&member.key), Some(&member.value)),
                "changed {}",
                member.key
            );
        }
    }
    // Removing ours again gives back the original hooks (minus the stale one).
    let mut expected = original.get("hooks").unwrap().clone();
    expected.set("TeammateIdle", None);
    assert!(Json::equivalent(
        without_our_hooks(&settings).get("hooks"),
        Some(&expected)
    ));

    // Outside `hooks`, the file keeps its bytes.
    let text = String::from_utf8(written_bytes(&plan)).unwrap();
    let (before, after) = REALISTIC.split_once("  \"hooks\": {").unwrap();
    assert!(text.starts_with(before));
    assert!(text.ends_with(after.rsplit_once("  },\n").unwrap().1));
}

fn install_with_version(data: &[u8], version: Option<ClaudeCodeVersion>) -> SettingsPlan {
    Install {
        version,
        ..Install::default()
    }
    .plan(Some(data))
}

#[test]
fn permission_request_waits_for_the_app() {
    let settings = written(&install(None));
    let hooks = settings.get("hooks").unwrap();
    let entry = |event: &str| {
        hooks.get(event).unwrap().items().unwrap()[0]
            .get("hooks")
            .unwrap()
            .items()
            .unwrap()[0]
            .clone()
    };
    assert!(Json::equivalent(
        entry("PermissionRequest").get("timeout"),
        Some(&Json::int(86400))
    ));
    assert_eq!(
        keys(&entry("PermissionRequest")),
        ["type", "command", "timeout"]
    );
    // No other event waits.
    for event in hook_events(latest()) {
        if event != "PermissionRequest" {
            let settings = written(&install_with_version(b"{}", latest()));
            let first = settings
                .get("hooks")
                .unwrap()
                .get(event)
                .unwrap()
                .items()
                .unwrap()[0]
                .get("hooks")
                .unwrap()
                .items()
                .unwrap()[0]
                .clone();
            assert_eq!(keys(&first), ["type", "command"], "{event}");
        }
    }
    // Exec form waits the same.
    let exec = Install {
        form: exec_form(),
        ..Install::default()
    };
    let settings = written(&exec.plan(None));
    let entry = settings
        .get("hooks")
        .unwrap()
        .get("PermissionRequest")
        .unwrap()
        .items()
        .unwrap()[0]
        .get("hooks")
        .unwrap()
        .items()
        .unwrap()[0]
        .clone();
    assert_eq!(keys(&entry), ["type", "command", "args", "timeout"]);
    assert!(Json::equivalent(
        entry.get("timeout"),
        Some(&Json::int(86400))
    ));
}

#[test]
fn a_second_plan_is_a_no_op() {
    let first = wrapping(latest()).plan(Some(REALISTIC.as_bytes()));
    assert_eq!(first.status_line, StatusLineIntent::Wrap);
    let data = written_bytes(&first);
    let saved = object(&saved_bytes(&first));

    let second = Install {
        saved: Some(saved),
        ..wrapping(latest())
    }
    .plan(Some(&data));
    assert_eq!(
        second,
        SettingsPlan {
            settings: SettingsWrite::AlreadyCurrent,
            previous_status_line: None,
            status_line: StatusLineIntent::UpdateCommand,
        }
    );
}

#[test]
fn a_reformatted_but_equal_file_is_not_rewritten() {
    let first = written(
        &Install {
            version: latest(),
            ..Install::default()
        }
        .plan(None),
    );
    // Claude Code (or an editor) rewrote the file compactly, keys reordered,
    // same content.
    let mut members = first.members().unwrap().to_vec();
    members.reverse();
    let mut hooks = first.get("hooks").unwrap().members().unwrap().to_vec();
    hooks.reverse();
    let mut reordered = Json::Object(members);
    reordered.set("hooks", Some(Json::Object(hooks)));
    reordered.set("model", Some(Json::string("opus")));
    let with_model = {
        let mut value = first.clone();
        value.set("model", Some(Json::string("opus")));
        value
    };
    assert!(reordered.is_equivalent(&with_model));

    for (unit, newline) in [("", ""), ("    ", "\n"), ("\t", "\r\n")] {
        let text = reordered.serialized_with("", unit, newline);
        assert_eq!(
            install_with_version(text.as_bytes(), latest()).settings,
            SettingsWrite::AlreadyCurrent,
            "unit {unit:?}"
        );
        let with_bom = [BOM, text.as_bytes()].concat();
        assert_eq!(
            install_with_version(&with_bom, latest()).settings,
            SettingsWrite::AlreadyCurrent
        );
    }
}

#[test]
fn refuses_settings_that_are_not_a_json_object() {
    for text in [
        "{\"hooks\": {", // truncated mid-save
        "[1, 2, 3]",     // not an object
        "hello",         // garbage
        "\"just a string\"",
    ] {
        let data = text.as_bytes();
        let refused = SettingsPlan {
            settings: SettingsWrite::Refuse(Refusal::Unreadable),
            previous_status_line: None,
            status_line: StatusLineIntent::Nothing,
        };
        assert_eq!(wrapping(latest()).plan(Some(data)), refused, "{text}");
        assert_eq!(uninstall(Some(data), None, None), refused, "{text}");
        assert_eq!(plan_upstream_removal(Some(data)), (refused, 0), "{text}");
    }
    assert_eq!(
        Refusal::Unreadable.message(),
        "settings.json isn't valid JSON, so it was left alone. Fix it and try again."
    );
}

#[test]
fn refuses_binary_garbage() {
    let garbage: &[u8] = &[0xFF, 0xFE, 0x00, 0x7B];
    assert_eq!(
        install(Some(garbage)).settings,
        SettingsWrite::Refuse(Refusal::Unreadable)
    );
    assert_eq!(
        uninstall(Some(garbage), None, None).settings,
        SettingsWrite::Refuse(Refusal::Unreadable)
    );
    // UTF-16, as Notepad once saved by default.
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain("{}".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    assert_eq!(
        install(Some(&utf16)).settings,
        SettingsWrite::Refuse(Refusal::Unreadable)
    );
}

/// A `hooks` value that isn't an object is the user's, however odd.
#[test]
fn refuses_a_hooks_value_that_is_not_an_object() {
    for text in [
        r#"{"hooks": []}"#,
        r#"{"hooks": null}"#,
        r#"{"hooks": "disabled"}"#,
        r#"{"hooks": 3}"#,
    ] {
        let data = text.as_bytes();
        let refused = SettingsPlan {
            settings: SettingsWrite::Refuse(Refusal::HooksNotAnObject),
            previous_status_line: None,
            status_line: StatusLineIntent::Nothing,
        };
        assert_eq!(install(Some(data)), refused, "{text}");
        assert_eq!(wrapping(None).plan(Some(data)), refused, "{text}");
        assert_eq!(uninstall(Some(data), None, None), refused, "{text}");
        assert_eq!(plan_upstream_removal(Some(data)), (refused, 0), "{text}");
    }
    assert_eq!(
        Refusal::HooksNotAnObject.message(),
        "settings.json has a \"hooks\" value that isn't an object, so it was left alone."
    );
}

#[test]
fn malformed_event_values_are_left_alone() {
    let settings = written(&install(Some(
        br#"{"hooks":{"Stop":"not-a-list","PreToolUse":[]}}"#,
    )));
    let hooks = settings.get("hooks").unwrap();
    assert_eq!(hooks.get("Stop").and_then(Json::as_str), Some("not-a-list"));
    assert_eq!(commands(&settings, "PreToolUse"), [COMMAND]);

    // Groups and entries of a shape not recognised stay where they are.
    let odd = r#"{"hooks":{"Stop":["text",{"hooks":"nope"},{"hooks":[7,{"type":"command"},{"command":3}]}]}}"#;
    let settings = written(&install(Some(odd.as_bytes())));
    let stop = settings.get("hooks").unwrap().get("Stop").unwrap();
    let before = json(odd);
    let kept = before
        .get("hooks")
        .unwrap()
        .get("Stop")
        .unwrap()
        .items()
        .unwrap();
    assert_eq!(&stop.items().unwrap()[..3], kept);
    assert_eq!(commands(&settings, "Stop"), [COMMAND]);
    // And an uninstall finds nothing of ours in them.
    assert_eq!(
        uninstall(Some(odd.as_bytes()), None, None).settings,
        SettingsWrite::AlreadyCurrent
    );
}

#[test]
fn a_blank_file_starts_from_empty() {
    let settings = written(&install(Some(b"  \n")));
    assert_eq!(commands(&settings, "Stop"), [COMMAND]);
    let settings = written(&install(Some(b"")));
    assert_eq!(commands(&settings, "Stop"), [COMMAND]);
    // No file at all: the same.
    let settings = written(&install(None));
    assert_eq!(commands(&settings, "Stop"), [COMMAND]);
    assert_eq!(keys(&settings), ["hooks"]);
}

const EXE: &str = r"C:\Users\me\.claude-work\hooks\agentnotch-hook.exe";

fn exec_form() -> CommandForm {
    CommandForm::Exec {
        command: PathBuf::from(EXE),
        args: vec!["hook".to_owned(), "--exec".to_owned()],
    }
}

fn stop_entries(settings: &Json) -> Vec<Json> {
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get("Stop"))
        .and_then(Json::items)
        .unwrap_or_default()
        .iter()
        .flat_map(|group| group.get("hooks").and_then(Json::items).unwrap_or_default())
        .cloned()
        .collect()
}

/// Entries written in another form are replaced, not kept beside the new
/// ones: exec for string and string for exec.
#[test]
fn entries_in_the_other_form_are_replaced() {
    let exec_entry = r#"{"type":"command","command":"C:\\Users\\me\\.claude-work\\hooks\\agentnotch-hook.exe","args":["hook","--exec"]}"#;
    let string_entry = format!(r#"{{"type":"command","command":"{COMMAND}"}}"#);
    let with = |entry: &str| format!(r#"{{"hooks":{{"Stop":[{{"hooks":[{entry}]}}]}}}}"#);

    // Exec form on disk, string form wanted (an older Claude Code appeared).
    let settings = written(&install(Some(with(exec_entry).as_bytes())));
    let entries = stop_entries(&settings);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_equivalent(&json(&string_entry)));
    assert!(entries[0].get("args").is_none());

    // String form on disk, exec form wanted.
    let exec = Install {
        form: exec_form(),
        ..Install::default()
    };
    let settings = written(&exec.plan(Some(with(&string_entry).as_bytes())));
    let entries = stop_entries(&settings);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_equivalent(&json(exec_entry)));

    // The form already there: nothing to write once every event has it.
    let installed = written_bytes(&exec.plan(None));
    assert_eq!(
        exec.plan(Some(&installed)).settings,
        SettingsWrite::AlreadyCurrent
    );
    assert_ne!(
        install(Some(&installed)).settings,
        SettingsWrite::AlreadyCurrent
    );

    // A quoted, backslashed, or `/c/` spelling of ours is replaced as well.
    for old in [
        r#"\"C:\\Users\\me\\.claude-work\\hooks\\agentnotch-hook.exe\" hook"#,
        "/c/Users/me/.claude-work/hooks/agentnotch-hook.exe hook",
        "C:/Users/me/.claude-work/hooks/AgentNotch-Hook.EXE hook --exec",
        "C:/somewhere/else/hooks/agentnotch-hook.exe hook",
    ] {
        let entry = format!(r#"{{"type":"command","command":"{old}"}}"#);
        let settings = written(&install(Some(with(&entry).as_bytes())));
        assert_eq!(commands(&settings, "Stop"), [COMMAND], "{old}");
    }
}

/// A folder written with its 8.3 name and one written with its long name
/// replace each other; an exe named by an 8.3 name is known only through the
/// long-path lookup.
#[test]
fn short_and_long_spellings_replace_each_other() {
    let short = "C:/Users/JOHNSM~1/.claude-work/hooks/agentnotch-hook.exe hook";
    let with = |command: &str| {
        format!(
            r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"{command}"}}]}}]}}}}"#
        )
    };

    // 8.3 on disk, long wanted.
    let settings = written(&install(Some(with(short).as_bytes())));
    assert_eq!(commands(&settings, "Stop"), [COMMAND]);

    // Long on disk, 8.3 wanted.
    let shortened = Install {
        form: CommandForm::Text(short.to_owned()),
        ..Install::default()
    };
    let settings = written(&shortened.plan(Some(with(COMMAND).as_bytes())));
    assert_eq!(commands(&settings, "Stop"), [short]);
    assert_eq!(
        shortened
            .plan(Some(&written_bytes(&shortened.plan(None))))
            .settings,
        SettingsWrite::AlreadyCurrent
    );

    // The exe itself under an 8.3 name.
    let all_short = "C:/Users/JOHNSM~1/.claude-work/hooks/AGENTN~1.EXE hook";
    let asked = Cell::new(0);
    let long_path = |path: &Path| -> Option<PathBuf> {
        asked.set(asked.get() + 1);
        (path == Path::new(r"C:\Users\JOHNSM~1\.claude-work\hooks\AGENTN~1.EXE"))
            .then(|| PathBuf::from(r"C:\Users\John Smith\.claude-work\hooks\agentnotch-hook.exe"))
    };
    let recogniser = Recogniser::with_long_paths(PathStyle::Windows, &long_path);
    let plan = plan_install(
        Some(with(all_short).as_bytes()),
        &text_form(),
        &events(None),
        StatusLineWish::Leave,
        &Saved::NONE,
        &recogniser,
    );
    assert_eq!(commands(&written(&plan), "Stop"), [COMMAND]);
    assert!(asked.get() > 0);
    // Without the lookup it can't be told from someone else's, so it stays.
    let settings = written(&install(Some(with(all_short).as_bytes())));
    assert_eq!(commands(&settings, "Stop"), [all_short, COMMAND]);
}

/// A third party's hook that merely mentions the exe is not ours.
#[test]
fn third_party_hooks_mentioning_the_exe_are_not_removed() {
    let settings = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify.cmd --skip agentnotch-hook.exe"},{"type":"command","command":"C:/x/hooks/agentnotch-hook.exe hook"},{"type":"command","command":"C:/x/hooks/agentnotch-hook.exe statusline"},{"type":"command","command":"C:/x/hooks/agentnotch-hook.exe hook --verbose"}]}]}}"#;
    let cleaned = written(&uninstall(Some(settings.as_bytes()), None, None));
    assert_eq!(
        commands(&cleaned, "Stop"),
        [
            "notify.cmd --skip agentnotch-hook.exe",
            "C:/x/hooks/agentnotch-hook.exe statusline",
            "C:/x/hooks/agentnotch-hook.exe hook --verbose"
        ]
    );
}

/// A form that can't be written adds nothing (the caller doesn't hook such a
/// folder); entries already there still come out.
#[test]
fn a_form_that_cannot_be_written_adds_nothing() {
    let not_possible = Install {
        form: CommandForm::NotPossible("no".to_owned()),
        ..Install::default()
    };
    assert_eq!(
        not_possible.plan(Some(br#"{"model":"opus"}"#)).settings,
        SettingsWrite::AlreadyCurrent
    );
    let settings = written(&not_possible.plan(Some(REALISTIC.as_bytes())));
    assert!(first_hook_entry(&settings, &|entry| windows().is_our_hook(entry)).is_none());
}

#[test]
fn uninstall_takes_only_ours() {
    let installed = written_bytes(&install_with_version(REALISTIC.as_bytes(), latest()));
    let plan = uninstall(Some(&installed), None, None);
    assert_eq!(plan.previous_status_line, None);
    assert_eq!(plan.status_line, StatusLineIntent::Nothing);
    let mut expected = json(REALISTIC);
    let mut hooks = expected.get("hooks").unwrap().clone();
    hooks.set("TeammateIdle", None);
    expected.set("hooks", Some(hooks));
    let cleaned = written(&plan);
    assert!(cleaned.is_equivalent(&expected));
    assert_eq!(hook_entries(&cleaned).count(), 5);
    assert_eq!(
        hook_entries(&cleaned)
            .filter(|entry| is_upstream_hook(entry))
            .count(),
        2
    );

    // Nothing of ours: nothing to write. No file: nothing either.
    assert_eq!(
        uninstall(Some(&written_bytes(&plan)), None, None).settings,
        SettingsWrite::AlreadyCurrent
    );
    assert_eq!(
        uninstall(None, None, None),
        SettingsPlan {
            settings: SettingsWrite::AlreadyCurrent,
            previous_status_line: None,
            status_line: StatusLineIntent::Nothing,
        }
    );
    // The last hook going takes the empty `hooks` with it.
    let only_ours = written_bytes(&install(Some(br#"{"model":"opus"}"#)));
    assert_eq!(
        written_bytes(&uninstall(Some(&only_ours), None, None)),
        br#"{"model":"opus"}"#
    );
}

#[test]
fn removing_hooks_counts_and_drops_what_is_left_empty() {
    let hooks = json(
        r#"{"A":[{"matcher":"*","hooks":[{"command":"x hook"},{"command":"keep"}]},{"hooks":[{"command":"x hook"}]}],"B":[{"hooks":[{"command":"x hook"},{"command":"x hook"}]}],"C":"odd","D":[{"hooks":[{"command":"keep"}]}]}"#,
    );
    let (cleaned, removed) = removing_hooks(&hooks, &|entry| {
        entry.get("command").and_then(Json::as_str) == Some("x hook")
    });
    assert_eq!(removed, 4);
    assert_eq!(
        cleaned,
        json(
            r#"{"A":[{"matcher":"*","hooks":[{"command":"keep"}]}],"C":"odd","D":[{"hooks":[{"command":"keep"}]}]}"#
        )
    );
    let (same, none) = removing_hooks(&cleaned, &|_| false);
    assert_eq!((same, none), (cleaned, 0));
}

// ---- Status line ----

#[test]
fn wraps_the_existing_status_line_keeping_every_key() {
    let settings = r#"{"statusLine":{"type":"command","command":"~/.claude/statusline.sh","padding":2,"hideVimModeIndicator":true,"refreshInterval":5}}"#;
    let original = json(settings);
    let plan = wrapping(None).plan(Some(settings.as_bytes()));
    assert_eq!(plan.status_line, StatusLineIntent::Wrap);
    let written = written(&plan);
    let status_line = written.get("statusLine").unwrap();
    assert_eq!(
        status_line.get("command").and_then(Json::as_str),
        Some(STATUS)
    );
    assert_eq!(
        keys(status_line),
        [
            "type",
            "command",
            "padding",
            "hideVimModeIndicator",
            "refreshInterval"
        ]
    );
    assert!(Json::equivalent(
        status_line.get("hideVimModeIndicator"),
        Some(&Json::Bool(true))
    ));

    let saved = saved_bytes(&plan);
    assert!(Json::equivalent(
        Some(&object(&saved)),
        original.get("statusLine")
    ));
    assert_eq!(saved.last(), Some(&b'\n'));
}

#[test]
fn uninstall_restores_the_status_line_exactly() {
    let install = wrapping(latest()).plan(Some(REALISTIC.as_bytes()));
    let installed = written_bytes(&install);
    let saved = object(&saved_bytes(&install));

    let plan = uninstall(Some(&installed), Some(&saved), None);
    assert_eq!(
        plan.previous_status_line,
        Some(PreviousStatusLineChange::Remove)
    );
    assert_eq!(plan.status_line, StatusLineIntent::Unwrap);
    // As it was, minus our own stale entry that install cleaned up.
    let mut expected = json(REALISTIC);
    let mut hooks = expected.get("hooks").unwrap().clone();
    hooks.set("TeammateIdle", None);
    expected.set("hooks", Some(hooks));
    let restored = written(&plan);
    assert!(restored.is_equivalent(&expected));
    assert_eq!(
        restored.get("statusLine").unwrap().serialized(),
        expected.get("statusLine").unwrap().serialized()
    );
}

/// Settings the user changed on the wrapper entry while wrapped stay through
/// reinstalls and come along when it is unwrapped.
#[test]
fn user_edits_on_the_wrapper_are_kept() {
    let previous = json(r#"{"type":"command","command":"starship prompt","padding":1}"#);
    let mut wrapper = previous.clone();
    wrapper.set("command", Some(Json::string(STATUS)));
    wrapper.set("padding", Some(Json::int(5)));
    wrapper.set("hideVimModeIndicator", Some(Json::Bool(true)));
    let settings = Json::object([("statusLine", wrapper.clone())]).serialized();

    // Reinstall: the wrapper entry stays as the user left it.
    let reinstall = Install {
        saved: Some(previous.clone()),
        ..wrapping(None)
    }
    .plan(Some(settings.as_bytes()));
    assert_eq!(reinstall.status_line, StatusLineIntent::UpdateCommand);
    assert_eq!(
        written(&reinstall).get("statusLine").unwrap().serialized(),
        wrapper.serialized()
    );

    // Unwrap: the saved command, with the user's padding and addition.
    let restored = written(&uninstall(Some(settings.as_bytes()), Some(&previous), None));
    let status_line = restored.get("statusLine").unwrap();
    assert_eq!(
        status_line.get("command").and_then(Json::as_str),
        Some("starship prompt")
    );
    assert!(Json::equivalent(
        status_line.get("padding"),
        Some(&Json::int(5))
    ));
    assert!(Json::equivalent(
        status_line.get("hideVimModeIndicator"),
        Some(&Json::Bool(true))
    ));
    assert_eq!(
        keys(status_line),
        ["type", "command", "padding", "hideVimModeIndicator"]
    );
}

/// A `padding: 0` only the wrapper has isn't carried back onto a status line
/// that never had one.
#[test]
fn a_padding_of_zero_is_not_carried_back() {
    let previous = json(r#"{"type":"command","command":"starship prompt"}"#);
    let wrapper = json(&format!(
        r#"{{"type":"command","command":"{STATUS}","padding":0}}"#
    ));
    assert_eq!(
        restored(&previous, Some(&wrapper)).serialized(),
        previous.serialized()
    );
    // Any other padding the user set on the wrapper is theirs, and a zero the
    // saved one has too stays.
    let padded = json(&format!(
        r#"{{"type":"command","command":"{STATUS}","padding":4}}"#
    ));
    assert!(Json::equivalent(
        restored(&previous, Some(&padded)).get("padding"),
        Some(&Json::int(4))
    ));
    let had_zero = json(r#"{"type":"command","command":"starship prompt","padding":0}"#);
    assert_eq!(
        restored(&had_zero, Some(&wrapper)).serialized(),
        had_zero.serialized()
    );
    // Nothing of the wrapper's own comes along, and no wrapper changes nothing.
    let exec_wrapper =
        json(r#"{"type":"command","command":"C:\\x\\agentnotch-hook.exe","args":["statusline"]}"#);
    assert_eq!(restored(&previous, Some(&exec_wrapper)), previous);
    assert_eq!(restored(&previous, None), previous);
}

#[test]
fn no_previous_status_line_means_remove_on_uninstall() {
    let original = br#"{"model":"sonnet"}"#;
    let install = wrapping(None).plan(Some(original));
    // Saved as "nothing", so no older status line in a backup stands in.
    assert_eq!(
        install.previous_status_line,
        Some(PreviousStatusLineChange::ChainNothing)
    );
    assert_eq!(CHAINS_NOTHING, b"{}\n");
    assert_eq!(install.status_line, StatusLineIntent::Wrap);
    let installed = written_bytes(&install);
    let status_line = object(&installed).get("statusLine").cloned().unwrap();
    assert_eq!(
        status_line,
        json(&format!(r#"{{"type":"command","command":"{STATUS}"}}"#))
    );

    // The saved file says "nothing" (`{}`), so an older status line in a
    // backup never comes back.
    let nothing = json("{}");
    let older = json(r#"{"type":"command","command":"old.sh"}"#);
    let plan = uninstall(Some(&installed), Some(&nothing), Some(&older));
    assert_eq!(
        plan.previous_status_line,
        Some(PreviousStatusLineChange::Remove)
    );
    let restored = written(&plan);
    assert!(restored.get("statusLine").is_none());
    assert!(restored.get("hooks").is_none());
    assert_eq!(restored.get("model").and_then(Json::as_str), Some("sonnet"));
    assert_eq!(written_bytes(&plan), original);

    // The same with no saved file and no backup.
    assert_eq!(
        written_bytes(&uninstall(Some(&installed), None, None)),
        original
    );
}

/// The saved copy lost: the status line comes back from a backup instead of
/// disappearing.
#[test]
fn a_lost_saved_status_line_is_restored_from_a_backup() {
    let install = wrapping(latest()).plan(Some(REALISTIC.as_bytes()));
    let installed = written_bytes(&install);
    let backup = json(REALISTIC).get("statusLine").cloned().unwrap();
    let restored = written(&uninstall(Some(&installed), None, Some(&backup)));
    assert!(Json::equivalent(restored.get("statusLine"), Some(&backup)));

    // While it stays wrapped, the lost copy is put back at the next pass even
    // though settings.json itself needs no change.
    let again = Install {
        backup: Some(backup.clone()),
        ..wrapping(latest())
    }
    .plan(Some(&installed));
    assert_eq!(again.settings, SettingsWrite::AlreadyCurrent);
    assert_eq!(
        again.previous_status_line,
        Some(PreviousStatusLineChange::Recover(
            format!("{}\n", backup.serialized()).into_bytes()
        ))
    );

    // A backup that holds a wrapper is never chained to.
    let wrapper = object(&installed).get("statusLine").cloned().unwrap();
    let restored = written(&uninstall(Some(&installed), None, Some(&wrapper)));
    assert!(restored.get("statusLine").is_none());
}

/// The backups are read only when the saved copy is missing.
#[test]
fn the_backups_are_only_read_when_the_saved_copy_is_gone() {
    let saved = json(r#"{"type":"command","command":"starship prompt"}"#);
    let never = || -> Option<Json> { panic!("the saved copy is there") };
    let recogniser = windows();
    assert_eq!(
        chain_target(
            &Saved {
                previous_status_line: Some(&saved),
                backup_status_line: &never,
            },
            &recogniser
        ),
        Some(saved.clone())
    );
    let nothing = json("{}");
    assert_eq!(
        chain_target(
            &Saved {
                previous_status_line: Some(&nothing),
                backup_status_line: &never,
            },
            &recogniser
        ),
        None
    );
    let backup = || Some(saved.clone());
    assert_eq!(
        chain_target(
            &Saved {
                previous_status_line: None,
                backup_status_line: &backup,
            },
            &recogniser
        ),
        Some(saved.clone())
    );
    assert_eq!(chain_target(&Saved::NONE, &recogniser), None);

    // A plan that leaves the status line alone never asks for them either.
    let plan = plan_install(
        Some(REALISTIC.as_bytes()),
        &text_form(),
        &events(None),
        StatusLineWish::Leave,
        &Saved {
            previous_status_line: None,
            backup_status_line: &never,
        },
        &recogniser,
    );
    assert_eq!(plan.previous_status_line, None);
}

#[test]
fn turning_the_integration_off_restores_the_status_line() {
    let previous = json(
        r#"{"type": "command", "command": "~/.claude/statusline.sh", "padding": 2, "refreshInterval": 5}"#,
    );
    let installed = written_bytes(&wrapping(latest()).plan(Some(REALISTIC.as_bytes())));
    let disabled = Install {
        version: latest(),
        wish: StatusLineWish::Unwrap,
        saved: Some(previous.clone()),
        ..Install::default()
    }
    .plan(Some(&installed));
    let settings = written(&disabled);
    assert!(Json::equivalent(
        settings.get("statusLine"),
        Some(&previous)
    ));
    assert!(commands(&settings, "Stop").contains(&COMMAND.to_owned()));
    assert_eq!(
        disabled.previous_status_line,
        Some(PreviousStatusLineChange::Remove)
    );
    assert_eq!(disabled.status_line, StatusLineIntent::Unwrap);

    // Someone else's status line is not "unwrapped".
    let untouched = Install {
        wish: StatusLineWish::Unwrap,
        saved: Some(previous),
        ..Install::default()
    }
    .plan(Some(REALISTIC.as_bytes()));
    assert_eq!(untouched.previous_status_line, None);
    assert_eq!(untouched.status_line, StatusLineIntent::Nothing);
    assert!(Json::equivalent(
        written(&untouched).get("statusLine"),
        json(REALISTIC).get("statusLine")
    ));
}

#[test]
fn a_rewrap_keeps_the_entry_and_updates_the_command() {
    let previous = json(r#"{"type":"command","command":"starship prompt","padding":3}"#);
    let settings = br#"{"statusLine":{"type":"command","command":"C:/x/hooks/agentnotch-hook.exe statusline","padding":3}}"#;
    let plan = Install {
        saved: Some(previous.clone()),
        ..wrapping(None)
    }
    .plan(Some(settings));
    assert_eq!(plan.status_line, StatusLineIntent::UpdateCommand);
    let written_settings = written(&plan);
    let status_line = written_settings.get("statusLine").unwrap();
    assert_eq!(
        status_line.get("command").and_then(Json::as_str),
        Some(STATUS)
    );
    assert!(Json::equivalent(
        status_line.get("padding"),
        Some(&Json::int(3))
    ));
    assert_eq!(keys(status_line), ["type", "command", "padding"]);
    // Already ours: keep chaining to the saved status line, saved beside this
    // folder's wrapper too (it may have come from another folder's).
    assert!(Json::equivalent(
        Some(&object(&saved_bytes(&plan))),
        Some(&previous)
    ));

    // A wrapper someone wrote in exec form becomes the string it must be.
    let exec = br#"{"statusLine":{"type":"command","command":"C:\\x\\hooks\\agentnotch-hook.exe","args":["statusline"],"padding":3}}"#;
    let plan = wrapping(None).plan(Some(exec));
    assert_eq!(plan.status_line, StatusLineIntent::UpdateCommand);
    assert_eq!(plan.previous_status_line, None);
    assert_eq!(
        written(&plan).get("statusLine").unwrap(),
        &json(&format!(
            r#"{{"type":"command","command":"{STATUS}","padding":3}}"#
        ))
    );

    // Without Git Bash ours is still kept current: it chains through Git Bash
    // only when it runs.
    let no_bash = Install {
        wish: StatusLineWish::Wrap {
            command: STATUS,
            git_bash: false,
        },
        ..Install::default()
    }
    .plan(Some(settings));
    assert_eq!(no_bash.status_line, StatusLineIntent::UpdateCommand);
}

#[test]
fn a_rewrap_never_chains_to_itself() {
    for ours in [
        r#"{"type":"command","command":"C:/x/hooks/agentnotch-hook.exe statusline"}"#,
        r#"{"type":"command","command":"\"C:\\x\\hooks\\AGENTNOTCH-HOOK.EXE\" statusline"}"#,
    ] {
        let ours = json(ours);
        let settings = Json::object([("statusLine", ours.clone())]).serialized();
        let plan = Install {
            saved: Some(ours.clone()),
            ..wrapping(None)
        }
        .plan(Some(settings.as_bytes()));
        assert_eq!(plan.previous_status_line, None);
        let restored = written(&uninstall(Some(settings.as_bytes()), Some(&ours), None));
        assert!(restored.get("statusLine").is_none());
    }

    // Nor to a wrapper in a spelling only the loop guard knows: the Mac app's
    // script (a settings.json synced from a Mac), or the exe run through
    // something else.
    let settings = format!(r#"{{"statusLine":{{"type":"command","command":"{STATUS}"}}}}"#);
    for command in [
        "python3 '/x/hooks/agentnotch-statusline.py'",
        "python3 ~/.claude/hooks/superpowered-codenotch-statusline.py",
        "python3 ~/.claude/hooks/Superpowered-Notch-Statusline.py",
        "env X=1 C:/x/agentnotch-hook.exe statusline --verbose",
    ] {
        let saved = Json::object([
            ("type", Json::string("command")),
            ("command", Json::string(command)),
        ]);
        assert!(is_a_wrapper(&saved, &windows()), "{command}");
        let plan = Install {
            saved: Some(saved.clone()),
            ..wrapping(None)
        }
        .plan(Some(settings.as_bytes()));
        assert_eq!(plan.previous_status_line, None, "{command}");
        let plan = uninstall(Some(settings.as_bytes()), Some(&saved), None);
        assert_eq!(written_bytes(&plan), b"{}", "{command}");
    }
    assert!(!is_a_wrapper(
        &json(r#"{"type":"command","command":"starship prompt"}"#),
        &windows()
    ));
}

/// A status line that is wrapped only when it must behave the same through
/// Git Bash: anything else is left exactly as it is, with the reason.
#[test]
fn a_status_line_it_does_not_understand_is_left_alone() {
    let cases: [(&str, bool, &str); 14] = [
        (r#""echo hi""#, true, "it isn't a plain command"),
        (r#"{"command":"echo hi"}"#, true, "it isn't a plain command"),
        (
            r#"{"type":"command","command":"  "}"#,
            true,
            "it isn't a plain command",
        ),
        (
            r#"{"type":"command","command":"C:\\tools\\status.exe"}"#,
            true,
            "its command uses Windows paths",
        ),
        (
            r#"{"type":"command","command":"~/.claude/status.ps1"}"#,
            true,
            "its command needs PowerShell",
        ),
        (
            r#"{"type":"command","command":"echo $env:USERNAME"}"#,
            true,
            "its command needs PowerShell",
        ),
        (
            r#"{"type":"command","command":"PowerShell -File status"}"#,
            true,
            "its command needs PowerShell",
        ),
        (
            r#"{"type":"command","command":"pwsh -c status"}"#,
            true,
            "its command needs PowerShell",
        ),
        (
            r#"{"type":"command","command":"status","shell":"powershell"}"#,
            true,
            "its command needs PowerShell",
        ),
        (
            r#"{"type":"command","command":"status","shell":"bash"}"#,
            true,
            "it names its own shell",
        ),
        (
            r#"{"type":"command","command":"CMD.EXE /c status.bat"}"#,
            true,
            "its command needs cmd.exe",
        ),
        (
            r#"{"type":"command","command":"cmd /C status.bat"}"#,
            true,
            "its command needs cmd.exe",
        ),
        (
            r#"{"type":"command","command":"~/.claude/statusline.sh","padding":2}"#,
            false,
            "Git Bash isn't installed",
        ),
        (
            r#"{"type":"command","command":"python3 ~/.claude/hooks/agentnotch-statusline.py"}"#,
            true,
            "it runs this app's own wrapper",
        ),
    ];
    for (status_line, git_bash, why) in cases {
        let reason = StatusLineIntent::LeaveAlone(format!("Status line left alone: {why}"));
        let wish = StatusLineWish::Wrap {
            command: STATUS,
            git_bash,
        };
        // Spaced oddly on purpose: the entry must keep its bytes.
        let settings =
            format!("{{\r\n\t\"statusLine\":   {status_line} ,\r\n\t\"model\": \"opus\"\r\n}}\r\n");

        // Nothing else to do: nothing is written and nothing is saved.
        let plan = plan_install(
            Some(settings.as_bytes()),
            &text_form(),
            &[],
            wish,
            &Saved::NONE,
            &windows(),
        );
        assert_eq!(
            plan,
            SettingsPlan {
                settings: SettingsWrite::AlreadyCurrent,
                previous_status_line: None,
                status_line: reason.clone(),
            },
            "{status_line}"
        );

        // Hooks go in around it.
        let plan = Install {
            wish,
            ..Install::default()
        }
        .plan(Some(settings.as_bytes()));
        assert_eq!(plan.status_line, reason, "{status_line}");
        assert_eq!(plan.previous_status_line, None, "{status_line}");
        let text = String::from_utf8(written_bytes(&plan)).unwrap();
        assert!(
            text.starts_with(&format!(
                "{{\r\n\t\"statusLine\":   {status_line} ,\r\n\t\"model\": \"opus\",\r\n\t\"hooks\": {{"
            )),
            "{status_line}: {text}"
        );
        assert_eq!(commands(&object(text.as_bytes()), "Stop"), [COMMAND]);

        // And an uninstall gives the file back as it was.
        let back = uninstall(Some(text.as_bytes()), None, None);
        assert_eq!(back.status_line, StatusLineIntent::Nothing);
        assert_eq!(written_bytes(&back), settings.as_bytes(), "{status_line}");
    }

    // The three reasons the design names, word for word.
    let reason = |status_line: &str, git_bash: bool| {
        let settings = format!(r#"{{"statusLine":{status_line}}}"#);
        let plan = Install {
            wish: StatusLineWish::Wrap {
                command: STATUS,
                git_bash,
            },
            ..Install::default()
        }
        .plan(Some(settings.as_bytes()));
        match plan.status_line {
            StatusLineIntent::LeaveAlone(reason) => reason,
            other => panic!("expected it left alone, got {other:?}"),
        }
    };
    assert_eq!(
        reason(r#"{"type":"command","command":"C:\\s.exe"}"#, true),
        "Status line left alone: its command uses Windows paths"
    );
    assert_eq!(
        reason(r#"{"type":"command","command":"s.ps1"}"#, true),
        "Status line left alone: its command needs PowerShell"
    );
    assert_eq!(
        reason(r#"{"type":"command","command":"s.sh"}"#, false),
        "Status line left alone: Git Bash isn't installed"
    );
}

// ---- Settings stay byte for byte ----

/// As Claude Code writes it: `JSON.stringify(value, null, 2)`, unsorted keys,
/// numbers and escapes as the user typed them.
const CLAUDE_STYLE: &str = r#"{
  "permissions": {
    "allow": [
      "Bash(npm test:*)"
    ],
    "deny": []
  },
  "feedbackSurveyRate": 0.1,
  "tiny": 1e-7,
  "one": 1.0,
  "path": "caf\u00e9 \/ slash",
  "env": {
    "ZED": "1",
    "ALPHA": "2"
  },
  "statusLine": {
    "type": "command",
    "command": "~/.claude/statusline.sh",
    "hideVimModeIndicator": true
  },
  "model": "opus"
}"#;

/// Install then uninstall on `original`, checking the untouched members on
/// the way; `newline` is the file's line ending.
fn round_trip(original: &[u8], newline: &str, bom: bool) {
    let install = wrapping(None).plan(Some(original));
    let installed = written_bytes(&install);
    let saved = object(&saved_bytes(&install));

    assert_eq!(installed.starts_with(BOM), bom);
    let body = installed.strip_prefix(BOM).unwrap_or(&installed);
    let text = std::str::from_utf8(body).unwrap();
    // Untouched members keep their exact spelling and order.
    for fragment in [
        "\"feedbackSurveyRate\": 0.1,".to_owned(),
        "\"tiny\": 1e-7,".to_owned(),
        "\"one\": 1.0,".to_owned(),
        r#""path": "caf\u00e9 \/ slash","#.to_owned(),
        format!("\"ZED\": \"1\",{newline}    \"ALPHA\": \"2\""),
        "\"deny\": []".to_owned(),
    ] {
        assert!(text.contains(&fragment), "lost {fragment}");
    }
    // (An editor's newline at the very end stays after the brace.)
    assert!(text
        .trim_end_matches(['\r', '\n'])
        .ends_with(&format!("{newline}}}")));
    assert_eq!(
        text.ends_with(newline),
        original.ends_with(newline.as_bytes())
    );
    // What was spliced in uses the file's own line ending, and only it.
    if newline == "\r\n" {
        assert_eq!(text.matches('\n').count(), text.matches("\r\n").count());
    } else {
        assert!(!text.contains('\r'));
    }
    assert!(text.contains(&format!(
        "{newline}  \"hooks\": {{{newline}    \"UserPromptSubmit\": ["
    )));
    let parsed = object(&installed);
    assert_eq!(commands(&parsed, "Stop"), [COMMAND]);
    assert_eq!(
        parsed
            .get("statusLine")
            .and_then(|status_line| status_line.get("command"))
            .and_then(Json::as_str),
        Some(STATUS)
    );
    // The saved copy is the wrapper's own file: always `\n`, never a BOM.
    assert_eq!(
        saved.serialized(),
        "{\n  \"type\": \"command\",\n  \"command\": \"~/.claude/statusline.sh\",\n  \"hideVimModeIndicator\": true\n}"
    );

    let uninstall = uninstall(Some(&installed), Some(&saved), None);
    assert_eq!(written_bytes(&uninstall), original);
}

#[test]
fn install_then_uninstall_gives_back_the_same_bytes() {
    round_trip(CLAUDE_STYLE.as_bytes(), "\n", false);
    // With the newline at the end an editor adds.
    round_trip(format!("{CLAUDE_STYLE}\n").as_bytes(), "\n", false);
}

#[test]
fn install_then_uninstall_gives_back_the_same_bytes_with_crlf() {
    let crlf = CLAUDE_STYLE.replace('\n', "\r\n");
    round_trip(crlf.as_bytes(), "\r\n", false);
    round_trip(format!("{crlf}\r\n").as_bytes(), "\r\n", false);
}

#[test]
fn install_then_uninstall_gives_back_the_same_bytes_with_a_bom() {
    round_trip(&[BOM, CLAUDE_STYLE.as_bytes()].concat(), "\n", true);
    let crlf = CLAUDE_STYLE.replace('\n', "\r\n");
    round_trip(&[BOM, crlf.as_bytes()].concat(), "\r\n", true);
    round_trip(&[BOM, crlf.as_bytes(), b"\r\n"].concat(), "\r\n", true);
}

#[test]
fn a_compact_file_stays_compact() {
    let original = r#"{"model":"opus","env":{"B":"1","A":"2"}}"#;
    for bom in [false, true] {
        let bytes = if bom {
            [BOM, original.as_bytes()].concat()
        } else {
            original.as_bytes().to_vec()
        };
        let data = written_bytes(&install(Some(&bytes)));
        assert_eq!(data.starts_with(BOM), bom);
        let text = std::str::from_utf8(data.strip_prefix(BOM).unwrap_or(&data)).unwrap();
        assert!(text.starts_with(r#"{"model":"opus","env":{"B":"1","A":"2"},"hooks":{"#));
        assert!(!text.contains('\n') && !text.contains('\r'));
        assert!(!text.contains(": ") && !text.contains(", "));
        let back = uninstall(Some(&data), None, None);
        assert_eq!(written_bytes(&back), bytes);
    }
}

/// A blank file keeps its BOM and line ending when it is first written.
#[test]
fn a_blank_file_keeps_its_bom_and_line_ending() {
    let blank = [BOM, b"\r\n"].concat();
    let data = written_bytes(&install(Some(&blank)));
    assert!(data.starts_with(BOM));
    let text = std::str::from_utf8(&data[BOM.len()..]).unwrap();
    assert_eq!(text.matches('\n').count(), text.matches("\r\n").count());
    assert!(text.contains("\r\n"));
    assert_eq!(commands(&object(&data), "Stop"), [COMMAND]);
}

// ---- The official app's hooks ----

/// `remove_codenotch_hooks`: the official app's entries go, under every name
/// its hook exe has had, in either form; ours and third parties' stay.
#[test]
fn upstream_removal_counts_and_leaves_everyone_else_alone() {
    let settings = r#"{"model":"opus","hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"~/bin/guard-bash.sh","timeout":30}]},{"matcher":"*","hooks":[{"type":"command","command":"C:/Users/me/.claude/hooks/codenotch-hook.exe"}]},{"matcher":"*","hooks":[{"type":"command","command":"C:/Users/me/.claude/hooks/agentnotch-hook.exe hook"}]}],"Stop":[{"hooks":[{"type":"command","command":"notify.cmd --skip codenotch-hook.exe"},{"type":"command","command":"\"C:\\Users\\me\\.claude\\hooks\\CODENOTCH-HOOK.EXE\""},{"type":"command","command":"C:\\Users\\me\\.claude\\hooks\\eatbean-hook.exe","args":[]},{"type":"command","command":"C:/Users/me/.claude/hooks/agentnotch-hook.exe hook"}]}],"SessionEnd":[{"hooks":[{"type":"command","command":"/c/Users/me/.claude/hooks/pacman-hook"}]}]},"statusLine":{"type":"command","command":"C:/Users/me/.claude/hooks/agentnotch-hook.exe statusline"}}"#;
    let (plan, removed) = plan_upstream_removal(Some(settings.as_bytes()));
    assert_eq!(removed, 4);
    assert_eq!(plan.previous_status_line, None);
    assert_eq!(plan.status_line, StatusLineIntent::Nothing);

    // Everything but the four entries keeps its bytes; the group and the event
    // left empty go with them.
    let expected = r#"{"model":"opus","hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"~/bin/guard-bash.sh","timeout":30}]},{"matcher":"*","hooks":[{"type":"command","command":"C:/Users/me/.claude/hooks/agentnotch-hook.exe hook"}]}],"Stop":[{"hooks":[{"type":"command","command":"notify.cmd --skip codenotch-hook.exe"},{"type":"command","command":"C:/Users/me/.claude/hooks/agentnotch-hook.exe hook"}]}]},"statusLine":{"type":"command","command":"C:/Users/me/.claude/hooks/agentnotch-hook.exe statusline"}}"#;
    assert_eq!(String::from_utf8(written_bytes(&plan)).unwrap(), expected);

    // Nothing of its left: nothing to write, nothing counted.
    assert_eq!(
        plan_upstream_removal(Some(expected.as_bytes())),
        (
            SettingsPlan {
                settings: SettingsWrite::AlreadyCurrent,
                previous_status_line: None,
                status_line: StatusLineIntent::Nothing,
            },
            0
        )
    );
}

#[test]
fn upstream_removal_keeps_the_files_layout() {
    // The realistic file holds two of its entries, one sharing a group with a
    // third party's.
    let (plan, removed) = plan_upstream_removal(Some(REALISTIC.as_bytes()));
    assert_eq!(removed, 2);
    let cleaned = written(&plan);
    assert_eq!(commands(&cleaned, "PreToolUse"), ["~/bin/guard-bash.sh"]);
    assert_eq!(
        commands(&cleaned, "PermissionRequest"),
        ["node C:/tools/bridge.js --source claude"]
    );
    // Ours (the stale one here) is not its business.
    assert_eq!(
        commands(&cleaned, "TeammateIdle"),
        ["C:/old/path/hooks/agentnotch-hook.exe hook"]
    );
    let mut expected = json(REALISTIC);
    let (hooks, count) = removing_hooks(expected.get("hooks").unwrap(), &is_upstream_hook);
    assert_eq!(count, 2);
    expected.set("hooks", Some(hooks));
    assert!(cleaned.is_equivalent(&expected));
    let text = String::from_utf8(written_bytes(&plan)).unwrap();
    let (before, after) = REALISTIC.split_once("  \"hooks\": {").unwrap();
    assert!(text.starts_with(before));
    assert!(text.ends_with(after.rsplit_once("  },\n").unwrap().1));

    // CRLF and a BOM are kept.
    let crlf = [BOM, REALISTIC.replace('\n', "\r\n").as_bytes()].concat();
    let (plan, removed) = plan_upstream_removal(Some(&crlf));
    assert_eq!(removed, 2);
    let data = written_bytes(&plan);
    assert!(data.starts_with(BOM));
    let text = std::str::from_utf8(&data[BOM.len()..]).unwrap();
    assert_eq!(text.matches('\n').count(), text.matches("\r\n").count());
    assert!(object(&data).is_equivalent(&expected));
}

#[test]
fn upstream_removal_with_nothing_to_remove_writes_nothing() {
    let nothing = SettingsPlan {
        settings: SettingsWrite::AlreadyCurrent,
        previous_status_line: None,
        status_line: StatusLineIntent::Nothing,
    };
    assert_eq!(plan_upstream_removal(None), (nothing.clone(), 0));
    assert_eq!(plan_upstream_removal(Some(b"")), (nothing.clone(), 0));
    assert_eq!(
        plan_upstream_removal(Some(CLAUDE_STYLE.as_bytes())),
        (nothing.clone(), 0)
    );
    // Ours alone, and an odd event value: both stay.
    let installed = written_bytes(&wrapping(latest()).plan(Some(CLAUDE_STYLE.as_bytes())));
    assert_eq!(
        plan_upstream_removal(Some(&installed)),
        (nothing.clone(), 0)
    );
    assert_eq!(
        plan_upstream_removal(Some(br#"{"hooks":{"Stop":"codenotch-hook.exe"}}"#)),
        (nothing, 0)
    );
    // Its last entry going takes the empty `hooks` along.
    let only = br#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"C:/x/codenotch-hook.exe"}]}]}}"#;
    let (plan, removed) = plan_upstream_removal(Some(only));
    assert_eq!(removed, 1);
    assert_eq!(written_bytes(&plan), br#"{"model":"opus"}"#);
}
