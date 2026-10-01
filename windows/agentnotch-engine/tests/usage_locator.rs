//! `usage::locator` (ClaudeBinaryLocator.swift, design §4.6): which Claude
//! Code a usage check runs, in which order, and never Claude Desktop's.
//!
//! Everything is pure over an `exists` closure and a `PATH` value the test
//! passes in: nothing here reads the real `PATH` or disk. The Mac's
//! shell-resolution tests (`plainAnswer`, `ignoresBanners`, `rejectsNonPaths`,
//! `lastPathWins`, `fixedCandidatesIncludeAccountLocalInstalls`,
//! `environmentPutsTheBinaryDirectoryOnPath`) do not apply on Windows: there
//! is no login shell to ask and no per-account `local\claude` install. The
//! version tests of the same Swift suite are in `usage_versions.rs`.
//!
//! Paths are joined with `Path::join`, so they carry the host's separator
//! (see `usage_support`); the Desktop exclusions, which are about
//! backslashes, are also checked with literal Windows strings.

mod usage_support;

use agentnotch_engine::usage::locator::{
    binary_for, candidates, fixed_candidates, installed, is_desktop_owned, is_shim, locate_claude,
    locate_claude_from, path_candidates,
};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use usage_support::*;

/// A disk that holds exactly `present`.
fn disk(present: &[&PathBuf]) -> impl Fn(&Path) -> bool {
    let present: HashSet<PathBuf> = present.iter().map(|p| (*p).clone()).collect();
    move |path: &Path| present.contains(path)
}

fn no_path() -> OsString {
    OsString::new()
}

// MARK: - Order (D 1797-1800)

#[test]
fn the_candidate_order_is_settings_remembered_installers_shims_then_path() {
    let base = fake_drive();
    let roots = windows_roots(&base);
    let settings = join(&base, &["Tools", "claude-custom", "claude.exe"]);
    let remembered = join(&base, &["Tools", "remembered", "claude.exe"]);
    let (bin_a, bin_b) = (join(&base, &["bin-a"]), join(&base, &["bin-b"]));
    let env_path = path_list(&[bin_a.clone(), bin_b.clone()]);

    let found = candidates(&roots, Some(&settings), Some(&remembered), &env_path);

    let home = &roots.home;
    let expected = vec![
        settings,
        remembered,
        join(home, &[".local", "bin", "claude.exe"]),
        join(home, &[".bun", "bin", "claude.exe"]),
        join(home, &[".volta", "bin", "claude.exe"]),
        join(&appdata(&roots), &["npm", "claude.cmd"]),
        join(&localappdata(&roots), &["pnpm", "claude.cmd"]),
        bin_a.join("claude.exe"),
        bin_a.join("claude.cmd"),
        bin_b.join("claude.exe"),
        bin_b.join("claude.cmd"),
    ];
    assert_eq!(found, expected);
}

#[test]
fn without_a_choice_or_a_remembered_path_the_installers_come_first() {
    let roots = windows_roots(&fake_drive());
    let found = candidates(&roots, None, None, &no_path());
    assert_eq!(found, fixed_candidates(&roots));
    assert_eq!(found.len(), 5);
    assert_eq!(
        found[0],
        join(&roots.home, &[".local", "bin", "claude.exe"])
    );
}

#[test]
fn claude_exe_comes_before_claude_cmd_in_each_path_folder() {
    let (a, b) = (join(&fake_drive(), &["a"]), join(&fake_drive(), &["b"]));
    let found = path_candidates(&path_list(&[a.clone(), b.clone()]));
    assert_eq!(
        found,
        vec![
            a.join("claude.exe"),
            a.join("claude.cmd"),
            b.join("claude.exe"),
            b.join("claude.cmd"),
        ]
    );
}

