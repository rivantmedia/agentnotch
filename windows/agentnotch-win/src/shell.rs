//! Explorer and the shell for the panel and Settings (DESIGN-WIN §3.5; WP9):
//! - [`reveal`]: a folder or file shown in Explorer, selected (`SHParseDisplayName` +
//!   `SHOpenFolderAndSelectItems`) on a path the engine resolved; never a command line, so no
//!   character in a path can become an argument;
//! - [`open_uri`]: a URL or `ms-settings:` page handed to its registered handler
//!   (`ShellExecuteW` "open"); which ones may be opened is the glue's rule, not this module's;
//! - [`pick_folder`]: the native folder picker (`IFileOpenDialog` with `FOS_PICKFOLDERS`);
//! - [`scheme_command`] and [`start_menu_shortcut`]: what the installer registered, for the
//!   doctor (the `agentnotch:` links' handler, and the shortcut Windows needs before it shows
//!   this app's notifications).
//!
//! The shell's COM objects want a single-threaded apartment, and callers arrive on Tauri's
//! blocking pool, whose threads belong to no apartment; each call therefore runs on a short-lived
//! thread of its own that enters one and leaves it again.

use std::path::{Path, PathBuf};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, ERROR_SUCCESS, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, FileOpenDialog, IFileOpenDialog, ILFree, SHGetKnownFolderPath,
    SHOpenFolderAndSelectItems, SHParseDisplayName, ShellExecuteW, FOS_FORCEFILESYSTEM,
    FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, KF_FLAG_DEFAULT, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Runs `work` on a new thread inside a single-threaded apartment and waits for its answer.
fn in_apartment<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    std::thread::Builder::new()
        .name("an-shell".into())
        .spawn(move || {
            // SAFETY: this new thread enters an apartment once and leaves it below.
            let entered =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            if entered.is_err() {
                return Err(format!("the shell isn't available ({})", entered.message()));
            }
            let answer = work();
            // SAFETY: balances the successful CoInitializeEx above on the same thread.
            unsafe { CoUninitialize() };
            answer
        })
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "the shell call failed unexpectedly".to_string())?
}

/// Shows `path` in Explorer: its folder opens with it selected (a folder itself is selected in
/// its parent).
pub fn reveal(path: &Path) -> Result<(), String> {
    let target = HSTRING::from(path.as_os_str());
    in_apartment(move || {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        // SAFETY: `target` is NUL-terminated and outlives the call; `pidl` receives an allocation
        // freed below.
        unsafe { SHParseDisplayName(&target, None, &mut pidl, 0, None) }
            .map_err(|e| format!("that isn't a place Explorer can show ({})", e.message()))?;
        // SAFETY: `pidl` is the absolute item list just parsed; no child list is passed, so the
        // item itself is selected in its parent folder.
        let shown = unsafe { SHOpenFolderAndSelectItems(pidl, None, 0) };
        // SAFETY: allocated by SHParseDisplayName and not used after this.
        unsafe { ILFree(Some(pidl)) };
        shown.map_err(|e| e.message())
    })
}

/// Opens `uri` with its registered handler (the default browser for `https:`, Windows Settings
/// for `ms-settings:`). The caller decides which URIs are allowed.
pub fn open_uri(uri: &str) -> Result<(), String> {
    let target = HSTRING::from(uri);
    let verb = HSTRING::from("open");
    in_apartment(move || {
        // SAFETY: every string is NUL-terminated and outlives the call; no parameters, no folder.
        let result = unsafe { ShellExecuteW(None, &verb, &target, None, None, SW_SHOWNORMAL) };
        // ShellExecuteW's "HINSTANCE" is a status: above 32 is success.
        if result.0 as usize > 32 {
            Ok(())
        } else {
            Err(format!(
                "Windows couldn't open it (code {})",
                result.0 as usize
            ))
        }
    })
}

