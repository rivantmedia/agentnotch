//! The sessions panel's window on Windows (DESIGN-WIN §5.3; WP9): `WS_EX_NOACTIVATE` for an
//! auto-opened panel and dropping it on a click, `SW_SHOWNOACTIVATE`, topmost re-assertion,
//! saving and restoring the foreground window, and confirming the panel really is the foreground
//! window before its keyboard gate opens. Nothing here yet.
