//! The session registry reader (SessionRegistryScanner.swift): the entry
//! file's fields, which names are ever opened, the liveness rule, grouping
//! of shared folders and attribution of their entries (SessionCoreRegression,
//! DesktopHostedSessions and PP_Session tests, and the Windows vectors).
//! Temporary folders and a scripted process table only.

mod sessions_support;

use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::core::time::{from_ms, to_ms};
use agentnotch_engine::model::RegistryEntry;
use agentnotch_engine::platform::{EnvRead, Liveness, ProcessTable, Processes};
use agentnotch_engine::sessions::registry::{
    attribute, attribute_entry, check_live, grouped_by_sessions_folder, is_linked_sessions_folder,
    parse_entry, parse_proc_start, read_registry, resolve_path, sessions_folder_of,
    MAX_ENTRY_BYTES,
};
use agentnotch_engine::testkit::process::FakeProcesses;
use serde_json::json;
use sessions_support::LinkedFiles;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

/// 2026-09-21 14:13:20 UTC.
const MON_SEP_21: u64 = 1_790_000_000;

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn entry(pid: u32, session: &str) -> RegistryEntry {
    RegistryEntry::new(pid, session)
}

fn write_entry(dir: &Path, pid: u32, body: serde_json::Value) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(format!("{pid}.json")), body.to_string()).unwrap();
}

// ---- Parsing ----

#[test]
fn registry_entry_parses_name_source() {
    let entry = parse_entry(&json!({
        "pid": 123, "sessionId": "s1", "name": "proj-3", "nameSource": "derived", "status": "idle",
    }))
    .unwrap();
    assert!(entry.is_name_derived());
    assert_eq!(entry.name.as_deref(), Some("proj-3"));
    let chosen = parse_entry(&json!({"pid": 123, "sessionId": "s1", "name": "Mine"})).unwrap();
    assert!(!chosen.is_name_derived());
}

#[test]
fn every_field_is_read_and_times_are_epoch_milliseconds() {
    let entry = parse_entry(&json!({
        "pid": 4242, "sessionId": "s-1", "cwd": r"C:\Users\me\proj", "kind": "interactive",
        "entrypoint": "claude-vscode", "name": "proj", "version": "2.1.282",
        "status": "waiting", "waitingFor": "permission prompt",
        "startedAt": 1_790_000_000_000u64, "updatedAt": 1_790_000_005_500u64,
        "statusUpdatedAt": 1_790_000_003_250u64, "procStart": "Mon Sep 21 14:13:20 2026",
    }))
    .unwrap();
    assert_eq!(entry.pid, 4242);
    assert_eq!(entry.session_id.as_str(), "s-1");
    assert_eq!(entry.cwd.as_deref(), Some(r"C:\Users\me\proj"));
    assert_eq!(entry.status.as_deref(), Some("waiting"));
    assert_eq!(entry.waiting_for.as_deref(), Some("permission prompt"));
    assert_eq!(entry.started_at, Some(from_ms(1_790_000_000_000)));
    assert_eq!(entry.updated_at, Some(from_ms(1_790_000_005_500)));
    assert_eq!(entry.status_changed_at(), Some(from_ms(1_790_000_003_250)));
    assert_eq!(
        entry.proc_start.as_deref(),
        Some("Mon Sep 21 14:13:20 2026")
    );
    assert!(entry.is_tracked());
    // The last update stands in for a status change the file doesn't date.
    let plain = parse_entry(&json!({"pid": 1, "sessionId": "s", "updatedAt": 5000})).unwrap();
    assert_eq!(plain.status_changed_at(), Some(from_ms(5000)));
    // Zero, negative, text and missing times are none; a numeric string is read.
    let odd = parse_entry(&json!({
        "pid": "77", "sessionId": "s", "startedAt": 0, "updatedAt": -4, "statusUpdatedAt": "soon",
    }))
    .unwrap();
    assert_eq!(odd.pid, 77);
    assert_eq!(
        (odd.started_at, odd.updated_at, odd.status_updated_at),
        (None, None, None)
    );
    // Both required fields.
    assert!(parse_entry(&json!({"pid": 5})).is_none());
    assert!(parse_entry(&json!({"sessionId": "s"})).is_none());
    assert!(parse_entry(&json!([1, 2])).is_none());
    // Tracking: background kinds and SDK entrypoints are ignored.
    for (kind, entrypoint, tracked) in [
        ("bg", "cli", false),
        ("daemon", "cli", false),
        ("daemon-worker", "cli", false),
        ("interactive", "sdk-ts", false),
        ("interactive", "claude-vscode", true),
    ] {
        let entry = parse_entry(
            &json!({"pid": 1, "sessionId": "s", "kind": kind, "entrypoint": entrypoint}),
        )
        .unwrap();
        assert_eq!(entry.is_tracked(), tracked, "{kind} {entrypoint}");
    }
    assert!(parse_entry(&json!({"pid": 1, "sessionId": "s"}))
        .unwrap()
        .is_tracked());
}

