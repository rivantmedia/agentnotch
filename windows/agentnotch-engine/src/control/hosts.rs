//! Which app hosts a session's terminal (§4.9): read off the process tree by
//! executable name, then corrected by what the session's own console window
//! says. The platform supplies the facts (the process table, the console
//! window's class and owner, the windows on screen); every choice is made
//! here, so it is tested on every OS.
//!
//! Names match exactly, never by substring: a substring match on names like
//! "code" or "st" would take Xcode-like strangers for terminals.

use crate::platform::{HostApp, HostKind, ProcEntry, ProcessTable};
use std::path::PathBuf;

/// The shells that sit between Claude and its terminal. They are also the
/// only processes, besides Claude and its children, that may share Claude's
/// console while a reply is typed (§4.8).
pub const SHELLS: [&str; 5] = [
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "bash.exe",
    "sh.exe",
];

/// Windows Terminal's one process, which owns every one of its windows.
pub const WINDOWS_TERMINAL: &str = "windowsterminal.exe";

/// The console host: a child of the shell for a classic console window, of
/// the terminal for a pseudo-console.
const CONSOLE_HOSTS: [&str; 2] = ["openconsole.exe", "conhost.exe"];

/// VS Code and its forks, with the name rows show for each.
const EDITORS: [(&str, &str); 5] = [
    ("code.exe", "VS Code"),
    ("code - insiders.exe", "VS Code"),
    ("cursor.exe", "Cursor"),
    ("windsurf.exe", "Windsurf"),
    ("vscodium.exe", "VSCodium"),
];

/// Other terminals that host a console of their own, with their names.
const TERMINALS: [(&str, &str); 6] = [
    ("wezterm-gui.exe", "WezTerm"),
    ("alacritty.exe", "Alacritty"),
    ("hyper.exe", "Hyper"),
    ("tabby.exe", "Tabby"),
    ("mintty.exe", "mintty"),
    ("conemu64.exe", "ConEmu"),
];

/// JetBrains IDEs, whose terminal tab hosts the session.
const JETBRAINS: [&str; 14] = [
    "idea64.exe",
    "pycharm64.exe",
    "webstorm64.exe",
    "phpstorm64.exe",
    "clion64.exe",
    "goland64.exe",
    "rider64.exe",
    "rubymine64.exe",
    "datagrip64.exe",
    "dataspell64.exe",
    "rustrover64.exe",
    "aqua64.exe",
    "writerside64.exe",
    "studio64.exe",
];

/// The desktop shell and the session's system processes: a shell started
/// from the Start menu has Explorer as its parent, which says nothing about
/// where its console is, and whose windows (folders, the desktop) are never
/// a terminal.
const NEVER_A_HOST: [&str; 10] = [
    "explorer.exe",
    "sihost.exe",
    "svchost.exe",
    "services.exe",
    "winlogon.exe",
    "wininit.exe",
    "csrss.exe",
    "dwm.exe",
    "userinit.exe",
    "system",
];

/// A process name as the tables hold it: the file name, lower-cased (Windows
/// names compare case-insensitively).
fn exe_key(name: &str) -> String {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    file.trim().to_lowercase()
}

pub fn is_shell(exe: &str) -> bool {
    SHELLS.contains(&exe_key(exe).as_str())
}

/// "VS Code", "Cursor", … for an editor of the VS Code family.
pub fn editor_product(exe: &str) -> Option<&'static str> {
    let key = exe_key(exe);
    EDITORS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, product)| *product)
}

fn terminal_name(exe: &str) -> Option<&'static str> {
    let key = exe_key(exe);
    TERMINALS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, product)| *product)
}

fn is_windows_terminal(exe: &str) -> bool {
    exe_key(exe) == WINDOWS_TERMINAL
}

fn is_console_host(exe: &str) -> bool {
    CONSOLE_HOSTS.contains(&exe_key(exe).as_str())
}

fn is_jetbrains(exe: &str) -> bool {
    JETBRAINS.contains(&exe_key(exe).as_str())
}

fn never_a_host(exe: &str) -> bool {
    NEVER_A_HOST.contains(&exe_key(exe).as_str())
}

/// A terminal or an editor with terminals: a window of one of these on
/// screen means the user can see a session (the visibility check, UI§3.8).
pub fn is_terminal_process(exe: &str) -> bool {
    is_windows_terminal(exe)
        || is_console_host(exe)
        || editor_product(exe).is_some()
        || terminal_name(exe).is_some()
        || is_jetbrains(exe)
}

