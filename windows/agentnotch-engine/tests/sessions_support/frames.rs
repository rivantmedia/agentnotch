//! The hook pipe's framing as the end-to-end tests see it: a little-endian
//! `u32` length, then a JSON object. The pipe server reads the length and
//! hands the engine the body; `tests/sessions_e2e.rs` checks each frame it
//! builds with [`unframe`] and injects the body, which WP1's real
//! `HookIngress` decodes (this file held a test-only decoder until the
//! ingress landed).
#![allow(dead_code)]

use serde_json::Value;

/// The JSON of a frame (`u32` little endian length, then the bytes).
pub fn unframe(bytes: &[u8]) -> Result<Value, &'static str> {
    let (length, body) = bytes
        .split_first_chunk::<4>()
        .ok_or("a frame shorter than its header")?;
    if u32::from_le_bytes(*length) as usize != body.len() {
        return Err("a frame whose length is not its body's");
    }
    serde_json::from_slice(body).map_err(|_| "a frame that is not JSON")
}
