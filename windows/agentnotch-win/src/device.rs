//! This PC and this app's token (DESIGN-WIN §3.2 `Device`; WP8): the computer name for cloud sync,
//! the user's SID (the pipe's name), the app's own elevation and Smart App Control's state for
//! the doctor.

#![cfg(windows)]

use std::mem::size_of;

use agentnotch_engine::cloud::contract::{clamp_utf16, limit};
use agentnotch_engine::platform::{Device, Roots};
use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
use windows::Win32::System::SystemInformation::{
    ComputerNamePhysicalDnsHostname, GetComputerNameExW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[derive(Debug, Default)]
pub struct WinDevice;

impl WinDevice {
    pub fn new(_roots: &Roots) -> Self {
        WinDevice
    }
}

impl Device for WinDevice {
    fn computer_name(&self) -> String {
        // The host name the user gave this PC (not the 15-character NetBIOS name), as the website
        // lists devices; the variable only when the call fails. Clamped to what the website keeps.
        let name = physical_dns_host_name()
            .or_else(|| std::env::var("COMPUTERNAME").ok())
            .unwrap_or_default();
        clamp_utf16(name.trim(), limit::DEVICE_NAME)
    }
    fn user_sid(&self) -> Option<String> {
        crate::sid::current_user_sid()
    }
    fn elevated(&self) -> bool {
        token_elevated().unwrap_or(false)
    }
    fn smart_app_control(&self) -> Option<String> {
        smart_app_control_state().and_then(|state| {
            match state {
                0 => Some("off"),
                1 => Some("on"),
                2 => Some("evaluation"),
                _ => None,
            }
            .map(str::to_owned)
        })
    }
}

fn physical_dns_host_name() -> Option<String> {
    // The size query fails with ERROR_MORE_DATA by design and reports the length with its NUL.
    let mut size = 0u32;
    // SAFETY: a size query: no buffer, a valid out pointer for the length.
    let _ = unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, None, &mut size) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; size as usize];
    // SAFETY: `buffer` holds `size` UTF-16 units, as `size` says; the call writes at most that.
    unsafe {
        GetComputerNameExW(
            ComputerNamePhysicalDnsHostname,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut size,
        )
    }
    .ok()?;
    // On success `size` is the length without the NUL.
    let name = String::from_utf16_lossy(&buffer[..(size as usize).min(buffer.len())]);
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

fn token_elevated() -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo-handle of the current process needs no closing; `token` is a valid out
    // pointer and is closed below once it was opened.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: `elevation` is a TOKEN_ELEVATION of exactly the size passed, alive for the call.
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    // SAFETY: `token` was opened above and is not used after this.
    let _ = unsafe { CloseHandle(token) };
    read.ok()?;
    Some(elevation.TokenIsElevated != 0)
}

/// `VerifiedAndReputablePolicyState` (0 off, 1 on, 2 evaluation); `None` when the value is absent
/// (Windows before 11 22H2) or unreadable. RegGetValueW opens and closes the key itself.
fn smart_app_control_state() -> Option<u32> {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `value` is a DWORD of exactly `size` bytes, alive for the call; the type filter
    // makes the call refuse anything that isn't a DWORD.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SYSTEM\CurrentControlSet\Control\CI\Policy"),
            w!("VerifiedAndReputablePolicyState"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> WinDevice {
        WinDevice
    }

    #[test]
    fn the_computer_name_is_known_and_fits_the_website() {
        let name = device().computer_name();
        assert!(!name.is_empty());
        assert!(name.encode_utf16().count() <= limit::DEVICE_NAME, "{name}");
        assert_eq!(name, name.trim());
    }

    #[test]
    fn elevation_and_smart_app_control_answer_without_panicking() {
        let _ = device().elevated();
        let state = device().smart_app_control();
        assert!(
            state
                .as_deref()
                .is_none_or(|s| ["on", "off", "evaluation"].contains(&s)),
            "{state:?}"
        );
    }
}