#[test]
fn empty_path_entries_are_skipped() {
    // `;;` (or a trailing `;`) in PATH is an empty entry, not the current
    // folder: a check must never run whatever `claude` sits in its cwd.
    let folder = join(&fake_drive(), &["a"]);
    let separator = if cfg!(windows) { ";" } else { ":" };
    let mut raw = OsString::from(separator);
    raw.push(folder.as_os_str());
    raw.push(separator);
    assert_eq!(
        path_candidates(&raw),
        vec![folder.join("claude.exe"), folder.join("claude.cmd")]
    );
    assert!(path_candidates(&no_path()).is_empty());
}

#[test]
fn relative_path_entries_are_skipped() {
    // `.`, `bin` (and on Windows `C:tools` or `\tools`) are relative to the
    // app's working folder: as unsafe as an empty entry.
    let folder = join(&fake_drive(), &["a"]);
    let relative = [PathBuf::from("."), PathBuf::from("bin"), folder.clone()];
    assert_eq!(
        path_candidates(&path_list(&relative)),
        vec![folder.join("claude.exe"), folder.join("claude.cmd")]
    );
    // Nor is Node taken from one for an npm shim.
    let shim_dir = join(&fake_drive(), &["npm"]);
    let shim = shim_dir.join("claude.cmd");
    let cli = join(
        &shim_dir,
        &["node_modules", "@anthropic-ai", "claude-code", "cli.js"],
    );
    let relative_node = PathBuf::from("bin").join("node.exe");
    let exists = |path: &Path| path == cli || path == relative_node;
    let binary = binary_for(&shim, &path_list(&[PathBuf::from("bin")]), &exists);
    assert_eq!(binary.program, shim);
    assert!(binary.shim);
}

#[test]
fn a_moved_appdata_is_searched_before_the_default_place() {
    // Roaming and local app data come from the roots: the parent of upstream's
    // config folder, and the folder that holds the app's own support folder.
    // A redirected profile (another drive) is found first; the default place
    // under the profile is the fallback.
    let base = fake_drive();
    let mut roots = windows_roots(&base);
    let roaming = join(&base, &["D", "Roaming"]);
    let local = join(&base, &["D", "Local"]);
    roots.data = roaming.join("Agent Notch");
    roots.support = join(&local, &["com.rivantmedia.agentnotch", "Claude"]);

    let found = fixed_candidates(&roots);
    let home = &roots.home;
    assert_eq!(
        found,
        vec![
            join(home, &[".local", "bin", "claude.exe"]),
            join(home, &[".bun", "bin", "claude.exe"]),
            join(home, &[".volta", "bin", "claude.exe"]),
            join(&roaming, &["npm", "claude.cmd"]),
            join(home, &["AppData", "Roaming", "npm", "claude.cmd"]),
            join(&local, &["pnpm", "claude.cmd"]),
            join(home, &["AppData", "Local", "pnpm", "claude.cmd"]),
        ]
    );
}

#[test]
fn an_overridden_support_folder_does_not_invent_a_local_appdata() {
    // AGENTNOTCH_SUPPORT_DIR points anywhere: only a folder that really is
    // `…\com.rivantmedia.agentnotch\Claude` says where %LOCALAPPDATA% is.
    let base = fake_drive();
    let mut roots = windows_roots(&base);
    roots.support = join(&base, &["scratch", "support"]);
    let found = fixed_candidates(&roots);
    let pnpm: Vec<&PathBuf> = found
        .iter()
        .filter(|p| p.components().any(|c| c.as_os_str() == "pnpm"))
        .collect();
    assert_eq!(
        pnpm,
        vec![&join(
            &roots.home,
            &["AppData", "Local", "pnpm", "claude.cmd"]
        )]
    );
}

#[test]
fn repeats_are_dropped_whatever_the_letter_case() {
    let roots = windows_roots(&fake_drive());
    let exe = join(&roots.home, &[".local", "bin", "claude.exe"]);
    // The same file named by the Settings choice, by PATH (another case) and
    // by the installer's own place: listed once, at its first position.
    let shouting = PathBuf::from(exe.to_string_lossy().to_uppercase());
    let folder = exe.parent().unwrap().to_path_buf();
    let found = candidates(&roots, Some(&exe), Some(&shouting), &path_list(&[folder]));
    assert_eq!(found[0], exe);
    let count = found
        .iter()
        .filter(|p| {
            p.to_string_lossy()
                .eq_ignore_ascii_case(&exe.to_string_lossy())
        })
        .count();
    assert_eq!(count, 1);
    // The PATH folder's `claude.cmd` is still there, once, after the fixed list.
    let cmd = exe.with_file_name("claude.cmd");
    assert_eq!(found.iter().filter(|p| **p == cmd).count(), 1);
}

