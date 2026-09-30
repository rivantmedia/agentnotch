//! One read of an account's `.claude.json` for the engine (`Job::
//! ReadClaudeJson`, AU§4): who is signed in, and Claude Code's own cached
//! usage when it belongs to that login (a `/login` to another account
//! leaves the old account's snapshot behind in the file).
//!
//! Blocking; runs on the `an-io` lanes. The file is parsed only when it
//! changed, and a read caught mid-write keeps the last good parse
//! (`core::claude_json`).

use crate::core::claude_json::{ClaudeGlobalConfig, ClaudeJsonReader};
use crate::model::{AccountId, AccountUsage};
use crate::runtime_types::ClaudeJsonRead;
use crate::usage::parser;
use std::path::Path;
use std::sync::OnceLock;

/// The reader the usage jobs share: the 20-second poll and a probe's checks
/// before and after it read the same files, so a change is parsed once.
pub fn reader() -> &'static ClaudeJsonReader {
    static READER: OnceLock<ClaudeJsonReader> = OnceLock::new();
    READER.get_or_init(ClaudeJsonReader::new)
}

/// Claude Code's cached usage in `config`, only when it is the signed-in
/// account's: a full snapshot dated when Claude Code fetched it, under the
/// login's own `uuid:<account>` until the store ties it to an identity.
pub fn cached_usage_of(config: &ClaudeGlobalConfig) -> Option<AccountUsage> {
    let raw = config.matching_cached_usage()?;
    parser::cached_usage_from_raw(raw).map(|snapshot| snapshot.account_usage())
}

/// `Job::ReadClaudeJson`, through the shared reader.
pub fn read_claude_json(folder: &AccountId, path: &Path) -> ClaudeJsonRead {
    read_claude_json_with(reader(), folder, path)
}

/// `Job::ReadClaudeJson` through `reader` (tests use their own).
pub fn read_claude_json_with(
    reader: &ClaudeJsonReader,
    folder: &AccountId,
    path: &Path,
) -> ClaudeJsonRead {
    match reader.read_stamped(path) {
        Some((config, stamp)) => ClaudeJsonRead {
            folder: folder.clone(),
            cached_usage: cached_usage_of(&config),
            identity: config.identity,
            stamp: Some(stamp),
            error: None,
        },
        None => ClaudeJsonRead {
            folder: folder.clone(),
            identity: None,
            cached_usage: None,
            stamp: None,
            // A folder nobody ran Claude Code in yet has no file: no error.
            error: path
                .exists()
                .then(|| "its .claude.json can't be read right now".to_owned()),
        },
    }
}
