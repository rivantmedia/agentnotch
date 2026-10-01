//! Session summaries (CL§9): the text a summary is made from and what of it
//! may leave this PC (`text`: the excerpt, the redaction, the path scrub and
//! the names it knows), the command and its runner (`run`), and the store
//! of what was written, the failures to wait out and the caps (`store`).
//!
//! Owner: WP8.

pub mod run;
pub mod store;
pub mod text;
