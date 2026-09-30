//! The commands the installer writes into settings.json, and how this app's
//! (and the official Codenotch's) commands are recognised again
//! (HookCommands.swift; DESIGN-WIN §4.3). Pure.
//!
//! Claude Code runs a string command through Git Bash or PowerShell,
//! whichever that build uses, so a command we write must parse the same in
//! both. That leaves exactly one spelling: the path of the hook copy
//! unquoted, with forward slashes, followed by its subcommand:
//!
//! ```text
//! C:/Users/me/.claude/hooks/agentnotch-hook.exe hook
//! ```
//!
//! It is only used when every character of the path is one neither shell
//! gives a meaning to. A folder whose path has anything else (a space in the
//! user name, say) is written with its 8.3 short name when the volume has
//! one. There are no quotes and no "does the file exist" guard (neither is
//! the same in both shells): a missing exe ends with 127 in bash and 1 in
//! PowerShell, which Claude Code only logs. Only exit status 2 blocks, and
//! the hook exe never exits 2.
//!
//! The other form is Claude Code's exec form, which runs the exe directly
//! with no shell at all (`{"command": <exe>, "args": ["hook", "--exec"]}`).
//! It may only be written once every Claude Code on this PC is known to
//! understand it ([`exec_form_allowed`]): an older one would not run the
//! hook, and one older still ignores a settings.json it can't read in full.
//!
//! Recognising a command: its last simple command must run a path whose file
//! name is the hook exe's, with `hook` or `statusline` and nothing after it
//! but `--exec`. A third-party hook that merely mentions the name, like
//! `notify.cmd --skip agentnotch-hook.exe`, is not a match.

use super::facts::ClaudeCodeFacts;
use super::shell_words::{self, file_name};
use super::version::ClaudeCodeVersion;
use crate::core::paths::PathStyle;
use crate::core::settings_doc::Json;
use crate::runtime_types::{CommandForm, VersionSighting};
use std::path::{Path, PathBuf};

/// The helper exe, as it is named in every run folder's `hooks\`.
pub const HOOK_EXE_NAME: &str = "agentnotch-hook.exe";
/// The folder inside a config folder that holds the hook copy.
pub const HOOKS_DIR_NAME: &str = "hooks";
/// Exec-form hooks say so: only then is the hook's parent Claude Code itself
/// (§1.4's pid rule).
pub const EXEC_MARKER: &str = "--exec";
/// How long Claude Code waits for a PermissionRequest hook: a day, so a
/// prompt can sit in the panel (HS§3.5).
pub const PERMISSION_TIMEOUT_SECONDS: i64 = 86_400;

/// What the hook exe is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subcommand {
    /// A hook event on stdin.
    Hook,
    /// The status line wrapper.
    StatusLine,
}

impl Subcommand {
    pub fn arg(self) -> &'static str {
        match self {
            Subcommand::Hook => "hook",
            Subcommand::StatusLine => "statusline",
        }
    }

    fn parse(word: &str) -> Option<Subcommand> {
        match word {
            "hook" => Some(Subcommand::Hook),
            "statusline" => Some(Subcommand::StatusLine),
            _ => None,
        }
    }
}

/// `<config folder>\hooks\agentnotch-hook.exe`.
pub fn hook_copy_path(config_dir: &Path) -> PathBuf {
    config_dir.join(HOOKS_DIR_NAME).join(HOOK_EXE_NAME)
}

// ---- Writing ----

/// `path` with forward slashes, as both shells accept a Windows path.
pub fn forward_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

/// Whether a path (forward slashes) can be written into a command with no
/// quotes and mean the same in Git Bash and PowerShell: every character is a
/// letter or digit (of any script) or one of `_ . - / :`, or a `~` anywhere
/// but first (mid-word it is literal in both shells, and 8.3 names have one).
pub fn carries_unquoted(path: &str) -> bool {
    !path.is_empty()
        && path.chars().enumerate().all(|(index, c)| {
            c.is_alphanumeric()
                || matches!(c, '_' | '.' | '-' | '/' | ':')
                || (c == '~' && index > 0)
        })
}

