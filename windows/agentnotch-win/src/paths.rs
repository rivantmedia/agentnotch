//! The engine's folders on Windows (DESIGN-WIN §1.5, §3.2 `Roots`; WP3).
//!
//! Each root is resolved the way its owner resolves it, so both sides always agree: `home` the
//! way Claude Code's `os.homedir()` does (`%USERPROFILE%`, the Known Folder only when it is
//! empty); `data` is handed in by the glue (the folder of upstream's `config.json`); `support` from
//! `AGENTNOTCH_SUPPORT_DIR`, else Tauri's app-local-data folder (Known Folder LocalAppData +
//! identifier) + `Claude`; Claude Desktop's folders the way Electron finds its userData (the Known
//! Folder, never the environment).

use std::path::PathBuf;

use agentnotch_engine::platform::Roots;
use windows::core::GUID;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_RoamingAppData, SHGetKnownFolderPath,
    KF_FLAG_DEFAULT,
};

/// The folders the hub works in, resolved once at launch.
///
/// `app_identifier` is the Tauri identifier (`com.rivantmedia.agentnotch`), whose LocalAppData
/// folder already holds WebView2's data; `install_dir` is the folder of `agentnotch.exe`.
pub fn roots(
    app_identifier: &str,
    data: PathBuf,
    install_dir: Option<PathBuf>,
) -> Result<Roots, String> {
    let home = non_empty_env("USERPROFILE")
        .or_else(|| known_folder(&FOLDERID_Profile))
        .ok_or("the user profile folder can't be found")?;
    let support = match non_empty_env("AGENTNOTCH_SUPPORT_DIR") {
        Some(dir) => dir,
        None => known_folder(&FOLDERID_LocalAppData)
            .ok_or("the local application data folder can't be found")?
            .join(app_identifier)
            .join("Claude"),
    };
    // Claude Desktop's own folder. Its Microsoft Store package keeps a second one under
    // Packages\…\LocalCache\Roaming\Claude; finding those is part of account discovery.
    let claude_desktop = known_folder(&FOLDERID_RoamingAppData)
        .map(|roaming| vec![roaming.join("Claude")])
        .unwrap_or_default();
    // `C:` + `\Users`, not `Path::join`: joining onto a bare drive gives the drive-relative
    // `C:Users`.
    let system_users = non_empty_env("SystemDrive").map(|drive| {
        let mut users = drive.into_os_string();
        users.push("\\Users");
        PathBuf::from(users)
    });
    Ok(Roots {
        home,
        data,
        support,
        claude_desktop,
        system_users,
        install_dir,
    })
}

fn non_empty_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    // SAFETY: `id` is a valid GUID; on success the returned string is CoTaskMemAlloc'd and freed
    // below after it has been copied.
    let raw = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: `raw` is the NUL-terminated path the call just returned.
    let path = unsafe { raw.to_string() }.ok().map(PathBuf::from);
    // SAFETY: `raw` came from SHGetKnownFolderPath and is not used after this.
    unsafe { CoTaskMemFree(Some(raw.0 as *const _)) };
    path.filter(|p| !p.as_os_str().is_empty())
}
