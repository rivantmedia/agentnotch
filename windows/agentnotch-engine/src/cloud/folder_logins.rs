//! When the app first saw each config folder signed in as the account it
//! names now (CL§7.4; the Mac's `CloudFolderLogins`): "signed in as <login>
//! since <date>". Set the first time the app sees the folder's login, and
//! again whenever that changes; a folder seen signed out keeps its record
//! (logging out and back in as the same account changes nothing). Before the
//! app first saw it, nothing is known, so the backfill reads only transcripts
//! begun after this date. Local only (`cloud-folder-logins.json`, private;
//! logins are digests); never sent. Kept whether or not sync is on.
//!
//! Folders are keyed the way `core::paths` keys them (`Paths::key`): on
//! Windows `C:\Users\Me\.claude` and `c:\users\me\.claude` are one folder.

use super::contract::Stamp;
use super::files::{lock, StateFile};
use crate::core::paths::Paths;
use crate::platform::SecureFiles;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub const FILE_NAME: &str = "cloud-folder-logins.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub login: String,
    pub since: Stamp,
}

/// `cloud-folder-logins.json`: the Mac's `CloudFolderLogins.Contents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contents {
    pub version: u32,
    /// By the folder's key (`Paths::key`).
    pub folders: BTreeMap<String, Record>,
}

impl Contents {
    pub const CURRENT_VERSION: u32 = 1;
}

pub struct CloudFolderLogins {
    paths: Paths,
    contents: Arc<Mutex<Contents>>,
    file: StateFile<Contents>,
}

impl CloudFolderLogins {
    /// `file`: in memory or `<support>\cloud-folder-logins.json`. A file of
    /// another version or a damaged one starts fresh.
    pub fn new(file: StateFile<Contents>, paths: Paths) -> Self {
        let contents = match file.load() {
            Some(saved) if saved.version == Contents::CURRENT_VERSION => saved,
            _ => Contents {
                version: Contents::CURRENT_VERSION,
                folders: BTreeMap::new(),
            },
        };
        CloudFolderLogins {
            paths,
            contents: Arc::new(Mutex::new(contents)),
            file,
        }
    }

    pub fn in_support(
        support: &Path,
        files: Arc<dyn SecureFiles>,
        persist: bool,
        paths: Paths,
    ) -> Self {
        let file = if persist {
            StateFile::new(Some(support.join(FILE_NAME)), Some(files), Duration::ZERO)
        } else {
            StateFile::memory()
        };
        Self::new(file, paths)
    }

    /// The folders' logins as the app reads them now (folder → login; a
    /// folder nobody is signed in to is left out).
    pub fn observe(&self, logins: &BTreeMap<String, String>, now: SystemTime) {
        let changed = {
            let mut contents = lock(&self.contents);
            let mut changed = false;
            for (folder, login) in logins {
                let key = self.paths.key(folder);
                if contents
                    .folders
                    .get(&key)
                    .is_some_and(|r| r.login == *login)
                {
                    continue;
                }
                contents.folders.insert(
                    key,
                    Record {
                        login: login.clone(),
                        since: Stamp(now),
                    },
                );
                changed = true;
            }
            changed
        };
        if changed {
            let shared = self.contents.clone();
            self.file.save(move || lock(&shared).clone());
        }
    }

    /// Since when `folder` has been seen signed in as `login`; `None` when it
    /// never was, or someone else is recorded there.
    pub fn since(&self, folder: &str, login: Option<&str>) -> Option<SystemTime> {
        let login = login?;
        let contents = lock(&self.contents);
        let record = contents.folders.get(&self.paths.key(folder))?;
        (record.login == login).then_some(record.since.0)
    }

    pub fn save_now(&self) {
        let snapshot = lock(&self.contents).clone();
        self.file.save_now(&snapshot);
    }

    /// Waits until every background write has landed (tests).
    pub fn flush(&self) {
        self.file.flush();
    }
}