/// The native folder picker, owned by `owner` (a window handle as `isize`) while it is up.
/// `Ok(None)` when the user cancels.
pub fn pick_folder(owner: Option<isize>, title: &str) -> Result<Option<PathBuf>, String> {
    let title = HSTRING::from(title);
    in_apartment(move || {
        // SAFETY: creates the system's file dialog object in this apartment.
        let dialog: IFileOpenDialog =
            unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| e.message())?;
        // SAFETY: plain setters on the dialog just created; `title` outlives the calls.
        unsafe {
            let options = dialog.GetOptions().map_err(|e| e.message())?;
            dialog
                .SetOptions(
                    options
                        | FOS_PICKFOLDERS
                        | FOS_FORCEFILESYSTEM
                        | FOS_PATHMUSTEXIST
                        | FOS_NOCHANGEDIR,
                )
                .map_err(|e| e.message())?;
            dialog.SetTitle(&title).map_err(|e| e.message())?;
        }
        let owner = owner.map(|raw| HWND(raw as *mut _));
        // SAFETY: modal on this thread; the owner (possibly another thread's window) is only
        // disabled while the dialog is up.
        if let Err(e) = unsafe { dialog.Show(owner) } {
            return if e.code() == ERROR_CANCELLED.to_hresult() {
                Ok(None)
            } else {
                Err(e.message())
            };
        }
        // SAFETY: the dialog closed with a choice; the display name is CoTaskMemAlloc'd and freed
        // below after it has been copied.
        let path = unsafe {
            let item = dialog.GetResult().map_err(|e| e.message())?;
            let name = item
                .GetDisplayName(SIGDN_FILESYSPATH)
                .map_err(|e| e.message())?;
            let text = name.to_string().map_err(|e| e.to_string());
            CoTaskMemFree(Some(name.0 as *const _));
            text?
        };
        Ok(Some(PathBuf::from(path)))
    })
}

/// The command Windows runs for `<scheme>:` links for this user
/// (`HKCU\Software\Classes\<scheme>\shell\open\command`), as the installer wrote it; `None`
/// when the scheme isn't registered. A scheme name is a letter followed by letters, digits, `+`,
/// `-` and `.` (RFC 3986); anything else is not looked up.
pub fn scheme_command(scheme: &str) -> Option<String> {
    let valid = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid {
        return None;
    }
    let key = HSTRING::from(format!("Software\\Classes\\{scheme}\\shell\\open\\command"));
    let mut bytes = 0u32;
    // SAFETY: a size query: no buffer, a valid out pointer for the length. A null value name
    // asks for the key's unnamed (default) value.
    let sized = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if sized != ERROR_SUCCESS || bytes < 2 {
        return None;
    }
    // One unit of slack, so a value that grew between the two calls by its terminator still fits.
    let mut units = vec![0u16; (bytes as usize).div_ceil(2) + 1];
    let mut capacity = (units.len() * 2) as u32;
    // SAFETY: `units` holds `capacity` writable bytes and outlives the call.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(units.as_mut_ptr().cast()),
            Some(&mut capacity),
        )
    };
    if read != ERROR_SUCCESS {
        return None;
    }
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..end]);
    (!text.is_empty()).then_some(text)
}

/// The Start-menu shortcut the installer made for `product` (`<Programs>\<product>.lnk`), when it
/// is there. Windows shows an unpackaged app's notifications only while such a shortcut carries
/// its AppUserModelID; whether this one does is the notification service's to find out.
pub fn start_menu_shortcut(product: &str) -> Option<PathBuf> {
    // A name, never a path: the shortcut sits directly in the Programs folder.
    if product.is_empty() || product.contains(['\\', '/', ':']) {
        return None;
    }
    // SAFETY: a valid known-folder id; on success the returned string is CoTaskMemAlloc'd and
    // freed below after it has been copied.
    let raw = unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: `raw` is the NUL-terminated path the call just returned.
    let folder = unsafe { raw.to_string() }.ok().map(PathBuf::from);
    // SAFETY: `raw` came from SHGetKnownFolderPath and is not used after this.
    unsafe { CoTaskMemFree(Some(raw.0 as *const _)) };
    let shortcut = folder?.join(format!("{product}.lnk"));
    shortcut.is_file().then_some(shortcut)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_that_does_not_exist_is_an_error_not_a_window() {
        let missing = std::env::temp_dir().join("agentnotch-test-no-such-folder-5f1c2a");
        assert!(reveal(&missing).is_err());
    }

    #[test]
    fn an_unregistered_scheme_has_no_command() {
        assert_eq!(scheme_command("agentnotch-test-unregistered-5f1c2a"), None);
        // Never a registry path built from something that isn't a scheme name.
        assert_eq!(scheme_command(""), None);
        assert_eq!(scheme_command("a\\b"), None);
        assert_eq!(scheme_command("..\\.."), None);
        assert_eq!(scheme_command("1abc"), None);
        assert_eq!(scheme_command("https://example.com"), None);
    }

    #[test]
    fn a_shortcut_that_was_never_made_is_absent() {
        assert_eq!(
            start_menu_shortcut("Agent Notch Test No Such App 5f1c2a"),
            None
        );
        assert_eq!(start_menu_shortcut(""), None);
        assert_eq!(start_menu_shortcut("..\\Startup\\x"), None);
    }
}
