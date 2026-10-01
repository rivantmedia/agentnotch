//! Jumping to a session's terminal: which app hosts it (`control::hosts`),
//! the steps worth trying (`control::focus`), which Windows Terminal tab is
//! its own, and what came of a jump. Ported from `TerminalFocusTests`
//! (`editorTerminalOpensTheFolder`, `anEditorTerminalInASubfolderOpensTheWorkspaceRoot`,
//! `theWorkspaceRootIsTheNearestGitFolderBelowHome`, `vsCodeExtensionUsesARunningEditor`,
//! `otherTerminalsAreActivated`, `nothingToFocusWithoutAnApp`,
//! `ancestorsWalkUpAndStopAtCycles`, `helperProcessesMapToTheirApps`,
//! `hostResolutionUsesTheRunningAppIndex`, `terminalNamesMatchExactlyNotBySubstring`,
//! `bundleClassification`) and `A3_FocusAndMessagingTests` (`vsCodeFamilyOpensTheFolder`,
//! `otherAppsAreActivatedAndNothingIsAnEmptyPlan`), plus the Windows vectors
//! of design §4.9: host classification and tab matching.
//!
//! Mapped, not ported one to one:
//! - The Mac's "activate the app" step is `RaiseWindow` (the host's window)
//!   or `ActivatePid` (only its process known); "open the folder" is
//!   `OpenInEditor` with the editor's own exe.
//! - Bundle ids and `.app` paths become exe names, matched exactly and
//!   case-insensitively; the running-app index becomes the process table
//!   plus the windows on screen and the console's own window.
//! - iTerm2/Terminal.app's tab by TTY becomes Windows Terminal's tab by the
//!   console's title, selected only when exactly one tab carries it.
//!
//! Skipped (no Windows counterpart):
//! - `iTermSessionSelectsItsTabThenActivates`, `terminalAppWithoutTTYOnlyActivates`,
//!   `scriptableTerminalsSelectTheTabByTTY`: AppleScript tab selection by TTY.
//! - `tmuxGoesFirstAndSkipsTTYScripting`, `tmuxComesFirst`, `tmuxClientsParse`,
//!   `MessageRouteTests`: there is no tmux route on Windows.
//! - `ghosttyAndCmuxAskTheHost`, `externalFocusRunsOffTheMainActorSafely`:
//!   the host app's external tab focus (Ghostty, cmux) is Mac-only.
//! - yabai window focus, `devicePath`, the kernel process table and
//!   `iTermServer`/`wezterm-mux-server` helper hints: Mac-only mechanisms.

mod control_support;

use agentnotch_engine::control::focus::*;
use agentnotch_engine::control::focus_plan as reexported_plan;
use agentnotch_engine::control::hosts::*;
use agentnotch_engine::core::paths::{PathStyle, Paths};
use agentnotch_engine::platform::{
    ConsoleInfo, FocusOutcome, FocusStep, HostApp, HostKind, ProcessTable, Terminals,
};
use agentnotch_engine::testkit::terminal::FakeTerminals;
use control_support::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const CLAUDE: u32 = 4242;
const WT_WINDOW: u64 = 0x2_0400;
const CONSOLE_WINDOW: u64 = 0x50_0A12;
const CODE_EXE: &str = r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe";

/// Image paths for the hosts these tests use.
fn exe_path(pid: u32) -> Option<PathBuf> {
    match pid {
        700 => Some(PathBuf::from(CODE_EXE)),
        900 => Some(PathBuf::from(
            r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.21\WindowsTerminal.exe",
        )),
        _ => None,
    }
}

fn no_path(_: u32) -> Option<PathBuf> {
    None
}

fn top(window: u64, pid: u32, exe: &str) -> TopWindow {
    TopWindow {
        window,
        pid,
        exe_name: exe.into(),
    }
}

/// Claude in PowerShell inside `host_exe` (pid 900), which Explorer started.
fn under(host_exe: &str) -> ProcessTable {
    table(vec![
        proc_entry(100, 4, "explorer.exe", -1000),
        proc_entry(900, 100, host_exe, -600),
        proc_entry(300, 900, "pwsh.exe", -400),
        proc_entry(CLAUDE, 300, "claude.exe", 0),
    ])
}

/// Claude in PowerShell in VS Code's terminal: the shell runs under the
/// editor's pty host (a helper `Code.exe`, 710), under the main `Code.exe`
/// (700) that owns the windows.
fn in_vscode(exe: &str) -> ProcessTable {
    table(vec![
        proc_entry(100, 4, "explorer.exe", -1000),
        proc_entry(700, 100, exe, -900),
        proc_entry(710, 700, exe, -800),
        proc_entry(300, 710, "pwsh.exe", -400),
        proc_entry(CLAUDE, 300, "claude.exe", 0),
    ])
}

/// Claude in cmd, which Explorer started in a classic console window.
fn in_cmd() -> ProcessTable {
    table(vec![
        proc_entry(100, 4, "explorer.exe", -1000),
        proc_entry(200, 100, "cmd.exe", -500),
        proc_entry(210, 200, "conhost.exe", -499),
        proc_entry(CLAUDE, 200, "claude.exe", 0),
    ])
}

