//! Claude's own limits, read out of Claude Desktop's HTTP cache
//! (ClaudeDesktopUsageCache.swift, DesktopUsageSource.swift, AU§12).
//!
//! Claude Desktop is Chromium, so the response its own usage panel draws
//! (`GET /api/organizations/<id>/usage`) lands in a disk-cache entry, and this
//! reads that entry: no token, no cookie, no request, no subprocess, no
//! write. If Desktop is absent, closed, has no cache or the entry is
//! unreadable, the answer is "nothing" and the caller keeps the sources it
//! had.
//!
//! Only Chromium's **Simple Cache** (`<16 hex>_0` files) is read. On Windows
//! Chromium may use the older blockfile backend (`index`, `data_0..data_3`);
//! that is reported as [`DesktopReading::UnsupportedFormat`] rather than
//! guessed at (DEGRADE, AU§12.2, risk R3), and [`desktop_cache_format`] tells
//! the doctor and Settings which one a PC has.
//!
//! The amount looked at is the point: of every other file in the folder only
//! the first 24 + 8 KiB bytes are read (enough for the cached URL), and a body
//! is decompressed only for an entry whose URL is this organization's usage
//! endpoint. Every bound (entry size, key size, output size, scan depth) is a
//! constant, not a setting: a wrong value is a hang, and nobody could know
//! what to set.
//!
//! Files are read, never memory-mapped: Chromium rewrites and truncates them
//! under us, and a read of a bounded copy can only tear, which parses to
//! nothing. The timestamp comes off the same open handle as the bytes, so it
//! cannot disagree with what was read. Rust opens files with the full share
//! mode (read, write, delete), so Desktop holding an entry open rarely
//! matters; a file that still cannot be opened (Windows error 32, a sharing
//! violation, or any other error) is a miss for that file, never a failure.
//!
//! Engine code: `std::fs` only, no OS calls, no `unsafe`.

use crate::core::time;
use crate::model::{
    DesktopCacheFormat, DesktopReading, DesktopResets, DesktopWindow, ResetCredits, ResetGrant,
    UsageWindow,
};
use crate::platform::Roots;
use crate::usage::parser::number;
use crate::usage::ring_windows::{label_for_id, order_of_ids, SCOPED_PREFIX, SESSION_ID};
use chrono::NaiveDate;
use serde_json::{Map, Value};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// MARK: - Limits

/// Entries larger than this are skipped without being read. A usage response
/// is about 4 kB on disk; this refuses the multi-megabyte asset entries that
/// share the folder.
pub const MAX_ENTRY_BYTES: usize = 512 * 1024;
/// How many entries a scan looks inside, newest first. The one wanted is
/// rewritten whenever Desktop refreshes its usage, so it is always among the
/// most recently modified.
pub const MAX_ENTRIES_EXAMINED: usize = 400;
/// How far past the newest usable entry to keep looking for a reset block. It
/// lives under its own key and is refreshed separately, so it can lag the
/// newest usage entry by a few writes; an account with none must not pay for
/// the whole cap on every poll.
pub const RESET_SEARCH_DEPTH: usize = 20;
/// Cap on the decompressed body, enforced on the decoder's output (a frame that
/// would exceed it is refused, never allocated).
pub const MAX_DECOMPRESSED_BYTES: usize = 256 * 1024;
/// A key longer than this is not one of ours (Chromium's own limit is far
/// lower) and reading it would be the one unbounded read here.
pub const MAX_KEY_BYTES: usize = 8 * 1024;
/// `SimpleFileHeader`: a 64-bit magic and three 32-bit fields, then four bytes
/// of padding because Chromium writes the C++ struct verbatim and its 64-bit
/// member aligns the whole to 8. Counting to 20 lands inside the key.
pub const HEADER_BYTES: usize = 24;
/// The largest zstd window accepted. RFC 8878 §3.1.1.1.2 asks web content to
/// stay within 8 MiB; a larger one is not from a server, and the decoder
/// would size its history buffer by it.
const MAX_WINDOW_BYTES: u64 = 8 * 1024 * 1024;
/// `kSimpleInitialMagicNumber`: what says a file is a Simple Cache entry.
const ENTRY_MAGIC: u64 = 0xfcfb_6d1b_a772_5c30;
/// What every zstd frame starts with, checked before the decoder sees bytes.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

