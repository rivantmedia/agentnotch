//! Accounts (AU§2-7): the folder classifier and discovery, identities
//! ("the email decides"), naming, monograms, ring ids, `accounts.json`,
//! the default folder's timeline, process attribution.
//!
//! An account is a signed-in identity, not a folder. Claude Code keeps one
//! login per config folder (`CLAUDE_CONFIG_DIR`, `~\.claude` when unset), so
//! the folders found on disk ([`snapshot`], [`classify`]) are grouped by who
//! is signed in to them ([`identities`]), named apart ([`naming`]) and kept,
//! with what the user chose for each, by the [`registry`]. [`for_cloud`] is
//! what the cloud thread reads of it, [`inspect`] the read-only report of
//! `agentnotch.exe inspect-accounts`, and [`watch`] the cheap look that
//! notices a new VS Code window of Claude Parallel Profiles.
//!
//! Claude Parallel Profiles does nothing on native Windows (AU§0.2): there
//! every folder is a run folder, and the rules for its stores, window copies
//! and mirrored `~\.claude` are kept, pure and tested, for the setups they
//! describe.
//!
//! Owner: WP3.

pub mod classify;
pub mod folder;
pub mod for_cloud;
pub mod identities;
pub mod inspect;
pub mod naming;
pub mod registry;
pub mod snapshot;
pub mod timeline;
pub mod watch;

pub use classify::{FolderSuggestion, Layout, SuggestionReason};
pub use folder::Folder;
pub use identities::{IdentityAccount, IdentityPrefs};
pub use registry::{
    AccountError, AccountRegistry, CreatedAccount, FolderRings, DISCOVERY_INTERVAL, SAVE_DELAY,
    SIGHTING_RESOLUTION,
};
pub use snapshot::{read_folder_snapshot, DiskProbe, FolderMarkers, FolderProbe};
pub use timeline::{FolderAttribution, FolderIdentityTimeline};