/// A ConPTY console shown by Windows Terminal's window.
fn wt_console() -> ConsoleWindow {
    ConsoleWindow {
        window: 0x7_0010,
        class: "PseudoConsoleWindow".into(),
        visible: false,
        root_owner: WT_WINDOW,
        root_owner_class: WT_WINDOW_CLASS.into(),
        root_owner_pid: Some(900),
    }
}

/// A classic console window of its own (conhost 210).
fn classic_console() -> ConsoleWindow {
    ConsoleWindow {
        window: CONSOLE_WINDOW,
        class: CONSOLE_WINDOW_CLASS.into(),
        visible: true,
        root_owner: CONSOLE_WINDOW,
        root_owner_class: CONSOLE_WINDOW_CLASS.into(),
        root_owner_pid: Some(210),
    }
}

fn host_with(kind: HostKind, window: Option<u64>, pid: Option<u32>, exe: Option<&str>) -> HostApp {
    HostApp {
        kind,
        window,
        host_pid: pid,
        exe_path: exe.map(PathBuf::from),
    }
}

fn vscode() -> HostKind {
    HostKind::VsCode {
        product: "VS Code".into(),
    }
}

fn win_paths() -> Paths {
    Paths::new(PathStyle::Windows, r"C:\Users\me")
}

// ---- Host classification ----

#[test]
fn windows_terminal_in_the_tree_hosts_the_session() {
    let tree = under("WindowsTerminal.exe");
    let matched = classify(&tree, CLAUDE);
    assert_eq!(matched.kind, HostKind::WindowsTerminal);
    assert_eq!(matched.host_pid, Some(900));
    assert_eq!(matched.chain, vec![300, 900]);

    // Without console facts, the window is the terminal's front-most one.
    let windows = [
        top(0x1, 555, "notepad.exe"),
        top(WT_WINDOW, 900, "WindowsTerminal.exe"),
        top(0x3, 900, "WindowsTerminal.exe"),
    ];
    let host = resolve(&tree, CLAUDE, None, Some(true), &windows, &exe_path);
    assert_eq!(host.kind, HostKind::WindowsTerminal);
    assert_eq!(host.window, Some(WT_WINDOW));
    assert_eq!(host.host_pid, Some(900));
    assert!(host.exe_path.is_some());
}

/// With Windows Terminal as the default terminal, a shell started from the
/// desktop has Explorer as its parent; the console's root owner tells.
#[test]
fn a_console_owned_by_a_windows_terminal_window_is_windows_terminals() {
    let tree = table(vec![
        proc_entry(100, 4, "explorer.exe", -1000),
        proc_entry(900, 77, "WindowsTerminal.exe", -2000),
        proc_entry(200, 100, "cmd.exe", -500),
        proc_entry(CLAUDE, 200, "claude.exe", 0),
    ]);
    assert_eq!(classify(&tree, CLAUDE).kind, HostKind::Unknown);
    let windows = [top(0x9, 100, "explorer.exe")];
    let host = resolve(
        &tree,
        CLAUDE,
        Some(&wt_console()),
        Some(true),
        &windows,
        &exe_path,
    );
    assert_eq!(host.kind, HostKind::WindowsTerminal);
    assert_eq!(host.window, Some(WT_WINDOW));
    assert_eq!(host.host_pid, Some(900));
}

#[test]
fn a_visible_classic_console_window_is_the_host() {
    let host = resolve(
        &in_cmd(),
        CLAUDE,
        Some(&classic_console()),
        Some(true),
        &[],
        &exe_path,
    );
    assert_eq!(host.kind, HostKind::Conhost);
    assert_eq!(host.window, Some(CONSOLE_WINDOW));

    // A hidden one says nothing: the tree decides.
    let hidden = ConsoleWindow {
        visible: false,
        ..classic_console()
    };
    let host = resolve(&in_cmd(), CLAUDE, Some(&hidden), None, &[], &no_path);
    assert_eq!(host.kind, HostKind::Unknown);
    assert_eq!(host.window, None);
}

/// The console host of a classic window is a child of the shell, never an
/// ancestor of Claude: it is found among the windows on screen.
#[test]
fn a_conhost_child_of_the_shell_is_found_among_the_windows() {
    let windows = [
        top(0x1, 555, "notepad.exe"),
        top(CONSOLE_WINDOW, 210, "conhost.exe"),
    ];
    let host = resolve(&in_cmd(), CLAUDE, None, Some(true), &windows, &no_path);
    assert_eq!(host.kind, HostKind::Conhost);
    assert_eq!(host.window, Some(CONSOLE_WINDOW));
    assert_eq!(host.host_pid, Some(210));

    // A conhost whose shell isn't on Claude's chain is someone else's.
    let stranger = table(vec![
        proc_entry(100, 4, "explorer.exe", -1000),
        proc_entry(200, 100, "cmd.exe", -500),
        proc_entry(201, 100, "cmd.exe", -450),
        proc_entry(211, 201, "conhost.exe", -449),
        proc_entry(CLAUDE, 200, "claude.exe", 0),
    ]);
    let windows = [top(0x5, 211, "conhost.exe")];
    let host = resolve(&stranger, CLAUDE, None, None, &windows, &no_path);
    assert_eq!(host.kind, HostKind::Unknown);
}

