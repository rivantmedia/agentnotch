//! Upstream's "Codenotch" copy, renamed to this app (§4.16; the Mac's
//! `Fork.rebranded`). Vectors: `tests/ui-contract/rebrand-vectors.json`,
//! shared with `ui/agentnotch/rebrand.js`.
//!
//! The rules, in order:
//!
//! - copy that names one of upstream's own products (the Codenotch phone app,
//!   Codenotch for Windows, its installer, its author's site) is left exactly
//!   as it is;
//! - text that names the app only through an argument (a folder or an account
//!   the user named) is the user's data and stays as it is ([`rebranded_with`]
//!   asks for the template to tell);
//! - English "a Codenotch" becomes "an Agent Notch" (the name starts with a
//!   vowel), French "de"/"que" and friends elide ("d'Agent Notch");
//! - compounds join every word ("Codenotch-Einstellungen" becomes
//!   "Agent-Notch-Einstellungen");
//! - every other "Codenotch" becomes "Agent Notch".
//!
//! `ui/agentnotch/rebrand.js` applies the same rules to a page's text without
//! a language: it takes the English article rule for every string and has no
//! French elision, so the page and this module agree on English copy (the
//! shared vectors) and differ only in French, which the pages do not draw.

use regex::{NoExpand, Regex};
use std::sync::OnceLock;

/// What upstream's copy calls the app.
pub const UPSTREAM_NAME: &str = "Codenotch";
/// What this app is called.
pub const DISPLAY_NAME: &str = "Agent Notch";

/// English copy naming a product other than this app that is called
/// Codenotch. Phrases rather than keys, so a reworded string upstream still
/// matches while it keeps naming the same product.
pub const UPSTREAM_PRODUCT_PHRASES: [&str; 6] = [
    "Codenotch app on your phone",
    "Codenotch on your phone",
    "Codenotch phone app",
    "Codenotch for Windows",
    "Codenotch-Setup",
    "hivinz.com",
];

/// Whether `text` names upstream at all: the cheap check every lookup makes
/// before anything else.
pub fn names_upstream(text: &str) -> bool {
    text.contains(UPSTREAM_NAME)
}

/// Whether `key` (upstream's English) is about a product that really is
/// called Codenotch.
pub fn names_upstream_product(key: &str) -> bool {
    UPSTREAM_PRODUCT_PHRASES
        .iter()
        .any(|phrase| key.contains(phrase))
}

/// `text` with upstream's name replaced by this app's, for English copy
/// (the tray, window titles, the pages' text): [`rebranded_with`] for
/// English, where `text` is its own template.
pub fn rebranded(text: &str) -> String {
    rebranded_with(text, "en", text, || text.to_owned())
}

/// The Mac's `Fork.rebranded`.
///
/// - `language` is the locale's language (`en`, `fr`, `pt-BR`, `de`, ...).
/// - `key` is upstream's English with `%@`-style placeholders. Copy about
///   products that really are called Codenotch keeps the name.
/// - `template` is the looked-up string before its arguments were filled in.
///   When it doesn't name Codenotch, an argument did (a project folder, a
///   device or account name): that is the user's data. Only asked for when
///   `text` names Codenotch.
pub fn rebranded_with(
    text: &str,
    language: &str,
    key: &str,
    template: impl FnOnce() -> String,
) -> String {
    if !names_upstream(text) || names_upstream_product(key) {
        return text.to_owned();
    }
    let template = template();
    if !names_upstream(&template) {
        return text.to_owned();
    }
    let language = language
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let rules = rules();
    let mut result = text.to_owned();
    // A key without a translation falls back to upstream's English.
    if language == "en" || template == key {
        // "an Agent Notch window", not "a Agent Notch window".
        result = rules
            .english_article
            .replace_all(&result, format!("${{1}}n {DISPLAY_NAME}"))
            .into_owned();
    } else if language == "fr" {
        // The name starts with a vowel, so "de" and "que" (lorsque,
        // puisque, ...) elide: "Reglages d'Agent Notch", "tant qu'Agent Notch".
        result = rules
            .french_elision
            .replace_all(&result, format!("${{1}}'{DISPLAY_NAME}"))
            .into_owned();
    }
    // Compounds join every word of the name: "Agent-Notch-Einstellungen".
    let compound = format!("{UPSTREAM_NAME}-");
    let joined = format!("{}-", DISPLAY_NAME.replace(' ', "-"));
    let result = result.replace(&compound, &joined);
    rules
        .name
        .replace_all(&result, NoExpand(DISPLAY_NAME))
        .into_owned()
}

/// Compiled once: the tray and the window titles ask often enough.
struct Rules {
    english_article: Regex,
    french_elision: Regex,
    name: Regex,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| Rules {
        english_article: Regex::new(&format!(r"\b([aA]) {UPSTREAM_NAME}\b")).expect("article rule"),
        french_elision: Regex::new(&format!(
            r"\b([dD]|[qQ]u|[lL]orsqu|[pP]uisqu|[jJ]usqu|[qQ]uoiqu)e {UPSTREAM_NAME}\b"
        ))
        .expect("elision rule"),
        name: Regex::new(UPSTREAM_NAME).expect("name rule"),
    })
}
