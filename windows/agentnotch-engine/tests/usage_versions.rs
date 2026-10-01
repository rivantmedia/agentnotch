//! `usage::versions` (ClaudeBinaryLocatorTests' version tests, design §4.3):
//! `claude --version` answers, the folder names of bundled copies the app
//! never runs, and the `--version` runs themselves through a scripted runner
//! (never a real Claude Code).
//!
//! The Mac's shell-resolution tests of the same Swift suite do not apply on
//! Windows (there is no login shell to ask); the Windows candidate order is in
//! `usage_locator.rs`.

mod usage_support;

use agentnotch_engine::platform::{CommandSpec, Exit};
use agentnotch_engine::runtime_types::{VersionSighting, VersionSource};
use agentnotch_engine::testkit::runner::Conversation;
use agentnotch_engine::testkit::{Script, ScriptedRunner};
use agentnotch_engine::usage::versions::{
    binary_version, bundled_roots, bundled_versions, parse_version, read_versions,
};
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;
use usage_support::*;

// MARK: - parse_version (parsesVersion, rejectsNonVersions, plainAnswer, ignoresBanners)

#[test]
fn parses_a_version() {
    for (text, expected) in [
        ("2.1.88", "2.1.88"),
        ("v2.1.88", "2.1.88"),
        ("2.1.280 (Claude Code)", "2.1.280"),
        ("claude 10.0.3 (something)", "10.0.3"),
        // Windows output ends in \r\n.
        ("2.1.280 (Claude Code)\r\n", "2.1.280"),
        // Leading zeros are not part of a version.
        ("2.01.007", "2.1.7"),
        ("2.1.88-beta.1", "2.1.88"),
        ("1.2.3.4", "1.2.3"),
    ] {
        assert_eq!(parse_version(text).as_deref(), Some(expected), "{text:?}");
    }
}

#[test]
fn rejects_non_versions() {
    for text in [
        "not a version",
        "",
        "2.1",
        "vNext",
        "2.x.3",
        "1..2",
        "v.2.3",
    ] {
        assert_eq!(parse_version(text), None, "{text:?}");
    }
}

/// `plainAnswer`: what `claude --version` prints is one line.
#[test]
fn a_plain_answer() {
    assert_eq!(
        parse_version("2.1.280 (Claude Code)\n").as_deref(),
        Some("2.1.280")
    );
}

/// `ignoresBanners`: a banner, a notice, then the version; a number that is
/// not a version ("7", "3.14") is skipped on the way.
#[test]
fn ignores_banners() {
    let out = "Last login: Mon Sep  1 09:14:22 on ttys003\r\nYou have 7 new messages, load 3.14.\r\n2.1.88 (Claude Code)\r\n";
    assert_eq!(parse_version(out).as_deref(), Some("2.1.88"));
}

/// `versionOrdering`: the engine keeps versions as `X.Y.Z` strings (the
/// hooks package compares them by the numbers), so what must hold here is
/// that the parsed text splits into numbers that order the way the Mac's
/// `ClaudeCodeVersion` does: 100 above 99, which a text comparison gets wrong.
#[test]
fn version_ordering() {
    let numbers = |text: &str| -> (u64, u64, u64) {
        let parsed = parse_version(text).unwrap();
        let mut parts = parsed.split('.').map(|p| p.parse::<u64>().unwrap());
        (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        )
    };
    assert!(numbers("2.1.84") > numbers("2.1.33"));
    assert!(numbers("2.1.100") > numbers("2.1.99"));
    assert!(numbers("3.0.0") > numbers("2.9.999"));
    assert!("2.1.100" < "2.1.99", "the text order is the wrong one");
}

// MARK: - bundled copies (D 1663-1673)

fn make_dir(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
}

fn sighting(source: VersionSource, path: &Path, version: Option<&str>) -> VersionSighting {
    VersionSighting {
        source,
        path: Some(path.to_path_buf()),
        version: version.map(str::to_owned),
    }
}