/// The console reports its window as owned by its first client: a shell, or
/// Claude itself when it was started from the desktop.
#[test]
fn a_console_window_reported_as_the_shells_is_still_a_console() {
    let windows = [top(CONSOLE_WINDOW, 200, "cmd.exe")];
    let host = resolve(&in_cmd(), CLAUDE, None, Some(true), &windows, &no_path);
    assert_eq!(host.kind, HostKind::Conhost);
    assert_eq!(host.window, Some(CONSOLE_WINDOW));

    let alone = table(vec![
        proc_entry(100, 4, "explorer.exe", -1000),
        proc_entry(CLAUDE, 100, "claude.exe", 0),
    ]);
    let windows = [top(CONSOLE_WINDOW, CLAUDE, "claude.exe")];
    let host = resolve(&alone, CLAUDE, None, Some(true), &windows, &no_path);
    assert_eq!(host.kind, HostKind::Conhost);
    assert_eq!(host.window, Some(CONSOLE_WINDOW));
}

/// `helperProcessesMapToTheirApps`: an editor's terminal runs under helper
/// processes of the same name; its windows belong to the outermost one.
#[test]
fn editor_helpers_map_to_the_outermost_editor() {
    for (exe, product) in [
        ("Code.exe", "VS Code"),
        ("Code - Insiders.exe", "VS Code"),
        ("Cursor.exe", "Cursor"),
        ("Windsurf.exe", "Windsurf"),
        ("VSCodium.exe", "VSCodium"),
    ] {
        let tree = in_vscode(exe);
        let matched = classify(&tree, CLAUDE);
        assert_eq!(
            matched.kind,
            HostKind::VsCode {
                product: product.into()
            },
            "{exe}"
        );
        assert_eq!(matched.host_pid, Some(700), "{exe}");

        // A pseudo-console's hidden window says nothing; the editor's front
        // window and image path come from the main process.
        let pseudo = ConsoleWindow {
            root_owner: 0x7_0010,
            root_owner_class: "PseudoConsoleWindow".into(),
            root_owner_pid: Some(710),
            ..wt_console()
        };
        let windows = [top(0xC0DE, 700, exe)];
        let host = resolve(
            &tree,
            CLAUDE,
            Some(&pseudo),
            Some(true),
            &windows,
            &exe_path,
        );
        assert_eq!(
            host.kind,
            HostKind::VsCode {
                product: product.into()
            }
        );
        assert_eq!(host.window, Some(0xC0DE));
        assert_eq!(host.host_pid, Some(700));
        assert_eq!(host.exe_path, Some(PathBuf::from(CODE_EXE)));
    }
}

#[test]
fn other_terminals_and_jetbrains_are_recognised() {
    for exe in [
        "wezterm-gui.exe",
        "alacritty.exe",
        "Hyper.exe",
        "Tabby.exe",
        "mintty.exe",
        "ConEmu64.exe",
    ] {
        let matched = classify(&under(exe), CLAUDE);
        assert_eq!(
            matched.kind,
            HostKind::OtherConsoleHost { exe: exe.into() },
            "{exe}"
        );
        assert_eq!(matched.host_pid, Some(900));
    }
    for exe in ["idea64.exe", "PyCharm64.exe", "rider64.exe"] {
        assert_eq!(
            classify(&under(exe), CLAUDE).kind,
            HostKind::JetBrains,
            "{exe}"
        );
    }
    let windows = [top(0x44, 900, "mintty.exe")];
    let host = resolve(
        &under("mintty.exe"),
        CLAUDE,
        None,
        Some(false),
        &windows,
        &no_path,
    );
    assert_eq!(
        host.kind,
        HostKind::OtherConsoleHost {
            exe: "mintty.exe".into()
        }
    );
    assert_eq!(host.window, Some(0x44));
}

/// `terminalNamesMatchExactlyNotBySubstring` and `bundleClassification`:
/// names match whole (any case, any folder), never as a part.
#[test]
fn names_match_exactly_not_by_substring() {
    for name in [
        "WindowsTerminal.exe",
        "windowsterminal.EXE",
        r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.21\WindowsTerminal.exe",
        "OpenConsole.exe",
        "conhost.exe",
        "CODE.EXE",
        r"C:\Users\me\AppData\Local\Programs\cursor\Cursor.exe",
        "wezterm-gui.exe",
        "idea64.exe",
    ] {
        assert!(is_terminal_process(name), "{name}");
    }
    for name in [
        "xcode.exe",
        "vscode-helper.exe",
        "code.exe.bak",
        "code",
        "terminal.exe",
        "wt.exe",
        "claude.exe",
        "cmd.exe",
        "explorer.exe",
        "",
    ] {
        assert!(!is_terminal_process(name), "{name}");
    }
    assert_eq!(editor_product("VSCodium.exe"), Some("VSCodium"));
    assert_eq!(
        editor_product(r"D:\tools\Code - Insiders.exe"),
        Some("VS Code")
    );
    assert_eq!(editor_product("WindowsTerminal.exe"), None);
    assert_eq!(editor_product("codium.exe"), None);
    assert!(is_shell("PowerShell.exe") && is_shell("pwsh.exe") && !is_shell("powershell_ise.exe"));

    // A stranger with a terminal-like name hosts nothing.
    assert_eq!(
        classify(&under("xcode.exe"), CLAUDE).kind,
        HostKind::Unknown
    );
    assert_eq!(
        classify(&under("vscode-helper.exe"), CLAUDE).kind,
        HostKind::Unknown
    );
}

