//! The watchdog of DESIGN-WIN §1.4: bounds every wait on the app except a PermissionRequest's
//! answer.
//!
//! A synchronous `WriteFile` of a large frame into the pipe blocks for as long as the server does
//! not read, and a connect can wait on a busy pipe. Neither may cost Claude Code more than the
//! budget, so once stdin is read a thread armed with a deadline ends the process with exit 0 when
//! it expires. A PermissionRequest disarms it once its frame is fully written: it then waits for
//! the answer with no deadline of its own (Claude Code's 86 400 s hook timeout ends it).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

/// Connect + write budget of every hook event (§1.4).
pub const HOOK_BUDGET: Duration = Duration::from_millis(1200);

pub struct Watchdog {
    disarmed: Arc<AtomicBool>,
    thread: Option<Thread>,
}

impl Watchdog {
    /// Starts the countdown. When the thread cannot be started the hook runs unguarded, which is
    /// no worse than having no watchdog; it never stops the hook from doing its work.
    pub fn arm(budget: Duration) -> Watchdog {
        let disarmed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&disarmed);
        let deadline = Instant::now() + budget;
        let spawned = thread::Builder::new()
            .name("watchdog".into())
            .stack_size(64 * 1024)
            .spawn(move || loop {
                if flag.load(Ordering::Acquire) {
                    return;
                }
                let now = Instant::now();
                if now >= deadline {
                    // Stdout is still empty here by design: nothing is printed before the frame
                    // is written, and a PermissionRequest has disarmed us before it waits.
                    std::process::exit(0);
                }
                thread::park_timeout(deadline - now);
            });
        Watchdog {
            disarmed,
            thread: spawned.ok().map(|handle| handle.thread().clone()),
        }
    }

    /// Stops the countdown for good (a PermissionRequest after its frame is written).
    pub fn disarm(&self) {
        self.disarmed.store(true, Ordering::Release);
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