/// The string command running `exe` with `subcommand`, when `exe` can be
/// written unquoted.
pub fn string_command(exe: &str, subcommand: Subcommand) -> Option<String> {
    let path = forward_slashes(exe);
    carries_unquoted(&path).then(|| format!("{path} {}", subcommand.arg()))
}

/// The string command for the hook copy at `exe`: its own path when that can
/// be written unquoted, else with its folder in 8.3 names when the volume
/// has them (`short_path`, asked about the config folder, which exists before
/// anything is installed; `hooks` and the exe's name need no short form).
/// `None` when neither works: no string command can carry this path.
pub fn string_command_for(
    exe: &Path,
    subcommand: Subcommand,
    short_path: &PathLookup<'_>,
) -> Option<String> {
    if let Some(command) = string_command(&exe.to_string_lossy(), subcommand) {
        return Some(command);
    }
    let hooks_dir = exe.parent()?;
    let config_dir = hooks_dir.parent()?;
    let short = short_path(config_dir)?;
    let shortened = format!(
        "{}/{}/{}",
        forward_slashes(&short.to_string_lossy()).trim_end_matches('/'),
        hooks_dir.file_name()?.to_string_lossy(),
        exe.file_name()?.to_string_lossy()
    );
    string_command(&shortened, subcommand)
}

/// The exec form of a hook entry for the copy at `exe`.
pub fn exec_form(exe: &Path) -> CommandForm {
    CommandForm::Exec {
        command: exe.to_path_buf(),
        args: vec![Subcommand::Hook.arg().to_owned(), EXEC_MARKER.to_owned()],
    }
}

/// The command as Settings and the doctor show it.
pub fn display_command(form: &CommandForm) -> Option<String> {
    match form {
        CommandForm::Exec { command, args } => {
            let mut text = command.to_string_lossy().into_owned();
            for arg in args {
                text.push(' ');
                text.push_str(arg);
            }
            Some(text)
        }
        CommandForm::Text(command) => Some(command.clone()),
        CommandForm::NotPossible(_) => None,
    }
}

/// `exec` | `string`, as the install record and the doctor name a form.
pub fn form_name(form: &CommandForm) -> Option<&'static str> {
    match form {
        CommandForm::Exec { .. } => Some("exec"),
        CommandForm::Text(_) => Some("string"),
        CommandForm::NotPossible(_) => None,
    }
}

// ---- Versions ----

/// The version hooks are written for: the lowest Claude Code seen anywhere
/// (a binary, a bundled copy, a live session, a status line), so an old
/// client never meets an event name it doesn't know. `None` (the baseline
/// events) when no version is known.
pub fn effective_version(versions: &[VersionSighting]) -> Option<ClaudeCodeVersion> {
    versions
        .iter()
        .filter_map(|sighting| sighting.version.as_deref())
        .filter_map(ClaudeCodeVersion::parse)
        .min()
}

/// Whether exec-form hooks may be written: the facts file establishes
/// `EXEC_FORM_MIN`, at least one Claude Code was seen, and every one seen
/// is known and at least that version. A copy whose version can't be told (a
/// bundled folder with an odd name, a binary that didn't answer) keeps
/// everyone on string form: before 2.1.101 an unknown key made Claude Code
/// ignore the whole settings file, deny rules included.
pub fn exec_form_allowed(versions: &[VersionSighting], facts: &ClaudeCodeFacts) -> bool {
    let Some(minimum) = facts.exec_form_min() else {
        return false;
    };
    !versions.is_empty()
        && versions.iter().all(|sighting| {
            sighting
                .version
                .as_deref()
                .and_then(ClaudeCodeVersion::parse)
                .is_some_and(|version| version >= minimum)
        })
}