/// Windows reuses pids: a "parent" younger than its child is a stranger
/// that inherited the id, and ends the walk.
#[test]
fn a_parent_younger_than_its_child_ends_the_walk() {
    let tree = table(vec![
        proc_entry(900, 100, "WindowsTerminal.exe", -600),
        // pwsh's real parent is gone; 77 is now a newer process.
        proc_entry(77, 100, "Code.exe", 30),
        proc_entry(300, 77, "pwsh.exe", -400),
        proc_entry(CLAUDE, 300, "claude.exe", 0),
    ]);
    let matched = classify(&tree, CLAUDE);
    assert_eq!(matched.kind, HostKind::Unknown);
    assert_eq!(matched.chain, vec![300]);
    // An unknown start time proves nothing either.
    let mut unknown = tree.clone();
    unknown.entries[1].started = None;
    assert_eq!(classify(&unknown, CLAUDE).chain, vec![300]);
}

/// `ancestorsWalkUpAndStopAtCycles`.
#[test]
fn ancestors_walk_up_and_stop_at_cycles() {
    let tree = table(vec![
        proc_entry(100, 90, "claude.exe", 0),
        proc_entry(90, 80, "pwsh.exe", -10),
        proc_entry(80, 4, "WindowsTerminal.exe", -20),
    ]);
    let pids = |table: &ProcessTable, pid: u32| -> Vec<u32> {
        table.ancestors(pid, 40).iter().map(|e| e.pid).collect()
    };
    assert_eq!(pids(&tree, 100), vec![90, 80]);

    // Equal start times let a racy snapshot loop.
    let cyclic = table(vec![
        proc_entry(10, 11, "a.exe", 0),
        proc_entry(11, 10, "b.exe", 0),
    ]);
    assert_eq!(pids(&cyclic, 10), vec![11]);
    assert_eq!(pids(&tree, 999), Vec::<u32>::new());
    assert_eq!(classify(&cyclic, 10).chain, vec![11]);
    // A process that names itself as its parent has none.
    let own = table(vec![proc_entry(12, 12, "c.exe", 0)]);
    assert_eq!(pids(&own, 12), Vec::<u32>::new());
}

/// `hostResolutionUsesTheRunningAppIndex`: one snapshot, several sessions;
/// nothing found is "no console" only when the helper said so.
#[test]
fn host_resolution_uses_one_snapshot() {
    let tree = table(vec![
        proc_entry(100, 4, "explorer.exe", -1000),
        proc_entry(900, 100, "WindowsTerminal.exe", -900),
        proc_entry(301, 900, "pwsh.exe", -800),
        proc_entry(303, 301, "claude.exe", 0),
        proc_entry(700, 100, "Code.exe", -900),
        proc_entry(710, 700, "Code.exe", -850),
        proc_entry(411, 710, "pwsh.exe", -700),
        proc_entry(413, 411, "claude.exe", 0),
        // The extension's Claude: a child of the extension host, no console.
        proc_entry(502, 710, "claude.exe", 0),
        // A service-started Claude with no app above it.
        proc_entry(600, 4, "services.exe", -2000),
        proc_entry(601, 600, "claude.exe", 0),
    ]);
    let windows = [
        top(WT_WINDOW, 900, "WindowsTerminal.exe"),
        top(0xC0DE, 700, "Code.exe"),
    ];
    let wt = resolve(&tree, 303, None, Some(true), &windows, &exe_path);
    assert_eq!(
        (wt.kind, wt.window),
        (HostKind::WindowsTerminal, Some(WT_WINDOW))
    );
    let code = resolve(&tree, 413, None, Some(true), &windows, &exe_path);
    assert_eq!((code.kind, code.window), (vscode(), Some(0xC0DE)));
    let ext = resolve(&tree, 502, None, Some(false), &windows, &exe_path);
    assert_eq!(ext.kind, vscode());
    assert_eq!(ext.host_pid, Some(700));

    let none = resolve(&tree, 601, None, Some(false), &windows, &exe_path);
    assert_eq!(none, host(HostKind::NoConsole));
    let unknown = resolve(&tree, 601, None, None, &windows, &exe_path);
    assert_eq!(unknown, host(HostKind::Unknown));
    let attached = resolve(&tree, 601, None, Some(true), &windows, &exe_path);
    assert_eq!(attached.kind, HostKind::Unknown);
    let gone = resolve(&tree, 999, None, Some(false), &[], &exe_path);
    assert_eq!(gone.kind, HostKind::NoConsole);
}

#[test]
fn a_running_editor_is_its_outermost_process() {
    let tree = table(vec![
        proc_entry(711, 700, "Code.exe", -800),
        proc_entry(700, 100, "Code.exe", -900),
        proc_entry(100, 4, "explorer.exe", -1000),
    ]);
    assert_eq!(running_editor(&tree), Some(700));
    assert_eq!(running_editor(&under("WindowsTerminal.exe")), None);
}

