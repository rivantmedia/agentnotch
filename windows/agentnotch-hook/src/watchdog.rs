//! The watchdog of DESIGN-WIN §1.4: bounds every wait on the app.
//!
//! A synchronous `WriteFile` of a large frame into the pipe blocks for as long as the server does
//! not read, and a connect can wait on a busy pipe. Neither may cost Claude Code more than the
//! budget, so once stdin is read a thread armed with a deadline ends the process with exit 0 when
//! it expires. Nothing has been printed by then: a hook prints only after its answer arrived.
//!
//! A PermissionRequest moves the deadline out once its frame is written: from then on it only
//! waits for the user, for as long as Claude Code lets a hook run (the installer registers the
//! same 86 400 s).

use std::sync::{Arc, Mutex};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use agentnotch_proto::limits::{DECISION_TIMEOUT_S, HOOK_WATCHDOG_MS};

/// Connect + write budget of every hook event.
pub const HOOK_BUDGET: Duration = Duration::from_millis(HOOK_WATCHDOG_MS);
/// How long a PermissionRequest waits for its answer at most.
pub const DECISION_WAIT: Duration = Duration::from_secs(DECISION_TIMEOUT_S);

pub struct Watchdog {
    /// When the process ends; `None` once disarmed for good.
    deadline: Arc<Mutex<Option<Instant>>>,
    thread: Option<Thread>,
}

impl Watchdog {
    /// Starts the countdown. When the thread cannot be started the exe runs unguarded, which is
    /// no worse than having no watchdog; it never stops the exe from doing its work.
    pub fn arm(budget: Duration) -> Watchdog {
        let deadline = Arc::new(Mutex::new(Some(Instant::now() + budget)));
        let shared = Arc::clone(&deadline);
        let spawned = thread::Builder::new()
            .name("watchdog".into())
            .stack_size(64 * 1024)
            .spawn(move || loop {
                let due = match shared.lock() {
                    Ok(guard) => *guard,
                    // A poisoned lock means the main thread panicked: it is on its way out.
                    Err(_) => return,
                };
                let Some(due) = due else {
                    return;
                };
                let now = Instant::now();
                if now >= due {
                    crate::trace::note(|| "watchdog: exit".into());
                    exit_now();
                }
                thread::park_timeout(due - now);
            });
        Watchdog {
            deadline,
            thread: spawned.ok().map(|handle| handle.thread().clone()),
        }
    }

    /// Moves the deadline to `budget` from now (a PermissionRequest, once its frame is written).
    pub fn rearm(&self, budget: Duration) {
        self.set(Instant::now().checked_add(budget));
    }

    /// Stops the countdown for good.
    pub fn disarm(&self) {
        self.set(None);
    }

    fn set(&self, deadline: Option<Instant>) {
        if let Ok(mut guard) = self.deadline.lock() {
            *guard = deadline;
        }
        if let Some(thread) = &self.thread {
            thread.unpark();
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.disarm();
    }
}

/// Ends the process with exit code 0, from the watchdog thread, while the main thread may be
/// blocked inside `WriteFile` or `ReadFile` on the pipe.
fn exit_now() -> ! {
    #[cfg(windows)]
    {
        // SAFETY: ends this process. `ExitProcess` stops the other threads first, a blocked
        // pipe write or read included, and runs no Rust destructor: there is nothing to flush
        // (stdout is written and flushed in one step, and only after an answer).
        unsafe { windows_sys::Win32::System::Threading::ExitProcess(0) }
    }
    #[cfg(not(windows))]
    {
        std::process::exit(0)
    }
}
