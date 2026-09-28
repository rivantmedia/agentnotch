//! The panel shortcut (DESIGN-WIN §5.3; WP9): a message-only window and a `RegisterHotKey` loop on
//! its own thread (`an-hotkey`), off by default; a registration refused because another app holds
//! the shortcut is reported so Settings can say so. Nothing here yet.