#[test]
fn rows_name_the_host() {
    let name = |kind: HostKind| host_app_name(Some(&host(kind)), Some("cli"));
    assert_eq!(
        name(HostKind::WindowsTerminal).as_deref(),
        Some("Windows Terminal")
    );
    assert_eq!(name(HostKind::Conhost).as_deref(), Some("Console"));
    assert_eq!(
        name(HostKind::VsCode {
            product: "Cursor".into()
        })
        .as_deref(),
        Some("Cursor")
    );
    assert_eq!(name(HostKind::JetBrains).as_deref(), Some("JetBrains IDE"));
    assert_eq!(
        name(HostKind::OtherConsoleHost {
            exe: "wezterm-gui.exe".into()
        })
        .as_deref(),
        Some("WezTerm")
    );
    assert_eq!(
        name(HostKind::OtherConsoleHost {
            exe: r"C:\Tools\Kitty.EXE".into()
        })
        .as_deref(),
        Some("Kitty")
    );
    assert_eq!(name(HostKind::Unknown), None);
    // The extension with no app found is still VS Code's.
    assert_eq!(
        host_app_name(Some(&host(HostKind::NoConsole)), Some("claude-vscode")).as_deref(),
        Some("VS Code")
    );
    assert_eq!(
        host_app_name(None, Some("Claude-VSCode")).as_deref(),
        Some("VS Code")
    );
    assert_eq!(host_app_name(None, None), None);
}

// ---- Focus plans ----

#[test]
fn a_console_window_is_raised() {
    let plan = focus_plan(
        &view("s1"),
        &host_with(HostKind::Conhost, Some(CONSOLE_WINDOW), Some(210), None),
        &console(CLAUDE),
    );
    assert_eq!(
        plan,
        vec![FocusStep::RaiseWindow {
            window: CONSOLE_WINDOW
        }]
    );
    // The console's own window wins over a window found another way.
    let plan = focus_plan(
        &view("s1"),
        &host_with(HostKind::Conhost, Some(0x99), None, None),
        &console(CLAUDE),
    );
    assert_eq!(
        plan,
        vec![FocusStep::RaiseWindow {
            window: CONSOLE_WINDOW
        }]
    );
}

#[test]
fn windows_terminal_selects_the_tab_by_title_then_raises() {
    let wt = host_with(HostKind::WindowsTerminal, Some(WT_WINDOW), Some(900), None);
    let plan = reexported_plan(&view("s1"), &wt, &console(CLAUDE));
    assert_eq!(
        plan,
        vec![
            FocusStep::SelectWtTab {
                window: WT_WINDOW,
                title: "✳ Refactor the parser".into(),
            },
            FocusStep::RaiseWindow { window: WT_WINDOW },
        ]
    );
    // No title, a blank one, or no console attached: the window only.
    for info in [
        ConsoleInfo {
            title: None,
            ..console(CLAUDE)
        },
        ConsoleInfo {
            title: Some("  ".into()),
            ..console(CLAUDE)
        },
        ConsoleInfo {
            attached: false,
            ..console(CLAUDE)
        },
    ] {
        assert_eq!(
            focus_plan(&view("s1"), &wt, &info),
            vec![FocusStep::RaiseWindow { window: WT_WINDOW }]
        );
    }
    // Only the terminal's process known.
    let pid_only = host_with(HostKind::WindowsTerminal, None, Some(900), None);
    assert_eq!(
        focus_plan(&view("s1"), &pid_only, &ConsoleInfo::default()),
        vec![FocusStep::ActivatePid { pid: 900 }]
    );
}

/// `editorTerminalOpensTheFolder` and `vsCodeFamilyOpensTheFolder`.
#[test]
fn an_editor_terminal_opens_its_workspace_root() {
    let code = host_with(vscode(), Some(0xC0DE), Some(700), Some(CODE_EXE));
    let root = Path::new(r"C:\Users\me\code\app");
    let extras = FocusExtras {
        workspace_root: Some(root),
        fallback_editor: None,
    };
    assert_eq!(
        focus_plan_with(&view("s1"), &code, &console(CLAUDE), &extras),
        vec![
            FocusStep::OpenInEditor {
                editor_exe: CODE_EXE.into(),
                folder: root.into(),
            },
            FocusStep::RaiseWindow { window: 0xC0DE },
        ]
    );
    // No workspace root known (BHV-8): the editor is raised, no folder opened.
    assert_eq!(
        focus_plan(&view("s1"), &code, &console(CLAUDE)),
        vec![FocusStep::RaiseWindow { window: 0xC0DE }]
    );
    // No image path for the editor: nothing to open the folder with.
    let pathless = host_with(vscode(), None, Some(700), None);
    assert_eq!(
        focus_plan_with(&view("s1"), &pathless, &console(CLAUDE), &extras),
        vec![FocusStep::ActivatePid { pid: 700 }]
    );
}