#[test]
fn out_of_range_pids_are_dropped_instead_of_trapping() {
    for bad in [
        json!(99_999_999_999u64),
        json!(-5),
        json!(0),
        json!(2_147_483_648u64),
        json!(1.0e30),
        json!(true),
        json!(null),
        json!("abc"),
        json!(u64::MAX),
    ] {
        assert!(
            parse_entry(&json!({"pid": bad, "sessionId": "s1"})).is_none(),
            "{bad}"
        );
    }
    for (good, pid) in [
        (json!(1), 1),
        (json!(2_147_483_647u64), 2_147_483_647),
        (json!(12.0), 12),
    ] {
        assert_eq!(
            parse_entry(&json!({"pid": good, "sessionId": "s1"}))
                .unwrap()
                .pid,
            pid
        );
    }
}

// ---- The job: which files are read ----

/// A registry folder with decoy files that parse as valid entries: if the
/// reader opened any name that isn't `<digits>.json` they would show up.
#[test]
fn only_the_entry_file_is_read() {
    let home = TempDir::new().unwrap();
    let sessions = home.path().join("sessions");
    let body = json!({"pid": 4242, "sessionId": "s-a", "entrypoint": "claude-desktop",
                      "hostSessionId": "local_0123abcd-89ef-4a5b-8c6d-001122334455"});
    write_entry(&sessions, 4242, body.clone());
    let decoy = |name: &str| {
        let mut decoy = body.clone();
        decoy["pid"] = json!(4343);
        decoy["sessionId"] = json!(format!("decoy-{name}"));
        std::fs::write(sessions.join(name), decoy.to_string()).unwrap();
    };
    for name in [
        "4343.key",
        "4343.json.key",
        "4343.json.bak",
        "4343.JSON",
        "notes.json",
        "-5.json",
        "+7.json",
        "1e3.json",
        ".json",
        "4343",
        "4343.json~",
        "abc4343.json",
    ] {
        decoy(name);
    }
    // A folder named like an entry is not a file.
    std::fs::create_dir_all(sessions.join("5555.json")).unwrap();
    std::fs::write(sessions.join("5555.json").join("1.json"), b"{}").unwrap();
    let procs = FakeProcesses::default();
    procs.add(4242, 1, "claude.exe", at(MON_SEP_21 - 600));
    let snapshot = read_registry(&sessions, false, &procs, at(MON_SEP_21));
    let sessions_read: Vec<&str> = snapshot
        .entries
        .iter()
        .map(|e| e.session_id.as_str())
        .collect();
    assert_eq!(sessions_read, ["s-a"]);
    assert_eq!(
        snapshot.entries[0].host_session_id.as_deref(),
        Some("local_0123abcd-89ef-4a5b-8c6d-001122334455")
    );
    assert_eq!(snapshot.error, None);
    assert_eq!(snapshot.read_at, at(MON_SEP_21));
    assert_eq!(snapshot.sessions_dir, sessions);
    assert!(!snapshot.via_link);
}

