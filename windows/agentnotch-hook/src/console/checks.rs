//! The decisions of the console helper, as plain data in and a verdict out (DESIGN-WIN §4.8).
//!
//! Typing into a console is only safe because of these checks, and the Win32 half of the helper
//! can't run anywhere but on Windows; so everything that decides is here, free of the OS, and is
//! tested on every system: what may share the console, which key records a text becomes, what the
//! second stdin line means, and the order of the two phases (`type_flow`, driven through the
//! [`Console`] trait by the real console on Windows and by a fake in the tests).

// Only the Windows build types; elsewhere this module is reached by its tests alone.
#![cfg_attr(not(windows), allow(dead_code))]

use agentnotch_proto::{TypeArgs, TypePhase, TypeRequest, TYPE_SUBMIT};

/// `AttachConsole` failed: a VS Code extension session, a mintty window, a process that is gone.
pub const NO_CONSOLE: &str = "This session has no console to type into";
/// A Claude started as administrator can't be reached by this user's helper.
pub const ELEVATED: &str = "Claude runs as administrator";
/// The pid names another process by now (Windows reuses pids quickly).
pub const PROCESS_ENDED: &str = "The session's process has ended";
/// The console is not the one the session was adopted with.
pub const OTHER_CONSOLE: &str = "The session's terminal can't be confirmed";
pub const OTHER_PROGRAM: &str = "Another program is reading this console";
pub const NOT_AT_PROMPT: &str = "The terminal isn't at Claude Code's prompt";
/// No `submit` came: the engine saw a prompt appear, or stopped answering.
pub const TYPED_NOT_SENT: &str =
    "Claude asked for something while your reply was typed; it's in the terminal, not sent.";
pub const NO_REQUEST: &str = "No reply was given to type";
pub const NOT_ONE_LINE: &str = "A reply is typed as one line";
pub const NOT_TYPED: &str = "The console didn't take the reply";
pub const PARTLY_TYPED: &str =
    "Only part of your reply could be typed; it's in the terminal, not sent.";
pub const RETURN_NOT_SENT: &str =
    "Return couldn't be pressed; your reply is in the terminal, not sent.";

/// `VK_RETURN` and the scan code of the main Return key.
pub const VK_RETURN: u16 = 0x0D;
const SCAN_RETURN: u16 = 0x1C;

/// A parent chain is followed this far; no real one is deeper.
const MAX_HOPS: usize = 32;

/// What the helper read about the console it is attached to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facts {
    /// When the target pid's process was created (Unix ms); `None` when it can't be opened.
    pub started_ms: Option<u64>,
    /// `GetConsoleWindow()`; `None` for a console without one.
    pub window: Option<u64>,
    /// `GetConsoleProcessList()`, the helper itself left out.
    pub attached: Vec<u32>,
    /// Those of `attached` that descend from the target ([`descendants`]).
    pub descendants: Vec<u32>,
    /// `ENABLE_LINE_INPUT` of the input buffer; `None` when the mode can't be read.
    pub line_input: Option<bool>,
}

/// May the helper type into this console? Run before the text and again before Return.
///
/// Everything unknown refuses: a start time that can't be read, a missing window when one is
/// expected, an input mode that can't be read.
pub fn console_checks(facts: &Facts, args: &TypeArgs) -> Result<(), &'static str> {
    if facts.started_ms != Some(args.started_ms) {
        return Err(PROCESS_ENDED);
    }
    if args.expect_window.is_some() && facts.window != args.expect_window {
        return Err(OTHER_CONSOLE);
    }
    // Attached to the pid's console, the pid is on its list; an empty or unreadable list is not
    // a console this helper knows anything about.
    if !facts.attached.contains(&args.pid) {
        return Err(NO_CONSOLE);
    }
    let known = |pid: &u32| {
        *pid == args.pid || facts.descendants.contains(pid) || args.allowed_shells.contains(pid)
    };
    if !facts.attached.iter().all(known) {
        return Err(OTHER_PROGRAM);
    }
    // Claude Code reads in raw mode; a shell prompt (cmd, PowerShell without PSReadLine) reads
    // lines, and a line typed there would be run as a command.
    if facts.line_input != Some(false) {
        return Err(NOT_AT_PROMPT);
    }
    Ok(())
}