// MARK: - Reading a cache folder

/// One reading of one cache folder, and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderReading {
    /// The same windows the other two Claude paths produce, in display order.
    pub windows: Vec<DesktopWindow>,
    /// When the numbers were true: the response's own `Date:`, else the
    /// entry's modification time, else the moment of the read. A cache entry
    /// is by nature already old, and how old is the whole question of whether
    /// it may be shown as live.
    pub captured_at: SystemTime,
    /// The entry it was read from (which of the two alternating keys answered
    /// is the first thing worth knowing when a reading looks wrong).
    pub entry: PathBuf,
    /// The newest `cedar_ember` block among the entries looked at.
    pub resets: Option<DesktopResets>,
}

impl FolderReading {
    /// Whether these numbers may still be presented as live: younger than
    /// `window`. A slightly negative age is two clocks disagreeing (the
    /// header's is the server's), not a reading from the future.
    pub fn is_fresh(&self, now: SystemTime, window: Duration) -> bool {
        match now.duration_since(self.captured_at) {
            Ok(age) => age < window,
            Err(ahead) => ahead.duration() < window,
        }
    }
}

/// The most recent usable reading of one `Cache_Data` folder for one
/// organization, or `None`.
///
/// `organization` is not cosmetic. Claude Desktop is signed in to exactly one
/// account while Agent Notch draws a ring per Claude account, so handing
/// Desktop's numbers to whichever ring asked first would put the personal
/// account's session percentage on the work ring. The cache key carries the
/// organization's UUID and Claude Code records the same one per account, so
/// the two are simply required to match.
///
/// It always scans and never remembers a winning file: Desktop asks for both
/// `…/usage` and `…/usage?skip_spend=1` (two keys, two files) and refreshes
/// them alternately, so one file's own age says nothing about whether the
/// other has since become the newer answer.
///
/// Freshness is not judged here: the newest reading there is comes back with
/// the timestamp it has.
pub fn read_folder(dir: &Path, organization: &str, now: SystemTime) -> Option<FolderReading> {
    // The reset block is written under its own key and refreshed separately,
    // so it can be older than the newest usage entry and is worth looking
    // past the first hit for, but only for a while.
    let mut newest: Option<FolderReading> = None;
    let mut resets: Option<DesktopResets> = None;
    for (index, entry) in recent_entries(dir).into_iter().enumerate() {
        let Some(mut reading) = reading_from(&entry, organization, now) else {
            continue;
        };
        if resets.is_none() {
            resets = reading.resets.take();
        }
        if newest.is_none() {
            newest = Some(reading);
        }
        if resets.is_some() {
            break;
        }
        if newest.is_some() && index >= RESET_SEARCH_DEPTH {
            break;
        }
    }
    // Desktop refreshes the plain and cedar_ember usage keys separately: keep
    // the newest windows while dating the reset block by its own entry.
    let mut newest = newest?;
    newest.resets = resets;
    Some(newest)
}

/// Candidate entry files, most recently modified first, capped: non-hidden,
/// not recursive, a name ending `_0` (the stream file that holds the body;
/// Chromium writes `_1` and `_s` beside it), a regular file, larger than the
/// header and no larger than [`MAX_ENTRY_BYTES`].
///
/// An entry that vanishes between the listing and the read simply drops out,
/// which happens constantly: Chromium evicts while we look.
fn recent_entries(dir: &Path) -> Vec<PathBuf> {
    // No Claude Desktop, no cache folder, or no permission to list it all
    // mean the same thing to the caller.
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut candidates: Vec<(SystemTime, String, PathBuf)> = listing
        .filter_map(Result::ok)
        .filter_map(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !name.ends_with("_0") {
                return None;
            }
            let meta = item.metadata().ok()?;
            let size = meta.len();
            if !meta.is_file() || size <= HEADER_BYTES as u64 || size > MAX_ENTRY_BYTES as u64 {
                return None;
            }
            Some((meta.modified().ok()?, name, item.path()))
        })
        .collect();
    // Newest first; the name breaks a tie so the order is the same every time.
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    candidates
        .into_iter()
        .take(MAX_ENTRIES_EXAMINED)
        .map(|(_, _, path)| path)
        .collect()
}