#[test]
fn a_file_of_a_megabyte_is_never_read_and_one_just_under_is() {
    let home = TempDir::new().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let pad = |pid: u32, total: u64| {
        let mut text = json!({"pid": pid, "sessionId": format!("s-{pid}")}).to_string();
        while (text.len() as u64) < total {
            text.push(' ');
        }
        std::fs::write(sessions.join(format!("{pid}.json")), text).unwrap();
    };
    pad(10, MAX_ENTRY_BYTES - 1);
    pad(11, MAX_ENTRY_BYTES);
    pad(12, MAX_ENTRY_BYTES + 5000);
    std::fs::write(sessions.join("13.json"), b"{not json").unwrap();
    std::fs::write(sessions.join("14.json"), b"[1,2]").unwrap();
    std::fs::write(sessions.join("15.json"), b"").unwrap();
    let snapshot = read_registry(&sessions, false, &FakeProcesses::default(), at(MON_SEP_21));
    let pids: Vec<u32> = snapshot.entries.iter().map(|e| e.pid).collect();
    assert_eq!(pids, [10]);
}

#[test]
fn entries_come_sorted_by_pid_and_a_missing_folder_is_an_empty_read() {
    let home = TempDir::new().unwrap();
    let sessions = home.path().join("sessions");
    for pid in [300, 20, 4000, 7] {
        write_entry(
            &sessions,
            pid,
            json!({"pid": pid, "sessionId": format!("s{pid}"), "kind": "bg"}),
        );
    }
    let snapshot = read_registry(&sessions, false, &FakeProcesses::default(), at(1));
    // Untracked entries are kept (the store filters); all are gone processes here.
    let read: Vec<(u32, bool)> = snapshot.entries.iter().map(|e| (e.pid, e.live)).collect();
    assert_eq!(read, [(7, false), (20, false), (300, false), (4000, false)]);
    assert!(snapshot.entries.iter().all(|e| !e.is_tracked()));
    // Absent: nothing, and not an error (a machine where Claude Code writes none).
    let missing = read_registry(
        &home.path().join("nowhere"),
        true,
        &FakeProcesses::default(),
        at(1),
    );
    assert!(missing.entries.is_empty());
    assert_eq!(missing.error, None);
    assert!(missing.via_link);
    // A file where the folder should be can't be listed: said, never a panic.
    let file = home.path().join("file");
    std::fs::write(&file, b"x").unwrap();
    let broken = read_registry(&file, false, &FakeProcesses::default(), at(1));
    assert!(broken.entries.is_empty());
    assert!(broken.error.is_some());
}

// ---- Liveness ----

#[test]
fn proc_start_is_utc_and_tolerates_a_space_padded_day() {
    assert_eq!(
        parse_proc_start("Mon Sep 21 14:13:20 2026"),
        Some(at(MON_SEP_21))
    );
    // `ps` pads a one-digit day with a space; so does any run of blanks.
    assert_eq!(
        parse_proc_start("Wed Sep  3 04:43:16 2026"),
        Some(at(MON_SEP_21 - (18 * 86400 + 9 * 3600 + 30 * 60 + 4)))
    );
    assert_eq!(
        parse_proc_start("  Wed   Sep 3   04:43:16   2026 "),
        parse_proc_start("Wed Sep  3 04:43:16 2026")
    );
    // The weekday is read past, not checked.
    assert_eq!(
        parse_proc_start("Fri Sep 21 14:13:20 2026"),
        Some(at(MON_SEP_21))
    );
    for bad in [
        "",
        "Mon Sep 21 14:13:20",
        "Mon Foo 21 14:13:20 2026",
        "2026-09-21T14:13:20Z",
        "Mon Sep 31 14:13:20 2026",
        "Mon Sep 21 25:13:20 2026",
    ] {
        assert_eq!(parse_proc_start(bad), None, "{bad}");
    }
}