/// What the process tree says about a session's host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostMatch {
    pub kind: HostKind,
    /// The process that owns the host's windows.
    pub host_pid: Option<u32>,
    /// Claude's ancestors that could own its terminal's window, nearest
    /// first: every valid parent link up to, and not including, the desktop
    /// shell.
    pub chain: Vec<u32>,
}

/// How far up the tree a host is looked for.
const MAX_DEPTH: usize = 40;

fn kind_of(entry: &ProcEntry) -> Option<HostKind> {
    let exe = &entry.exe_name;
    if is_windows_terminal(exe) {
        Some(HostKind::WindowsTerminal)
    } else if let Some(product) = editor_product(exe) {
        Some(HostKind::VsCode {
            product: product.to_owned(),
        })
    } else if terminal_name(exe).is_some() {
        Some(HostKind::OtherConsoleHost {
            exe: entry.exe_name.clone(),
        })
    } else if is_jetbrains(exe) {
        Some(HostKind::JetBrains)
    } else if is_console_host(exe) {
        Some(HostKind::Conhost)
    } else {
        None
    }
}

/// The nearest ancestor of `claude_pid` that is a known terminal or editor.
/// Parent links count only when the parent is older than its child (Windows
/// reuses pids, and a stale parent id can name an unrelated process).
pub fn classify(table: &ProcessTable, claude_pid: u32) -> HostMatch {
    let mut chain = Vec::new();
    let mut found: Option<(HostKind, u32)> = None;
    for ancestor in table.ancestors(claude_pid, MAX_DEPTH) {
        if never_a_host(&ancestor.exe_name) {
            break;
        }
        chain.push(ancestor.pid);
        match &found {
            None => {
                if let Some(kind) = kind_of(ancestor) {
                    found = Some((kind, ancestor.pid));
                }
            }
            // An editor runs its terminals in helper processes of the same
            // name; its windows belong to the outermost one.
            Some((kind, _)) if kind_of(ancestor).as_ref() == Some(kind) => {
                found = Some((kind.clone(), ancestor.pid));
            }
            Some(_) => break,
        }
    }
    match found {
        Some((kind, pid)) => HostMatch {
            kind,
            host_pid: Some(pid),
            chain,
        },
        None => HostMatch {
            kind: HostKind::Unknown,
            host_pid: None,
            chain,
        },
    }
}

/// A visible top-level window, as the platform lists them front to back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopWindow {
    pub window: u64,
    pub pid: u32,
    /// The owning process's file name.
    pub exe_name: String,
}

/// The window class of a Windows Terminal window.
pub const WT_WINDOW_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";
/// …and of a classic console window.
pub const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// What the platform finds out about the console window `console-info`
/// named for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleWindow {
    /// `GetConsoleWindow()` of Claude's console: the console window itself,
    /// or a pseudo-console's hidden window.
    pub window: u64,
    pub class: String,
    pub visible: bool,
    /// `GetAncestor(GA_ROOTOWNER)`: for a pseudo-console inside Windows
    /// Terminal, the terminal window that shows it.
    pub root_owner: u64,
    pub root_owner_class: String,
    pub root_owner_pid: Option<u32>,
}

/// The front-most window of the process tree's host; without a known host,
/// the console host's own window when the shell has one, else the window of
/// the outermost process of the chain (Claude last) that has any: the app the
/// terminal lives in, or a classic console window, which the console reports
/// as owned by its first client (`ConsoleSetWindowOwner`), a shell or Claude.
fn tree_window<'a>(
    matched: &HostMatch,
    table: &ProcessTable,
    claude_pid: u32,
    windows: &'a [TopWindow],
) -> Option<&'a TopWindow> {
    if let Some(host) = matched.host_pid {
        return windows.iter().find(|window| window.pid == host);
    }
    // A classic console window belongs to a conhost that is the child of
    // the shell (or of Claude, started from the desktop), never an ancestor.
    let on_chain = |pid: u32| pid == claude_pid || matched.chain.contains(&pid);
    let console = windows.iter().find(|window| {
        is_console_host(&window.exe_name)
            && table
                .parent(window.pid)
                .is_some_and(|parent| on_chain(parent.pid))
    });
    console.or_else(|| {
        matched
            .chain
            .iter()
            .rev()
            .chain(std::iter::once(&claude_pid))
            .find_map(|pid| windows.iter().find(|window| window.pid == *pid))
    })
}