/// One entry, read and parsed, or `None` for every way that can fail.
fn reading_from(entry: &Path, organization: &str, now: SystemTime) -> Option<FolderReading> {
    // The organization is checked twice on purpose: `contents_of` checks it
    // against a prefix to decide whether the entry is worth reading whole, and
    // this checks it against the bytes actually parsed. Chromium can rewrite
    // the file between the two reads, and another account's numbers must never
    // reach this ring.
    let (bytes, modified) = contents_of(entry, organization)?;
    let parsed = parse_entry(&bytes)?;
    if parsed.organization != organization {
        return None;
    }
    let response: Value = serde_json::from_slice(&parsed.body).ok()?;
    let response = UsageResponse::decode(&response)?;
    let windows = response.windows();
    // A response that parses but names no window is not a reading: letting it
    // through would draw a ring with a hole in it instead of falling through to
    // a source that works.
    if windows.is_empty() {
        return None;
    }
    let captured_at = parsed.date.or(modified).unwrap_or(now);
    let resets = (response.reports_reset_credits || parsed.requests_reset_credits).then_some(
        DesktopResets {
            value: response.cedar_ember,
            captured_at,
        },
    );
    Some(FolderReading {
        windows,
        captured_at,
        entry: entry.to_path_buf(),
        resets,
    })
}

/// The entry's bytes and when they were last written, both from one open
/// handle, bounded. Any error is a miss (a sharing violation, a permission
/// error, a file that vanished).
fn contents_of(entry: &Path, organization: &str) -> Option<(Vec<u8>, Option<SystemTime>)> {
    let mut file = File::open(entry).ok()?;

    // The header and at most a key's worth after it, first: every candidate is
    // then rejected on its URL alone, so a scan doesn't pull hundreds of
    // megabytes of unrelated cached responses through memory, and this app
    // never reads the body of a page it has no business looking at.
    let mut head = Vec::new();
    (&mut file)
        .take((HEADER_BYTES + MAX_KEY_BYTES) as u64)
        .read_to_end(&mut head)
        .ok()?;
    let key = entry_key(&head)?;
    if usage_organization(&key).as_deref() != Some(organization) {
        return None;
    }

    // One read of at most the cap. A short result is normal: a truncated entry
    // simply fails the parse.
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_ENTRY_BYTES as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    let modified = file.metadata().ok().and_then(|meta| meta.modified().ok());
    Some((bytes, modified))
}

// MARK: - Reading every root

/// The folder a Claude Desktop root keeps its HTTP cache in.
pub fn cache_folder(root: &Path) -> PathBuf {
    root.join("Cache").join("Cache_Data")
}