#[test]
fn liveness_follows_the_process_creation_time() {
    let proc_start = Some("Mon Sep 21 14:13:20 2026");
    let created = |offset_ms: i64| {
        Some(if offset_ms >= 0 {
            at(MON_SEP_21) + Duration::from_millis(offset_ms as u64)
        } else {
            at(MON_SEP_21) - Duration::from_millis(offset_ms.unsigned_abs())
        })
    };
    // procStart: within 2 s either way is the same process; 3 s off is not.
    assert!(check_live(Liveness::Alive, created(0), proc_start, None));
    assert!(check_live(Liveness::Alive, created(2000), proc_start, None));
    assert!(check_live(
        Liveness::Alive,
        created(-2000),
        proc_start,
        None
    ));
    assert!(!check_live(
        Liveness::Alive,
        created(3000),
        proc_start,
        None
    ));
    assert!(!check_live(
        Liveness::Alive,
        created(-3000),
        proc_start,
        None
    ));
    assert!(!check_live(
        Liveness::Alive,
        created(2001),
        proc_start,
        None
    ));
    // procStart wins over startedAt.
    assert!(check_live(
        Liveness::Alive,
        created(0),
        proc_start,
        Some(at(1))
    ));
    // No procStart: a reused pid's process started after the entry was written.
    let started_at = Some(at(MON_SEP_21));
    assert!(check_live(
        Liveness::Alive,
        created(-60_000),
        None,
        started_at
    ));
    assert!(check_live(Liveness::Alive, created(5000), None, started_at));
    assert!(!check_live(
        Liveness::Alive,
        created(5001),
        None,
        started_at
    ));
    // An unparseable procStart falls back to startedAt.
    assert!(!check_live(
        Liveness::Alive,
        created(9000),
        Some("garbage"),
        started_at
    ));
    assert!(check_live(
        Liveness::Alive,
        created(9000),
        Some("garbage"),
        None
    ));
    // Nothing to compare: running is live. Gone is never live. An unknown probe doesn't drop it.
    assert!(check_live(Liveness::Alive, created(0), None, None));
    assert!(check_live(Liveness::Alive, None, proc_start, started_at));
    assert!(!check_live(
        Liveness::Gone,
        created(0),
        proc_start,
        started_at
    ));
    assert!(!check_live(Liveness::Gone, None, None, None));
    assert!(check_live(Liveness::Unknown, None, None, None));
}

#[test]
fn the_job_fills_each_entrys_process_facts() {
    let home = TempDir::new().unwrap();
    let sessions = home.path().join("sessions");
    let procs = FakeProcesses::default();
    // 100: same process (procStart matches). 200: pid reused (created 60 s later).
    // 300: no procStart, started before its entry. 400: no process. 500: no times.
    write_entry(
        &sessions,
        100,
        json!({"pid": 100, "sessionId": "same", "procStart": "Mon Sep 21 14:13:20 2026"}),
    );
    procs.add(100, 1, "claude.exe", at(MON_SEP_21 + 1));
    write_entry(
        &sessions,
        200,
        json!({"pid": 200, "sessionId": "reused", "procStart": "Mon Sep 21 14:13:20 2026"}),
    );
    procs.add(200, 1, "claude.exe", at(MON_SEP_21 + 60));
    write_entry(
        &sessions,
        300,
        json!({"pid": 300, "sessionId": "startedAt", "startedAt": MON_SEP_21 * 1000}),
    );
    procs.add(300, 1, "claude.exe", at(MON_SEP_21 - 30));
    write_entry(
        &sessions,
        400,
        json!({"pid": 400, "sessionId": "gone", "startedAt": MON_SEP_21 * 1000}),
    );
    write_entry(&sessions, 500, json!({"pid": 500, "sessionId": "bare"}));
    procs.add(500, 1, "claude.exe", at(MON_SEP_21 + 3600));
    procs.set_config_dir_env(100, EnvRead::Set("C:/a".into()));
    procs.set_config_dir_env(200, EnvRead::Set("C:/b".into()));
    procs.set_config_dir_env(500, EnvRead::Unset);
    let read = |via_link| read_registry(&sessions, via_link, &procs, at(MON_SEP_21 + 100)).entries;

    let direct = read(false);
    let facts: Vec<(&str, bool, Option<SystemTime>)> = direct
        .iter()
        .map(|e| (e.session_id.as_str(), e.live, e.process_started))
        .collect();
    assert_eq!(
        facts,
        [
            ("same", true, Some(at(MON_SEP_21 + 1))),
            ("reused", false, Some(at(MON_SEP_21 + 60))),
            ("startedAt", true, Some(at(MON_SEP_21 - 30))),
            ("gone", false, None),
            ("bare", true, Some(at(MON_SEP_21 + 3600))),
        ]
    );
    // The environment is read only for a folder reached through a link, and
    // only for entries that are live.
    assert!(direct.iter().all(|e| e.process_config_dir.is_none()));
    let linked = read(true);
    let envs: Vec<(&str, Option<EnvRead>)> = linked
        .iter()
        .map(|e| (e.session_id.as_str(), e.process_config_dir.clone()))
        .collect();
    assert_eq!(
        envs,
        [
            ("same", Some(EnvRead::Set("C:/a".into()))),
            ("reused", None),
            ("startedAt", Some(EnvRead::Unreadable)),
            ("gone", None),
            ("bare", Some(EnvRead::Unset)),
        ]
    );
    assert_eq!(to_ms(at(5)), 5000);
}