/// Why a folder can't be hooked, for Settings.
pub fn not_hookable_reason(facts: &ClaudeCodeFacts) -> String {
    match facts.exec_form_min() {
        Some(minimum) => format!(
            "Can't be hooked here: its path needs Claude Code {minimum} or later everywhere on this PC"
        ),
        None => "Can't be hooked here: its path has characters a hook command can't carry".to_owned(),
    }
}

/// What Settings says when no status line command can carry a folder's path.
pub const STATUS_LINE_NOT_AVAILABLE: &str = "Live status line isn't available for this folder";

// ---- Recognising ----

/// One of this app's commands found in a settings.json.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognised {
    pub subcommand: Subcommand,
    /// The exe as the command names it.
    pub exe: String,
    /// Written in exec form (`command` + `args`).
    pub exec_form: bool,
    /// Carries `--exec`.
    pub exec_marker: bool,
}

/// A path-to-path lookup: `GetShortPathNameW` or `GetLongPathNameW`, `None`
/// when the file system has no such name.
pub type PathLookup<'a> = dyn Fn(&Path) -> Option<PathBuf> + 'a;

/// Reads commands back. `long_path` spells out an 8.3 path
/// (`GetLongPathNameW`), so a command naming the exe by its short name is
/// still known; without it only the exe's own name is.
#[derive(Clone, Copy)]
pub struct Recogniser<'a> {
    style: PathStyle,
    long_path: Option<&'a PathLookup<'a>>,
}

impl<'a> Recogniser<'a> {
    pub fn new(style: PathStyle) -> Recogniser<'static> {
        Recogniser {
            style,
            long_path: None,
        }
    }

    pub fn with_long_paths(style: PathStyle, long_path: &'a PathLookup) -> Recogniser<'a> {
        Recogniser {
            style,
            long_path: Some(long_path),
        }
    }

    /// The path a command word names, as the file system takes it: on
    /// Windows `/c/Users/me` (Git Bash's spelling) is `C:\Users\me`, and
    /// forward slashes are backslashes.
    pub fn native_path(&self, word: &str) -> PathBuf {
        match self.style {
            PathStyle::Posix => PathBuf::from(word),
            PathStyle::Windows => {
                let bytes = word.as_bytes();
                let drive = bytes.len() >= 2
                    && bytes[0] == b'/'
                    && bytes[1].is_ascii_alphabetic()
                    && (bytes.len() == 2 || bytes[2] == b'/');
                let spelled = if drive {
                    format!("{}:{}", (bytes[1] as char).to_ascii_uppercase(), &word[2..])
                } else {
                    word.to_owned()
                };
                PathBuf::from(spelled.replace('/', "\\"))
            }
        }
    }

    /// Whether a command word is a path to the hook exe: by its own name
    /// (any case), or by an 8.3 name that spells out to it.
    fn names_our_exe(&self, word: &str) -> bool {
        let name = file_name(word);
        if name.eq_ignore_ascii_case(HOOK_EXE_NAME) {
            return true;
        }
        if !name.contains('~') {
            return false;
        }
        self.long_path
            .and_then(|long_path| long_path(&self.native_path(word)))
            .is_some_and(|long| {
                file_name(&long.to_string_lossy()).eq_ignore_ascii_case(HOOK_EXE_NAME)
            })
    }

    /// A string command that runs the hook exe: the last simple command's
    /// program is the exe, followed by `hook` or `statusline` and nothing
    /// else but `--exec`.
    pub fn command(&self, command: &str) -> Option<Recognised> {
        let words = shell_words::last_simple_command(command)?;
        let start = shell_words::program_index(&words)?;
        let (program, rest) = words[start..].split_first()?;
        let (subcommand, exec_marker) = subcommand_of(rest)?;
        self.names_our_exe(program).then(|| Recognised {
            subcommand,
            exe: program.clone(),
            exec_form: false,
            exec_marker,
        })
    }

    /// A hook entry or a `statusLine` object that runs the hook exe, in
    /// either form. An entry with `args` is exec form, whose `command` is the
    /// exe alone.
    pub fn entry(&self, entry: &Json) -> Option<Recognised> {
        let command = entry.get("command")?.as_str()?;
        let Some(args) = entry.get("args") else {
            return self.command(command);
        };
        let args: Vec<String> = args
            .items()?
            .iter()
            .map(|arg| arg.as_str().map(str::to_owned))
            .collect::<Option<_>>()?;
        let (subcommand, exec_marker) = subcommand_of(&args)?;
        self.names_our_exe(command).then(|| Recognised {
            subcommand,
            exe: command.to_owned(),
            exec_form: true,
            exec_marker,
        })
    }

    /// A hook entry of ours.
    pub fn is_our_hook(&self, entry: &Json) -> bool {
        self.entry(entry)
            .is_some_and(|found| found.subcommand == Subcommand::Hook)
    }

    /// A `statusLine` that runs our wrapper.
    pub fn is_our_status_line(&self, status_line: Option<&Json>) -> bool {
        status_line
            .and_then(|value| self.entry(value))
            .is_some_and(|found| found.subcommand == Subcommand::StatusLine)
    }

    /// Where the exe a recognised command runs is, for the file system.
    pub fn exe_path(&self, recognised: &Recognised) -> PathBuf {
        self.native_path(&recognised.exe)
    }

    /// Whether a command is one the status line wrapper must never chain to,
    /// because it would run the wrapper again: it names the hook exe (in any
    /// spelling) together with `statusline`, or the Mac app's wrapper script
    /// under this name or a former one (a settings.json synced from a Mac).
    /// The wrapper applies the same rule when it runs.
    pub fn trips_loop_guard(&self, command: &str) -> bool {
        let lower = command.to_lowercase();
        if WRAPPER_SCRIPT_NAMES.iter().any(|name| lower.contains(name)) {
            return true;
        }
        if !lower.contains(Subcommand::StatusLine.arg()) {
            return false;
        }
        lower.contains(HOOK_EXE_STEM)
            || shell_words::words(command)
                .iter()
                .any(|word| self.names_our_exe(word))
    }
}

