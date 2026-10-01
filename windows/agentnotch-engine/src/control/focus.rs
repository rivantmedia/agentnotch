//! Jumping to a session's terminal or editor (HS§9, §4.9): which steps are
//! worth trying, best first, and what came of them. The platform carries
//! each step out (`Terminals::run_focus`) and reports plain outcomes.
//!
//! | Host | Steps |
//! |---|---|
//! | Windows Terminal | select the tab whose name is the console's title, then raise the window |
//! | console window | raise it |
//! | VS Code family | open the session's workspace folder with the editor (which focuses the window that has it open), then raise the editor |
//! | anything else with a window | raise it |
//!
//! The exact tab can only be selected in Windows Terminal, and only when
//! exactly one tab carries the console's title; everywhere else the window
//! comes forward and the outcome says so (`RaisedOnly`).

use super::hosts::is_vscode_extension;
use crate::core::paths::Paths;
use crate::model::SessionView;
use crate::platform::{ConsoleInfo, FocusOutcome, FocusStep, HostApp, HostKind};
use std::path::{Path, PathBuf};

/// What a plan may use besides the session, its host and its console.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FocusExtras<'a> {
    /// The workspace an editor most likely has open for the session's
    /// folder ([`workspace_root`]).
    pub workspace_root: Option<&'a Path>,
    /// A running editor of the VS Code family, for an extension session
    /// whose own process tree doesn't lead to one.
    pub fallback_editor: Option<&'a Path>,
}

/// The folder an editor window most likely has open for a session started
/// in `cwd`: the nearest folder at or above it holding `.git` (a folder, or
/// a file for a worktree), below the home folder. `None` when there is
/// none. Home itself (a dotfiles repository) is never the workspace.
pub fn workspace_root(cwd: &str, paths: &Paths, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    if cwd.trim().is_empty() {
        return None;
    }
    let mut folder = paths.normalize(cwd);
    loop {
        if folder.is_empty() || paths.is_root(&folder) || paths.same(&folder, paths.home()) {
            return None;
        }
        if exists(&paths.join(&folder, ".git")) {
            return Some(folder);
        }
        folder = paths.parent(&folder)?;
    }
}

fn raise(host: &HostApp, steps: &mut Vec<FocusStep>) {
    if let Some(window) = host.window {
        steps.push(FocusStep::RaiseWindow { window });
    } else if let Some(pid) = host.host_pid {
        steps.push(FocusStep::ActivatePid { pid });
    }
}

/// [`focus_plan_with`] when neither a workspace root nor a fallback editor
/// is known: a CLI in an editor's terminal only brings the editor forward.
pub fn focus_plan(view: &SessionView, host: &HostApp, info: &ConsoleInfo) -> Vec<FocusStep> {
    focus_plan_with(view, host, info, &FocusExtras::default())
}

/// The steps worth trying for a session, best first. Empty when nothing
/// could bring it forward (no window and no app found).
pub fn focus_plan_with(
    view: &SessionView,
    host: &HostApp,
    info: &ConsoleInfo,
    extras: &FocusExtras<'_>,
) -> Vec<FocusStep> {
    let mut steps = Vec::new();
    let is_extension = is_vscode_extension(view.entrypoint.as_deref());
    let console_window = if info.attached { info.window } else { None };
    let cwd = (!view.cwd.as_os_str().is_empty()).then(|| view.cwd.clone());
    match &host.kind {
        HostKind::WindowsTerminal => {
            // The window the platform resolved, else the console's own
            // window, which it resolves to the terminal window that owns it.
            if let Some(window) = host.window.or(console_window) {
                let title = info.title.as_deref().map(str::trim).unwrap_or("");
                if info.attached && !title.is_empty() {
                    steps.push(FocusStep::SelectWtTab {
                        window,
                        title: title.to_owned(),
                    });
                }
                steps.push(FocusStep::RaiseWindow { window });
            } else {
                raise(host, &mut steps);
            }
        }
        HostKind::Conhost => {
            if let Some(window) = console_window.or(host.window) {
                steps.push(FocusStep::RaiseWindow { window });
            }
        }
        HostKind::VsCode { .. } => {
            // Opening a folder brings the editor window that has it open to
            // the front, but opens a new window for any other folder. The
            // extension runs in its workspace's root; a CLI in the editor's
            // terminal may have been started in a subfolder, so its
            // workspace root is opened instead, and without one the editor
            // is only brought forward.
            let folder: Option<PathBuf> = if is_extension {
                cwd
            } else {
                extras.workspace_root.map(Path::to_path_buf)
            };
            let editor = host.exe_path.clone().or_else(|| {
                is_extension
                    .then(|| extras.fallback_editor.map(Path::to_path_buf))
                    .flatten()
            });
            if let (Some(editor_exe), Some(folder)) = (editor, folder) {
                steps.push(FocusStep::OpenInEditor { editor_exe, folder });
            }
            raise(host, &mut steps);
        }
        HostKind::JetBrains | HostKind::OtherConsoleHost { .. } => raise(host, &mut steps),
        HostKind::NoConsole | HostKind::Unknown => {
            // The extension with no app in its process tree uses a running
            // editor.
            if let (true, Some(editor), Some(folder)) = (is_extension, extras.fallback_editor, cwd)
            {
                steps.push(FocusStep::OpenInEditor {
                    editor_exe: editor.to_path_buf(),
                    folder,
                });
            }
            raise(host, &mut steps);
        }
    }
    steps
}