// ---- Attribution ----

fn posix() -> Paths {
    Paths::new(PathStyle::Posix, "/Users/me")
}

fn with_env(pid: u32, session: &str, env: Option<EnvRead>) -> RegistryEntry {
    let mut e = entry(pid, session);
    e.process_config_dir = env;
    e
}

fn no_resolve(path: &str) -> String {
    path.to_owned()
}

#[test]
fn attribution_follows_each_process() {
    let paths = posix();
    let home = "/Users/me";
    let aliases: Vec<String> = [
        "/Users/me/.claude",
        "/Users/me/.claude-windows/801f9dd51396",
        "/Users/me/.claude-windows/1bf3e8f92b11",
    ]
    .map(String::from)
    .to_vec();
    let entries = vec![
        with_env(
            11,
            "s11",
            Some(EnvRead::Set(
                "/Users/me/.claude-windows/801f9dd51396/".into(),
            )),
        ),
        with_env(12, "s12", Some(EnvRead::Unset)),
        with_env(
            13,
            "s13",
            Some(EnvRead::Set(
                "/Users/me/.claude-windows/1bf3e8f92b11".into(),
            )),
        ),
        with_env(14, "s14", Some(EnvRead::Unreadable)),
        // A window not known yet.
        with_env(
            15,
            "s15",
            Some(EnvRead::Set(
                "/Users/me/.claude-windows/0a1b2c3d4e5f".into(),
            )),
        ),
        // Never asked (a snapshot that wasn't linked): can't be asked either.
        with_env(16, "s16", None),
    ];
    let result = attribute(
        &entries,
        &aliases,
        true,
        &format!("{home}/.claude"),
        &paths,
        &no_resolve,
    );
    let pids = |dir: &str| {
        result
            .get(dir)
            .map(|v| v.iter().map(|e| e.pid).collect::<Vec<_>>())
    };
    assert_eq!(
        pids("/Users/me/.claude-windows/801f9dd51396"),
        Some(vec![11])
    );
    assert_eq!(pids("/Users/me/.claude"), Some(vec![12, 14, 16]));
    assert_eq!(
        pids("/Users/me/.claude-windows/1bf3e8f92b11"),
        Some(vec![13])
    );
    assert_eq!(
        pids("/Users/me/.claude-windows/0a1b2c3d4e5f"),
        Some(vec![15])
    );
    assert_eq!(result.len(), 4);

    // A folder only one config folder reaches directly: all of them are its own, no process asked.
    let single = attribute(
        &[with_env(11, "a", Some(EnvRead::Set("/elsewhere".into())))],
        &["/Users/me/.claude-work".to_owned()],
        false,
        &format!("{home}/.claude"),
        &paths,
        &no_resolve,
    );
    assert_eq!(single.len(), 1);
    assert_eq!(single["/Users/me/.claude-work"].len(), 1);
    assert!(attribute(
        &[],
        &["/Users/me/.claude-work".to_owned()],
        false,
        home,
        &paths,
        &no_resolve
    )
    .is_empty());
    assert!(attribute(&entries, &[], true, home, &paths, &no_resolve).is_empty());
}

