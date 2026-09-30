//! `usage::desktop` (ClaudeDesktopUsageCache.swift, DesktopUsageSource.swift,
//! ClaudeDesktopUsageCacheTests.swift, AU§12): Claude's limits out of Claude
//! Desktop's Simple Cache.
//!
//! Hermetic throughout: the one thing that must never decide whether these
//! pass is whether the developer running them has Claude Desktop open. Entries
//! are built byte by byte by `Entry` below in temporary folders, with bodies
//! compressed at run time by `ruzstd`. The Mac's fixtures were compressed once
//! by the real encoder and pasted in; the three used here (the full response
//! and the two oversized ones) are kept because a frame written by libzstd is
//! what tells the hand-written frame walker the truth.
//!
//! A sharing violation (Windows error 32) on one entry is a miss for that
//! entry; holding a file with no sharing needs `cfg(windows)`, which engine
//! tests don't carry, so that test is `agentnotch-win/tests/win_desktop_cache.rs`.
//!
//! Not ported: `testASlicedEntryParsesTheSameAsAWholeOne` (a Swift `Data`
//! slice that doesn't start at index 0; a Rust slice always does). The
//! oversized-entry test asserts on the result only: that a file was never
//! opened can't be observed from outside.

mod usage_support;

use agentnotch_engine::model::{DesktopCacheFormat, DesktopReading, DesktopResets, DesktopWindow};
use agentnotch_engine::platform::Roots;
use agentnotch_engine::usage::desktop::{
    cache_folder, desktop_cache_format, entry_key, folder_format, frame_layout, http_date,
    parse_entry, parse_imf_fixdate, read_desktop_cache, read_folder, usage_organization,
    FolderReading, HEADER_BYTES, MAX_ENTRY_BYTES, MAX_KEY_BYTES,
};
use base64::Engine as _;
use ruzstd::encoding::{compress_to_vec, CompressionLevel};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use usage_support::{join, windows_roots};

const ORGANIZATION: &str = "11111111-2222-3333-4444-555555555555";
const OTHER_ORGANIZATION: &str = "99999999-8888-7777-6666-555555555555";

// MARK: - Fixtures

/// A body the real encoder (libzstd) wrote: `five_hour`, `seven_day` and a
/// matching `limits` array, the shape the live response has (session 30%,
/// weekly 74%, both resetting in 2099).
const REAL_FULL: &str = "KLUv/WROAH0EACIIGxlQdw4862yq1vdy7GtBVDUou1+jNaOqbCoEwOcgypwRcnPq3g5+rfrB+rpAadPzoUpO9xo+rZKSc5dJEIWLEKxaa3rhe7DELifLFDssyxrn1whpbdrYhSX24FeWrLGTU9bsHh/LLAm2JqN7AQwAuSzCVXCUogCzLBIqWA3UBBZsCvNhCnOvBgBYAm4PaQlsxA==";
/// ~330 kB decompressed, with the size declared in the frame header, so it can
/// be refused before anything is allocated.
const REAL_OVERSIZED_DECLARED: &str = "KLUv/aTVIAUADAMAgkQRGHCrDoBU3Hh7qUirw4MA+FGlCzovUVXnBSi9uYHEfs8tzwtmNjChGFiLhVfXaIUUzqPEgYqyq3Na6Wp/Efu0MmmQ6eM3CABU/1fIB+PQcRRgkgIGAIoO4GoC9QEmHlQAAAABAP3/j/+5BgJVAAAQXX0BANAgOQACcYQ9gQ==";
/// The same body written from a stream, so the header declares no size, which
/// is what a chunked response looks like. Only the output cap can refuse it.
const REAL_OVERSIZED_UNDECLARED: &str = "KLUv/QRoDAMAgkQRGHCrDoBU3Hh7qUirw4MA+FGlCzovUVXnBSi9uYHEfs8tzwtmNjChGFiLhVfXaIUUzqPEgYqyq3Na6Wp/Efu0MmmQ6eM3CABU/1fIB+PQcRRgkgIGAIoO4GoC9QEmHlQAAAABAP3/j/+5BgJVAAAQXX0BANAgOQACcYQ9gQ==";

fn real(base64_text: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(base64_text)
        .unwrap()
}

fn compress(json: &str) -> Vec<u8> {
    compress_to_vec(json.as_bytes(), CompressionLevel::Fastest)
}

/// `five_hour`, `seven_day` and a matching `limits` array.
fn full() -> Vec<u8> {
    compress(
        r#"{"five_hour":{"utilization":30.0,"resets_at":"2099-01-01T05:00:00.000000+00:00"},"seven_day":{"utilization":74.0,"resets_at":"2099-01-05T00:00:00.000000+00:00"},"limits":[{"kind":"session","percent":30,"resets_at":"2099-01-01T05:00:00.000000+00:00"},{"kind":"weekly_all","percent":74,"resets_at":"2099-01-05T00:00:00.000000+00:00"}]}"#,
    )
}

/// A model-scoped weekly window, the one place the response names the model
/// rather than the window.
fn scoped() -> Vec<u8> {
    compress(
        r#"{"limits":[{"kind":"session","percent":12,"resets_at":"2099-01-01T05:00:00Z"},{"kind":"weekly_scoped","percent":55,"resets_at":"2099-01-05T00:00:00Z","scope":{"model":{"display_name":"Opus"}}}]}"#,
    )
}