#[test]
fn the_bundled_roots_are_the_editors_extensions_and_desktops_claude_code() {
    let base = fake_drive();
    let roots = windows_roots(&base);
    let desktop_two = join(
        &base,
        &["Packages", "Claude_x", "LocalCache", "Roaming", "Claude"],
    );
    let mut roots = roots;
    roots.claude_desktop.push(desktop_two.clone());
    assert_eq!(
        bundled_roots(&roots),
        vec![
            join(&roots.home, &[".vscode", "extensions"]),
            join(&roots.home, &[".vscode-insiders", "extensions"]),
            join(&roots.home, &[".cursor", "extensions"]),
            join(&roots.home, &[".windsurf", "extensions"]),
            join(&appdata(&roots), &["Claude", "claude-code"]),
            desktop_two.join("claude-code"),
        ]
    );
}

#[test]
fn bundled_versions_read_the_folder_names_of_a_temp_tree() {
    let dir = tempfile::tempdir().unwrap();
    let roots = windows_roots(dir.path());
    let home = &roots.home;
    let code = join(
        home,
        &[
            ".vscode",
            "extensions",
            "anthropic.claude-code-2.1.88-win32-x64",
        ],
    );
    let insiders = join(
        home,
        &[
            ".vscode-insiders",
            "extensions",
            "anthropic.claude-code-2.1.90-win32-x64",
        ],
    );
    let cursor = join(
        home,
        &[
            ".cursor",
            "extensions",
            "anthropic.claude-code-2.1.100-win32-arm64",
        ],
    );
    let windsurf = join(
        home,
        &[".windsurf", "extensions", "anthropic.claude-code-2.0.5"],
    );
    let desktop = join(&appdata(&roots), &["Claude", "claude-code", "2.1.85"]);
    for folder in [&code, &insiders, &cursor, &windsurf, &desktop] {
        make_dir(folder);
    }

    let found = bundled_versions(&bundled_roots(&roots));
    assert_eq!(
        found,
        vec![
            sighting(VersionSource::BundledVsCode, &code, Some("2.1.88")),
            sighting(VersionSource::BundledVsCode, &insiders, Some("2.1.90")),
            sighting(VersionSource::BundledVsCode, &cursor, Some("2.1.100")),
            sighting(VersionSource::BundledVsCode, &windsurf, Some("2.0.5")),
            sighting(VersionSource::BundledDesktop, &desktop, Some("2.1.85")),
        ]
    );
}

#[test]
fn an_unparsable_folder_name_is_unknown_not_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let roots = windows_roots(dir.path());
    let extensions = join(&roots.home, &[".vscode", "extensions"]);
    let next = extensions.join("anthropic.claude-code-next-win32-x64");
    let short = extensions.join("anthropic.claude-code-2.1-win32-x64");
    let latest = join(&appdata(&roots), &["Claude", "claude-code", "latest"]);
    for folder in [&next, &short, &latest] {
        make_dir(folder);
    }

    let found = bundled_versions(&bundled_roots(&roots));
    assert_eq!(found.len(), 3);
    assert!(found.contains(&sighting(VersionSource::BundledVsCode, &next, None)));
    assert!(found.contains(&sighting(VersionSource::BundledVsCode, &short, None)));
    assert!(found.contains(&sighting(VersionSource::BundledDesktop, &latest, None)));
}

#[test]
fn unrelated_extensions_and_loose_files_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let roots = windows_roots(dir.path());
    let extensions = join(&roots.home, &[".vscode", "extensions"]);
    make_dir(&extensions.join("ms-python.python-2024.1.0"));
    make_dir(&extensions.join("anthropic.other-tool-1.2.3"));
    make_dir(&extensions.join("anthropic.claude-code"));
    // A file, not a folder, even with the right name.
    std::fs::write(
        extensions.join("anthropic.claude-code-9.9.9-win32-x64"),
        b"x",
    )
    .unwrap();
    std::fs::write(extensions.join("extensions.json"), b"[]").unwrap();
    let kept = extensions.join("Anthropic.Claude-Code-2.1.7-win32-x64");
    make_dir(&kept);

    let found = bundled_versions(&bundled_roots(&roots));
    assert_eq!(
        found,
        vec![sighting(VersionSource::BundledVsCode, &kept, Some("2.1.7"))]
    );
}

#[test]
fn missing_folders_are_fine() {
    let dir = tempfile::tempdir().unwrap();
    let roots = windows_roots(dir.path());
    assert!(bundled_versions(&bundled_roots(&roots)).is_empty());
    assert!(bundled_versions(&[]).is_empty());
}

