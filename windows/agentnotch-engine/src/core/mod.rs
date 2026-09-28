//! Foundations every package uses: paths, the `.claude.json` reader, the
//! JSON field scanner, atomic private files, time, switches, sealed mode.

pub mod atomic;
pub mod claude_json;
pub mod flags;
pub mod json_scan;
pub mod paths;
pub mod rebrand;
pub mod roots;
pub mod sealed;
pub mod settings;
pub mod settings_doc;
pub mod time;
