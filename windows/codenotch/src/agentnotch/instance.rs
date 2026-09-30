//! Making sure this process is the only copy of the app (DESIGN-WIN §2.4 `setup.rs`).
//!
//! The single-instance plugin (seam WSI) guards the app with a named mutex and an invisible
//! event window. A second launch that finds the mutex and the window hands its argv to the
//! window and exits inside the plugin. But a second launch that finds the mutex and **not yet**
//! the window (the first copy was still starting) is let through by the plugin and would run as
//! a second app: two notches, two hubs on one pipe, a sign-in callback that reaches neither. So
//! the app's own setup looks again, waits for the window when it must, and does then what the
//! plugin would have done.
//!
//! Nothing here logs argv: a second launch is how a sign-in callback arrives, and it carries the
//! authorisation code.

use std::time::{Duration, Instant};

use agentnotch_win::window::{self, MutexClaim};
use tauri::AppHandle;

use super::IDENTIFIER;

/// `WMCOPYDATA_SINGLE_INSTANCE_DATA` of tauri-plugin-single-instance 2.4.4: the tag its event
/// window answers to.
const COPY_DATA_KIND: usize = 1542;
/// How long a duplicate waits for the first copy's event window.
const WAIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(50);

/// What this process saw of the plugin's guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Seen {
    /// The event window exists and belongs to this process id.
    Window(u32),
    /// No event window; the mutex as this thread finds it.
    NoWindow(MutexClaim),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// This process is the app.
    Continue,
    /// Another copy runs and can be reached: hand it argv, then exit 0.
    Forward,
    /// Another copy holds the mutex and has no window yet: look again shortly.
    Wait,
    /// Another copy holds the mutex and never showed a window: exit 0 without it.
    GiveUp,
}

/// The duplicate check's decision, for one look at the guard.
pub(super) fn decide(seen: Seen, this_process: u32, deadline_passed: bool) -> Step {
    match seen {
        // The first copy: the plugin's setup made the window before the app's setup ran.
        Seen::Window(owner) if owner == this_process => Step::Continue,
        // A duplicate the plugin let through, and the first copy's window is there by now.
        Seen::Window(_) => Step::Forward,
        // The first copy whose window the plugin could not make (its mutex is this thread's:
        // the plugin took it here), a copy whose plugin made no mutex at all, or the heir of a
        // copy that died (nobody held the mutex: it is this thread's from now on). In none of
        // them does another app run, so leaving would leave the user with no app at all.
        Seen::NoWindow(MutexClaim::Ours | MutexClaim::Missing | MutexClaim::Taken) => {
            Step::Continue
        }
        // A duplicate: another process holds the mutex and hasn't made its window yet.
        Seen::NoWindow(MutexClaim::Elsewhere) if deadline_passed => Step::GiveUp,
        Seen::NoWindow(MutexClaim::Elsewhere) => Step::Wait,
    }
}

/// The bytes the plugin's event window reads: the working folder and argv joined with `|`, as
/// a C string. (The plugin splits at every `|`, the folder first; an argument holding one
/// arrives as two, which is why `deeplink::from_args` takes a link only as a single argument.)
pub(super) fn payload(cwd: &str, args: &[String]) -> Vec<u8> {
    format!("{cwd}|{}\0", args.join("|")).into_bytes()
}

/// Returns only in the one copy of the app that should run; a duplicate exits here.
///
/// Must run on the thread that ran the plugin's setup (the main thread: Tauri runs both there),
/// and before anything of the fork is started: `process::exit` runs no destructor.
pub(super) fn ensure_single(app: &AppHandle) {
    // The guard exists only on Windows (the plugin's other platforms exit inside the plugin),
    // and the stub services would answer "no window, no mutex".
    if !cfg!(windows) {
        return;
    }
    let (class, title, mutex) = (
        format!("{IDENTIFIER}-sic"),
        format!("{IDENTIFIER}-siw"),
        format!("{IDENTIFIER}-sim"),
    );
    let this_process = window::current_process_id();
    let started = Instant::now();
    loop {
        let found = window::find_window(&class, &title);
        let seen = match found {
            Some((_, owner)) => Seen::Window(owner),
            // Only the mutex tells "we are first, without a window" from "we are a duplicate".
            None => Seen::NoWindow(window::claim_mutex(&mutex)),
        };
        match decide(seen, this_process, started.elapsed() >= WAIT) {
            Step::Continue => {
                if found.is_none() {
                    super::log(&format!(
                        "single instance: no event window ({seen:?}); running as the only copy"
                    ));
                }
                return;
            }
            Step::Forward => {
                let delivered = found.is_some_and(|(target, _)| forward(target));
                super::log(if delivered {
                    "single instance: another copy is running; this launch was handed to it"
                } else {
                    "single instance: another copy is running; it did not take this launch"
                });
                leave(app);
            }
            Step::Wait => std::thread::sleep(POLL),
            Step::GiveUp => {
                super::log(
                    "single instance: another copy is starting and showed no window in 2 s; \
                     this launch ends",
                );
                leave(app);
            }
        }
    }
}

