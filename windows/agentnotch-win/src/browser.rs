//! Opening the website sign-in in the default browser (DESIGN-WIN §3.2 `Browser`, §4.11; WP8):
//! `ShellExecuteW("open")`; with `AGENTNOTCH_DEV=1` and `AGENTNOTCH_DEV_BROWSER_LOG` (never sealed)
//! the URL is appended to that file instead, for the smoke test's sign-in.
//!
//! Not implemented in this build: the sign-in reports that no browser could be opened.

use agentnotch_engine::platform::Browser;

use crate::NOT_IMPLEMENTED;

#[derive(Debug, Default)]
pub struct ShellBrowser;

impl ShellBrowser {
    pub fn new() -> Self {
        ShellBrowser
    }
}

impl Browser for ShellBrowser {
    fn open(&self, _url: &str) -> Result<(), String> {
        Err(format!("Opening the browser is {NOT_IMPLEMENTED}."))
    }
}