/// One process of the Toolhelp snapshot, with its creation time (FILETIME units) when readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    pub created: Option<u64>,
}

/// Those of `candidates` that descend from `root`.
///
/// A parent pid in the snapshot may name a process that replaced the real parent, so a link only
/// counts when the parent was created no later than the child; a creation time that can't be
/// read breaks the chain.
pub fn descendants(
    root: u32,
    root_created: Option<u64>,
    candidates: &[u32],
    procs: &[Proc],
) -> Vec<u32> {
    let find = |pid: u32| procs.iter().find(|p| p.pid == pid);
    let descends = |candidate: u32| {
        let mut current = candidate;
        for _ in 0..MAX_HOPS {
            let Some(process) = find(current) else {
                return false;
            };
            let parent_created = if process.parent == root {
                root_created
            } else {
                find(process.parent).and_then(|p| p.created)
            };
            match (parent_created, process.created) {
                (Some(parent), Some(child)) if parent <= child => {}
                _ => return false,
            }
            if process.parent == root {
                return true;
            }
            current = process.parent;
        }
        false
    };
    candidates
        .iter()
        .copied()
        .filter(|candidate| *candidate != root && descends(*candidate))
        .collect()
}

/// One `KEY_EVENT` input record, without the Win32 types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRecord {
    pub key_down: bool,
    pub repeat: u16,
    pub virtual_key: u16,
    pub scan_code: u16,
    /// `uChar.UnicodeChar`: one UTF-16 unit.
    pub unit: u16,
}

fn pair(virtual_key: u16, scan_code: u16, unit: u16) -> [KeyRecord; 2] {
    [true, false].map(|key_down| KeyRecord {
        key_down,
        repeat: 1,
        virtual_key,
        scan_code,
        unit,
    })
}

/// A key-down/key-up pair per UTF-16 unit of `text`. A character outside the basic plane is two
/// units and so two pairs, the high surrogate first, as the console delivers one that was typed.
/// No virtual key: the character is what a raw-mode reader takes, and no layout has all of them.
pub fn key_records(text: &str) -> Vec<KeyRecord> {
    text.encode_utf16()
        .flat_map(|unit| pair(0, 0, unit))
        .collect()
}

/// Return: down and up, with `'\r'`.
pub fn return_records() -> [KeyRecord; 2] {
    pair(VK_RETURN, SCAN_RETURN, u16::from(b'\r'))
}

/// Stdin line 1 → the text to type.
///
/// The engine already made the text one line; a line break that got here anyway would submit
/// the reply before the engine's re-check, so it is refused rather than dropped. An empty text
/// is refused too: all it would do is press Return.
pub fn parse_request(line: &[u8]) -> Result<String, &'static str> {
    let request: TypeRequest = serde_json::from_slice(line).map_err(|_| NO_REQUEST)?;
    if request.text.is_empty() {
        return Err(NO_REQUEST);
    }
    if request.text.contains(['\r', '\n']) {
        return Err(NOT_ONE_LINE);
    }
    Ok(request.text)
}

/// What came on stdin after `{"phase":"typed"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Second {
    Submit,
    /// `abort`, or anything that is not exactly `submit`.
    Abort,
    /// Stdin ended: the app is gone.
    Eof,
    /// Nothing within `TYPE_SUBMIT_WAIT_MS`.
    Timeout,
}

/// Stdin line 2. Only the exact word presses Return.
pub fn second_line(line: &[u8]) -> Second {
    match std::str::from_utf8(line).map(str::trim) {
        Ok(TYPE_SUBMIT) => Second::Submit,
        _ => Second::Abort,
    }
}

fn outcome(outcome: &str, reason: Option<&str>) -> TypePhase {
    TypePhase::Outcome {
        outcome: outcome.to_owned(),
        reason: reason.map(str::to_owned),
    }
}

pub fn delivered() -> TypePhase {
    outcome(TypePhase::DELIVERED, None)
}

pub fn refused(reason: &str) -> TypePhase {
    outcome(TypePhase::REFUSED, Some(reason))
}

pub fn typed_not_submitted(reason: &str) -> TypePhase {
    outcome(TypePhase::TYPED_NOT_SUBMITTED, Some(reason))
}