/// Unreadable goes to `~\.claude` when it leads here, else to the first
/// folder that does (aliases are sorted).
#[test]
fn an_unreadable_process_goes_to_the_default_folder_when_it_leads_here() {
    let paths = posix();
    let aliases: Vec<String> = ["/Users/me/.claude-a", "/Users/me/.claude-b"]
        .map(String::from)
        .to_vec();
    let unreadable = with_env(1, "s", Some(EnvRead::Unreadable));
    assert_eq!(
        attribute_entry(
            &unreadable,
            &aliases,
            "/Users/me/.claude",
            &paths,
            &no_resolve
        ),
        "/Users/me/.claude-a"
    );
    let with_default: Vec<String> = ["/Users/me/.claude", "/Users/me/.claude-a"]
        .map(String::from)
        .to_vec();
    assert_eq!(
        attribute_entry(
            &unreadable,
            &with_default,
            "/Users/me/.claude",
            &paths,
            &no_resolve
        ),
        "/Users/me/.claude"
    );
    // Unset is the default folder even when it isn't among the aliases.
    let unset = with_env(1, "s", Some(EnvRead::Unset));
    assert_eq!(
        attribute_entry(&unset, &aliases, "/Users/me/.claude", &paths, &no_resolve),
        "/Users/me/.claude"
    );
}

/// A process whose `CLAUDE_CONFIG_DIR` names a folder by another path (the
/// resolved target of a linked config folder) goes to the alias that
/// resolves to the same place.
#[test]
fn a_resolved_path_goes_to_the_alias_that_resolves_there() {
    let paths = posix();
    let aliases: Vec<String> = ["/Users/me/.claude", "/Users/me/.claude-work"]
        .map(String::from)
        .to_vec();
    let resolve = |p: &str| {
        if p == "/Users/me/.claude-work" {
            "/Users/me/dotfiles/claude-work".to_owned()
        } else {
            p.to_owned()
        }
    };
    let by_target = with_env(
        1,
        "s",
        Some(EnvRead::Set("/Users/me/dotfiles/claude-work/".into())),
    );
    assert_eq!(
        attribute_entry(&by_target, &aliases, "/Users/me/.claude", &paths, &resolve),
        "/Users/me/.claude-work"
    );
    let stranger = with_env(
        2,
        "s",
        Some(EnvRead::Set("/Users/me/dotfiles/other".into())),
    );
    assert_eq!(
        attribute_entry(&stranger, &aliases, "/Users/me/.claude", &paths, &resolve),
        "/Users/me/dotfiles/other"
    );
}

/// Windows paths compare without case and either slash; the alias keeps its
/// own spelling.
#[test]
fn windows_spellings_of_a_folder_are_one_folder() {
    let paths = Paths::new(PathStyle::Windows, r"C:\Users\Me");
    let aliases: Vec<String> = [
        r"C:\Users\Me\.claude",
        r"C:\Users\Me\.claude-windows\801f9dd51396",
    ]
    .map(String::from)
    .to_vec();
    let entries = vec![
        with_env(
            1,
            "a",
            Some(EnvRead::Set(
                r"c:/users/me/.CLAUDE-windows/801F9DD51396\".into(),
            )),
        ),
        with_env(2, "b", Some(EnvRead::Unset)),
    ];
    let result = attribute(
        &entries,
        &aliases,
        true,
        r"C:\Users\Me\.claude",
        &paths,
        &no_resolve,
    );
    assert_eq!(
        result[r"C:\Users\Me\.claude-windows\801f9dd51396"][0].pid,
        1
    );
    assert_eq!(result[r"C:\Users\Me\.claude"][0].pid, 2);
}

// ---- Shared folders, against real folders ----

