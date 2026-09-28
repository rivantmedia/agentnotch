//! The chimes (DESIGN-WIN §3.2 `Sounds`, §4.10; WP6): `PlaySoundW(SND_ALIAS | SND_ASYNC)`,
//! `SystemExclamation` when a session needs you, `SystemAsterisk` when one finishes.
//!
//! Not implemented in this build: silent.

use agentnotch_engine::platform::{Chime, Sounds};

#[derive(Debug, Default)]
pub struct Chimes;

impl Chimes {
    pub fn new() -> Self {
        Chimes
    }
}

impl Sounds for Chimes {
    fn play(&self, _c: Chime) {}
}