pub fn failed(reason: &str) -> TypePhase {
    outcome(TypePhase::FAILED, Some(reason))
}

/// The console the helper is attached to.
pub trait Console {
    /// Read afresh on every call.
    fn facts(&mut self) -> Facts;
    /// Puts the records into the input buffer; how many it took.
    fn write(&mut self, records: &[KeyRecord]) -> usize;
}

/// The two phases, in order: check, type, let the text settle, announce it and wait for the
/// engine's word, check again, press Return. Returns the final line.
///
/// `typed` prints `{"phase":"typed"}` and waits for stdin line 2; it runs only when the whole
/// text is in the input buffer.
pub fn type_flow(
    console: &mut impl Console,
    args: &TypeArgs,
    text: &str,
    settle: impl FnOnce(),
    typed: impl FnOnce() -> Second,
) -> TypePhase {
    if let Err(reason) = console_checks(&console.facts(), args) {
        return refused(reason);
    }
    let records = key_records(text);
    let written = console.write(&records);
    if written == 0 {
        return failed(NOT_TYPED);
    }
    if written < records.len() {
        return typed_not_submitted(PARTLY_TYPED);
    }
    // A burst of records followed at once by Return reads as a paste.
    settle();
    if typed() != Second::Submit {
        return typed_not_submitted(TYPED_NOT_SENT);
    }
    // The console may have changed hands while the engine looked at the session.
    if let Err(reason) = console_checks(&console.facts(), args) {
        return typed_not_submitted(reason);
    }
    // The key-down alone submits, so only "nothing went in" is "not sent".
    if console.write(&return_records()) == 0 {
        return typed_not_submitted(RETURN_NOT_SENT);
    }
    delivered()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: u32 = 4242;
    const SHELL: u32 = 900;
    const STARTED: u64 = 1_790_000_000_123;
    const WINDOW: u64 = 0x1a2b3c;

    fn args() -> TypeArgs {
        TypeArgs {
            pid: CLAUDE,
            started_ms: STARTED,
            expect_window: Some(WINDOW),
            allowed_shells: vec![SHELL],
        }
    }

    fn good() -> Facts {
        Facts {
            started_ms: Some(STARTED),
            window: Some(WINDOW),
            attached: vec![CLAUDE],
            descendants: vec![],
            line_input: Some(false),
        }
    }

    #[test]
    fn a_console_that_holds_only_claude_in_raw_mode_passes() {
        assert_eq!(console_checks(&good(), &args()), Ok(()));
    }

    #[test]
    fn another_start_time_is_another_process() {
        for started_ms in [Some(STARTED + 1), Some(STARTED - 1), Some(0), None] {
            let facts = Facts {
                started_ms,
                ..good()
            };
            assert_eq!(console_checks(&facts, &args()), Err(PROCESS_ENDED));
        }
    }

    #[test]
    fn another_window_is_refused() {
        for window in [Some(WINDOW + 1), None] {
            let facts = Facts { window, ..good() };
            assert_eq!(console_checks(&facts, &args()), Err(OTHER_CONSOLE));
        }
    }

    #[test]
    fn without_an_expected_window_any_window_passes() {
        let args = TypeArgs {
            expect_window: None,
            ..args()
        };
        for window in [Some(WINDOW), Some(7), None] {
            let facts = Facts { window, ..good() };
            assert_eq!(console_checks(&facts, &args), Ok(()));
        }
    }

    #[test]
    fn a_foreign_process_on_the_console_is_refused() {
        let facts = Facts {
            attached: vec![CLAUDE, 5555],
            ..good()
        };
        assert_eq!(console_checks(&facts, &args()), Err(OTHER_PROGRAM));
    }

    #[test]
    fn an_ancestor_that_is_not_an_allowed_shell_is_refused() {
        // Claude's parent is attached (a launcher, an editor's helper) but the engine did not
        // name it among the shells: nothing here knows whether it reads the console.
        let launcher = 800;
        let facts = Facts {
            attached: vec![launcher, SHELL, CLAUDE],
            ..good()
        };
        assert_eq!(console_checks(&facts, &args()), Err(OTHER_PROGRAM));
        let named = TypeArgs {
            allowed_shells: vec![SHELL, launcher],
            ..args()
        };
        assert_eq!(console_checks(&facts, &named), Ok(()));
    }

    #[test]
    fn an_allowed_shell_may_share_the_console() {
        let facts = Facts {
            attached: vec![SHELL, CLAUDE],
            ..good()
        };
        assert_eq!(console_checks(&facts, &args()), Ok(()));
    }

    #[test]
    fn a_descendant_may_share_the_console() {
        let facts = Facts {
            attached: vec![CLAUDE, 6001, 6002],
            descendants: vec![6001, 6002],
            ..good()
        };
        assert_eq!(console_checks(&facts, &args()), Ok(()));
        let one_unknown = Facts {
            descendants: vec![6001],
            ..facts
        };
        assert_eq!(console_checks(&one_unknown, &args()), Err(OTHER_PROGRAM));
    }

    #[test]
    fn line_input_means_a_shell_prompt() {
        for line_input in [Some(true), None] {
            let facts = Facts {
                line_input,
                ..good()
            };
            assert_eq!(console_checks(&facts, &args()), Err(NOT_AT_PROMPT));
        }
    }

    #[test]
    fn a_console_without_the_pid_is_not_the_sessions() {
        for attached in [vec![], vec![SHELL]] {
            let facts = Facts { attached, ..good() };
            assert_eq!(console_checks(&facts, &args()), Err(NO_CONSOLE));
        }
    }

    #[test]
    fn the_checks_come_in_order() {
        // The process first: once the pid is someone else's, nothing else about it means anything.
        let facts = Facts {
            started_ms: Some(1),
            window: None,
            attached: vec![77],
            descendants: vec![],
            line_input: Some(true),
        };
        assert_eq!(console_checks(&facts, &args()), Err(PROCESS_ENDED));
    }

    fn proc(pid: u32, parent: u32, created: u64) -> Proc {
        Proc {
            pid,
            parent,
            created: Some(created),
        }
    }

    #[test]
    fn descendants_follow_links_that_the_creation_times_allow() {
        let procs = [
            proc(10, CLAUDE, 200),
            proc(11, 10, 300),
            proc(12, 11, 300),
            proc(20, 1, 250),
        ];
        assert_eq!(
            descendants(CLAUDE, Some(100), &[10, 11, 12, 20, CLAUDE, 99], &procs),
            vec![10, 11, 12]
        );
    }

    #[test]
    fn a_parent_created_after_its_child_took_over_the_pid() {
        // 10 was created before the process that now has Claude's pid: its real parent is gone.
        let procs = [proc(10, CLAUDE, 50), proc(11, 10, 300)];
        assert_eq!(
            descendants(CLAUDE, Some(100), &[10, 11], &procs),
            Vec::<u32>::new()
        );
        // The same one link further up.
        let procs = [proc(10, CLAUDE, 400), proc(11, 10, 300)];
        assert_eq!(descendants(CLAUDE, Some(100), &[10, 11], &procs), vec![10]);
    }

    #[test]
    fn an_unreadable_creation_time_breaks_the_chain() {
        let unreadable = Proc {
            pid: 10,
            parent: CLAUDE,
            created: None,
        };
        assert!(descendants(
            CLAUDE,
            Some(100),
            &[10, 11],
            &[unreadable, proc(11, 10, 300)]
        )
        .is_empty());
        assert!(descendants(CLAUDE, None, &[10], &[proc(10, CLAUDE, 200)]).is_empty());
    }

    #[test]
    fn a_loop_of_parents_ends() {
        let procs = [proc(10, 11, 200), proc(11, 10, 200)];
        assert!(descendants(CLAUDE, Some(100), &[10, 11], &procs).is_empty());
    }

    fn units(records: &[KeyRecord]) -> Vec<u16> {
        records
            .iter()
            .filter(|r| r.key_down)
            .map(|r| r.unit)
            .collect()
    }

    #[track_caller]
    fn assert_pairs(records: &[KeyRecord]) {
        assert_eq!(records.len() % 2, 0);
        for pair in records.chunks(2) {
            assert!(pair[0].key_down && !pair[1].key_down);
            assert_eq!(pair[0].unit, pair[1].unit);
            for record in pair {
                assert_eq!(record.repeat, 1);
            }
        }
    }

    #[test]
    fn ascii_is_one_pair_per_character() {
        let records = key_records("hi!");
        assert_eq!(records.len(), 6);
        assert_pairs(&records);
        assert_eq!(units(&records), vec![0x68, 0x69, 0x21]);
        assert!(records
            .iter()
            .all(|r| r.virtual_key == 0 && r.scan_code == 0));
    }

    #[test]
    fn text_outside_ascii_is_typed_by_utf16_unit() {
        let text = "héllo wörld ✓";
        let records = key_records(text);
        assert_pairs(&records);
        assert_eq!(records.len(), 2 * 13);
        assert_eq!(String::from_utf16(&units(&records)).unwrap(), text);
        assert_eq!(units(&records)[12], 0x2713);
    }

    #[test]
    fn a_character_outside_the_basic_plane_is_two_pairs() {
        let records = key_records("😀");
        assert_pairs(&records);
        assert_eq!(units(&records), vec![0xD83D, 0xDE00]);
        assert_eq!(records.len(), 4);
        assert_eq!(
            String::from_utf16(&units(&key_records("a😀b"))).unwrap(),
            "a😀b"
        );
    }

    #[test]
    fn an_empty_text_is_no_record() {
        assert!(key_records("").is_empty());
    }

    #[test]
    fn return_is_the_return_key_with_a_carriage_return() {
        let records = return_records();
        assert_pairs(&records);
        for record in records {
            assert_eq!(record.virtual_key, 0x0D);
            assert_eq!(record.unit, 0x0D);
        }
    }

    #[test]
    fn the_request_is_one_line_of_json() {
        assert_eq!(parse_request(br#"{"text":"hello"}"#), Ok("hello".into()));
        assert_eq!(
            parse_request(b"{\"text\":\"h\xC3\xA9 \"}\r\n"),
            Ok("hé ".into())
        );
        for line in [
            &b""[..],
            b"hello",
            b"{}",
            br#"{"text":7}"#,
            br#"{"text":""}"#,
        ] {
            assert_eq!(parse_request(line), Err(NO_REQUEST));
        }
        for line in [
            &br#"{"text":"a\nb"}"#[..],
            br#"{"text":"a\r"}"#,
            br#"{"text":"\n"}"#,
        ] {
            assert_eq!(parse_request(line), Err(NOT_ONE_LINE));
        }
    }

    #[test]
    fn only_the_exact_word_submits() {
        assert_eq!(second_line(b"submit"), Second::Submit);
        assert_eq!(second_line(b"submit\r"), Second::Submit);
        for line in [
            &b"abort"[..],
            b"",
            b"Submit",
            b"submit now",
            b"\xFF",
            b"{\"submit\":true}",
        ] {
            assert_eq!(second_line(line), Second::Abort);
        }
    }

    #[test]
    fn the_outcome_lines_are_the_protocols() {
        assert_eq!(delivered().to_line(), r#"{"outcome":"delivered"}"#);
        assert_eq!(
            refused(OTHER_PROGRAM).to_line(),
            r#"{"outcome":"refused","reason":"Another program is reading this console"}"#
        );
        assert_eq!(
            typed_not_submitted(TYPED_NOT_SENT).to_line(),
            r#"{"outcome":"typed_not_submitted","reason":"Claude asked for something while your reply was typed; it's in the terminal, not sent."}"#
        );
        assert_eq!(
            failed(NO_REQUEST).to_line(),
            r#"{"outcome":"failed","reason":"No reply was given to type"}"#
        );
        for phase in [
            delivered(),
            refused(NO_CONSOLE),
            refused(ELEVATED),
            refused(NOT_AT_PROMPT),
            typed_not_submitted(PARTLY_TYPED),
            failed(NOT_TYPED),
        ] {
            assert_eq!(TypePhase::from_line(&phase.to_line()), Some(phase));
        }
    }

    /// A console that records what was written and hands out one `Facts` per call.
    struct Fake {
        facts: Vec<Facts>,
        asked: usize,
        written: Vec<KeyRecord>,
        /// How many records the console takes in all.
        room: usize,
    }

    impl Fake {
        fn new(facts: Vec<Facts>) -> Fake {
            Fake {
                facts,
                asked: 0,
                written: Vec::new(),
                room: usize::MAX,
            }
        }

        fn text(&self) -> String {
            String::from_utf16(&units(&self.written)).unwrap()
        }
    }

    impl Console for Fake {
        fn facts(&mut self) -> Facts {
            let facts = self.facts[self.asked.min(self.facts.len() - 1)].clone();
            self.asked += 1;
            facts
        }

        fn write(&mut self, records: &[KeyRecord]) -> usize {
            let taken = records.len().min(self.room - self.written.len());
            self.written.extend_from_slice(&records[..taken]);
            taken
        }
    }

    fn flow(console: &mut Fake, text: &str, second: Second) -> (TypePhase, Vec<&'static str>) {
        let steps = std::cell::RefCell::new(Vec::new());
        let phase = type_flow(
            console,
            &args(),
            text,
            || steps.borrow_mut().push("settle"),
            || {
                steps.borrow_mut().push("typed");
                second
            },
        );
        (phase, steps.into_inner())
    }

    #[test]
    fn submit_presses_return_after_the_text_settled_and_a_second_check() {
        let mut console = Fake::new(vec![good()]);
        let (phase, steps) = flow(&mut console, "héllo", Second::Submit);
        assert_eq!(phase, delivered());
        assert_eq!(steps, vec!["settle", "typed"]);
        assert_eq!(console.asked, 2);
        assert_eq!(console.text(), "héllo\r");
        let tail = &console.written[console.written.len() - 2..];
        assert_eq!(tail, &return_records()[..]);
    }

    #[test]
    fn without_submit_the_text_stays_and_return_is_never_pressed() {
        for second in [Second::Abort, Second::Eof, Second::Timeout] {
            let mut console = Fake::new(vec![good()]);
            let (phase, steps) = flow(&mut console, "hello", second);
            assert_eq!(phase, typed_not_submitted(TYPED_NOT_SENT));
            assert_eq!(steps, vec!["settle", "typed"]);
            assert_eq!(console.text(), "hello");
            assert_eq!(console.asked, 1);
        }
    }

    #[test]
    fn a_refused_console_gets_no_key() {
        let foreign = Facts {
            attached: vec![CLAUDE, 5555],
            ..good()
        };
        let cooked = Facts {
            line_input: Some(true),
            ..good()
        };
        for (facts, reason) in [(foreign, OTHER_PROGRAM), (cooked, NOT_AT_PROMPT)] {
            let mut console = Fake::new(vec![facts]);
            let (phase, steps) = flow(&mut console, "hello", Second::Submit);
            assert_eq!(phase, refused(reason));
            assert!(steps.is_empty());
            assert!(console.written.is_empty());
        }
    }

    #[test]
    fn a_console_that_changed_before_return_keeps_the_text_unsent() {
        let cooked = Facts {
            line_input: Some(true),
            ..good()
        };
        let mut console = Fake::new(vec![good(), cooked]);
        let (phase, _) = flow(&mut console, "hello", Second::Submit);
        assert_eq!(phase, typed_not_submitted(NOT_AT_PROMPT));
        assert_eq!(console.text(), "hello");
    }

    #[test]
    fn a_console_that_takes_nothing_or_only_a_part_is_never_submitted() {
        let mut console = Fake::new(vec![good()]);
        console.room = 0;
        let (phase, steps) = flow(&mut console, "hello", Second::Submit);
        assert_eq!(phase, failed(NOT_TYPED));
        assert!(steps.is_empty());

        let mut console = Fake::new(vec![good()]);
        console.room = 4;
        let (phase, steps) = flow(&mut console, "hello", Second::Submit);
        assert_eq!(phase, typed_not_submitted(PARTLY_TYPED));
        assert!(steps.is_empty());
        assert_eq!(console.text(), "he");

        // The text went in, Return did not.
        let mut console = Fake::new(vec![good()]);
        console.room = 10;
        let (phase, _) = flow(&mut console, "hello", Second::Submit);
        assert_eq!(phase, typed_not_submitted(RETURN_NOT_SENT));
        assert_eq!(console.text(), "hello");
    }
}