#[test]
fn an_empty_choice_is_ignored() {
    let roots = windows_roots(&fake_drive());
    let found = candidates(&roots, Some(Path::new("")), Some(Path::new("")), &no_path());
    assert_eq!(found, fixed_candidates(&roots));
}

// MARK: - Claude Desktop's copies are never run

#[test]
fn desktop_owned_paths() {
    for owned in [
        r"C:\Users\me\AppData\Local\AnthropicClaude\app-1.2.3\claude.exe",
        r"C:\Users\me\AppData\Roaming\Claude\claude-code\2.1.85\claude.exe",
        r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\claude.exe",
        // Any case, either separator.
        r"c:\users\me\appdata\local\anthropicclaude\claude.exe",
        r"C:\PROGRAM FILES\WINDOWSAPPS\Claude_1.0\claude.exe",
        "C:/Users/me/AppData/Roaming/Claude/claude-code/2.1.85/claude.exe",
        r"C:\Users\me\AppData\Roaming\CLAUDE\CLAUDE-CODE\2.1.85\claude.exe",
    ] {
        assert!(is_desktop_owned(Path::new(owned)), "{owned}");
    }
    for mine in [
        r"C:\Users\me\.local\bin\claude.exe",
        r"C:\Users\me\AppData\Roaming\npm\claude.cmd",
        // A folder that merely starts like one of the three.
        r"C:\Tools\Claude\claude-code-helper\claude.exe",
        r"C:\Tools\AnthropicClaudeTools\claude.exe",
        r"C:\Tools\MyWindowsApps\claude.exe",
    ] {
        assert!(!is_desktop_owned(Path::new(mine)), "{mine}");
    }
}

#[test]
fn desktop_copies_are_left_out_wherever_they_come_from() {
    let base = fake_drive();
    let roots = windows_roots(&base);
    let desktop = |parts: &[&str]| join(&base, parts);
    let bundled = desktop(&[
        "Users",
        "me",
        "AppData",
        "Roaming",
        "Claude",
        "claude-code",
        "2.1.85",
        "claude.exe",
    ]);
    let squirrel = desktop(&[
        "Users",
        "me",
        "AppData",
        "Local",
        "AnthropicClaude",
        "app-1.2.3",
        "claude.exe",
    ]);
    let alias = desktop(&[
        "Users",
        "me",
        "AppData",
        "Local",
        "Microsoft",
        "WindowsApps",
        "claude.exe",
    ]);
    let fine = join(&base, &["tools", "claude.exe"]);

    // From the Settings choice, the remembered path and PATH alike.
    for owned in [&bundled, &squirrel, &alias] {
        let env_path = path_list(&[
            owned.parent().unwrap().to_path_buf(),
            fine.parent().unwrap().to_path_buf(),
        ]);
        let found = candidates(&roots, Some(owned), Some(owned), &env_path);
        assert!(found.iter().all(|p| !is_desktop_owned(p)), "{found:?}");
        assert!(found.contains(&fine));
    }

    // And a located binary is never one of them, even when it is the only file.
    let only_desktop = disk(&[&bundled, &squirrel, &alias]);
    let env_path = path_list(&[
        bundled.parent().unwrap().to_path_buf(),
        squirrel.parent().unwrap().to_path_buf(),
        alias.parent().unwrap().to_path_buf(),
    ]);
    assert_eq!(
        locate_claude(&roots, Some(&bundled), &env_path, &only_desktop),
        None
    );
    assert!(installed(&roots, Some(&bundled), &env_path, &only_desktop).is_empty());
}

// MARK: - locate_claude / locate_claude_from