/// The newest usable reading of Claude Desktop's cache for one organization
/// over every root in `roots.claude_desktop` (the usual `%APPDATA%\Claude`,
/// then the MSIX package's). A Job on `an-io`.
///
/// - A window whose reset time has passed makes the whole response out of
///   date however recently it was written (`DesktopUsageSource.translate`), so
///   that root gives no reading.
/// - A label equal to the standard label for its id is dropped, so it follows
///   the app's wording rather than the one it was read in.
/// - With no reading, [`DesktopReading::UnsupportedFormat`] says the cache is
///   Chromium's blockfile format; anything else is `NotFound` with the format
///   found.
///
/// Off when sealed or when the setting is off: that is the store's switch, not
/// this function's.
pub fn read_desktop_cache(
    roots: &Roots,
    organization_uuid: &str,
    now: SystemTime,
) -> DesktopReading {
    let mut best: Option<FolderReading> = None;
    if !organization_uuid.is_empty() {
        for root in &roots.claude_desktop {
            let Some(reading) = read_folder(&cache_folder(root), organization_uuid, now) else {
                continue;
            };
            let expired = reading
                .windows
                .iter()
                .any(|window| window.resets_at.is_some_and(|reset| reset <= now));
            if expired || reading.windows.is_empty() {
                continue;
            }
            // The first root keeps a tie.
            if best
                .as_ref()
                .is_none_or(|kept| reading.captured_at > kept.captured_at)
            {
                best = Some(reading);
            }
        }
    }
    if let Some(reading) = best {
        let windows = reading
            .windows
            .into_iter()
            .map(|mut window| {
                if window.label.as_deref() == Some(label_for_id(&window.id).as_str()) {
                    window.label = None;
                }
                window
            })
            .collect();
        return DesktopReading::Reading {
            organization_uuid: organization_uuid.to_owned(),
            windows,
            observed_at: reading.captured_at,
            resets: reading.resets,
        };
    }
    match desktop_cache_format(roots) {
        DesktopCacheFormat::Blockfile => DesktopReading::UnsupportedFormat,
        format => DesktopReading::NotFound { format },
    }
}

/// Which disk-cache backend Claude Desktop's cache uses, for the doctor
/// (`desktop-cache: simple|blockfile|absent`) and Settings. Over every root
/// the most readable wins: Simple, then Blockfile, then Unknown, then Absent.
pub fn desktop_cache_format(roots: &Roots) -> DesktopCacheFormat {
    fn rank(format: DesktopCacheFormat) -> u8 {
        match format {
            DesktopCacheFormat::Simple => 3,
            DesktopCacheFormat::Blockfile => 2,
            DesktopCacheFormat::Unknown => 1,
            DesktopCacheFormat::Absent => 0,
        }
    }
    roots
        .claude_desktop
        .iter()
        .map(|root| folder_format(&cache_folder(root)))
        .max_by_key(|format| rank(*format))
        .unwrap_or(DesktopCacheFormat::Absent)
}

/// The backend one `Cache_Data` folder holds:
///
/// - `Simple`: any `<16 hex>_0` entry (Simple Cache also keeps an `index` file
///   or an `index-dir` folder, so this is checked first);
/// - `Blockfile`: `index` and `data_0` … `data_3` and no such entry;
/// - `Absent`: no folder, or nothing in it;
/// - `Unknown`: something else, or a folder that can't be listed.
pub fn folder_format(dir: &Path) -> DesktopCacheFormat {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return DesktopCacheFormat::Absent
        }
        Err(_) => return DesktopCacheFormat::Unknown,
    };
    let (mut any, mut index, mut blocks) = (false, false, 0_u8);
    for item in listing.filter_map(Result::ok) {
        let name = item.file_name().to_string_lossy().into_owned();
        any = true;
        if is_simple_entry_name(&name) {
            return DesktopCacheFormat::Simple;
        }
        match name.as_str() {
            "index" => index = true,
            "data_0" => blocks |= 1,
            "data_1" => blocks |= 2,
            "data_2" => blocks |= 4,
            "data_3" => blocks |= 8,
            _ => {}
        }
    }
    if index && blocks == 0b1111 {
        DesktopCacheFormat::Blockfile
    } else if any {
        DesktopCacheFormat::Unknown
    } else {
        DesktopCacheFormat::Absent
    }
}

