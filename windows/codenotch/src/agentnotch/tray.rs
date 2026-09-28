//! The fork's mark on upstream's tray icon (DESIGN-WIN §2.4 `tray.rs`, §4.10): the needs-you
//! count in the tooltip ("Agent Notch — 2 need you") and, with the tray package (WP9), an amber
//! dot drawn on upstream's image. Windows' tray has no room for a digit, so the number lives in
//! the tooltip.

use std::sync::atomic::Ordering;

use tauri::AppHandle;

/// The hub's `TrayBadge(count)`.
pub(super) fn set_badge(app: &AppHandle, count: u32) {
    if super::NEEDS_YOU.swap(count, Ordering::Relaxed) == count {
        return;
    }
    // Upstream rebuilds its tooltip from its readings; with none to show it asks
    // `tray_tooltip()`, which reads the count above. Nudge it now instead of at its next poll.
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(super::tray_tooltip()));
    }
}