#[test]
fn the_first_existing_candidate_is_the_binary() {
    let roots = windows_roots(&fake_drive());
    let bun = join(&roots.home, &[".bun", "bin", "claude.exe"]);
    let volta = join(&roots.home, &[".volta", "bin", "claude.exe"]);
    let npm = join(&appdata(&roots), &["npm", "claude.cmd"]);

    let binary = locate_claude(&roots, None, &no_path(), &disk(&[&volta, &npm, &bun])).unwrap();
    assert_eq!(binary.program, bun);
    assert!(binary.prefix_args.is_empty());
    assert!(!binary.shim);
    assert_eq!(binary.version, None);

    assert_eq!(locate_claude(&roots, None, &no_path(), &disk(&[])), None);
}

#[test]
fn the_settings_choice_beats_the_remembered_path_which_beats_the_rest() {
    let roots = windows_roots(&fake_drive());
    let settings = join(&fake_drive(), &["pick", "claude.exe"]);
    let remembered = join(&fake_drive(), &["found-before", "claude.exe"]);
    let native = join(&roots.home, &[".local", "bin", "claude.exe"]);

    let all = disk(&[&settings, &remembered, &native]);
    let program =
        |choice: Option<&Path>, remembered: Option<&Path>, on_disk: &dyn Fn(&Path) -> bool| {
            locate_claude_from(&roots, choice, remembered, &no_path(), on_disk).map(|b| b.program)
        };
    assert_eq!(
        program(Some(&settings), Some(&remembered), &all),
        Some(settings.clone())
    );
    assert_eq!(
        program(None, Some(&remembered), &all),
        Some(remembered.clone())
    );
    // A choice that is gone falls through to the remembered one, then on.
    assert_eq!(
        program(
            Some(&settings),
            Some(&remembered),
            &disk(&[&remembered, &native])
        ),
        Some(remembered.clone())
    );
    assert_eq!(
        program(Some(&settings), Some(&remembered), &disk(&[&native])),
        Some(native.clone())
    );
    // `locate_claude` is the same lookup without a remembered path.
    assert_eq!(
        locate_claude(&roots, Some(&settings), &no_path(), &all).map(|b| b.program),
        Some(settings)
    );
}

#[test]
fn a_remembered_desktop_copy_is_not_reused() {
    let base = fake_drive();
    let roots = windows_roots(&base);
    let remembered = join(
        &base,
        &[
            "Users",
            "me",
            "AppData",
            "Local",
            "AnthropicClaude",
            "claude.exe",
        ],
    );
    let native = join(&roots.home, &[".local", "bin", "claude.exe"]);
    let found = locate_claude_from(
        &roots,
        None,
        Some(&remembered),
        &no_path(),
        &disk(&[&remembered, &native]),
    );
    assert_eq!(found.map(|b| b.program), Some(native));
}

#[test]
fn installed_lists_every_existing_candidate_in_order() {
    let roots = windows_roots(&fake_drive());
    let settings = join(&fake_drive(), &["pick", "claude.exe"]);
    let local = join(&roots.home, &[".local", "bin", "claude.exe"]);
    let npm = join(&appdata(&roots), &["npm", "claude.cmd"]);
    let on_path = join(&fake_drive(), &["tools"]);
    let path_exe = on_path.join("claude.exe");
    let path_cmd = on_path.join("claude.cmd");

    // (`.bun\bin\claude.exe` and the rest are not on this disk.)
    let all = disk(&[&path_cmd, &npm, &settings, &local, &path_exe]);
    let env_path = path_list(&[on_path]);
    assert_eq!(
        installed(&roots, Some(&settings), &env_path, &all),
        vec![settings, local, npm, path_exe, path_cmd]
    );
    assert!(installed(&roots, None, &env_path, &disk(&[])).is_empty());
}

// MARK: - .cmd shims (D 1797-1800)

