//! This PC and this app's token (DESIGN-WIN §3.2 `Device`; WP8): the computer name for cloud sync,
//! the user's SID (the pipe's name), the app's own elevation and Smart App Control's state for
//! the doctor.
//!
//! The SID is real already (`sid`); the rest reports "unknown" until WP8.

#![cfg(windows)]

use agentnotch_engine::platform::{Device, Roots};

#[derive(Debug, Default)]
pub struct WinDevice;

impl WinDevice {
    pub fn new(_roots: &Roots) -> Self {
        WinDevice
    }
}

impl Device for WinDevice {
    fn computer_name(&self) -> String {
        // The physical DNS host name is WP8's (GetComputerNameExW); the variable is its fallback.
        std::env::var("COMPUTERNAME").unwrap_or_default()
    }
    fn user_sid(&self) -> Option<String> {
        crate::sid::current_user_sid()
    }
    fn elevated(&self) -> bool {
        false
    }
    fn smart_app_control(&self) -> Option<String> {
        None
    }
}