/// What the row's jump button says; an extension session has no terminal.
pub fn focus_label(view: &SessionView) -> &'static str {
    if is_vscode_extension(view.entrypoint.as_deref()) {
        "Show in editor"
    } else {
        "Show terminal"
    }
}

/// Whether some step can plausibly bring the session forward (shows or
/// hides the jump button). A host that hasn't been looked up yet (`None`)
/// counts as focusable meanwhile; a session without a process only when it
/// is the extension's and an editor is running.
pub fn can_focus(
    view: &SessionView,
    host: Option<&HostApp>,
    info: Option<&ConsoleInfo>,
    extras: &FocusExtras<'_>,
) -> bool {
    if view.pid.is_none() {
        return is_vscode_extension(view.entrypoint.as_deref()) && extras.fallback_editor.is_some();
    }
    let Some(host) = host else { return true };
    let unknown = ConsoleInfo::default();
    !focus_plan_with(view, host, info.unwrap_or(&unknown), extras).is_empty()
}

/// Which Windows Terminal tab is the session's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabMatch {
    /// The index of the one tab whose name is the console's title.
    One(usize),
    /// No tab carries the title (renamed, `suppressApplicationTitle`, or the
    /// session sits in a pane that isn't the tab's active one).
    None,
    /// Several do: never guess between them.
    Ambiguous,
}

/// The tab to select for a console titled `title`, among a window's tabs
/// (name, selected) in UI Automation's order.
pub fn wt_tab_match(tabs: &[(String, bool)], title: &str) -> TabMatch {
    let title = title.trim();
    if title.is_empty() {
        return TabMatch::None;
    }
    let mut matches = tabs
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| name.trim() == title)
        .map(|(index, _)| index);
    match (matches.next(), matches.next()) {
        (Some(index), None) => TabMatch::One(index),
        (Some(_), Some(_)) => TabMatch::Ambiguous,
        (None, _) => TabMatch::None,
    }
}

/// Runs a plan's steps in order until one works.
///
/// A later step is a coarser fallback for the one before it (the window
/// instead of the tab, the editor instead of the folder's window), so when
/// it is the one that works the session was only brought near: `RaisedOnly`.
/// Nothing worked: `NotFound`, or the first failure's reason.
pub fn run_plan(
    steps: &[FocusStep],
    run: &mut dyn FnMut(&FocusStep) -> FocusOutcome,
) -> FocusOutcome {
    let mut fell_back = false;
    let mut failure: Option<String> = None;
    for step in steps {
        match run(step) {
            FocusOutcome::Focused if fell_back => return FocusOutcome::RaisedOnly,
            FocusOutcome::Focused => return FocusOutcome::Focused,
            FocusOutcome::RaisedOnly => return FocusOutcome::RaisedOnly,
            FocusOutcome::NotFound => fell_back = true,
            FocusOutcome::Failed(why) => {
                fell_back = true;
                failure.get_or_insert(why);
            }
        }
    }
    match failure {
        Some(why) => FocusOutcome::Failed(why),
        None => FocusOutcome::NotFound,
    }
}

/// Whether a jump brought something forward: that counts as looking at the
/// session (it is marked reviewed, and an unpinned panel closes).
pub fn jumped(outcome: &FocusOutcome) -> bool {
    matches!(outcome, FocusOutcome::Focused | FocusOutcome::RaisedOnly)
}

/// The `focus` call's outcome name and what to tell the user, if anything.
pub fn outcome_reply(outcome: &FocusOutcome, host: &HostApp) -> (&'static str, Option<String>) {
    match outcome {
        FocusOutcome::Focused => ("focused", None),
        FocusOutcome::RaisedOnly => (
            "raised_only",
            Some(match &host.kind {
                HostKind::WindowsTerminal => {
                    "Brought its Windows Terminal window forward".to_owned()
                }
                HostKind::VsCode { product } => format!("Brought {product} forward"),
                _ => "Brought its window forward".to_owned(),
            }),
        ),
        FocusOutcome::NotFound => (
            "not_found",
            Some("Its terminal window wasn't found".to_owned()),
        ),
        FocusOutcome::Failed(why) => ("failed", Some(why.clone())),
    }
}