#[test]
fn a_cmd_shim_becomes_node_with_the_packages_cli_js() {
    let base = fake_drive();
    let shim_dir = join(&base, &["npm-global"]);
    let shim = shim_dir.join("claude.cmd");
    let cli = join(
        &shim_dir,
        &["node_modules", "@anthropic-ai", "claude-code", "cli.js"],
    );
    let node_beside = shim_dir.join("node.exe");
    let on_path = join(&base, &["node-install"]);
    let node_on_path = on_path.join("node.exe");
    let env_path = path_list(&[on_path]);

    // Node beside the shim is the one the shim itself would run.
    let binary = binary_for(
        &shim,
        &env_path,
        &disk(&[&shim, &cli, &node_beside, &node_on_path]),
    );
    assert_eq!(binary.program, node_beside);
    assert_eq!(binary.prefix_args, vec![cli.clone().into_os_string()]);
    assert!(!binary.shim);

    // Otherwise the first node.exe on PATH.
    let binary = binary_for(&shim, &env_path, &disk(&[&shim, &cli, &node_on_path]));
    assert_eq!(binary.program, node_on_path);
    assert_eq!(binary.prefix_args, vec![cli.into_os_string()]);
    assert!(!binary.shim);
}

#[test]
fn a_shim_without_cli_js_or_without_node_runs_as_the_shim() {
    let base = fake_drive();
    let shim_dir = join(&base, &["npm-global"]);
    let shim = shim_dir.join("claude.cmd");
    let cli = join(
        &shim_dir,
        &["node_modules", "@anthropic-ai", "claude-code", "cli.js"],
    );
    let node = shim_dir.join("node.exe");
    let env_path = path_list(std::slice::from_ref(&shim_dir));

    for on_disk in [disk(&[&shim, &node]), disk(&[&shim, &cli]), disk(&[&shim])] {
        let binary = binary_for(&shim, &env_path, &on_disk);
        assert_eq!(binary.program, shim);
        assert!(binary.prefix_args.is_empty());
        assert!(binary.shim);
    }

    // A node.exe of Claude Desktop's (or a WindowsApps alias) is not Node.
    let alias_dir = join(
        &base,
        &[
            "Users",
            "me",
            "AppData",
            "Local",
            "Microsoft",
            "WindowsApps",
        ],
    );
    let alias = alias_dir.join("node.exe");
    let binary = binary_for(
        &shim,
        &path_list(&[alias_dir]),
        &disk(&[&shim, &cli, &alias]),
    );
    assert!(binary.shim);
    assert_eq!(binary.program, shim);
}

#[test]
fn a_shim_is_found_the_same_way_through_locate_claude() {
    let roots = windows_roots(&fake_drive());
    let shim = join(&appdata(&roots), &["npm", "claude.cmd"]);
    let shim_dir = shim.parent().unwrap().to_path_buf();
    let cli = join(
        &shim_dir,
        &["node_modules", "@anthropic-ai", "claude-code", "cli.js"],
    );
    let node = shim_dir.join("node.exe");

    let binary = locate_claude(&roots, None, &no_path(), &disk(&[&shim, &cli, &node])).unwrap();
    assert_eq!((binary.program, binary.shim), (node, false));
    assert_eq!(binary.prefix_args, vec![cli.into_os_string()]);

    let binary = locate_claude(&roots, None, &no_path(), &disk(&[&shim])).unwrap();
    assert_eq!((binary.program, binary.shim), (shim, true));
}

#[test]
fn exe_files_are_run_as_they_are() {
    let exe = join(&fake_drive(), &["bin", "claude.exe"]);
    let binary = binary_for(&exe, &no_path(), &disk(&[&exe]));
    assert_eq!(binary.program, exe);
    assert!(binary.prefix_args.is_empty() && !binary.shim);
}

#[test]
fn which_files_are_shims() {
    for shim in ["claude.cmd", "CLAUDE.CMD", "claude.bat", r"C:\x\claude.Cmd"] {
        assert!(is_shim(Path::new(shim)), "{shim}");
    }
    for plain in ["claude.exe", "claude", "claude.cmd.exe", ""] {
        assert!(!is_shim(Path::new(plain)), "{plain}");
    }
}