/// `<16 hex>_0`: a Simple Cache entry's stream file, named by the key's hash.
fn is_simple_entry_name(name: &str) -> bool {
    name.strip_suffix("_0")
        .is_some_and(|hash| hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

// MARK: - The entry format
//
// Chromium's Simple Cache, and only the three things needed out of it.

/// One entry's usage body, its organization and its `Date:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub body: Vec<u8>,
    /// Whose usage this is, from the URL in the key.
    pub organization: String,
    /// From the response's `Date:` header, when it was found.
    pub date: Option<SystemTime>,
    /// The key's query carries `cedar_ember=1`.
    pub requests_reset_credits: bool,
}

/// Pulls the usage JSON, its organization and its `Date:` out of one entry.
/// Public so the tests can drive it on synthetic entries: every branch is a
/// shape Chromium can produce, and most are unreachable through a file.
pub fn parse_entry(entry: &[u8]) -> Option<Parsed> {
    let key = entry_key(entry)?;
    let organization = usage_organization(&key)?;

    // `key_length` is a byte count and the key was decoded from exactly that
    // many bytes, so this is where the key ended.
    let body_start = HEADER_BYTES + key.len();
    if entry.len() <= body_start + ZSTD_MAGIC.len()
        || entry[body_start..body_start + ZSTD_MAGIC.len()] != ZSTD_MAGIC
    {
        // A usage entry that is not zstd. Chromium negotiates the encoding and
        // could be handed gzip or brotli tomorrow; nothing to do about it here.
        return None;
    }
    let frame = &entry[body_start..];
    let (body, frame_size) = decompress(frame)?;
    Some(Parsed {
        body,
        organization,
        // Everything after the frame is the stream Chromium wrote next, which
        // is where the header block lives. Bounding the search to it keeps
        // `date:` from being found in the body's own JSON.
        date: http_date(&frame[frame_size..]),
        requests_reset_credits: key
            .split('?')
            .filter(|part| !part.is_empty())
            .nth(1)
            .is_some_and(|query| query.split('&').any(|pair| pair == "cedar_ember=1")),
    })
}

/// The cache key an entry begins with, or `None` when these bytes are not a
/// Simple Cache entry. The key length is a number out of the file, so it is
/// corruption- and attacker-controlled and is bounded before it is believed.
pub fn entry_key(entry: &[u8]) -> Option<String> {
    if entry.len() <= HEADER_BYTES
        || u64::from_le_bytes(entry[0..8].try_into().ok()?) != ENTRY_MAGIC
    {
        return None;
    }
    let length = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
    if length == 0 || length > MAX_KEY_BYTES || HEADER_BYTES + length > entry.len() {
        return None;
    }
    String::from_utf8(entry[HEADER_BYTES..HEADER_BYTES + length].to_vec()).ok()
}

/// Which organization a cache key's usage URL names, or `None` when the key
/// is not a usage request.
///
/// Keys carry a cache-partition prefix (`1/0/https://claude.ai/…`), so this
/// matches the URL inside. The host check is not decoration: without it the
/// body of any site that served a path shaped like Anthropic's would be
/// decompressed.
pub fn usage_organization(key: &str) -> Option<String> {
    if !key.contains("claude.ai") && !key.contains("anthropic.com") {
        return None;
    }
    const MARKER: &str = "/api/organizations/";
    let after = &key[key.find(MARKER)? + MARKER.len()..];
    // Query and fragment are no part of the shape: the request has carried
    // `?skip_spend=1` and could carry anything next.
    let path = after.split(['?', '#']).next().unwrap_or("");
    // `<id>/usage`: one segment and then the endpoint, so that
    // `/api/organizations/<id>/usage/something-else` is not mistaken for it.
    let mut segments = path.split('/');
    match (segments.next(), segments.next(), segments.next()) {
        (Some(id), Some("usage"), None) if !id.is_empty() => Some(id.to_owned()),
        _ => None,
    }
}

/// The response's `Date:`, found in the header block Chromium stores after the
/// body: a run of NUL-separated `name:value` strings, enough structure to read
/// one field without parsing the pickle around it. Names arrive lower-cased
/// over HTTP/2 and HTTP/3 and as written over HTTP/1.1, so both are tried.
pub fn http_date(trailer: &[u8]) -> Option<SystemTime> {
    for name in ["date:", "Date:"] {
        let mut needle = vec![0_u8];
        needle.extend_from_slice(name.as_bytes());
        let Some(start) = find(trailer, &needle) else {
            continue;
        };
        let value = &trailer[start + needle.len()..];
        let Some(end) = value.iter().position(|b| *b == 0) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&value[..end]) else {
            continue;
        };
        if let Some(date) = parse_imf_fixdate(text) {
            return Some(date);
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// RFC 9110's `IMF-fixdate` (`Wed, 09 Sep 2026 16:12:08 GMT`), the only form a
/// `Date:` header written this decade takes. Read by hand: the day name is
/// only checked to be one, the zone must be `GMT`.
pub fn parse_imf_fixdate(text: &str) -> Option<SystemTime> {
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let text = text.trim_matches(|c: char| c == ' ' || c == '\t');
    let (day_name, rest) = text.split_once(',')?;
    if !DAYS.contains(&day_name.trim().to_ascii_lowercase().as_str()) {
        return None;
    }
    let mut parts = rest.split_ascii_whitespace();
    let day: u32 = parts.next()?.parse().ok()?;
    let month_name = parts.next()?.to_ascii_lowercase();
    let month = MONTHS.iter().position(|m| *m == month_name)? as u32 + 1;
    let year: i32 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':');
    let hour: u32 = clock.next()?.parse().ok()?;
    let minute: u32 = clock.next()?.parse().ok()?;
    let second: u32 = clock.next()?.parse().ok()?;
    if clock.next().is_some()
        || !parts.next()?.eq_ignore_ascii_case("gmt")
        || parts.next().is_some()
    {
        return None;
    }
    let seconds = NaiveDate::from_ymd_opt(year, month, day)?
        .and_hms_opt(hour, minute, second)?
        .and_utc()
        .timestamp();
    Some(UNIX_EPOCH + Duration::from_secs(u64::try_from(seconds).ok()?))
}

// MARK: - zstd

/// The decompressed body of the frame at the start of `bytes` and how much of
/// `bytes` the frame took, or `None`.
///
/// `bytes` runs to the end of the entry, so it holds the frame and whatever
/// Chromium wrote after it: the frame's end is found by walking it, then
/// exactly that much is decoded with the output capped.
fn decompress(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    let frame = frame_layout(bytes)?;
    // The declared size, for the frames that declare one. Chunked responses
    // do not, which is why the output is capped as well: that cap always
    // applies.
    if frame
        .content_size
        .is_some_and(|size| size > MAX_DECOMPRESSED_BYTES as u64)
    {
        return None;
    }
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(&bytes[..frame.size]).ok()?;
    let mut out = Vec::new();
    // One byte past the cap tells "exactly at the cap" from "over it".
    (&mut decoder)
        .take(MAX_DECOMPRESSED_BYTES as u64 + 1)
        .read_to_end(&mut out)
        .ok()?;
    // Corrupt, truncated mid-frame, empty or bigger than the cap all get the
    // same answer.
    (!out.is_empty() && out.len() <= MAX_DECOMPRESSED_BYTES).then_some((out, frame.size))
}

/// What walking a zstd frame by hand found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLayout {
    /// The frame's whole length, magic to checksum.
    pub size: usize,
    /// The header's declared content size, when it declares one.
    pub content_size: Option<u64>,
}

/// Finds where a zstd frame ends without decoding it (RFC 8878 §3.1.1): the
/// header, then 3-byte block headers until the last block, then the optional
/// 4-byte checksum. `None` when the frame isn't all there or isn't shaped like
/// one (a reserved bit, a reserved block type, an unreasonable window).
pub fn frame_layout(bytes: &[u8]) -> Option<FrameLayout> {
    if bytes.len() < ZSTD_MAGIC.len() + 2 || bytes[..4] != ZSTD_MAGIC {
        return None;
    }
    let descriptor = bytes[4];
    if descriptor & 0b0000_1000 != 0 {
        return None; // the reserved bit
    }
    let single_segment = descriptor & 0b0010_0000 != 0;
    let has_checksum = descriptor & 0b0000_0100 != 0;
    let dictionary_bytes = [0_usize, 1, 2, 4][(descriptor & 0b11) as usize];
    let content_bytes = match descriptor >> 6 {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let mut at = 5;

    if !single_segment {
        let window = *bytes.get(at)?;
        at += 1;
        let base = 1_u64 << (10 + u32::from(window >> 3));
        if base + (base / 8) * u64::from(window & 7) > MAX_WINDOW_BYTES {
            return None;
        }
    }
    at += dictionary_bytes;
    let content_size = match content_bytes {
        0 => None,
        count => {
            let field = bytes.get(at..at + count)?;
            let mut value = 0_u64;
            for byte in field.iter().rev() {
                value = value << 8 | u64::from(*byte);
            }
            Some(if count == 2 { value + 256 } else { value })
        }
    };
    at += content_bytes;

    loop {
        let header = bytes.get(at..at + 3)?;
        let header = u32::from(header[0]) | u32::from(header[1]) << 8 | u32::from(header[2]) << 16;
        at += 3;
        let last = header & 1 == 1;
        let size = (header >> 3) as usize;
        match (header >> 1) & 3 {
            0 | 2 => at += size, // raw, compressed: `size` bytes follow
            1 => at += 1,        // RLE: one byte, repeated `size` times
            _ => return None,    // reserved
        }
        if at > bytes.len() {
            return None;
        }
        if last {
            break;
        }
    }
    if has_checksum {
        at += 4;
        if at > bytes.len() {
            return None;
        }
    }
    Some(FrameLayout {
        size: at,
        content_size,
    })
}

// MARK: - The response

/// `GET /api/organizations/<id>/usage`'s body as the Mac decodes it
/// (`UsageResponse`, ClaudeOAuthProvider.swift 601-718), and with the same
/// strictness: `limits`, `five_hour` and `seven_day` are the reading itself and
/// must decode (one bad element costs the whole response), while the
/// `cedar_ember` block is enrichment and never costs the reading.
struct UsageResponse {
    limits: Vec<Limit>,
    five_hour: Option<Window>,
    seven_day: Option<Window>,
    cedar_ember: Option<ResetCredits>,
    /// The body carries a non-null `cedar_ember`, even one that didn't parse:
    /// a malformed block still supersedes older cached reset data.
    reports_reset_credits: bool,
}

struct Limit {
    kind: String,
    percent: f64,
    resets_at: Option<SystemTime>,
    /// `scope.model.display_name`, the one place the response names the model
    /// of the `weekly_scoped` window.
    model: Option<String>,
}

struct Window {
    utilization: f64,
    resets_at: Option<SystemTime>,
}

impl UsageResponse {
    fn decode(value: &Value) -> Option<Self> {
        let body = value.as_object()?;
        let limits = match body.get("limits") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items.iter().map(Limit::decode).collect::<Option<_>>()?,
            Some(_) => return None,
        };
        let cedar = body.get("cedar_ember");
        Some(Self {
            limits,
            five_hour: Window::decode(body.get("five_hour"))?,
            seven_day: Window::decode(body.get("seven_day"))?,
            cedar_ember: cedar.and_then(decode_reset_credits),
            reports_reset_credits: cedar.is_some_and(|block| !block.is_null()),
        })
    }

    /// `limits` is the forward-compatible shape (it grows new kinds as
    /// Anthropic adds them), with the two named windows merged in when
    /// missing: a window that has just rolled over disappears from `limits`
    /// while `five_hour` still carries it, and relying on the array alone
    /// would lose the session exactly when it resets. Session first, then
    /// `weekly_all`, then by id.
    fn windows(&self) -> Vec<DesktopWindow> {
        let mut windows: Vec<DesktopWindow> = self
            .limits
            .iter()
            .filter(|limit| limit.resets_at.is_some())
            .map(|limit| DesktopWindow {
                id: limit.kind.clone(),
                label: Some(limit.label()),
                utilization: limit.percent,
                resets_at: limit.resets_at,
                duration_s: duration_of(&limit.kind),
            })
            .collect();
        let mut merge = |window: &Option<Window>, id: &str| {
            let Some(window) = window else { return };
            if window.resets_at.is_none() || windows.iter().any(|w| w.id == id) {
                return;
            }
            windows.push(DesktopWindow {
                id: id.to_owned(),
                label: Some(label_for_id(id)),
                utilization: window.utilization,
                resets_at: window.resets_at,
                duration_s: duration_of(id),
            });
        };
        merge(&self.five_hour, SESSION_ID);
        merge(&self.seven_day, "weekly_all");
        windows.sort_by(|a, b| order_of_ids(&a.id, &b.id));
        windows
    }
}

impl Limit {
    fn decode(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let percent = match object.get("percent")? {
            Value::Number(n) => n.as_f64()?,
            _ => return None,
        };
        Some(Self {
            kind: object.get("kind")?.as_str()?.to_owned(),
            percent,
            resets_at: date_field(object, "resets_at")?,
            // Tolerated rather than required: the scope is only a nicer name
            // for the window, so a shape change falls back to the kind's own
            // wording instead of costing the response.
            model: object
                .get("scope")
                .and_then(|scope| scope.get("model"))
                .and_then(|model| model.get("display_name"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    /// The window's own name: the model where the response names one, the
    /// kind's own wording otherwise.
    fn label(&self) -> String {
        match self.model.as_deref().map(|m| m.trim_matches([' ', '\t'])) {
            Some(named) if !named.is_empty() => named.to_owned(),
            _ => label_for_id(&self.kind),
        }
    }
}

impl Window {
    /// `Ok`-style: `Some(None)` for an absent or null window, `Some(Some(_))`
    /// for a good one, `None` for one that doesn't decode.
    fn decode(value: Option<&Value>) -> Option<Option<Self>> {
        match value {
            None | Some(Value::Null) => Some(None),
            Some(Value::Object(object)) => Some(Some(Self {
                utilization: match object.get("utilization")? {
                    Value::Number(n) => n.as_f64()?,
                    _ => return None,
                },
                resets_at: date_field(object, "resets_at")?,
            })),
            Some(_) => None,
        }
    }
}

fn duration_of(kind: &str) -> u64 {
    if kind == SESSION_ID {
        UsageWindow::SESSION_DURATION_S
    } else if kind.starts_with(SCOPED_PREFIX) {
        UsageWindow::WEEKLY_DURATION_S
    } else {
        0
    }
}

/// An optional date field: `Some(None)` when absent or null, `Some(Some(_))`
/// when it is a date, `None` when it is present and not one (which fails the
/// whole decode, as Swift's date strategy does).
fn date_field(object: &Map<String, Value>, key: &str) -> Option<Option<SystemTime>> {
    match object.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) if text.contains('T') => time::parse_iso8601(text).map(Some),
        Some(_) => None,
    }
}

/// The `cedar_ember` block (`ClaudeResetCredits`): `eligible` and `grants`
/// are required, a grant that doesn't decode is dropped and the rest kept.
fn decode_reset_credits(value: &Value) -> Option<ResetCredits> {
    let object = value.as_object()?;
    let grants = object
        .get("grants")?
        .as_array()?
        .iter()
        .filter_map(decode_grant)
        .collect();
    Some(ResetCredits {
        eligible: object.get("eligible")?.as_bool()?,
        ineligible_reason: object
            .get("ineligible_reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
        grants,
    })
}

fn decode_grant(value: &Value) -> Option<ResetGrant> {
    let object = value.as_object()?;
    let date = |key: &str| date_field(object, key)?;
    Some(ResetGrant {
        id: object.get("id")?.as_str()?.to_owned(),
        resets_left: whole_number(object.get("resets_left")?)?,
        starts_at: date("starts_at")?,
        ends_at: date("ends_at")?,
        paused: object.get("paused")?.as_bool()?,
    })
}

/// An integer, or a double with no fraction (Swift decodes `1.0` as an `Int`).
fn whole_number(value: &Value) -> Option<i64> {
    let number = number(Some(value))?;
    (matches!(value, Value::Number(_)) && number.fract() == 0.0 && number.abs() < 9.0e15)
        .then_some(number as i64)
}