/// Only `five_hour`: no `limits`, no `seven_day`.
fn five_hour_only() -> Vec<u8> {
    compress(r#"{"five_hour":{"utilization":7.0,"resets_at":"2099-01-01T05:00:00Z"}}"#)
}

/// Only `seven_day`.
fn seven_day_only() -> Vec<u8> {
    compress(r#"{"seven_day":{"utilization":88.0,"resets_at":"2099-01-05T00:00:00Z"}}"#)
}

/// Valid JSON, valid zstd, and nothing a usage response would contain.
fn not_a_usage_response() -> Vec<u8> {
    compress(r#"{"hello":"world"}"#)
}

/// A session window and a `cedar_ember` block with one unused reset.
fn available_reset() -> Vec<u8> {
    compress(
        r#"{"limits":[{"kind":"session","percent":8,"resets_at":"2099-01-01T00:00:00Z"}],"cedar_ember":{"eligible":true,"grants":[{"id":"launch-reset","resets_left":1,"starts_at":"2026-09-22T16:00:00+00:00","ends_at":"2099-10-22T16:00:00+00:00","paused":false}]}}"#,
    )
}

/// The same, with no reset left.
fn spent_reset() -> Vec<u8> {
    compress(
        r#"{"limits":[{"kind":"session","percent":8,"resets_at":"2099-01-01T00:00:00Z"}],"cedar_ember":{"eligible":true,"grants":[{"id":"launch-reset","resets_left":0,"starts_at":"2026-09-22T16:00:00+00:00","ends_at":"2099-10-22T16:00:00+00:00","paused":false}]}}"#,
    )
}

/// A `cedar_ember` that isn't a block at all.
fn malformed_reset() -> Vec<u8> {
    compress(
        r#"{"limits":[{"kind":"session","percent":8,"resets_at":"2099-01-01T00:00:00Z"}],"cedar_ember":42}"#,
    )
}

/// A session window whose reset has already passed.
fn expired() -> Vec<u8> {
    compress(r#"{"limits":[{"kind":"session","percent":50,"resets_at":"2020-01-01T00:00:00Z"}]}"#)
}

fn key_for(organization: &str, query: &str) -> String {
    format!("1/0/https://claude.ai/api/organizations/{organization}/usage{query}")
}

/// `Wed, 09 Sep 2026 16:12:08 GMT`.
fn fixture_date() -> SystemTime {
    at(1_788_970_328)
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// One Chromium Simple Cache entry file, written out here rather than copied
/// from a real cache so the format assumptions are stated: the 8-byte magic,
/// the three 32-bit fields, and the four bytes of struct padding a reader that
/// counts to 20 instead of 24 lands inside the key with.
struct Entry {
    magic: u64,
    version: u32,
    /// `None` means whatever the key actually is. Set it to lie about it.
    declared_key_length: Option<u32>,
    key: String,
    body: Vec<u8>,
    /// Chromium writes the header block after the body; this is the part of it
    /// the reader looks at.
    response_date: Option<String>,
}

impl Entry {
    fn new() -> Self {
        Self {
            magic: 0xfcfb_6d1b_a772_5c30,
            version: 5,
            declared_key_length: None,
            key: key_for(ORGANIZATION, "?skip_spend=1"),
            body: full(),
            response_date: Some("Wed, 09 Sep 2026 16:12:08 GMT".into()),
        }
    }

    fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    fn key(mut self, key: String) -> Self {
        self.key = key;
        self
    }

    fn date(mut self, date: Option<&str>) -> Self {
        self.response_date = date.map(str::to_owned);
        self
    }

    fn undated(self) -> Self {
        self.date(None)
    }

    fn data(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.magic.to_le_bytes());
        out.extend_from_slice(&self.version.to_le_bytes());
        let key = self.key.as_bytes();
        out.extend_from_slice(
            &self
                .declared_key_length
                .unwrap_or(key.len() as u32)
                .to_le_bytes(),
        );
        out.extend_from_slice(&0xdead_beef_u32.to_le_bytes());
        // The padding Chromium's C++ struct carries.
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(key);
        out.extend_from_slice(&self.body);
        out.extend_from_slice(&self.trailer());
        out
    }

    /// A stand-in for the pickled `HttpResponseInfo`: the NUL-separated header
    /// block, the only part of it that is read.
    fn trailer(&self) -> Vec<u8> {
        let mut out = vec![1, 0, 0, 0];
        out.extend_from_slice(b"HTTP/1.1 200\0");
        if let Some(date) = &self.response_date {
            out.extend_from_slice(format!("date:{date}").as_bytes());
            out.push(0);
        }
        out.extend_from_slice(b"content-encoding:zstd");
        out.extend_from_slice(&[0, 0]);
        out
    }
}

/// A throwaway folder shaped like `Cache_Data`.
fn cache_with(files: &[(&str, Vec<u8>)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, bytes) in files {
        std::fs::write(dir.path().join(name), bytes).unwrap();
    }
    dir
}

fn set_modified(path: &Path, when: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

fn read(dir: &Path) -> Option<FolderReading> {
    read_folder(dir, ORGANIZATION, SystemTime::now())
}

fn file_name(reading: &FolderReading) -> String {
    reading
        .entry
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn ids(windows: &[DesktopWindow]) -> Vec<&str> {
    windows.iter().map(|w| w.id.as_str()).collect()
}

fn labels(windows: &[DesktopWindow]) -> Vec<Option<&str>> {
    windows.iter().map(|w| w.label.as_deref()).collect()
}

// MARK: - A valid entry

#[test]
fn a_valid_entry_yields_the_session_and_weekly_windows() {
    let dir = cache_with(&[("a_0", Entry::new().data())]);
    let reading = read(dir.path()).unwrap();
    assert_eq!(ids(&reading.windows), ["session", "weekly_all"]);
    // The engine's own wording (what Swift's `L10n.t` gives).
    assert_eq!(
        labels(&reading.windows),
        [Some("Current session"), Some("All models")]
    );
    assert_eq!(reading.windows[0].utilization, 30.0);
    assert_eq!(reading.windows[1].utilization, 74.0);
    assert_eq!(reading.windows[0].duration_s, 5 * 3600);
    assert_eq!(reading.windows[1].duration_s, 7 * 86_400);
    // Absolute reset times survive the round trip, which is what lets them keep
    // being used after the reading itself has gone stale.
    let expected = at(4_070_926_800); // 2099-01-01T05:00:00Z
    assert_eq!(reading.windows[0].resets_at, Some(expected));
    assert!(reading.windows[1].resets_at.is_some());
}

#[test]
fn an_entry_written_by_libzstd_parses_like_one_written_here() {
    let parsed = parse_entry(&Entry::new().body(real(REAL_FULL)).data()).unwrap();
    assert_eq!(parsed.organization, ORGANIZATION);
    let body: serde_json::Value = serde_json::from_slice(&parsed.body).unwrap();
    assert_eq!(body["five_hour"]["utilization"], 30.0);
    assert_eq!(body["limits"][1]["kind"], "weekly_all");
    assert_eq!(parsed.date, Some(fixture_date()));

    let dir = cache_with(&[("a_0", Entry::new().body(real(REAL_FULL)).data())]);
    let reading = read(dir.path()).unwrap();
    assert_eq!(ids(&reading.windows), ["session", "weekly_all"]);
    assert_eq!(reading.windows[1].utilization, 74.0);
}

#[test]
fn the_frame_walker_finds_the_end_of_real_and_written_frames() {
    // The frame is followed by trailer bytes; the walker stops at its own end.
    for body in [real(REAL_FULL), full(), scoped(), five_hour_only()] {
        let mut bytes = body.clone();
        bytes.extend_from_slice(b"\0date:whatever\0");
        let layout = frame_layout(&bytes).unwrap();
        assert_eq!(layout.size, body.len());
    }
    // A libzstd frame that declares its size, and one that doesn't.
    assert_eq!(
        frame_layout(&real(REAL_OVERSIZED_DECLARED))
            .unwrap()
            .content_size,
        Some(336_085)
    );
    assert_eq!(
        frame_layout(&real(REAL_OVERSIZED_UNDECLARED))
            .unwrap()
            .content_size,
        None
    );
}

#[test]
fn the_response_date_becomes_the_timestamp() {
    let parsed = parse_entry(&Entry::new().data()).unwrap();
    assert_eq!(parsed.date, Some(fixture_date()));
    let dir = cache_with(&[("a_0", Entry::new().data())]);
    // The header, not the file's modification time.
    set_modified(&dir.path().join("a_0"), at(1_700_000_000));
    assert_eq!(read(dir.path()).unwrap().captured_at, fixture_date());
}

/// The header block is lower-cased over HTTP/2 and HTTP/3 and left alone over
/// HTTP/1.1, and Claude Desktop negotiates all three.
#[test]
fn the_date_is_found_in_either_case() {
    for name in ["date", "Date"] {
        let mut trailer = vec![0_u8];
        trailer.extend_from_slice(format!("{name}:Wed, 09 Sep 2026 16:12:08 GMT").as_bytes());
        trailer.push(0);
        assert_eq!(http_date(&trailer), Some(fixture_date()), "{name}: unread");
    }
}

#[test]
fn an_entry_with_no_date_header_still_parses() {
    let parsed = parse_entry(&Entry::new().undated().data()).unwrap();
    // `None` here, not a guess: the caller substitutes the file's own
    // modification time, tested against the filesystem below.
    assert_eq!(parsed.date, None);
    assert!(!parsed.body.is_empty());
}

#[test]
fn an_unparseable_date_header_is_ignored_rather_than_fatal() {
    let parsed = parse_entry(&Entry::new().date(Some("not a date at all")).data()).unwrap();
    assert_eq!(parsed.date, None);
}

#[test]
fn a_date_is_read_only_in_its_one_form() {
    assert_eq!(
        parse_imf_fixdate("Wed, 09 Sep 2026 16:12:08 GMT"),
        Some(fixture_date())
    );
    assert_eq!(
        parse_imf_fixdate("  Thu, 01 Jan 1970 00:00:00 GMT "),
        Some(UNIX_EPOCH)
    );
    for text in [
        "",
        "Wed, 09 Sep 2026 16:12:08",           // no zone
        "Wed, 09 Sep 2026 16:12:08 PST",       // another zone
        "Wed, 31 Sep 2026 16:12:08 GMT",       // no such day
        "Wed, 09 Sep 2026 24:00:00 GMT",       // no such hour
        "Wed, 09 Foo 2026 16:12:08 GMT",       // no such month
        "Wed, 09 Sep 2026 16:12:08 GMT extra", // more than the form
        "Wed 09 Sep 2026 16:12:08 GMT",        // no comma
        "Mon, 09 Sep 1960 16:12:08 GMT",       // before the epoch
    ] {
        assert_eq!(parse_imf_fixdate(text), None, "accepted {text:?}");
    }
}

#[test]
fn a_date_is_looked_for_after_the_body_only() {
    // A `date:` inside the JSON body is not the response's date.
    let body = compress(
        r#"{"limits":[{"kind":"session","percent":1,"resets_at":"2099-01-01T05:00:00Z"}],"x":"\u0000date:Wed, 09 Sep 2026 16:12:08 GMT\u0000"}"#,
    );
    let parsed = parse_entry(&Entry::new().body(body).undated().data()).unwrap();
    assert_eq!(parsed.date, None);
}

// MARK: - Windows the response may or may not carry

#[test]
fn five_hour_alone_is_a_session_window() {
    let dir = cache_with(&[("a_0", Entry::new().body(five_hour_only()).data())]);
    let reading = read(dir.path()).unwrap();
    assert_eq!(ids(&reading.windows), ["session"]);
    assert_eq!(reading.windows[0].utilization, 7.0);
}

#[test]
fn seven_day_alone_is_the_all_models_window() {
    let dir = cache_with(&[("a_0", Entry::new().body(seven_day_only()).data())]);
    let reading = read(dir.path()).unwrap();
    assert_eq!(ids(&reading.windows), ["weekly_all"]);
    assert_eq!(reading.windows[0].utilization, 88.0);
}

/// `weekly_scoped` comes back for whichever model the plan scopes, so the kind
/// alone can only say "Scoped": the name is in `scope.model`.
#[test]
fn a_scoped_weekly_window_keeps_the_model_name() {
    let dir = cache_with(&[("a_0", Entry::new().body(scoped()).data())]);
    let reading = read(dir.path()).unwrap();
    assert_eq!(ids(&reading.windows), ["session", "weekly_scoped"]);
    assert_eq!(
        labels(&reading.windows),
        [Some("Current session"), Some("Opus")]
    );
    assert_eq!(reading.windows[1].utilization, 55.0);
    assert_eq!(reading.windows[1].duration_s, 7 * 86_400);
}

#[test]
fn a_window_that_rolled_out_of_limits_is_merged_in_from_the_named_field() {
    // `limits` no longer lists the session (its reset passed) while
    // `five_hour` still carries it; the weekly one is only in `limits`.
    let body = compress(
        r#"{"five_hour":{"utilization":3.0,"resets_at":"2099-01-01T05:00:00Z"},"limits":[{"kind":"weekly_sonnet","percent":40,"resets_at":"2099-01-05T00:00:00Z"},{"kind":"weekly_all","percent":10,"resets_at":"2099-01-05T00:00:00Z"}]}"#,
    );
    let dir = cache_with(&[("a_0", Entry::new().body(body).data())]);
    let reading = read(dir.path()).unwrap();
    // Session, weekly_all, then the rest by id.
    assert_eq!(
        ids(&reading.windows),
        ["session", "weekly_all", "weekly_sonnet"]
    );
    assert_eq!(reading.windows[0].utilization, 3.0);
    assert_eq!(reading.windows[2].label.as_deref(), Some("Sonnet"));
}

#[test]
fn a_limit_without_a_reset_time_is_not_a_window() {
    let body = compress(r#"{"limits":[{"kind":"session","percent":30}]}"#);
    let dir = cache_with(&[("a_0", Entry::new().body(body).data())]);
    assert!(read(dir.path()).is_none());
}

#[test]
fn a_bad_element_costs_the_whole_response() {
    // As on the Mac: `limits` is the reading itself and must decode.
    for json in [
        r#"{"limits":[{"kind":"session","percent":"30","resets_at":"2099-01-01T05:00:00Z"}]}"#,
        r#"{"limits":[{"percent":30,"resets_at":"2099-01-01T05:00:00Z"}]}"#,
        r#"{"limits":[{"kind":"session","percent":30,"resets_at":"2099-01-01T05:00:00Z"},7]}"#,
        r#"{"limits":[{"kind":"session","percent":30,"resets_at":"yesterday"}]}"#,
        r#"{"limits":"none","five_hour":{"utilization":3.0,"resets_at":"2099-01-01T05:00:00Z"}}"#,
        r#"{"five_hour":{"resets_at":"2099-01-01T05:00:00Z"}}"#,
        r#"[{"limits":[]}]"#,
    ] {
        let dir = cache_with(&[("a_0", Entry::new().body(compress(json)).data())]);
        assert!(read(dir.path()).is_none(), "read {json}");
    }
}

#[test]
fn a_malformed_scope_only_costs_the_name() {
    let body = compress(
        r#"{"limits":[{"kind":"weekly_opus","percent":5,"resets_at":"2099-01-05T00:00:00Z","scope":7}]}"#,
    );
    let dir = cache_with(&[("a_0", Entry::new().body(body).data())]);
    let reading = read(dir.path()).unwrap();
    assert_eq!(reading.windows[0].label.as_deref(), Some("Opus"));
}

// MARK: - Keys

#[test]
fn the_usage_key_is_recognised_with_and_without_a_query() {
    let base = key_for(ORGANIZATION, "");
    for key in [
        base.clone(),
        format!("{base}?skip_spend=1"),
        format!("{base}?a=1&b=2"),
        format!("{base}#fragment"),
    ] {
        assert_eq!(
            usage_organization(&key).as_deref(),
            Some(ORGANIZATION),
            "did not recognise {key}"
        );
    }
}

#[test]
fn keys_that_are_not_the_usage_endpoint_are_ignored() {
    for key in [
        // Another endpoint under the same organization.
        format!("1/0/https://claude.ai/api/organizations/{ORGANIZATION}/projects"),
        // The usage endpoint with something after it.
        format!("1/0/https://claude.ai/api/organizations/{ORGANIZATION}/usage/detail"),
        // No organization segment at all.
        "1/0/https://claude.ai/api/usage".to_owned(),
        // An empty organization.
        "1/0/https://claude.ai/api/organizations//usage".to_owned(),
        // The right shape on the wrong host. Without this check the reader
        // would decompress any site's body that matched the path.
        format!("1/0/https://example.com/api/organizations/{ORGANIZATION}/usage"),
        String::new(),
        "not a url".to_owned(),
    ] {
        assert_eq!(usage_organization(&key), None, "wrongly accepted {key}");
    }
    // Anthropic's own domain is as good as claude.ai.
    assert_eq!(
        usage_organization("1/0/https://api.anthropic.com/api/organizations/o/usage").as_deref(),
        Some("o")
    );
}

/// The key is read out of a prefix of the file before anything else is read,
/// so the thousands of unrelated entries in the folder are rejected on their
/// URL and never pulled into memory whole.
#[test]
fn the_key_is_readable_from_a_prefix_of_the_entry() {
    let whole = Entry::new().data();
    let prefix = &whole[..whole.len().min(HEADER_BYTES + MAX_KEY_BYTES)];
    assert_eq!(entry_key(prefix), Some(Entry::new().key));
    let short = &whole[..HEADER_BYTES + Entry::new().key.len()];
    assert_eq!(entry_key(short), Some(Entry::new().key));
}

#[test]
fn key_reading_refuses_anything_it_cannot_trust() {
    // Too short, wrong magic, and a length that runs past the buffer.
    assert_eq!(entry_key(&[]), None);
    assert_eq!(entry_key(&[0; HEADER_BYTES]), None);
    let mut wrong_magic = Entry::new();
    wrong_magic.magic = 0;
    assert_eq!(entry_key(&wrong_magic.data()), None);
    let mut overlong = Entry::new();
    overlong.declared_key_length = Some(1 << 20);
    assert_eq!(entry_key(&overlong.data()), None);
    // A key that isn't UTF-8.
    let mut bytes = Entry::new().data();
    bytes[HEADER_BYTES] = 0xff;
    assert_eq!(entry_key(&bytes), None);
}

#[test]
fn an_entry_whose_key_is_not_a_usage_request_is_ignored() {
    let entry = Entry::new().key(format!(
        "1/0/https://claude.ai/api/organizations/{ORGANIZATION}/projects"
    ));
    assert!(parse_entry(&entry.data()).is_none());
}

#[test]
fn the_cedar_ember_query_marks_a_reset_request() {
    let asks = |query: &str| {
        parse_entry(&Entry::new().key(key_for(ORGANIZATION, query)).data())
            .unwrap()
            .requests_reset_credits
    };
    assert!(asks("?cedar_ember=1"));
    assert!(asks("?skip_spend=1&cedar_ember=1"));
    assert!(!asks(""));
    assert!(!asks("?skip_spend=1"));
    assert!(!asks("?cedar_ember=0"));
    assert!(!asks("?cedar_ember=10"));
}

// MARK: - Malformed entries

#[test]
fn a_file_without_the_simple_cache_magic_is_ignored() {
    let mut entry = Entry::new();
    entry.magic = 0x0123_4567_89ab_cdef;
    assert!(parse_entry(&entry.data()).is_none());
}

#[test]
fn an_empty_or_tiny_file_is_ignored() {
    assert!(parse_entry(&[]).is_none());
    assert!(parse_entry(&[0; 4]).is_none());
    assert!(parse_entry(&[0; HEADER_BYTES]).is_none());
}

/// The key length is read out of the file, so it is corruption- or
/// attacker-controlled. It must never be believed far enough to read past the
/// buffer.
#[test]
fn a_key_length_beyond_the_entry_is_ignored() {
    for declared in [0, 1 << 20, u32::MAX, MAX_KEY_BYTES as u32 + 1] {
        let mut entry = Entry::new();
        entry.declared_key_length = Some(declared);
        assert!(
            parse_entry(&entry.data()).is_none(),
            "believed a key length of {declared}"
        );
    }
}

#[test]
fn an_entry_with_no_body_is_ignored() {
    assert!(parse_entry(&Entry::new().body(Vec::new()).undated().data()).is_none());
}

#[test]
fn a_body_that_is_not_zstd_is_ignored() {
    // Plausible uncompressed JSON: what a `content-encoding: identity` response
    // would leave here.
    let entry = Entry::new().body(br#"{"five_hour":{"utilization":30.0}}"#.to_vec());
    assert!(parse_entry(&entry.data()).is_none());
}

#[test]
fn a_corrupt_zstd_frame_is_ignored() {
    let mut body = full();
    // Keep the magic, so the reader gets as far as the decoder, and wreck
    // everything after it.
    for byte in &mut body[4..] {
        *byte ^= 0xff;
    }
    assert!(parse_entry(&Entry::new().body(body).data()).is_none());
}

/// A prefix that stops inside the compressed frame cannot decode, and must say
/// so rather than return half a body.
#[test]
fn an_entry_truncated_inside_the_frame_is_ignored() {
    let whole = Entry::new().data();
    let body_start = HEADER_BYTES + Entry::new().key.len();
    // Just the frame magic, and a little way past it: in every case there is no
    // complete frame to find.
    for length in [body_start, body_start + 4, body_start + 16, body_start + 40] {
        assert!(
            parse_entry(&whole[..length]).is_none(),
            "a {length}-byte prefix decoded a frame that is not all there"
        );
    }
    // Also a frame missing only its last byte.
    let frame = full();
    let short = Entry::new().body(frame[..frame.len() - 1].to_vec()).data();
    let end = HEADER_BYTES + Entry::new().key.len() + frame.len() - 1;
    assert!(parse_entry(&short[..end]).is_none());
}

/// Every prefix of a real entry: what a half-written or evicted file looks
/// like. Once the frame is complete a prefix legitimately parses, because the
/// header block after it is not needed; what must never happen is a panic or a
/// body that differs from the whole entry's.
#[test]
fn no_prefix_of_an_entry_panics_or_parses_to_something_else() {
    for body in [full(), real(REAL_FULL)] {
        let whole = Entry::new().body(body).data();
        let expected = parse_entry(&whole).unwrap().body;
        for length in 0..=whole.len() {
            if let Some(parsed) = parse_entry(&whole[..length]) {
                assert_eq!(
                    parsed.body, expected,
                    "a {length}-byte prefix produced a different body"
                );
                assert_eq!(parsed.organization, ORGANIZATION);
            }
        }
    }
}

#[test]
fn valid_zstd_carrying_something_other_than_usage_is_ignored() {
    let entry = Entry::new().body(not_a_usage_response());
    // Parsing gets as far as a body, the frame being genuinely valid, and the
    // reader rejects it a step later for naming no window.
    assert!(parse_entry(&entry.data()).is_some());
    let dir = cache_with(&[("a_0", entry.data())]);
    assert!(read(dir.path()).is_none());
}

#[test]
fn frames_that_are_not_shaped_like_frames_are_refused() {
    assert!(frame_layout(&[]).is_none());
    assert!(frame_layout(&[0x28, 0xb5, 0x2f, 0xfd]).is_none());
    // The reserved bit of the frame header descriptor.
    assert!(frame_layout(&[0x28, 0xb5, 0x2f, 0xfd, 0b0010_1000, 0, 0, 0, 0, 0]).is_none());
    // A reserved block type (3), as the first and last block.
    assert!(frame_layout(&[0x28, 0xb5, 0x2f, 0xfd, 0b0010_0000, 0, 0b111, 0, 0]).is_none());
    // A window beyond what web content may use (2^31 bytes).
    assert!(frame_layout(&[0x28, 0xb5, 0x2f, 0xfd, 0, 21 << 3, 1, 0, 0]).is_none());
    // A raw last block of zero bytes: an empty frame the size of its headers.
    let empty = [0x28, 0xb5, 0x2f, 0xfd, 0b0010_0000, 0, 1, 0, 0];
    assert_eq!(frame_layout(&empty).unwrap().size, empty.len());
    // The same frame that claims a checksum it doesn't have.
    let no_checksum = [0x28, 0xb5, 0x2f, 0xfd, 0b0010_0100, 0, 1, 0, 0];
    assert!(frame_layout(&no_checksum).is_none());
}

#[test]
fn a_frame_that_decodes_to_nothing_is_not_a_usage_body() {
    let empty = vec![0x28, 0xb5, 0x2f, 0xfd, 0b0010_0000, 0, 1, 0, 0];
    assert!(parse_entry(&Entry::new().body(empty).data()).is_none());
}

// MARK: - Size caps

#[test]
fn a_declared_oversized_body_is_refused_before_it_is_decompressed() {
    let entry = Entry::new().body(real(REAL_OVERSIZED_DECLARED));
    assert!(parse_entry(&entry.data()).is_none());
}

/// The case that matters, because a chunked response declares no size at all:
/// the only thing standing between a 330 kB body and the cap is the limit put
/// on the decoder's output.
#[test]
fn an_undeclared_oversized_body_is_refused_by_the_output_cap() {
    let entry = Entry::new().body(real(REAL_OVERSIZED_UNDECLARED));
    assert!(parse_entry(&entry.data()).is_none());
}

#[test]
fn a_body_written_here_over_the_cap_is_refused_and_one_at_it_is_not() {
    // Valid JSON of exactly the cap, then one byte over: only the second fails.
    let json = |size: usize| {
        let head = r#"{"five_hour":{"utilization":1.0,"resets_at":"2099-01-01T05:00:00Z"},"pad":""#;
        let tail = r#""}"#;
        let pad = size - head.len() - tail.len();
        format!("{head}{}{tail}", "x".repeat(pad))
    };
    let at_cap = Entry::new().body(compress(&json(256 * 1024)));
    assert_eq!(parse_entry(&at_cap.data()).unwrap().body.len(), 256 * 1024);
    let over = Entry::new().body(compress(&json(256 * 1024 + 1)));
    assert!(parse_entry(&over.data()).is_none());
}

#[test]
fn an_oversized_entry_file_is_never_opened() {
    // Bigger than the per-entry cap, and otherwise a perfectly good entry.
    let entry = Entry::new().key(key_for(ORGANIZATION, "?skip_spend=1&pad=x"));
    let mut bytes = entry.data();
    bytes.resize(MAX_ENTRY_BYTES + 1, 0);
    let dir = cache_with(&[("big_0", bytes)]);
    assert!(read(dir.path()).is_none());
    // The same entry padded to exactly the cap is read.
    let mut bytes = entry.data();
    bytes.resize(MAX_ENTRY_BYTES, 0);
    let dir = cache_with(&[("big_0", bytes)]);
    assert!(read(dir.path()).is_some());
}

// MARK: - Scanning a folder

/// Whatever else is wrong, an absent Claude Desktop is not an error.
#[test]
fn a_missing_cache_folder_reads_as_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read(&dir.path().join("absent")).is_none());
}

#[test]
fn an_empty_cache_folder_reads_as_nothing() {
    let dir = cache_with(&[]);
    assert!(read(dir.path()).is_none());
}

#[test]
fn files_that_are_not_cache_entries_are_ignored() {
    let dir = cache_with(&[
        ("the-real-index", vec![0x7f; 4096]),
        ("notanentry", b"hello".to_vec()),
        // A stream file that is not `_0`.
        ("somefile_1", Entry::new().data()),
        // A hidden file.
        (".hidden_0", Entry::new().data()),
    ]);
    assert!(read(dir.path()).is_none());
}

#[test]
fn a_folder_named_like_an_entry_is_not_an_entry() {
    let dir = cache_with(&[]);
    std::fs::create_dir(dir.path().join("0123456789abcdef_0")).unwrap();
    assert!(read(dir.path()).is_none());
}

#[test]
fn an_entry_for_another_organization_is_ignored() {
    let dir = cache_with(&[("a_0", Entry::new().data())]);
    // The reading exists, and it is not this account's: Claude Desktop is
    // signed in to one account while a ring is drawn per account, so the wrong
    // account's session percentage must not reach the wrong ring.
    assert!(read_folder(dir.path(), OTHER_ORGANIZATION, SystemTime::now()).is_none());
    assert!(read_folder(dir.path(), "", SystemTime::now()).is_none());
}

#[test]
fn the_most_recent_of_several_usage_entries_wins() {
    // Both keys occur at once: Claude Desktop asks for `…/usage` and
    // `…/usage?skip_spend=1`, and they hash to two different files.
    let older = Entry::new()
        .key(key_for(ORGANIZATION, ""))
        .body(five_hour_only()) // 7%
        .undated();
    let newer = Entry::new().body(full()).undated(); // 30%
    let dir = cache_with(&[("older_0", older.data()), ("newer_0", newer.data())]);
    set_modified(&dir.path().join("older_0"), at(1_000_000));
    set_modified(&dir.path().join("newer_0"), at(2_000_000));

    let reading = read(dir.path()).unwrap();
    assert_eq!(reading.windows[0].utilization, 30.0);
    assert_eq!(file_name(&reading), "newer_0");
}

/// With no `Date:` header the entry's own modification time stands in, and it
/// has to be the file's, not "now", or a cache Desktop stopped writing hours
/// ago would read as current forever.
#[test]
fn with_no_date_header_the_files_modification_time_is_used() {
    let dir = cache_with(&[("a_0", Entry::new().undated().data())]);
    let written = at(1_700_000_000);
    set_modified(&dir.path().join("a_0"), written);
    let reading = read_folder(dir.path(), ORGANIZATION, at(1_900_000_000)).unwrap();
    assert_eq!(reading.captured_at, written);
}

#[test]
fn with_neither_a_date_header_nor_a_usable_modification_time_it_is_now() {
    // The mtime is the same handle's; there is no way to make a file without
    // one here, so the order of preference is pinned with both present instead:
    // the header beats the file, the file beats `now`.
    let dir = cache_with(&[("a_0", Entry::new().data())]);
    set_modified(&dir.path().join("a_0"), at(1_700_000_000));
    let reading = read_folder(dir.path(), ORGANIZATION, at(1_900_000_000)).unwrap();
    assert_eq!(reading.captured_at, fixture_date());
}

/// Chromium evicts entries while we are looking at them. An entry that cannot
/// be read is not an error and must not stop the scan finding the one that is.
#[cfg(unix)]
#[test]
fn an_entry_that_cannot_be_read_is_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let doomed = Entry::new().undated();
    let survivor = Entry::new().body(five_hour_only()).undated();
    let dir = cache_with(&[("doomed_0", doomed.data()), ("survivor_0", survivor.data())]);
    // The unreadable one is newer, so the scan reaches it first.
    set_modified(&dir.path().join("doomed_0"), at(2_000_000));
    set_modified(&dir.path().join("survivor_0"), at(1_000_000));
    let doomed_path = dir.path().join("doomed_0");
    std::fs::set_permissions(&doomed_path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A process that may read anyway (root) cannot make this test's point.
    let readable = std::fs::File::open(&doomed_path).is_ok();

    let reading = read(dir.path());
    std::fs::set_permissions(&doomed_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let reading = reading.unwrap();
    assert_eq!(
        file_name(&reading),
        if readable { "doomed_0" } else { "survivor_0" }
    );
}

/// An entry that goes away between the listing and the read is skipped: here
/// a dangling link stands for it (a listing names it, opening it fails).
#[cfg(unix)]
#[test]
fn an_entry_that_vanishes_during_the_scan_is_skipped() {
    let survivor = Entry::new().body(five_hour_only()).undated();
    let dir = cache_with(&[("survivor_0", survivor.data())]);
    std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("doomed_0")).unwrap();
    assert_eq!(file_name(&read(dir.path()).unwrap()), "survivor_0");
}

// MARK: - Fresh, stale, unavailable

fn read_modified(modified: SystemTime) -> FolderReading {
    let dir = cache_with(&[("a_0", Entry::new().undated().data())]);
    set_modified(&dir.path().join("a_0"), modified);
    read(dir.path()).unwrap()
}

#[test]
fn a_recent_snapshot_is_fresh() {
    let now = at(1_800_000_000);
    let reading = read_modified(now);
    assert!(reading.is_fresh(now, Duration::from_secs(30 * 60)));
}

#[test]
fn a_snapshot_older_than_the_window_is_not_fresh() {
    let now = at(1_800_000_000);
    let captured = now - Duration::from_secs(90 * 60);
    let reading = read_modified(captured);
    assert!(!reading.is_fresh(now, Duration::from_secs(30 * 60)));
    // Still a reading, with its real age and its absolute reset times: the
    // caller may still say "as of an hour ago", it just may not say "now".
    assert_eq!(reading.captured_at, captured);
    assert!(reading.windows[0].resets_at.is_some());
}

/// The `Date:` header is the server's clock, so it can read slightly ahead of
/// ours. A reading from "the future" is two clocks disagreeing, not a reason to
/// call it infinitely stale.
#[test]
fn a_snapshot_slightly_in_the_future_is_still_fresh() {
    let now = at(1_800_000_000);
    let reading = read_modified(now + Duration::from_secs(60));
    assert!(reading.is_fresh(now, Duration::from_secs(30 * 60)));
    let far = read_modified(now + Duration::from_secs(3 * 3600));
    assert!(!far.is_fresh(now, Duration::from_secs(30 * 60)));
}

// MARK: - The two alternating keys, and reset blocks

fn reset_entry(body: Vec<u8>) -> Entry {
    Entry::new()
        .key(key_for(ORGANIZATION, "?skip_spend=1&cedar_ember=1"))
        .body(body)
        .undated()
}

fn credits(resets: &DesktopResets, now: SystemTime) -> Option<i64> {
    resets.credits(now).map(|c| c.available_count)
}

#[test]
fn reset_metadata_survives_a_newer_plain_usage_reading_without_changing_its_windows() {
    let now = at(1_800_000_000);
    let dir = cache_with(&[
        ("reset_0", reset_entry(available_reset()).data()),
        ("plain_0", Entry::new().undated().data()),
    ]);
    set_modified(&dir.path().join("reset_0"), now - Duration::from_secs(60));
    set_modified(&dir.path().join("plain_0"), now);

    let reading = read_folder(dir.path(), ORGANIZATION, now).unwrap();
    assert_eq!(file_name(&reading), "plain_0");
    assert_eq!(reading.windows[0].utilization, 30.0);
    let resets = reading.resets.unwrap();
    assert_eq!(credits(&resets, now), Some(1));
    // A three-hour-old observation is still drawn, with its date: keeping it
    // is the point.
    let later = now + Duration::from_secs(3 * 3600);
    let grant = resets.credits(later).unwrap();
    assert_eq!(grant.available_count, 1);
    assert_eq!(
        grant.checked_at,
        Some(now - Duration::from_secs(60)),
        "newer usage must not re-date the cached reset observation"
    );
    // Only a capture from the future (a clock that jumped) is refused.
    assert_eq!(credits(&resets, now - Duration::from_secs(3 * 3600)), None);
    assert!(read_folder(dir.path(), "other-account", now).is_none());
}

#[test]
fn spent_or_malformed_reset_metadata_supersedes_older_available_grants() {
    for body in [spent_reset(), malformed_reset(), full()] {
        let now = at(1_800_000_000);
        let older = Entry::new().body(available_reset()).undated();
        let newer = reset_entry(body);
        let dir = cache_with(&[("older_0", older.data()), ("newer_0", newer.data())]);
        set_modified(&dir.path().join("older_0"), now - Duration::from_secs(60));
        set_modified(&dir.path().join("newer_0"), now);
        let reading = read_folder(dir.path(), ORGANIZATION, now).unwrap();
        let resets = reading
            .resets
            .expect("a newer block supersedes the older one");
        assert_eq!(credits(&resets, now).unwrap_or(0), 0);
    }
}

#[test]
fn an_ordinary_response_with_no_block_has_no_resets() {
    let dir = cache_with(&[("a_0", Entry::new().undated().data())]);
    assert_eq!(read(dir.path()).unwrap().resets, None);
}

#[test]
fn a_null_block_on_a_plain_request_is_no_observation() {
    let body = compress(
        r#"{"limits":[{"kind":"session","percent":8,"resets_at":"2099-01-01T00:00:00Z"}],"cedar_ember":null}"#,
    );
    let dir = cache_with(&[("a_0", Entry::new().body(body).undated().data())]);
    assert_eq!(read(dir.path()).unwrap().resets, None);
    // The same body asked for with `cedar_ember=1` is an observation of "none".
    let dir = cache_with(&[("a_0", reset_entry(compress(
        r#"{"limits":[{"kind":"session","percent":8,"resets_at":"2099-01-01T00:00:00Z"}],"cedar_ember":null}"#,
    )).data())]);
    let resets = read(dir.path()).unwrap().resets.unwrap();
    assert_eq!(resets.value, None);
}

#[test]
fn a_grant_that_does_not_decode_is_dropped_and_the_rest_kept() {
    let body = compress(
        r#"{"limits":[{"kind":"session","percent":8,"resets_at":"2099-01-01T00:00:00Z"}],"cedar_ember":{"eligible":true,"grants":[{"id":"bad"},{"id":"ok","resets_left":2,"starts_at":"2026-09-22T16:00:00Z","ends_at":"2099-10-22T16:00:00Z","paused":false}]}}"#,
    );
    let dir = cache_with(&[("a_0", reset_entry(body).data())]);
    let resets = read(dir.path()).unwrap().resets.unwrap();
    assert_eq!(resets.value.as_ref().unwrap().grants.len(), 1);
    assert_eq!(credits(&resets, at(1_800_000_000)), Some(2));
}

/// Entries that are not this account's still take up an index: the reset search
/// is bounded by how many candidates were looked at, not by how many matched.
#[test]
fn the_reset_search_stops_twenty_entries_past_the_newest_usable_one() {
    // `total` entries, newest first; `usable` are this account's (at those
    // indexes), the rest another account's. `reset` marks the one carrying a
    // block. Indexes are the scan order.
    fn scan(total: usize, usable: &[usize], reset: usize) -> FolderReading {
        let dir = cache_with(&[]);
        for index in 0..total {
            let entry = if index == reset {
                reset_entry(available_reset())
            } else if usable.contains(&index) {
                Entry::new().undated()
            } else {
                Entry::new().key(key_for(OTHER_ORGANIZATION, "")).undated()
            };
            let name = format!("e{index:03}_0");
            std::fs::write(dir.path().join(&name), entry.data()).unwrap();
            set_modified(&dir.path().join(&name), at(2_000_000 - index as u64 * 10));
        }
        read_folder(dir.path(), ORGANIZATION, at(1_800_000_000)).unwrap()
    }
    // A block a few writes behind the newest usable entry is found.
    assert!(scan(30, &[0, 10], 10).resets.is_some());
    // At index 20 the first check is for a block, so it is still found.
    assert!(scan(30, &[0, 20], 20).resets.is_some());
    // The first usable entry past index 20 ends the search before a later one.
    assert!(scan(40, &[0, 21, 25], 25).resets.is_none());
    // The windows are the newest usable entry's either way.
    assert_eq!(
        file_name(&scan(30, &[0, 10], 10)),
        "e000_0",
        "windows come from the newest entry"
    );
}

/// The regression the Mac reader hit on a real cache. Desktop asks for both
/// `…/usage` and `…/usage?skip_spend=1` (two keys, two files) and refreshes
/// them independently. Every call must find the true newest across both keys,
/// not just revalidate whichever one it saw last.
#[test]
fn the_newer_of_the_two_alternating_keys_wins_even_when_both_are_fresh() {
    let bare = Entry::new()
        .key(key_for(ORGANIZATION, ""))
        .body(full()) // 30%
        .undated();
    let skip = Entry::new().body(five_hour_only()).undated(); // 7%
    let dir = cache_with(&[("bare_0", bare.data()), ("scoped_0", skip.data())]);
    let now = at(1_800_000_000);
    set_modified(&dir.path().join("scoped_0"), now - Duration::from_secs(300));
    set_modified(&dir.path().join("bare_0"), now);

    let reading = read_folder(dir.path(), ORGANIZATION, now).unwrap();
    assert_eq!(file_name(&reading), "bare_0");
    assert_eq!(reading.windows[0].utilization, 30.0);
}

/// And a call made shortly after must find the new newest once the keys swap:
/// nothing may be remembered between calls that could pin the answer to
/// whichever file happened to win last time.
#[test]
fn a_second_call_finds_a_newly_updated_key() {
    let bare_key = key_for(ORGANIZATION, "");
    let bare = Entry::new()
        .key(bare_key.clone())
        .body(five_hour_only()) // 7%, older
        .undated();
    let skip = Entry::new().body(full()).undated(); // 30%, older too
    let dir = cache_with(&[("bare_0", bare.data()), ("scoped_0", skip.data())]);
    let now = at(1_800_000_000);
    set_modified(&dir.path().join("bare_0"), now - Duration::from_secs(120));
    set_modified(&dir.path().join("scoped_0"), now - Duration::from_secs(119));

    let first = read_folder(dir.path(), ORGANIZATION, now).unwrap();
    assert_eq!(file_name(&first), "scoped_0");

    // "bare" is refreshed with a new value and becomes the newest entry.
    let updated = Entry::new().key(bare_key).body(scoped()).undated(); // 12%
    std::fs::write(dir.path().join("bare_0"), updated.data()).unwrap();
    set_modified(&dir.path().join("bare_0"), now);
    let second = read_folder(dir.path(), ORGANIZATION, now).unwrap();
    assert_eq!(file_name(&second), "bare_0");
    assert_eq!(second.windows[0].utilization, 12.0);
}

// MARK: - read_desktop_cache over the roots

/// Roots in the Windows layout with Claude Desktop's usual folder, and the
/// MSIX package's as a second one.
struct Desktop {
    /// Kept so the folders live as long as the test.
    _base: tempfile::TempDir,
    roots: Roots,
}

impl Desktop {
    fn new() -> Self {
        let base = tempfile::tempdir().unwrap();
        let mut roots = windows_roots(base.path());
        let msix = join(
            &roots.home,
            &[
                "AppData",
                "Local",
                "Packages",
                "Claude_abc123",
                "LocalCache",
                "Roaming",
                "Claude",
            ],
        );
        roots.claude_desktop.push(msix);
        Self { _base: base, roots }
    }

    /// The `Cache_Data` folder of the usual root (0) or the MSIX one (1).
    fn cache(&self, root: usize) -> PathBuf {
        let dir = cache_folder(&self.roots.claude_desktop[root]);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(&self, root: usize, name: &str, bytes: &[u8], modified: u64) {
        let path = self.cache(root).join(name);
        std::fs::write(&path, bytes).unwrap();
        set_modified(&path, at(modified));
    }

    fn read(&self, organization: &str) -> DesktopReading {
        read_desktop_cache(&self.roots, organization, at(1_800_000_000))
    }
}

fn reading_of(result: DesktopReading) -> (Vec<DesktopWindow>, SystemTime, Option<DesktopResets>) {
    match result {
        DesktopReading::Reading {
            windows,
            observed_at,
            resets,
            organization_uuid,
        } => {
            assert_eq!(organization_uuid, ORGANIZATION);
            (windows, observed_at, resets)
        }
        other => panic!("not a reading: {other:?}"),
    }
}

#[test]
fn the_cache_folder_is_cache_cache_data_under_the_root() {
    let root = Path::new("root");
    assert_eq!(
        cache_folder(root),
        Path::new("root").join("Cache").join("Cache_Data")
    );
}

#[test]
fn a_reading_carries_the_organization_windows_and_date() {
    let desktop = Desktop::new();
    desktop.put(0, "0123456789abcdef_0", &Entry::new().data(), 1_700_000_000);
    let (windows, observed_at, resets) = reading_of(desktop.read(ORGANIZATION));
    assert_eq!(ids(&windows), ["session", "weekly_all"]);
    assert_eq!(observed_at, fixture_date());
    assert_eq!(resets, None);
    // Standard labels are dropped, so the ring follows the app's language.
    assert_eq!(labels(&windows), [None, None]);
}

#[test]
fn a_scoped_label_is_kept_and_a_standard_one_dropped() {
    let desktop = Desktop::new();
    desktop.put(
        0,
        "0123456789abcdef_0",
        &Entry::new().body(scoped()).data(),
        1,
    );
    let (windows, ..) = reading_of(desktop.read(ORGANIZATION));
    assert_eq!(labels(&windows), [None, Some("Opus")]);
}

#[test]
fn a_model_named_like_the_standard_label_loses_its_label_too() {
    // A `weekly_opus` window says "Opus", which is what the ring would call it.
    let body = compress(
        r#"{"limits":[{"kind":"weekly_opus","percent":5,"resets_at":"2099-01-05T00:00:00Z","scope":{"model":{"display_name":"Opus"}}}]}"#,
    );
    let desktop = Desktop::new();
    desktop.put(0, "0123456789abcdef_0", &Entry::new().body(body).data(), 1);
    let (windows, ..) = reading_of(desktop.read(ORGANIZATION));
    assert_eq!(ids(&windows), ["weekly_opus"]);
    assert_eq!(labels(&windows), [None]);
}

#[test]
fn a_window_that_has_already_reset_leaves_no_reading() {
    let desktop = Desktop::new();
    desktop.put(
        0,
        "0123456789abcdef_0",
        &Entry::new().body(expired()).data(),
        1_799_999_000,
    );
    assert_eq!(
        desktop.read(ORGANIZATION),
        DesktopReading::NotFound {
            format: DesktopCacheFormat::Simple
        }
    );
    // One expired window makes the whole response out of date, however
    // recently it was written.
    let mixed = compress(
        r#"{"limits":[{"kind":"session","percent":50,"resets_at":"2020-01-01T00:00:00Z"},{"kind":"weekly_all","percent":50,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
    );
    let other = Desktop::new();
    other.put(0, "0123456789abcdef_0", &Entry::new().body(mixed).data(), 2);
    assert!(matches!(
        other.read(ORGANIZATION),
        DesktopReading::NotFound { .. }
    ));
}

#[test]
fn an_empty_or_absent_folder_is_not_found_absent() {
    let absent = Desktop::new();
    assert_eq!(
        absent.read(ORGANIZATION),
        DesktopReading::NotFound {
            format: DesktopCacheFormat::Absent
        }
    );
    let empty = Desktop::new();
    empty.cache(0);
    assert_eq!(
        empty.read(ORGANIZATION),
        DesktopReading::NotFound {
            format: DesktopCacheFormat::Absent
        }
    );
    // No roots at all.
    let mut none = Desktop::new();
    none.roots.claude_desktop.clear();
    assert_eq!(
        none.read(ORGANIZATION),
        DesktopReading::NotFound {
            format: DesktopCacheFormat::Absent
        }
    );
}

#[test]
fn another_organizations_entry_is_ignored() {
    let desktop = Desktop::new();
    desktop.put(0, "0123456789abcdef_0", &Entry::new().data(), 1);
    assert_eq!(
        desktop.read(OTHER_ORGANIZATION),
        DesktopReading::NotFound {
            format: DesktopCacheFormat::Simple
        }
    );
    assert_eq!(
        desktop.read(""),
        DesktopReading::NotFound {
            format: DesktopCacheFormat::Simple
        }
    );
}

fn blockfile(desktop: &Desktop, root: usize) {
    for name in ["index", "data_0", "data_1", "data_2", "data_3", "f_000001"] {
        // data_0 is the block file with the allocation bitmaps: it is larger
        // than an entry header and named `_0`, and must not be read as one.
        desktop.put(
            root,
            name,
            &[
                0xc3, 0xca, 0x04, 0xc1, 0, 0, 0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
                7, 7, 7, 7, 7, 7, 7, 7,
            ],
            1,
        );
    }
}

#[test]
fn a_blockfile_cache_is_reported_as_an_unsupported_format() {
    let desktop = Desktop::new();
    blockfile(&desktop, 0);
    assert_eq!(
        desktop.read(ORGANIZATION),
        DesktopReading::UnsupportedFormat
    );
    assert_eq!(
        desktop_cache_format(&desktop.roots),
        DesktopCacheFormat::Blockfile
    );
}

#[test]
fn a_readable_cache_on_another_root_beats_a_blockfile_one() {
    let desktop = Desktop::new();
    blockfile(&desktop, 0);
    desktop.put(1, "0123456789abcdef_0", &Entry::new().data(), 1);
    let (windows, ..) = reading_of(desktop.read(ORGANIZATION));
    assert_eq!(ids(&windows), ["session", "weekly_all"]);
    // With another organization asked, the readable cache says "not found".
    assert_eq!(
        desktop.read(OTHER_ORGANIZATION),
        DesktopReading::NotFound {
            format: DesktopCacheFormat::Simple
        }
    );
    assert_eq!(
        desktop_cache_format(&desktop.roots),
        DesktopCacheFormat::Simple
    );
}

#[test]
fn two_roots_the_newest_reading_wins_and_the_msix_one_is_second() {
    let desktop = Desktop::new();
    desktop.put(
        0,
        "aaaaaaaaaaaaaaaa_0",
        &Entry::new()
            .body(five_hour_only())
            .date(Some("Wed, 09 Sep 2026 16:00:00 GMT"))
            .data(),
        1,
    );
    // Only the MSIX root has an entry: it is read.
    let only_second = Desktop::new();
    only_second.put(1, "bbbbbbbbbbbbbbbb_0", &Entry::new().data(), 1);
    let (windows, observed_at, _) = reading_of(only_second.read(ORGANIZATION));
    assert_eq!(windows[0].utilization, 30.0);
    assert_eq!(observed_at, fixture_date());

    // Both have one: the newer by its date wins.
    desktop.put(
        1,
        "bbbbbbbbbbbbbbbb_0",
        &Entry::new().data(), // dated 16:12:08
        1,
    );
    let (windows, observed_at, _) = reading_of(desktop.read(ORGANIZATION));
    assert_eq!(observed_at, fixture_date());
    assert_eq!(windows[0].utilization, 30.0);

    // And the first root's, when it is the newer.
    desktop.put(
        0,
        "aaaaaaaaaaaaaaaa_0",
        &Entry::new()
            .body(five_hour_only())
            .date(Some("Wed, 09 Sep 2026 17:00:00 GMT"))
            .data(),
        1,
    );
    let (windows, observed_at, _) = reading_of(desktop.read(ORGANIZATION));
    assert_eq!(
        observed_at,
        fixture_date() + Duration::from_secs(47 * 60 + 52)
    );
    assert_eq!(ids(&windows), ["session"]);
}

#[test]
fn an_expired_reading_on_one_root_does_not_hide_a_current_one_on_the_other() {
    let desktop = Desktop::new();
    desktop.put(
        0,
        "aaaaaaaaaaaaaaaa_0",
        &Entry::new()
            .body(expired())
            .date(Some("Wed, 09 Sep 2026 23:00:00 GMT"))
            .data(),
        1,
    );
    desktop.put(1, "bbbbbbbbbbbbbbbb_0", &Entry::new().data(), 1);
    let (windows, observed_at, _) = reading_of(desktop.read(ORGANIZATION));
    assert_eq!(windows[0].utilization, 30.0);
    assert_eq!(observed_at, fixture_date());
}

#[test]
fn a_date_header_beats_the_file_time_and_the_file_time_beats_now() {
    let desktop = Desktop::new();
    desktop.put(0, "0123456789abcdef_0", &Entry::new().data(), 1_700_000_000);
    assert_eq!(reading_of(desktop.read(ORGANIZATION)).1, fixture_date());
    desktop.put(
        0,
        "0123456789abcdef_0",
        &Entry::new().undated().data(),
        1_700_000_000,
    );
    assert_eq!(reading_of(desktop.read(ORGANIZATION)).1, at(1_700_000_000));
}

#[test]
fn an_oversized_body_is_a_miss_not_a_reading() {
    let desktop = Desktop::new();
    desktop.put(
        0,
        "0123456789abcdef_0",
        &Entry::new().body(real(REAL_OVERSIZED_DECLARED)).data(),
        1,
    );
    desktop.put(
        0,
        "fedcba9876543210_0",
        &Entry::new().body(real(REAL_OVERSIZED_UNDECLARED)).data(),
        2,
    );
    assert_eq!(
        desktop.read(ORGANIZATION),
        DesktopReading::NotFound {
            format: DesktopCacheFormat::Simple
        }
    );
}

#[test]
fn resets_come_with_the_reading() {
    let desktop = Desktop::new();
    desktop.put(
        0,
        "0123456789abcdef_0",
        &reset_entry(available_reset()).data(),
        1_799_000_000,
    );
    let (windows, observed_at, resets) = reading_of(desktop.read(ORGANIZATION));
    assert_eq!(ids(&windows), ["session"]);
    let resets = resets.unwrap();
    assert_eq!(resets.captured_at, observed_at);
    assert_eq!(credits(&resets, at(1_800_000_000)), Some(1));
}

// MARK: - The format, for the doctor and Settings

#[test]
fn the_format_is_told_from_the_names_in_the_folder() {
    let hex = "0123456789abcdef_0";
    let cases: Vec<(&str, Vec<&str>, DesktopCacheFormat)> = vec![
        ("simple", vec![hex], DesktopCacheFormat::Simple),
        // Simple Cache keeps an index of its own beside its entries.
        (
            "simple with an index file",
            vec!["index", hex, "data_1"],
            DesktopCacheFormat::Simple,
        ),
        (
            "blockfile",
            vec!["index", "data_0", "data_1", "data_2", "data_3", "f_0000a1"],
            DesktopCacheFormat::Blockfile,
        ),
        // Missing a block file: not a blockfile cache we know.
        (
            "a torn blockfile",
            vec!["index", "data_0", "data_1", "data_2"],
            DesktopCacheFormat::Unknown,
        ),
        (
            "other files",
            vec!["something", "f_000001"],
            DesktopCacheFormat::Unknown,
        ),
        // `a_0` is a fine candidate for the reader but not a Simple Cache name.
        ("a short name", vec!["a_0"], DesktopCacheFormat::Unknown),
        (
            "a long hash",
            vec!["0123456789abcdef0_0"],
            DesktopCacheFormat::Unknown,
        ),
        (
            "a non-hex hash",
            vec!["0123456789abcdeg_0"],
            DesktopCacheFormat::Unknown,
        ),
        (
            "a stream-1 file",
            vec!["0123456789abcdef_1"],
            DesktopCacheFormat::Unknown,
        ),
        ("nothing", vec![], DesktopCacheFormat::Absent),
    ];
    for (name, files, expected) in cases {
        let dir = cache_with(
            &files
                .iter()
                .map(|f| (*f, vec![1_u8; 40]))
                .collect::<Vec<_>>(),
        );
        assert_eq!(folder_format(dir.path()), expected, "{name}");
    }
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        folder_format(&dir.path().join("absent")),
        DesktopCacheFormat::Absent
    );
    // A file where the folder should be can't be listed: unknown, not absent.
    let file = cache_with(&[("Cache_Data", vec![1; 40])]);
    assert_eq!(
        folder_format(&file.path().join("Cache_Data")),
        DesktopCacheFormat::Unknown
    );
}

#[test]
fn the_format_over_every_root_is_the_most_readable_one() {
    let desktop = Desktop::new();
    assert_eq!(
        desktop_cache_format(&desktop.roots),
        DesktopCacheFormat::Absent
    );
    desktop.put(1, "something", &[1; 40], 1);
    assert_eq!(
        desktop_cache_format(&desktop.roots),
        DesktopCacheFormat::Unknown
    );
    blockfile(&desktop, 0);
    assert_eq!(
        desktop_cache_format(&desktop.roots),
        DesktopCacheFormat::Blockfile
    );
    desktop.put(1, "0123456789abcdef_0", &[1; 40], 1);
    assert_eq!(
        desktop_cache_format(&desktop.roots),
        DesktopCacheFormat::Simple
    );
    let mut none = Desktop::new();
    none.roots.claude_desktop.clear();
    assert_eq!(
        desktop_cache_format(&none.roots),
        DesktopCacheFormat::Absent
    );
}

#[test]
fn the_format_names_are_the_doctors() {
    assert_eq!(DesktopCacheFormat::Simple.as_str(), "simple");
    assert_eq!(DesktopCacheFormat::Blockfile.as_str(), "blockfile");
    assert_eq!(DesktopCacheFormat::Absent.as_str(), "absent");
    assert_eq!(DesktopCacheFormat::Unknown.as_str(), "unknown");
}

#[test]
fn the_entry_cap_is_the_macs() {
    assert_eq!(MAX_ENTRY_BYTES, 512 * 1024);
    assert_eq!(MAX_KEY_BYTES, 8 * 1024);
    assert_eq!(HEADER_BYTES, 24);
}

#[test]
fn more_than_four_hundred_candidates_are_not_all_looked_at() {
    // Newer entries of another account fill the cap; the one that is ours is
    // older than all of them and so is never reached.
    let dir = cache_with(&[]);
    for index in 0..400_u64 {
        let name = format!("o{index:03}_0");
        let entry = Entry::new().key(key_for(OTHER_ORGANIZATION, "")).undated();
        std::fs::write(dir.path().join(&name), entry.data()).unwrap();
        set_modified(&dir.path().join(&name), at(2_000_000 + index));
    }
    std::fs::write(dir.path().join("mine_0"), Entry::new().undated().data()).unwrap();
    set_modified(&dir.path().join("mine_0"), at(1_000_000));
    assert!(read(dir.path()).is_none());
    // One fewer of the others, and it is the 400th candidate and is read.
    std::fs::remove_file(dir.path().join("o000_0")).unwrap();
    assert!(read(dir.path()).is_some());
}