#[test]
fn a_desktop_folder_is_named_by_its_version_only() {
    // Desktop's `claude-code\<ver>` has no extension prefix: an `anthropic.claude-code-`
    // name there is not a version, and an extensions folder's plain number is not one.
    let dir = tempfile::tempdir().unwrap();
    let roots = windows_roots(dir.path());
    let desktop_root = join(&appdata(&roots), &["Claude", "claude-code"]);
    make_dir(&desktop_root.join("anthropic.claude-code-2.1.88"));
    let extensions = join(&roots.home, &[".cursor", "extensions"]);
    make_dir(&extensions.join("2.1.88"));

    let found = bundled_versions(&bundled_roots(&roots));
    assert_eq!(
        found,
        vec![sighting(
            VersionSource::BundledDesktop,
            &desktop_root.join("anthropic.claude-code-2.1.88"),
            None
        )]
    );
}

// MARK: - claude --version through a scripted runner (readsTheVersionOfABinary)

const SHORT: Duration = Duration::from_secs(5);

fn base_env(path: &OsStr) -> Vec<(OsString, OsString)> {
    [
        ("CLAUDECODE", "1".into()),
        ("CLAUDE_PID", "4242".into()),
        ("CLAUDE_EFFORT", "high".into()),
        ("AI_AGENT", "x".into()),
        ("CLAUDE_CONFIG_DIR", "C:\\somewhere".into()),
        ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "C:\\elsewhere".into()),
        ("TEMP", "C:\\temp".into()),
        ("Path", path.to_owned()),
    ]
    .into_iter()
    .map(|(name, value): (&str, OsString)| (OsString::from(name), value))
    .collect()
}

fn names(spec: &CommandSpec) -> Vec<String> {
    spec.env
        .iter()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn reads_the_version_of_a_binary() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude.exe");
    let runner = ScriptedRunner::default();
    runner.push(Script::ok("2.1.280 (Claude Code)\r\n"));

    let env = base_env(OsStr::new("C:\\Windows"));
    let version = binary_version(&claude, &runner, &env, OsStr::new(""), SHORT);
    assert_eq!(version.as_deref(), Some("2.1.280"));

    let spawned = runner.spawned();
    assert_eq!(spawned.len(), 1);
    let spec = &spawned[0];
    assert_eq!(spec.program, claude);
    assert_eq!(spec.args, vec![OsString::from("--version")]);
    // Runs beside the binary, with the scrubbed environment: none of the
    // variables that make Claude Code act as a session, its folder first on PATH.
    assert_eq!(spec.cwd, dir.path());
    let env_names = names(spec);
    for scrubbed in [
        "CLAUDECODE",
        "CLAUDE_PID",
        "CLAUDE_EFFORT",
        "AI_AGENT",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ] {
        assert!(!env_names.iter().any(|n| n == scrubbed), "{scrubbed} kept");
    }
    assert!(env_names.iter().any(|n| n == "TEMP"));
    let path = spec
        .env
        .iter()
        .find(|(name, _)| name == "Path")
        .map(|(_, value)| value.clone())
        .expect("PATH keeps its spelling");
    assert_eq!(std::env::split_paths(&path).next().unwrap(), dir.path());
    // Nothing was asked on stdin.
    assert_eq!(runner.stdin_closed(0), Some(true));
    assert_eq!(runner.stdin_of(0), Some(Vec::new()));
}

#[test]
fn a_banner_line_before_the_version_is_fine() {
    let runner = ScriptedRunner::default();
    runner.push(Script::ok(
        "Update available: 2.2.0\n2.1.88 (Claude Code)\n",
    ));
    // The first X.Y.Z is what counts, as on the Mac.
    let version = binary_version(
        &join(&fake_drive(), &["x", "claude.exe"]),
        &runner,
        &[],
        OsStr::new(""),
        SHORT,
    );
    assert_eq!(version.as_deref(), Some("2.2.0"));
}

#[test]
fn a_failing_or_silent_binary_has_no_version() {
    let claude = join(&fake_drive(), &["x", "claude.exe"]);
    let runner = ScriptedRunner::default();
    // A non-zero exit, even with a version on stdout (readsTheVersionOfABinary's `exit 3`).
    runner.push(Script {
        stdout: b"2.1.88\n".to_vec(),
        stderr: Vec::new(),
        exit: Exit::Code(3),
    });
    // Killed, output that holds no version, no output at all.
    runner.push(Script {
        stdout: b"2.1.88\n".to_vec(),
        stderr: Vec::new(),
        exit: Exit::Killed,
    });
    runner.push(Script::ok("not a version\n"));
    runner.push(Script::ok(""));
    runner.push_spawn_error(std::io::ErrorKind::NotFound, "no such file");
    for _ in 0..5 {
        assert_eq!(
            binary_version(&claude, &runner, &[], OsStr::new(""), SHORT),
            None
        );
    }
    assert_eq!(runner.spawned().len(), 5);
}

