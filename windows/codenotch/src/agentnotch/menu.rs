//! The notch's right-click menu (seams WNM, DESIGN-WIN §2.4 `menu.rs`): a Claude ring's menu
//! offers "Open sessions panel". Upstream's handler hands every `notch:` item to
//! `notch_menu_event` first; the fork's items carry `an-` ids, so neither side ever acts on the
//! other's.

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::{AppHandle, Wry};

use super::panel;

/// Upstream's prefix for the notch menu's ids (notchmenu.rs), plus the fork's own.
const OPEN_PANEL: &str = "an-panel:";

/// Adds the fork's items for the ring under the pointer (`provider` = its cell id).
pub fn notch_menu_items<'m>(
    app: &AppHandle,
    menu: MenuBuilder<'m, Wry, AppHandle>,
    provider: Option<&str>,
) -> MenuBuilder<'m, Wry, AppHandle> {
    let Some(ring) = provider.filter(|p| is_claude_ring(p)) else {
        return menu;
    };
    match MenuItemBuilder::with_id(format!("notch:{OPEN_PANEL}{ring}"), "Open sessions panel")
        .build(app)
    {
        Ok(item) => menu.item(&item),
        Err(e) => {
            super::log(&format!("notch menu: {e}"));
            menu
        }
    }
}

/// Handles the fork's items (`item` is the id without upstream's `notch:` prefix). `true` when
/// the item was the fork's.
pub fn notch_menu_event(app: &AppHandle, item: &str) -> bool {
    let Some(ring) = item.strip_prefix(OPEN_PANEL) else {
        return item.starts_with("an-");
    };
    // "claude" is upstream's single Claude cell: no ring of its own to highlight.
    let ring_id = (ring != "claude").then(|| ring.to_string());
    panel::open_route(app, "sessions".into(), ring_id, "ring_click".into());
    true
}

/// Claude's cells: upstream's `claude`, and the fork's per-account rings (`claude-acct-…`, and
/// the older `claude-<slug>` / `claude-dir-…` ids): the ones upstream's refresh hands the fork.
pub(super) fn is_claude_ring(provider: &str) -> bool {
    super::is_claude_provider(provider)
}