/// Hands this process's argv to the first copy's event window, in the plugin's own format.
fn forward(target: isize) -> bool {
    let cwd = std::env::current_dir().unwrap_or_default();
    let args: Vec<String> = std::env::args().collect();
    window::send_copy_data(
        target,
        COPY_DATA_KIND,
        &payload(cwd.to_str().unwrap_or_default(), &args),
    )
}

/// Ends a duplicate the way the plugin ends one: upstream's tray icon and windows, made before
/// the fork's setup runs, are taken down first.
fn leave(app: &AppHandle) -> ! {
    app.cleanup_before_exit();
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use agentnotch_win::window::MutexClaim;

    use super::super::deeplink;
    use super::{decide, payload, Seen, Step};

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    /// The plugin's own reading of a payload (its `WM_COPYDATA` arm): a C string, split at
    /// every `|`, the working folder first.
    fn as_the_plugin_reads(bytes: &[u8]) -> (String, Vec<String>) {
        let text = std::ffi::CStr::from_bytes_until_nul(bytes)
            .expect("a NUL ends the payload")
            .to_string_lossy();
        let mut parts = text.split('|');
        let cwd = parts.next().unwrap().to_string();
        (cwd, parts.map(str::to_string).collect())
    }

    #[test]
    fn the_payload_is_the_plugins() {
        assert_eq!(
            payload(r"C:\Users\me", &argv(&["agentnotch.exe", "--silent"])),
            b"C:\\Users\\me|agentnotch.exe|--silent\0"
        );
        // Exactly one NUL, at the end.
        let bytes = payload("", &argv(&["agentnotch.exe"]));
        assert_eq!(bytes, b"|agentnotch.exe\0");
        assert_eq!(bytes.iter().filter(|b| **b == 0).count(), 1);
    }

    #[test]
    fn a_forwarded_link_reaches_the_running_copy() {
        let link = "agentnotch://auth-callback?code=a";
        let sent = argv(&[r"C:\Program Files\Agent Notch\agentnotch.exe", link]);
        let (cwd, args) = as_the_plugin_reads(&payload(r"C:\Windows\System32", &sent));
        assert_eq!(cwd, r"C:\Windows\System32");
        assert_eq!(args, sent);
        assert_eq!(deeplink::from_args(&args), Some(link));
    }

    #[test]
    fn a_forwarded_link_holding_a_bar_is_ignored() {
        let sent = argv(&["agentnotch.exe", "agentnotch://auth-callback?code=a|b"]);
        let (_, args) = as_the_plugin_reads(&payload(r"C:\", &sent));
        assert_eq!(
            args,
            argv(&["agentnotch.exe", "agentnotch://auth-callback?code=a", "b"])
        );
        assert_eq!(deeplink::from_args(&args), None);
    }

    #[test]
    fn the_first_copy_always_goes_on() {
        // Its own window exists: the plugin's setup ran before the app's.
        for late in [false, true] {
            assert_eq!(decide(Seen::Window(41), 41, late), Step::Continue);
            // No window, but no other copy either.
            for claim in [MutexClaim::Ours, MutexClaim::Missing, MutexClaim::Taken] {
                assert_eq!(
                    decide(Seen::NoWindow(claim), 41, late),
                    Step::Continue,
                    "{claim:?}"
                );
            }
        }
    }

    #[test]
    fn a_duplicate_forwards_to_the_other_copys_window() {
        for late in [false, true] {
            assert_eq!(decide(Seen::Window(7), 41, late), Step::Forward);
        }
    }

    #[test]
    fn a_duplicate_without_a_window_to_reach_waits_then_leaves() {
        let seen = Seen::NoWindow(MutexClaim::Elsewhere);
        assert_eq!(decide(seen, 41, false), Step::Wait);
        assert_eq!(decide(seen, 41, true), Step::GiveUp);
    }
}
