//! Which process is Claude Code, when `CLAUDE_PID` is absent (Claude Code
//! before 2.1.214). Windows reuses pids quickly and a process's parent pid
//! may name a process that ended long ago, so every parent link is kept only
//! when the parent was created no later than its child.

use serde::{Deserialize, Serialize};

/// One process in the chain: its pid, its image (a file name or a full
/// path) and its creation time (FILETIME, 100 ns since 1601).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcLink {
    pub pid: u32,
    pub image: String,
    pub created: u64,
}

/// Shells a string-form hook command runs under (Git Bash or PowerShell;
/// cmd for completeness), which exit right after the hook.
const SHELLS: [&str; 6] = [
    "bash.exe",
    "sh.exe",
    "dash.exe",
    "cmd.exe",
    "pwsh.exe",
    "powershell.exe",
];
/// What Claude Code runs as: the native build, or Node or Bun running the npm
/// package.
const CLAUDE_IMAGES: [&str; 3] = ["claude.exe", "node.exe", "bun.exe"];
/// At most this many parent links are followed.
const MAX_HOPS: usize = 4;

/// DESIGN-WIN §1.4's walk. `me` is the hook exe, `ancestors[0]` its parent,
/// `ancestors[1]` the grandparent, and so on.
///
/// - Exec form (`--exec`): Claude Code spawned the exe itself, so the parent
///   is Claude Code.
/// - String form: the parent is `bash.exe` or `powershell.exe`, so shells
///   are skipped (at most four links) and the first other ancestor counts
///   only when its image is `claude.exe`, `node.exe` or `bun.exe`; anything
///   else (VS Code, a terminal) means the pid is unknown, and the engine keys
///   the session by its id instead.
pub fn pid_guess(exec_form: bool, me: &ProcLink, ancestors: &[ProcLink]) -> Option<u32> {
    let mut child = me;
    for ancestor in ancestors.iter().take(MAX_HOPS) {
        if ancestor.created > child.created || ancestor.pid == 0 {
            return None;
        }
        if exec_form {
            return valid(ancestor.pid);
        }
        let image = file_name(&ancestor.image);
        if SHELLS.iter().any(|shell| image.eq_ignore_ascii_case(shell)) {
            child = ancestor;
            continue;
        }
        return CLAUDE_IMAGES
            .iter()
            .any(|claude| image.eq_ignore_ascii_case(claude))
            .then_some(ancestor.pid)
            .and_then(valid);
    }
    None
}

fn valid(pid: u32) -> Option<u32> {
    (1..=i32::MAX as u32).contains(&pid).then_some(pid)
}

fn file_name(image: &str) -> &str {
    image.rsplit(['\\', '/']).next().unwrap_or(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(pid: u32, image: &str, created: u64) -> ProcLink {
        ProcLink {
            pid,
            image: image.into(),
            created,
        }
    }

    fn hook() -> ProcLink {
        link(900, "agentnotch-hook.exe", 1000)
    }

    #[test]
    fn exec_form_takes_the_parent() {
        assert_eq!(
            pid_guess(true, &hook(), &[link(40, "claude.exe", 10)]),
            Some(40)
        );
        // Whatever it is: in exec form Claude Code spawned the exe itself.
        assert_eq!(
            pid_guess(true, &hook(), &[link(41, "C:\\x\\node.exe", 10)]),
            Some(41)
        );
    }

    #[test]
    fn bash_then_claude() {
        let chain = [
            link(50, "C:\\Program Files\\Git\\usr\\bin\\bash.exe", 900),
            link(40, "claude.exe", 10),
        ];
        assert_eq!(pid_guess(false, &hook(), &chain), Some(40));
    }

    #[test]
    fn powershell_then_node() {
        let chain = [link(51, "powershell.exe", 900), link(41, "NODE.EXE", 10)];
        assert_eq!(pid_guess(false, &hook(), &chain), Some(41));
    }

    #[test]
    fn cmd_then_bash_then_claude() {
        let chain = [
            link(52, "cmd.exe", 950),
            link(50, "bash.exe", 900),
            link(40, "claude.exe", 10),
        ];
        assert_eq!(pid_guess(false, &hook(), &chain), Some(40));
    }

    #[test]
    fn bun_counts() {
        assert_eq!(
            pid_guess(
                false,
                &hook(),
                &[link(50, "sh.exe", 900), link(42, "bun.exe", 10)]
            ),
            Some(42)
        );
    }

    #[test]
    fn a_parent_younger_than_its_child_is_not_a_link() {
        // The shell's recorded parent pid was reused by a newer process.
        let chain = [link(50, "bash.exe", 900), link(40, "claude.exe", 950)];
        assert_eq!(pid_guess(false, &hook(), &chain), None);
        assert_eq!(
            pid_guess(true, &hook(), &[link(40, "claude.exe", 1001)]),
            None
        );
    }

    #[test]
    fn vs_code_as_first_non_shell_is_none() {
        let chain = [
            link(50, "pwsh.exe", 900),
            link(30, "Code.exe", 10),
            link(40, "claude.exe", 5),
        ];
        assert_eq!(pid_guess(false, &hook(), &chain), None);
    }

    #[test]
    fn no_parent_known() {
        assert_eq!(pid_guess(false, &hook(), &[]), None);
        assert_eq!(pid_guess(true, &hook(), &[]), None);
    }

    #[test]
    fn at_most_four_hops() {
        let chain = [
            link(54, "bash.exe", 990),
            link(53, "bash.exe", 980),
            link(52, "bash.exe", 970),
            link(51, "bash.exe", 960),
            link(40, "claude.exe", 10),
        ];
        assert_eq!(pid_guess(false, &hook(), &chain), None);
        assert_eq!(
            pid_guess(false, &hook(), &[&chain[1..4], &chain[4..]].concat()),
            Some(40)
        );
    }
}
