//! A manual clock: time moves only when the test says so.

use crate::core::time;
use crate::platform::Clock;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

pub struct FakeClock {
    now: Mutex<SystemTime>,
    base: Instant,
    elapsed: Mutex<Duration>,
}

impl FakeClock {
    pub fn new(start: SystemTime) -> FakeClock {
        FakeClock {
            now: Mutex::new(start),
            base: Instant::now(),
            elapsed: Mutex::new(Duration::ZERO),
        }
    }

    pub fn at_ms(epoch_ms: u64) -> FakeClock {
        FakeClock::new(time::from_ms(epoch_ms))
    }

    /// Moves both the wall clock and the monotonic clock forward.
    pub fn advance(&self, by: Duration) {
        *super::lock(&self.now) += by;
        *super::lock(&self.elapsed) += by;
    }

    /// Sets the wall clock (the monotonic clock never goes back).
    pub fn set(&self, to: SystemTime) {
        *super::lock(&self.now) = to;
    }

    pub fn now_ms(&self) -> u64 {
        time::to_ms(self.now())
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        *super::lock(&self.now)
    }

    fn monotonic(&self) -> Instant {
        self.base + *super::lock(&self.elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_only_when_told() {
        let clock = FakeClock::at_ms(1_000);
        let (wall, mono) = (clock.now(), clock.monotonic());
        assert_eq!(clock.now(), wall);
        clock.advance(Duration::from_secs(3));
        assert_eq!(clock.now_ms(), 4_000);
        assert_eq!(clock.monotonic() - mono, Duration::from_secs(3));
        clock.set(time::from_ms(10));
        assert_eq!(clock.now_ms(), 10);
        assert_eq!(clock.monotonic() - mono, Duration::from_secs(3));
    }
}