/// `anEditorTerminalInASubfolderOpensTheWorkspaceRoot`.
#[test]
fn an_editor_terminal_in_a_subfolder_opens_the_workspace_root() {
    let paths = win_paths();
    let gits: BTreeSet<String> = [r"C:\Users\me\project\.git".to_owned()].into();
    let exists = |path: &str| gits.contains(path);
    let code = host_with(vscode(), Some(0xC0DE), Some(700), Some(CODE_EXE));
    let plan_in = |cwd: &str| {
        let mut session = view("s1");
        session.cwd = PathBuf::from(cwd);
        let root = workspace_root(cwd, &paths, &exists);
        let extras = FocusExtras {
            workspace_root: root.as_deref().map(Path::new),
            fallback_editor: None,
        };
        focus_plan_with(&session, &code, &console(CLAUDE), &extras)
    };
    assert_eq!(
        plan_in(r"C:\Users\me\project\packages\api"),
        vec![
            FocusStep::OpenInEditor {
                editor_exe: CODE_EXE.into(),
                folder: r"C:\Users\me\project".into(),
            },
            FocusStep::RaiseWindow { window: 0xC0DE },
        ]
    );
    assert_eq!(
        plan_in(r"C:\Users\me\scratch"),
        vec![FocusStep::RaiseWindow { window: 0xC0DE }]
    );

    // The extension always runs in its workspace root.
    let mut ext = view("s1");
    ext.entrypoint = Some("claude-vscode".into());
    ext.pid = None;
    ext.cwd = PathBuf::from(r"C:\Users\me\project");
    let extras = FocusExtras {
        workspace_root: None,
        fallback_editor: Some(Path::new(CODE_EXE)),
    };
    assert_eq!(
        focus_plan_with(
            &ext,
            &host(HostKind::NoConsole),
            &ConsoleInfo::default(),
            &extras
        ),
        vec![FocusStep::OpenInEditor {
            editor_exe: CODE_EXE.into(),
            folder: r"C:\Users\me\project".into(),
        }]
    );
}

/// `theWorkspaceRootIsTheNearestGitFolderBelowHome`, in both path styles.
#[test]
fn the_workspace_root_is_the_nearest_git_folder_below_home() {
    let posix = Paths::new(PathStyle::Posix, "/Users/me");
    let gits: BTreeSet<&str> = ["/Users/me/project/.git", "/Users/me/.git"].into();
    let root = |cwd: &str| workspace_root(cwd, &posix, &|path| gits.contains(path));
    assert_eq!(
        root("/Users/me/project/packages/api").as_deref(),
        Some("/Users/me/project")
    );
    assert_eq!(
        root("/Users/me/project").as_deref(),
        Some("/Users/me/project")
    );
    // Home itself (a dotfiles repo) is never the workspace.
    assert_eq!(root("/Users/me/notes"), None);
    assert_eq!(root("/Users/me"), None);
    assert_eq!(root("/opt/x"), None);
    assert_eq!(root(""), None);

    // Windows: case-insensitive home, a worktree's `.git` file counts the
    // same, and a repository outside the home folder (another drive) is
    // found as on the Mac, whose walk stops only at home or a root.
    let paths = win_paths();
    let gits: BTreeSet<&str> = [
        r"C:\Users\me\.git",
        r"C:\Users\me\wt\feature\.git",
        r"D:\code\repo\.git",
    ]
    .into();
    let root = |cwd: &str| workspace_root(cwd, &paths, &|path| gits.contains(path));
    assert_eq!(
        root(r"C:\Users\me\wt\feature\src").as_deref(),
        Some(r"C:\Users\me\wt\feature")
    );
    assert_eq!(root(r"c:\users\ME\notes"), None);
    assert_eq!(root(r"D:\code\repo\src").as_deref(), Some(r"D:\code\repo"));
    assert_eq!(root(r"D:\scratch"), None);
    assert_eq!(root("   "), None);
}

/// `vsCodeExtensionUsesARunningEditor`.
#[test]
fn the_vscode_extension_uses_a_running_editor() {
    let mut ext = view("s1");
    ext.entrypoint = Some("claude-vscode".into());
    let cursor = Path::new(r"C:\Users\me\AppData\Local\Programs\cursor\Cursor.exe");
    let extras = FocusExtras {
        workspace_root: None,
        fallback_editor: Some(cursor),
    };
    let no_console = ConsoleInfo {
        attached: false,
        ..ConsoleInfo::default()
    };
    assert_eq!(
        focus_plan_with(&ext, &host(HostKind::NoConsole), &no_console, &extras),
        vec![FocusStep::OpenInEditor {
            editor_exe: cursor.into(),
            folder: r"C:\Users\me\code\app".into(),
        }]
    );
    // No running editor: nothing to do.
    assert!(focus_plan(&ext, &host(HostKind::NoConsole), &no_console).is_empty());
    // An extension whose tree leads to its editor opens the cwd with it.
    let code = host_with(vscode(), Some(0xC0DE), Some(700), Some(CODE_EXE));
    assert_eq!(
        focus_plan_with(&ext, &code, &no_console, &extras),
        vec![
            FocusStep::OpenInEditor {
                editor_exe: CODE_EXE.into(),
                folder: r"C:\Users\me\code\app".into(),
            },
            FocusStep::RaiseWindow { window: 0xC0DE },
        ]
    );
    // A CLI session never borrows a running editor.
    let cli_extras = FocusExtras {
        workspace_root: Some(Path::new(r"C:\Users\me\code\app")),
        fallback_editor: Some(cursor),
    };
    assert!(focus_plan_with(
        &view("s1"),
        &host(HostKind::NoConsole),
        &no_console,
        &cli_extras
    )
    .is_empty());
}