/// `hook` or `statusline`, then nothing or `--exec`.
fn subcommand_of(args: &[String]) -> Option<(Subcommand, bool)> {
    let (first, rest) = args.split_first()?;
    let subcommand = Subcommand::parse(first)?;
    match rest {
        [] => Some((subcommand, false)),
        [marker] if marker == EXEC_MARKER => Some((subcommand, true)),
        _ => None,
    }
}

/// The hook exe's name without its extension, lowercase.
const HOOK_EXE_STEM: &str = "agentnotch-hook";

/// The Mac app's status line wrapper scripts: this name's, the former
/// name's, and the app it was ported from.
const WRAPPER_SCRIPT_NAMES: [&str; 3] = [
    "agentnotch-statusline",
    "superpowered-codenotch-statusline",
    "superpowered-notch-statusline",
];

// ---- The official app's hooks ----

/// The hook exes of upstream's Windows app, under every name it has used.
const UPSTREAM_HOOK_STEMS: [&str; 3] = ["codenotch-hook", "eatbean-hook", "pacman-hook"];

fn is_upstream_hook_exe(word: &str) -> bool {
    let name = file_name(word).to_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    UPSTREAM_HOOK_STEMS.contains(&stem)
}

/// Whether a hook entry runs the official app's hook exe. Its entries are
/// fire and forget and never answer a prompt, so they can stay beside ours;
/// they are only reported, and removed when the user asks. As with ours, a
/// command that merely mentions the name is not a match.
pub fn is_upstream_hook(entry: &Json) -> bool {
    let Some(command) = entry.get("command").and_then(Json::as_str) else {
        return false;
    };
    if entry.get("args").is_some() {
        return is_upstream_hook_exe(command);
    }
    shell_words::last_simple_command(command)
        .and_then(|words| {
            let start = shell_words::program_index(&words)?;
            Some(is_upstream_hook_exe(&words[start]))
        })
        .unwrap_or(false)
}