/// The session's host, from everything known about it.
///
/// - A console whose window is owned by a Windows Terminal window runs in
///   that window, whatever the process tree says: with Windows Terminal as
///   the default terminal, a shell started from the desktop has Explorer as
///   its parent.
/// - A visible classic console window is the host itself.
/// - Otherwise the process tree decides, and the window is the host's
///   front-most one (one process owns every window of an editor, so which
///   of several it is can't be told from outside).
/// - `console_attached` false with no host found: the session has no
///   console at all (the VS Code extension, an SDK host).
///
/// `exe_path` gives a process's image path (the editor to open a folder
/// with).
pub fn resolve(
    table: &ProcessTable,
    claude_pid: u32,
    console: Option<&ConsoleWindow>,
    console_attached: Option<bool>,
    windows: &[TopWindow],
    exe_path: &dyn Fn(u32) -> Option<PathBuf>,
) -> HostApp {
    let matched = classify(table, claude_pid);
    if let Some(console) = console {
        if console.root_owner_class == WT_WINDOW_CLASS {
            let host_pid = console.root_owner_pid.or(matched.host_pid);
            return HostApp {
                kind: HostKind::WindowsTerminal,
                window: Some(console.root_owner),
                host_pid,
                exe_path: host_pid.and_then(exe_path),
            };
        }
        if console.class == CONSOLE_WINDOW_CLASS && console.visible {
            return HostApp {
                kind: HostKind::Conhost,
                window: Some(console.window),
                host_pid: console.root_owner_pid,
                exe_path: None,
            };
        }
    }
    let window = tree_window(&matched, table, claude_pid, windows);
    match (&matched.kind, window) {
        (HostKind::Unknown, Some(window)) => {
            // A shell or Claude itself has a top-level window only as the
            // owner the console names for its classic console window.
            let console_window = is_console_host(&window.exe_name)
                || is_shell(&window.exe_name)
                || window.pid == claude_pid;
            let kind = if console_window {
                HostKind::Conhost
            } else {
                HostKind::OtherConsoleHost {
                    exe: window.exe_name.clone(),
                }
            };
            HostApp {
                kind,
                window: Some(window.window),
                host_pid: Some(window.pid),
                exe_path: exe_path(window.pid),
            }
        }
        (HostKind::Unknown, None) => HostApp {
            kind: if console_attached == Some(false) {
                HostKind::NoConsole
            } else {
                HostKind::Unknown
            },
            window: None,
            host_pid: None,
            exe_path: None,
        },
        (kind, window) => HostApp {
            kind: kind.clone(),
            window: window.map(|window| window.window),
            host_pid: matched.host_pid,
            exe_path: matched.host_pid.and_then(exe_path),
        },
    }
}

/// Where a session runs, as rows name it ("Windows Terminal", "VS Code",
/// "WezTerm"); the VS Code extension with no app found is still VS Code's.
/// `None` when nothing is known.
pub fn host_app_name(host: Option<&HostApp>, entrypoint: Option<&str>) -> Option<String> {
    let named = host.and_then(|host| match &host.kind {
        HostKind::WindowsTerminal => Some("Windows Terminal".to_owned()),
        HostKind::Conhost => Some("Console".to_owned()),
        HostKind::VsCode { product } => Some(product.clone()),
        HostKind::JetBrains => Some("JetBrains IDE".to_owned()),
        HostKind::OtherConsoleHost { exe } => Some(match terminal_name(exe) {
            Some(name) => name.to_owned(),
            None => {
                let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe).trim();
                let stem = match file.len().checked_sub(4) {
                    Some(cut)
                        if file.is_char_boundary(cut)
                            && file[cut..].eq_ignore_ascii_case(".exe") =>
                    {
                        &file[..cut]
                    }
                    _ => file,
                };
                stem.to_owned()
            }
        })
        .filter(|name| !name.is_empty()),
        HostKind::NoConsole | HostKind::Unknown => None,
    });
    named.or_else(|| is_vscode_extension(entrypoint).then(|| "VS Code".to_owned()))
}

/// The `claude-vscode` extension: a session with no terminal at all.
pub fn is_vscode_extension(entrypoint: Option<&str>) -> bool {
    entrypoint.is_some_and(|entry| entry.eq_ignore_ascii_case("claude-vscode"))
}

/// A running editor of the VS Code family, to open the folder of an
/// extension session whose own process tree doesn't lead to one: the
/// outermost process of the first one found.
pub fn running_editor(table: &ProcessTable) -> Option<u32> {
    let editor = table
        .entries
        .iter()
        .find(|entry| editor_product(&entry.exe_name).is_some())?;
    let family = exe_key(&editor.exe_name);
    let mut outermost = editor.pid;
    for ancestor in table.ancestors(editor.pid, MAX_DEPTH) {
        if exe_key(&ancestor.exe_name) != family {
            break;
        }
        outermost = ancestor.pid;
    }
    Some(outermost)
}