struct CountingProcesses {
    inner: FakeProcesses,
    asked: Mutex<Vec<u32>>,
}

impl Processes for CountingProcesses {
    fn liveness(&self, pid: u32) -> Liveness {
        self.inner.liveness(pid)
    }
    fn start_time(&self, pid: u32) -> Option<SystemTime> {
        self.inner.start_time(pid)
    }
    fn table(&self) -> ProcessTable {
        self.inner.table()
    }
    fn config_dir_env(&self, pid: u32) -> EnvRead {
        self.asked.lock().unwrap().push(pid);
        self.inner.config_dir_env(pid)
    }
    fn same_user(&self, pid: u32) -> Option<bool> {
        self.inner.same_user(pid)
    }
    fn elevated(&self, pid: u32) -> Option<bool> {
        self.inner.elevated(pid)
    }
    fn exe_path(&self, pid: u32) -> Option<PathBuf> {
        self.inner.exe_path(pid)
    }
}

/// Parallel Profiles' layout: `.claude-shared\sessions` holds every
/// registry file and four config folders' `sessions` link there (declared by
/// `LinkedFiles`, as a Windows runner may not make links).
struct Shared {
    _home: TempDir,
    paths: Paths,
    home: PathBuf,
    files: LinkedFiles,
    dirs: Vec<String>,
}

impl Shared {
    fn new() -> Shared {
        let home = TempDir::new().unwrap();
        let root = std::fs::canonicalize(home.path()).unwrap();
        let paths = Paths::native(&root);
        let shared = root.join(".claude-shared").join("sessions");
        std::fs::create_dir_all(&shared).unwrap();
        let mut dirs = vec![root.join(".claude")];
        for window in ["801f9dd51396", "b9fbb9ecd7cb", "1bf3e8f92b11"] {
            dirs.push(root.join(".claude-windows").join(window));
        }
        let mut links = Vec::new();
        for dir in &dirs {
            std::fs::create_dir_all(dir).unwrap();
            // The link itself is declared, not made: `<dir>/sessions` leads to the shared folder.
            links.push((dir.join("sessions"), shared.clone()));
        }
        let dirs = dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        Shared {
            _home: home,
            paths,
            home: root,
            files: LinkedFiles { links },
            dirs,
        }
    }

    fn shared(&self) -> PathBuf {
        self.home.join(".claude-shared").join("sessions")
    }
}

