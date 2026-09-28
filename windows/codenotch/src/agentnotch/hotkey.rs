//! The panel shortcut (DESIGN-WIN §2.4 `hotkey.rs`, §5.3): registers the `hotKey` setting
//! (off, Ctrl+Alt+Space, Ctrl+Alt+J) through `agentnotch_win::hotkey` and reports each attempt to
//! the hub as `hotkey_status`, so Settings can say "That shortcut is taken by another app."
//!
//! The shortcut is off by default and its Windows side comes with WP9; until then nothing is
//! registered, which is exactly the default's behaviour.
