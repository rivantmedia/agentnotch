//! The wall clock and the monotonic clock, for the hub and the sealed fixture hub alike.
//!
//! Plain `std`, so it is the same on every OS; tests use the engine's `testkit` clock instead.

use std::time::{Instant, SystemTime};

use agentnotch_engine::platform::Clock;

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn monotonic(&self) -> Instant {
        Instant::now()
    }
}
