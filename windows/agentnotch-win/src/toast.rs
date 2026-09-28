//! Windows notifications (DESIGN-WIN §3.2 `Notifier`, §4.10; WP6): WinRT toasts through
//! `CreateToastNotifierWithId("com.rivantmedia.agentnotch")`, silent, protocol activation
//! (`agentnotch://open?…`), withdrawn by tag and group.
//!
//! Not implemented in this build: nothing is posted, and the permission reads "unavailable" so
//! Settings says banners aren't available rather than pretending they were sent.

#![cfg(windows)]

use agentnotch_engine::platform::{Notifier, NotifyPermission, Toast};

#[derive(Debug, Default)]
pub struct Toasts;

impl Toasts {
    pub fn new() -> Self {
        Toasts
    }
}

impl Notifier for Toasts {
    fn post(&self, _t: &Toast) {}
    fn withdraw(&self, _tag: &str, _group: &str) {}
    fn permission(&self) -> NotifyPermission {
        NotifyPermission::Unavailable
    }
}
