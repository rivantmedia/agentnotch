//! The chimes (DESIGN-WIN §3.2 `Sounds`, §4.10; WP6): `PlaySoundW(SND_ALIAS | SND_ASYNC)`,
//! `SystemExclamation` when a session needs you, `SystemAsterisk` when one finishes. The user's
//! own sound scheme decides what those aliases play, so "no sound" there is respected for free.
//!
//! Compiled on Windows only; other systems get the stub's silent `Sounds`.

#![cfg(windows)]

use agentnotch_engine::platform::{Chime, Sounds};
use windows::core::{w, PCWSTR};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC};

#[derive(Debug, Default)]
pub struct Chimes;

impl Chimes {
    pub fn new() -> Self {
        Chimes
    }
}

/// The registry alias a chime plays.
fn alias(c: Chime) -> PCWSTR {
    match c {
        Chime::Blocked => w!("SystemExclamation"),
        Chime::Finished => w!("SystemAsterisk"),
    }
}

impl Sounds for Chimes {
    fn play(&self, c: Chime) {
        // SAFETY: the alias is a static NUL-terminated string and no module handle is passed.
        // SND_ASYNC returns at once, so nothing here waits on audio. A failure (no sound device,
        // alias missing) is not worth telling anyone: the banner and the ring still say it.
        unsafe {
            let _ = PlaySoundW(alias(c), None, SND_ALIAS | SND_ASYNC);
        }
    }
}