#[test]
fn a_hanging_binary_is_killed_after_the_timeout() {
    let runner = ScriptedRunner::default();
    runner.push_conversation(Conversation::hanging());
    let version = binary_version(
        &join(&fake_drive(), &["x", "claude.exe"]),
        &runner,
        &[],
        OsStr::new(""),
        Duration::from_millis(50),
    );
    assert_eq!(version, None);
    assert_eq!(runner.kills_of(0), Some(1));
}

#[test]
fn an_npm_shim_is_asked_through_node() {
    let dir = tempfile::tempdir().unwrap();
    let shim = dir.path().join("claude.cmd");
    let cli = join(
        dir.path(),
        &["node_modules", "@anthropic-ai", "claude-code", "cli.js"],
    );
    let node = dir.path().join("node.exe");
    std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
    for file in [&shim, &cli, &node] {
        std::fs::write(file, b"").unwrap();
    }
    let runner = ScriptedRunner::default();
    runner.push(Script::ok("2.1.280 (Claude Code)\n"));

    let version = binary_version(&shim, &runner, &[], OsStr::new(""), SHORT);
    assert_eq!(version.as_deref(), Some("2.1.280"));
    let spec = &runner.spawned()[0];
    assert_eq!(spec.program, node);
    assert_eq!(
        spec.args,
        vec![cli.into_os_string(), OsString::from("--version")]
    );
}

// MARK: - read_versions (Job::Versions)

#[test]
fn read_versions_lists_binaries_then_bundled_copies() {
    let dir = tempfile::tempdir().unwrap();
    let roots = windows_roots(dir.path());
    let extension = join(
        &roots.home,
        &[
            ".vscode",
            "extensions",
            "anthropic.claude-code-2.1.50-win32-x64",
        ],
    );
    make_dir(&extension);
    let (native, broken) = (
        join(dir.path(), &["a", "claude.exe"]),
        join(dir.path(), &["b", "claude.exe"]),
    );

    let runner = ScriptedRunner::default();
    runner.push(Script::ok("2.1.280 (Claude Code)\n"));
    runner.push(Script {
        stdout: Vec::new(),
        stderr: b"boom".to_vec(),
        exit: Exit::Code(1),
    });
    let found = read_versions(
        &[native.clone(), broken.clone()],
        &bundled_roots(&roots),
        &runner,
        &base_env(OsStr::new("C:\\Windows")),
    );
    assert_eq!(
        found,
        vec![
            sighting(VersionSource::Binary, &native, Some("2.1.280")),
            // Seen but unknown: counted, never dropped.
            sighting(VersionSource::Binary, &broken, None),
            sighting(VersionSource::BundledVsCode, &extension, Some("2.1.50")),
        ]
    );
}

#[test]
fn read_versions_finds_node_for_a_shim_on_the_environments_path() {
    // The PATH the shim's node is searched on is the one in the base
    // environment, whatever its spelling (`Path`).
    let dir = tempfile::tempdir().unwrap();
    let shim_dir = dir.path().join("npm");
    let node_dir = dir.path().join("node");
    let cli = join(
        &shim_dir,
        &["node_modules", "@anthropic-ai", "claude-code", "cli.js"],
    );
    std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&node_dir).unwrap();
    let shim = shim_dir.join("claude.cmd");
    let node = node_dir.join("node.exe");
    for file in [&shim, &cli, &node] {
        std::fs::write(file, b"").unwrap();
    }
    let runner = ScriptedRunner::default();
    runner.push(Script::ok("2.1.99\n"));

    let env = base_env(&path_list(std::slice::from_ref(&node_dir)));
    let found = read_versions(std::slice::from_ref(&shim), &[], &runner, &env);
    assert_eq!(
        found,
        vec![sighting(VersionSource::Binary, &shim, Some("2.1.99"))]
    );
    assert_eq!(runner.spawned()[0].program, node);
}