// ---- Taking over a status line ----

/// What "wrap the status line" comes to for the `statusLine` that is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Takeover {
    /// There is none: ours is installed and chains nothing.
    Install,
    /// Ours already: only its command is kept current.
    Update,
    /// Someone else's, safe to run again through Git Bash: wrapped.
    Wrap,
    /// Left as it is; the reason is shown in Settings.
    LeaveAlone(String),
}

/// Decides whether the wrapper may take over a status line (§4.3).
///
/// The wrapper runs the previous command itself, through Git Bash. How
/// Claude Code itself would have run it (which shell, after which rewrite)
/// is not established for every build, so someone else's status line is
/// wrapped only when it must behave the same either way: a plain command
/// object with no `shell` of its own, whose text has nothing only PowerShell
/// or cmd understands and no backslash (which bash would eat), on a PC where
/// Git Bash is installed. Anything else is left alone, working as it did.
pub fn takeover(current: Option<&Json>, recogniser: &Recogniser, git_bash: bool) -> Takeover {
    let Some(current) = current else {
        return Takeover::Install;
    };
    if recogniser.is_our_status_line(Some(current)) {
        return Takeover::Update;
    }
    let leave = |why: &str| Takeover::LeaveAlone(format!("Status line left alone: {why}"));
    let is_command = current.get("type").and_then(Json::as_str) == Some("command");
    // With `args` the command is a program run without a shell: wrapping
    // would leave those on our entry (neither ours nor theirs any more), and
    // the wrapper chains a command line, not a program and its arguments.
    let command = match current.get("command").and_then(Json::as_str) {
        Some(command)
            if is_command && !command.trim().is_empty() && current.get("args").is_none() =>
        {
            command
        }
        _ => return leave("it isn't a plain command"),
    };
    if let Some(shell) = current.get("shell") {
        let shell = shell.as_str().unwrap_or_default().to_lowercase();
        return if shell.contains("powershell") || shell.contains("pwsh") {
            leave("its command needs PowerShell")
        } else {
            leave("it names its own shell")
        };
    }
    if recogniser.trips_loop_guard(command) {
        return leave("it runs this app's own wrapper");
    }
    let lower = command.to_lowercase();
    if [".ps1", "$env:", "powershell", "pwsh"]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return leave("its command needs PowerShell");
    }
    if lower.contains("cmd.exe") || lower.contains("cmd /c") {
        return leave("its command needs cmd.exe");
    }
    if command.contains('\\') {
        return leave("its command uses Windows paths");
    }
    if !git_bash {
        return leave("Git Bash isn't installed");
    }
    Takeover::Wrap
}

/// Where Git Bash may be, best first: Claude Code's own order (its
/// `CLAUDE_CODE_GIT_BASH_PATH`, then Git for Windows under Program Files,
/// 64-bit then 32-bit).
pub fn git_bash_candidates(get_env: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let value = |name: &str| get_env(name).filter(|value| !value.trim().is_empty());
    let mut candidates = Vec::new();
    if let Some(chosen) = value("CLAUDE_CODE_GIT_BASH_PATH") {
        candidates.push(PathBuf::from(chosen));
    }
    for (variable, default) in [
        ("ProgramFiles", r"C:\Program Files"),
        ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
    ] {
        let root = value(variable).unwrap_or_else(|| default.to_owned());
        let bash = PathBuf::from(format!(r"{}\Git\bin\bash.exe", root.trim_end_matches('\\')));
        if !candidates.contains(&bash) {
            candidates.push(bash);
        }
    }
    candidates
}