/// `otherTerminalsAreActivated` and `otherAppsAreActivatedAndNothingIsAnEmptyPlan`.
#[test]
fn other_terminals_are_raised_and_nothing_is_an_empty_plan() {
    let other = HostKind::OtherConsoleHost {
        exe: "wezterm-gui.exe".into(),
    };
    for kind in [other.clone(), HostKind::JetBrains] {
        assert_eq!(
            focus_plan(
                &view("s1"),
                &host_with(kind.clone(), Some(0x42), Some(42), None),
                &console(CLAUDE)
            ),
            vec![FocusStep::RaiseWindow { window: 0x42 }]
        );
        assert_eq!(
            focus_plan(
                &view("s1"),
                &host_with(kind, None, Some(42), None),
                &console(CLAUDE)
            ),
            vec![FocusStep::ActivatePid { pid: 42 }]
        );
    }
    // `nothingToFocusWithoutAnApp`.
    for kind in [HostKind::Unknown, HostKind::NoConsole, other] {
        assert!(focus_plan(&view("s1"), &host(kind), &ConsoleInfo::default()).is_empty());
    }
    assert!(focus_plan(
        &view("s1"),
        &host(HostKind::Conhost),
        &ConsoleInfo::default()
    )
    .is_empty());
}

#[test]
fn the_jump_button_label_and_whether_it_shows() {
    let mut ext = view("s1");
    ext.entrypoint = Some("claude-vscode".into());
    assert_eq!(focus_label(&ext), "Show in editor");
    assert_eq!(focus_label(&view("s1")), "Show terminal");

    let none = FocusExtras::default();
    let editor = FocusExtras {
        workspace_root: None,
        fallback_editor: Some(Path::new(CODE_EXE)),
    };
    // Not looked up yet: shown meanwhile.
    assert!(can_focus(&view("s1"), None, None, &none));
    let wt = host_with(HostKind::WindowsTerminal, Some(WT_WINDOW), Some(900), None);
    assert!(can_focus(&view("s1"), Some(&wt), None, &none));
    assert!(!can_focus(
        &view("s1"),
        Some(&host(HostKind::Unknown)),
        Some(&console(CLAUDE)),
        &none
    ));
    // A Conhost found by its console window alone.
    assert!(can_focus(
        &view("s1"),
        Some(&host(HostKind::Conhost)),
        Some(&console(CLAUDE)),
        &none
    ));
    // Without a process: only the extension's, with an editor running.
    let mut gone = view("s1");
    gone.pid = None;
    assert!(!can_focus(&gone, Some(&wt), None, &editor));
    ext.pid = None;
    assert!(can_focus(&ext, None, None, &editor));
    assert!(!can_focus(&ext, None, None, &none));
}

// ---- Windows Terminal tabs ----

fn tabs(names: &[(&str, bool)]) -> Vec<(String, bool)> {
    names
        .iter()
        .map(|(name, selected)| ((*name).to_owned(), *selected))
        .collect()
}

#[test]
fn exactly_one_tab_with_the_title_is_selected() {
    let title = "✳ Refactor the parser";
    // Whatever tab is selected now.
    let list = tabs(&[
        ("PowerShell", true),
        ("✳ Refactor the parser", false),
        ("Ubuntu", false),
    ]);
    assert_eq!(wt_tab_match(&list, title), TabMatch::One(1));
    let list = tabs(&[("✳ Refactor the parser", true), ("PowerShell", false)]);
    assert_eq!(wt_tab_match(&list, title), TabMatch::One(0));
    // Surrounding whitespace on either side is not a difference.
    let list = tabs(&[("  ✳ Refactor the parser ", false)]);
    assert_eq!(
        wt_tab_match(&list, " ✳ Refactor the parser\t"),
        TabMatch::One(0)
    );
}

/// Picking the wrong tab puts the user's keystrokes into another session:
/// anything but exactly one equal name selects nothing.
#[test]
fn no_tab_or_several_select_nothing() {
    let title = "Refactor the parser";
    assert_eq!(wt_tab_match(&[], title), TabMatch::None);
    assert_eq!(
        wt_tab_match(&tabs(&[("PowerShell", true)]), title),
        TabMatch::None
    );
    assert_eq!(
        wt_tab_match(
            &tabs(&[(title, true), ("PowerShell", false), (title, false)]),
            title
        ),
        TabMatch::Ambiguous
    );
    // A blank title matches nothing, not even blank tabs.
    let blank = tabs(&[("", true), ("  ", false)]);
    assert_eq!(wt_tab_match(&blank, ""), TabMatch::None);
    assert_eq!(wt_tab_match(&blank, "   "), TabMatch::None);
    // No fuzzy matching: a spinner glyph, a prefix or another case is
    // another name.
    for name in [
        "✳ Refactor the parser",
        "⠂ Refactor the parser",
        "Refactor the parser (2)",
        "Refactor",
        "refactor the parser",
    ] {
        assert_eq!(
            wt_tab_match(&tabs(&[(name, false)]), title),
            TabMatch::None,
            "{name}"
        );
    }
}