/// Two live processes in the shared folder, four config folders leading
/// there: one read, each session under its own folder.
#[test]
fn the_shared_folder_is_scanned_once_and_split() {
    let shared = Shared::new();
    let groups = grouped_by_sessions_folder(&shared.dirs, &shared.paths, &shared.files);
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0].folder, shared.shared());
    assert_eq!(groups[0].aliases.len(), 4);
    let mut sorted = groups[0].aliases.clone();
    sorted.sort();
    assert_eq!(groups[0].aliases, sorted);
    assert!(is_linked_sessions_folder(
        &shared.dirs[0],
        &shared.paths,
        &shared.files
    ));
    assert_eq!(
        sessions_folder_of(&shared.dirs[2], &shared.paths, &shared.files),
        shared.shared()
    );

    let (paras_dir, biios_dir) = (shared.dirs[1].clone(), shared.dirs[3].clone());
    let procs = CountingProcesses {
        inner: FakeProcesses::default(),
        asked: Mutex::new(Vec::new()),
    };
    let (paras, biios) = (4111u32, 4222u32);
    for (pid, session, dir) in [
        (paras, "s-paras", &paras_dir),
        (biios, "s-biios", &biios_dir),
    ] {
        write_entry(
            &shared.shared(),
            pid,
            json!({"pid": pid, "sessionId": session, "kind": "interactive",
                   "entrypoint": "claude-vscode", "status": "idle", "updatedAt": MON_SEP_21 * 1000}),
        );
        procs.inner.add(pid, 1, "claude.exe", at(MON_SEP_21 - 100));
        procs
            .inner
            .set_config_dir_env(pid, EnvRead::Set(dir.clone()));
    }
    // One read of the folder, however many config folders lead to it.
    let snapshot = read_registry(&groups[0].folder, true, &procs, at(MON_SEP_21));
    assert_eq!(snapshot.entries.len(), 2);
    // Each process was asked once for the read, not once per config folder (4 a read).
    let mut asked = procs.asked.lock().unwrap().clone();
    asked.sort();
    assert_eq!(asked, [paras, biios]);
    let default_dir = shared.paths.default_config_dir();
    let split = attribute(
        &snapshot.entries,
        &groups[0].aliases,
        true,
        &default_dir,
        &shared.paths,
        &|p| {
            resolve_path(&shared.files, Path::new(p))
                .to_string_lossy()
                .into_owned()
        },
    );
    let sessions_of = |dir: &str| -> Option<Vec<String>> {
        split
            .get(&shared.paths.normalize(dir))
            .map(|v| v.iter().map(|e| e.session_id.to_string()).collect())
    };
    assert_eq!(sessions_of(&paras_dir), Some(vec!["s-paras".to_owned()]));
    assert_eq!(sessions_of(&biios_dir), Some(vec!["s-biios".to_owned()]));
    assert_eq!(sessions_of(&default_dir), None);
    assert_eq!(split.len(), 2);

    // Biios's session ends: its window has nothing left in the next read.
    std::fs::remove_file(shared.shared().join(format!("{biios}.json"))).unwrap();
    let snapshot = read_registry(&groups[0].folder, true, &procs, at(MON_SEP_21 + 3));
    let split = attribute(
        &snapshot.entries,
        &groups[0].aliases,
        true,
        &default_dir,
        &shared.paths,
        &no_resolve,
    );
    assert_eq!(split.len(), 1);
    assert!(split.contains_key(&shared.paths.normalize(&paras_dir)));
}

/// A config folder with its own real `sessions` folder isn't shared: each is
/// its own group and none is linked.
#[test]
fn separate_sessions_folders_are_separate_groups() {
    let home = TempDir::new().unwrap();
    let root = std::fs::canonicalize(home.path()).unwrap();
    let paths = Paths::native(&root);
    let (a, b) = (root.join(".claude"), root.join(".claude-work"));
    std::fs::create_dir_all(a.join("sessions")).unwrap();
    // `b` has no sessions folder yet: grouped by where it would be.
    std::fs::create_dir_all(&b).unwrap();
    let dirs = vec![
        a.to_string_lossy().into_owned(),
        b.to_string_lossy().into_owned(),
        a.to_string_lossy().into_owned(),
    ];
    let groups = grouped_by_sessions_folder(
        &dirs,
        &paths,
        &agentnotch_engine::core::atomic::StdSecureFiles,
    );
    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|g| g.aliases.len() == 1));
    let files = agentnotch_engine::core::atomic::StdSecureFiles;
    assert!(!is_linked_sessions_folder(&dirs[0], &paths, &files));
    assert!(!is_linked_sessions_folder(&dirs[1], &paths, &files));
    assert_eq!(
        sessions_folder_of(&dirs[1], &paths, &files),
        b.join("sessions")
    );
    let names: BTreeSet<String> = groups
        .iter()
        .map(|g| g.folder.to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2);
}

/// A transcript path through the shared history's own folder names no
/// account: the environment decides.
#[test]
fn a_resolved_shared_path_falls_back_to_the_environment() {
    let paths = Paths::new(PathStyle::Windows, r"C:\Users\me");
    let shared = r"C:\Users\me\.claude-shared";
    let transcript = format!(r"{shared}\projects\-x\s.jsonl");
    let infrastructure = |dir: &str| paths.same(dir, shared);
    assert_eq!(
        paths.session_config_dir(
            Some(&transcript),
            Some(r"C:\Users\me\.claude-windows\1bf3e8f92b11"),
            infrastructure
        ),
        r"C:\Users\me\.claude-windows\1bf3e8f92b11"
    );
    assert_eq!(
        paths.session_config_dir(Some(&transcript), None, infrastructure),
        paths.default_config_dir()
    );
}