// ---- Running a plan ----

fn scripted(outcomes: Vec<FocusOutcome>) -> impl FnMut(&FocusStep) -> FocusOutcome {
    let mut outcomes = outcomes.into_iter();
    move |_| outcomes.next().unwrap_or(FocusOutcome::NotFound)
}

#[test]
fn a_plan_runs_until_a_step_works() {
    let tab = FocusStep::SelectWtTab {
        window: WT_WINDOW,
        title: "x".into(),
    };
    let raise = FocusStep::RaiseWindow { window: WT_WINDOW };
    let steps = vec![tab, raise];
    let run = |outcomes: Vec<FocusOutcome>| {
        let mut ran = 0;
        let mut script = scripted(outcomes);
        let outcome = run_plan(&steps, &mut |step| {
            ran += 1;
            script(step)
        });
        (outcome, ran)
    };
    // The first step working is the session itself.
    assert_eq!(run(vec![FocusOutcome::Focused]), (FocusOutcome::Focused, 1));
    // A later one only brought it near.
    assert_eq!(
        run(vec![FocusOutcome::NotFound, FocusOutcome::Focused]),
        (FocusOutcome::RaisedOnly, 2)
    );
    assert_eq!(
        run(vec![FocusOutcome::RaisedOnly]),
        (FocusOutcome::RaisedOnly, 1)
    );
    assert_eq!(
        run(vec![FocusOutcome::NotFound, FocusOutcome::NotFound]),
        (FocusOutcome::NotFound, 2)
    );
    // The first failure's reason is kept when nothing works.
    assert_eq!(
        run(vec![
            FocusOutcome::Failed("UI Automation timed out".into()),
            FocusOutcome::Failed("refused".into()),
        ]),
        (FocusOutcome::Failed("UI Automation timed out".into()), 2)
    );
    assert_eq!(
        run(vec![
            FocusOutcome::Failed("UI Automation timed out".into()),
            FocusOutcome::Focused,
        ]),
        (FocusOutcome::RaisedOnly, 2)
    );
    assert_eq!(
        run_plan(&[], &mut |_| FocusOutcome::Focused),
        FocusOutcome::NotFound
    );
}

#[test]
fn what_a_jump_tells_the_user() {
    assert!(jumped(&FocusOutcome::Focused));
    assert!(jumped(&FocusOutcome::RaisedOnly));
    assert!(!jumped(&FocusOutcome::NotFound));
    assert!(!jumped(&FocusOutcome::Failed("x".into())));

    let wt = host(HostKind::WindowsTerminal);
    assert_eq!(
        outcome_reply(&FocusOutcome::Focused, &wt),
        ("focused", None)
    );
    assert_eq!(
        outcome_reply(&FocusOutcome::RaisedOnly, &wt),
        (
            "raised_only",
            Some("Brought its Windows Terminal window forward".into())
        )
    );
    assert_eq!(
        outcome_reply(
            &FocusOutcome::RaisedOnly,
            &host(HostKind::VsCode {
                product: "Cursor".into()
            })
        ),
        ("raised_only", Some("Brought Cursor forward".into()))
    );
    assert_eq!(
        outcome_reply(&FocusOutcome::RaisedOnly, &host(HostKind::JetBrains)),
        ("raised_only", Some("Brought its window forward".into()))
    );
    assert_eq!(
        outcome_reply(&FocusOutcome::NotFound, &wt),
        ("not_found", Some("Its terminal window wasn't found".into()))
    );
    assert_eq!(
        outcome_reply(&FocusOutcome::Failed("refused".into()), &wt),
        ("failed", Some("refused".into()))
    );
}

/// The whole jump through the test platform: the host and console it
/// reports, the plan, and the steps it was asked to run.
#[test]
fn a_jump_through_the_fake_terminals() {
    let terminals = FakeTerminals::default();
    let wt = host_with(HostKind::WindowsTerminal, Some(WT_WINDOW), Some(900), None);
    terminals.set_host(CLAUDE, wt.clone());
    terminals.set_console(CLAUDE, console(CLAUDE));

    let host = terminals.classify_host(CLAUDE, &ProcessTable::default());
    let info = terminals.console_info(CLAUDE);
    let plan = focus_plan(&view("s1"), &host, &info);

    // No tab carries the title: the window comes forward instead.
    let outcome = run_plan(&plan, &mut |step| terminals.run_focus(step));
    assert_eq!(outcome, FocusOutcome::NotFound);
    assert_eq!(terminals.focus_steps(), plan);

    let terminals = FakeTerminals::default();
    terminals.set_host(CLAUDE, wt.clone());
    terminals.set_console(CLAUDE, console(CLAUDE));
    terminals.set_focus_outcome(FocusOutcome::Focused);
    let outcome = run_plan(&plan, &mut |step| terminals.run_focus(step));
    assert_eq!(outcome, FocusOutcome::Focused);
    assert_eq!(terminals.focus_steps(), plan[..1].to_vec());
    assert!(jumped(&outcome));

    // A session the platform knows nothing about has nothing to run.
    let unknown = terminals.classify_host(1, &ProcessTable::default());
    assert!(focus_plan(&view("s2"), &unknown, &terminals.console_info(1)).is_empty());
}
