//! The Claude Code settings pane's state and the setup banners' state, as
//! pure functions of what the stores hold (design §4.3, §5.4; UI§5.2, UI§7).
//! Ports of `ClaudeControlHub.currentSetupState`, `AccountHookManager.
//! setupState`, `SettingsPaneItems`/`SettingsPaneModel` (account rows, folder
//! rows, the Hooks summary, "Last change" notice, usage lines),
//! `AccountHookSummary`, `HookHealth`, `UsageCheckCopy`, `ConsentScope`, with
//! the Windows changes: `~\` paths, no legacy-app hooks (the official
//! Codenotch's own entries are reported instead), the hook pipe in place of a
//! socket path, and the Windows notification permission.
//!
//! Nothing here reads a file, a clock or a store: the runtime hands over what
//! it holds in a [`SettingsInput`] of borrowed values. The statuses of the
//! folders' settings.json files are the hook manager's (`absorb_status` takes
//! them in at launch, before any consent, so the consent card can say what is
//! there).
//!
//! Round trips: `setup.codenotch_hooks_folders` holds folder ids (the page
//! sends the same string back in `remove_codenotch_hooks`); a suggestion's
//! `path` is the `~\`-abbreviated path (`Paths` expands it again).
//!
//! Owner: WP7.

use crate::accounts::classify::is_window_dir;
use crate::accounts::{naming, AccountRegistry, Folder, IdentityAccount};
use crate::attention::rows::age_text;
use crate::core::paths::Paths;
use crate::core::settings::ControlSettings;
use crate::hooks::commands::effective_version;
use crate::hooks::manager::{consent_files, is_install_target, HookManager};
use crate::model::*;
use crate::platform::NotifyPermission;
use crate::runtime_types::{FolderHookStatus, RingReading, VersionSighting, VersionSource};
use crate::usage::desktop::UNSUPPORTED_FORMAT_TEXT;
use crate::usage::ring_windows::{self, SESSION_ID, WEEKLY_ID};
use std::collections::BTreeMap;
use std::time::SystemTime;

/// The minutes the Usage picker offers; 0 is Off.
pub const INTERVAL_OPTIONS: [u32; 5] = [0, 5, 10, 15, 30];

/// Highest percentage shown; anything above reads as "999%".
const MAX_DISPLAYED_PERCENT: f64 = 999.0;

/// The footnotes under Hooks and status line (§5.4).
pub const FOOTNOTES: [&str; 2] = [
    "Sessions inside WSL aren't tracked yet.",
    "Uninstalling keeps Claude Code's hooks unless you tick Delete the application data; turn Claude Code control off first to remove them.",
];

/// Why "Ring in notch" is off and disabled for an untracked account (the page
/// carries the same words; the engine words it for the usage lines).
const NOT_TRACKED_USAGE: &str = "Not checked while it isn't tracked";
const RING_OFF_USAGE: &str = "Not checked while its ring is off";

// ---- input ----

/// What the setup banners and the consent card are made from.
pub struct SetupInput<'a> {
    pub registry: &'a AccountRegistry,
    /// Holds each folder's settings.json status (`absorb_status`).
    pub hooks: &'a HookManager,
    pub settings: &'a ControlSettings,
    /// A VS Code workspace's folder named after its project (by folder id),
    /// when a session there told. Inert on native Windows: Claude Parallel
    /// Profiles has no Windows build, but its layout is ported and tested.
    pub window_names: &'a BTreeMap<String, String>,
    /// The hook pipe couldn't be opened.
    pub transport_error: Option<&'a str>,
    pub sealed: bool,
}

/// Everything the Claude Code settings pane shows, borrowed.
pub struct SettingsInput<'a> {
    pub now: SystemTime,
    pub setup: SetupInput<'a>,
    /// Each identity's ring reading (`UsageStore::ring_reading`).
    pub readings: &'a BTreeMap<IdentityId, RingReading>,
    /// Every Claude Code the app has seen (`Job::Versions`, hooks, status
    /// lines): the oldest decides what the hooks say.
    pub versions: &'a [VersionSighting],
    /// Folders whose settings.json the last pass changed.
    pub changed_folders: &'a [AccountId],
    /// `HubConfig.pipe_name`.
    pub pipe_name: &'a str,
    /// An install job is running.
    pub busy: bool,
    /// A usage refresh is running.
    pub refreshing: bool,
    /// What Claude Desktop's cache holds, when it was looked at.
    pub desktop_format: Option<DesktopCacheFormat>,
    pub notify_permission: NotifyPermission,
    /// The global shortcut is registered (or none is wanted).
    pub hotkey_ok: bool,
    pub hotkey_message: Option<&'a str>,
    pub cloud: &'a CloudState,
    pub session_count: u32,
    pub review_count: u32,
}

// ---- setup state ----

/// The consent card and the banners (`AccountHookManager.setupState`).
/// Sealed: nothing to ask, nothing found, nothing written, and no banner
/// (the Mac's sealed setup state is the empty one; the panel's
/// `install_disabled` banner reads "--no-install"). The Settings pane still
/// says installing is off (`hooks.install_allowed`).
pub fn setup_state(input: &SetupInput<'_>) -> SetupState {
    let settings = input.settings;
    let installs_disabled = input.sealed || input.hooks.installs_disabled();
    if input.sealed {
        return SetupState {
            hook_consent: settings.hook_consent,
            needs_hook_consent: false,
            consent_files: Vec::new(),
            codenotch_hooks_folders: Vec::new(),
            new_install_folders: Vec::new(),
            transport_error: input.transport_error.map(str::to_owned),
            control_off: false,
            missing_hooks_accounts: Vec::new(),
            install_disabled: false,
        };
    }
    let registry = input.registry;
    let paths = registry.paths();
    // As the passes see them (a mirrored `~\.claude` is everyone's).
    let accounts = super::wire_hooks::hook_accounts(registry);
    let folders = registry.folders();
    SetupState {
        hook_consent: settings.hook_consent,
        needs_hook_consent: settings.hook_consent.is_none(),
        consent_files: consent_files(&accounts, &folders, paths),
        // Read before any consent: the card says what is there.
        codenotch_hooks_folders: input.hooks.codenotch_hooks_folders(),
        new_install_folders: folders_beyond_consent(input, &accounts, &folders),
        transport_error: input.transport_error.map(str::to_owned),
        control_off: settings.hook_consent == Some(true) && !settings.hooks_enabled,
        missing_hooks_accounts: accounts_without_hooks(input, &accounts, &folders),
        install_disabled: installs_disabled,
    }
}

/// VS Code workspaces' folders that get the hooks under a yes given before
/// they were covered: said once (`HookManager`'s scope, `foldersBeyondConsent`).
fn folders_beyond_consent(
    input: &SetupInput<'_>,
    accounts: &[Account],
    folders: &[RunFolder],
) -> Vec<String> {
    let settings = input.settings;
    if settings.hook_consent != Some(true)
        || !settings.hooks_enabled
        || settings.hook_consent_scope >= ControlSettings::CURRENT_CONSENT_SCOPE
    {
        return Vec::new();
    }
    let paths = input.registry.paths();
    folders
        .iter()
        .filter(|folder| is_install_target(accounts, folder))
        .filter(|folder| is_window_dir(paths, &folder.config_dir.to_string_lossy()))
        .map(|folder| folder_display(paths, folder.id.as_str(), input.window_names))
        .collect()
}

/// Tracked accounts with a folder to hook where our hooks aren't in place,
/// as far as is known (`HookHealth.make`): only once hooks are on, since
/// before consent the card already explains why nothing is live. An account
/// that runs nowhere now (only a Claude Parallel Profiles store holds it) has
/// nothing to hook.
fn accounts_without_hooks(
    input: &SetupInput<'_>,
    accounts: &[Account],
    folders: &[RunFolder],
) -> Vec<String> {
    if !input.settings.hooks_active() || input.hooks.installs_disabled() {
        return Vec::new();
    }
    let missing = input.hooks.folders_missing_hooks(accounts, folders);
    accounts
        .iter()
        .filter(|account| account.is_tracked)
        .filter(|account| account.run_dirs.iter().any(|dir| missing.contains(dir)))
        .map(|account| account.label.clone())
        .collect()
}

// ---- the pane ----

/// The Claude Code settings pane's whole state.
pub fn settings_snapshot(input: &SettingsInput<'_>) -> SettingsSnapshot {
    let setup = &input.setup;
    let registry = setup.registry;
    let paths = registry.paths();
    let defaults = default_names(registry);
    let rows: Vec<AccountView> = registry
        .identities()
        .iter()
        .zip(&defaults)
        .map(|(identity, default)| account_view(input, identity, default))
        .collect();
    let installs_disabled = setup.sealed || setup.hooks.installs_disabled();
    let mut ui = setup.settings.ui();
    ui.hotkey_ok = input.hotkey_ok;
    ui.hotkey_message = input.hotkey_message.map(str::to_owned);
    SettingsSnapshot {
        suggestions: registry
            .suggestions()
            .iter()
            .map(|suggestion| Suggestion {
                path: paths.abbreviate(&suggestion.config_dir),
                reason: suggestion.reason.text().to_owned(),
            })
            .collect(),
        unsigned_folders: registry
            .unsigned_folders()
            .iter()
            .map(|folder| folder_display(paths, folder.dir(), setup.window_names))
            .collect(),
        hooks: hooks_section(input, &rows, installs_disabled),
        usage: usage_section(input, &rows),
        cloud: input.cloud.clone(),
        attention: ui,
        notifications: notifications_section(setup.settings, input.notify_permission),
        advanced: AdvancedSection {
            session_count: input.session_count,
            review_count: input.review_count,
        },
        sealed: setup.sealed,
        setup: setup_state(setup),
        accounts: rows.into_iter().map(|view| view.row).collect(),
    }
}

/// Each identity's name without the user's own (the rename field's
/// placeholder), told apart from the others' as the registry does: the
/// registry's own default names keep a custom name for a renamed account.
fn default_names(registry: &AccountRegistry) -> Vec<String> {
    let paths = registry.paths();
    let stand_ins: Vec<Folder> = registry
        .identities()
        .iter()
        .map(|identity| {
            let mut folder = identity.representative(paths);
            folder.custom_label = None;
            folder
        })
        .collect();
    naming::named(&stand_ins, paths)
        .into_iter()
        .zip(&stand_ins)
        .map(|(named, plain)| named.default_label.unwrap_or_else(|| plain.label(paths)))
        .collect()
}

/// One account row with what the Hooks section sums up.
struct AccountView {
    row: AccountRow,
    tracked: bool,
    state: HookState,
    /// The folders its hooks go into, and how many have them.
    hook_folders: usize,
    hooked_folders: usize,
}

// ---- hook state of an account ----

/// What an account's settings.json files add up to (`AccountSettingsItem.
/// HookState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookState {
    Installed,
    NotInstalled,
    /// Hooks are switched off, the account isn't tracked, or nothing runs it.
    Off,
    Unreadable,
    MissingFolder,
}

/// One status over an account's run folders: hooks in place only when in
/// every one, unreadable when any is, the status line only when in every one.
/// `None` while any folder is still unread (`SettingsPaneItems.aggregate`).
fn aggregate(statuses: &[Option<FolderHookStatus>]) -> Option<FolderHookStatus> {
    if statuses.is_empty() || statuses.iter().any(Option::is_none) {
        return None;
    }
    let known: Vec<&FolderHookStatus> = statuses.iter().flatten().collect();
    let mut status = known[0].clone();
    if known.len() > 1 {
        status.config_dir_exists = known.iter().any(|s| s.config_dir_exists);
        status.settings_readable = known.iter().all(|s| s.settings_readable);
        status.hooks_registered = known.iter().any(|s| s.hooks_registered);
        status.hooks_installed = known.iter().all(|s| s.hooks_installed);
        status.status_line_installed = known.iter().all(|s| s.status_line_installed);
        status.codenotch_hooks = known.iter().any(|s| s.codenotch_hooks);
        status.last_error = known.iter().find_map(|s| s.last_error.clone());
        status.not_hookable = known.iter().find_map(|s| s.not_hookable.clone());
        status.status_line_left_alone = known.iter().find_map(|s| s.status_line_left_alone.clone());
        status.newest_backup = known.iter().find_map(|s| s.newest_backup.clone());
    }
    Some(status)
}

/// `AccountHookSummary.make`: the kind and, when the hooks aren't there, why,
/// in words that say what the user loses.
fn hook_summary(
    status: Option<&FolderHookStatus>,
    hooks_enabled: bool,
    installs_disabled: bool,
    hidden: bool,
) -> (HookKind, Option<String>) {
    if hidden && status.is_none_or(|s| !s.hooks_installed) {
        return (HookKind::Hidden, None);
    }
    let Some(status) = status else {
        return (HookKind::Unknown, None);
    };
    if !status.config_dir_exists {
        return (
            HookKind::MissingFolder,
            Some("The folder doesn't exist any more.".to_owned()),
        );
    }
    if !status.settings_readable {
        return (
            HookKind::Unreadable,
            Some(
                "settings.json isn't valid JSON, so it is left untouched. Fix it, then reinstall."
                    .to_owned(),
            ),
        );
    }
    if status.hooks_installed {
        return (HookKind::Installed, None);
    }
    let why = if installs_disabled {
        "Installing is off for this run (--no-install).".to_owned()
    } else if !hooks_enabled {
        "Hooks are turned off (see Hooks below).".to_owned()
    } else if let Some(error) = status.last_error.as_deref().filter(|e| !e.is_empty()) {
        error.to_owned()
    } else if let Some(reason) = status.not_hookable.as_deref().filter(|e| !e.is_empty()) {
        reason.to_owned()
    } else {
        // They are found through Claude Code's own session files.
        "Its sessions still show; answer their prompts where Claude Code runs (VS Code or the terminal) until the hooks are in."
            .to_owned()
    };
    (HookKind::NotInstalled, Some(why))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookKind {
    Installed,
    NotInstalled,
    Unreadable,
    MissingFolder,
    /// Not tracked, and none of our hooks left behind.
    Hidden,
    /// Not read yet.
    Unknown,
}

fn hook_state(kind: HookKind, hooks_enabled: bool, installs_disabled: bool) -> HookState {
    match kind {
        HookKind::Installed => HookState::Installed,
        HookKind::Unreadable => HookState::Unreadable,
        HookKind::MissingFolder => HookState::MissingFolder,
        HookKind::Hidden => HookState::Off,
        HookKind::NotInstalled | HookKind::Unknown => {
            if hooks_enabled && !installs_disabled {
                HookState::NotInstalled
            } else {
                HookState::Off
            }
        }
    }
}

/// The chip's text (`AccountSettingsItem.hookTitle`).
fn hook_title(
    state: HookState,
    hook_folders: usize,
    hooked_folders: usize,
    tracked: bool,
) -> String {
    if hook_folders == 0 && tracked {
        return "No folder runs it now".to_owned();
    }
    if hook_folders > 1 && matches!(state, HookState::Installed | HookState::NotInstalled) {
        return format!("Hooks in {hooked_folders} of {hook_folders} folders");
    }
    match state {
        HookState::Installed => "Hooks installed",
        HookState::NotInstalled => "Hooks not installed",
        HookState::Off => "Hooks off",
        HookState::Unreadable => "settings.json unreadable",
        HookState::MissingFolder => "Folder missing",
    }
    .to_owned()
}

fn hook_tone(state: HookState) -> &'static str {
    match state {
        HookState::Installed => "ok",
        HookState::NotInstalled => "warning",
        HookState::Unreadable => "critical",
        HookState::Off | HookState::MissingFolder => "neutral",
    }
}

// ---- one account ----

fn account_view(
    input: &SettingsInput<'_>,
    identity: &IdentityAccount,
    default_label: &str,
) -> AccountView {
    let setup = &input.setup;
    let paths = setup.registry.paths();
    let hooks = setup.hooks;
    let account = identity.to_account(paths);
    let hooks_enabled = setup.settings.hooks_active();
    let installs_disabled = setup.sealed || hooks.installs_disabled();

    let statuses: Vec<Option<FolderHookStatus>> = identity
        .run_dirs
        .iter()
        .map(|folder| {
            hooks
                .has_status(&folder.id)
                .then(|| hooks.folder_status(&folder.id))
        })
        .collect();
    let disk = aggregate(&statuses);
    let (kind, detail) = hook_summary(
        disk.as_ref(),
        hooks_enabled,
        installs_disabled,
        !account.is_tracked,
    );
    // Only its store holds it now: nothing to hook, nothing wrong.
    let runs_nowhere = identity.run_dirs.is_empty() && !identity.store_dirs.is_empty();
    let state = if runs_nowhere {
        HookState::Off
    } else {
        hook_state(kind, hooks_enabled, installs_disabled)
    };
    let hook_folders = if identity.run_dirs.is_empty() && identity.store_dirs.is_empty() {
        1
    } else {
        identity.run_dirs.len()
    };
    let installed_folders = statuses
        .iter()
        .flatten()
        .filter(|status| status.hooks_installed)
        .count();
    let hooked_folders = if hook_folders <= 1 {
        if state == HookState::Installed {
            hook_folders
        } else {
            0
        }
    } else {
        installed_folders.min(hook_folders)
    };

    let reading = input.readings.get(&identity.id);
    let usage_stale = matches!(
        reading,
        Some(RingReading::Reading {
            status: RingStatus::Stale,
            ..
        })
    );
    let can_install = hooks_enabled
        && !installs_disabled
        && !input.busy
        && account.is_tracked
        && hook_folders > 0
        && state != HookState::MissingFolder;
    let multi = identity.run_dirs.len() + identity.store_dirs.len() > 1;
    let tracked = account.is_tracked;
    let row = AccountRow {
        identity_id: identity.id.to_string(),
        ring_id: identity.ring_id.to_string(),
        label: account.label.clone(),
        default_label: default_label.to_owned(),
        has_custom_label: identity
            .custom_label
            .as_deref()
            .is_some_and(|label| !label.is_empty()),
        color_index: account.color_index,
        is_default: identity.includes_default,
        identity_line: [
            Some(
                identity
                    .email
                    .clone()
                    .unwrap_or_else(|| "Not signed in".into()),
            ),
            account.plan_name.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
        folder_summary: folder_summary(identity, setup),
        folders: folder_rows(identity, input, hooks_enabled),
        usage_line: usage_line(reading, account.ring_shown, tracked, input.now),
        usage_stale,
        hook_state: hook_title(state, hook_folders, hooked_folders, tracked),
        hook_state_tone: hook_tone(state).to_owned(),
        live_status_line: disk.as_ref().is_some_and(|s| s.status_line_installed),
        // Off by choice needs no explanation beside the switch.
        hook_problem: match state {
            HookState::Off | HookState::Installed => None,
            _ => detail,
        },
        hook_problem_critical: state == HookState::Unreadable,
        is_tracked: tracked,
        ring_shown: account.ring_shown,
        is_signed_in: account.is_signed_in,
        launch_command: account.launch_command.clone(),
        can_install,
        install_label: if state == HookState::Installed {
            "Reinstall hooks"
        } else {
            "Install hooks"
        }
        .to_owned(),
        can_forget: account.can_forget,
        forget_caption: forget_caption(identity, multi),
    };
    AccountView {
        row,
        tracked,
        state,
        hook_folders,
        hooked_folders,
    }
}

fn forget_caption(identity: &IdentityAccount, multi: bool) -> String {
    if !multi {
        return "This app's hooks are removed from its settings.json and it stops being tracked. The folder and its sessions are left alone."
            .to_owned();
    }
    let windows = if identity.window_dir_ids.is_empty() {
        ""
    } else {
        ", VS Code workspaces opened later included (~\\.claude keeps them while another account is tracked)"
    };
    format!(
        "This app's hooks are removed from every folder it runs in and it stops being tracked{windows}. Its folders, stores and sessions are left alone."
    )
}

/// Where the account lives: "Runs in ~\.claude and 2 VS Code workspaces", and
/// on lines of their own "Store (Claude Parallel Profiles): ~\.claude-paras"
/// or "Stores (…):" with one per line (a path never wraps in the middle). A
/// workspace's folder outlives its window, so they are counted as workspaces.
fn folder_summary(identity: &IdentityAccount, setup: &SetupInput<'_>) -> String {
    let paths = setup.registry.paths();
    let layout = setup.registry.layout();
    let shown = |folder: &Folder| {
        let path = paths.abbreviate(folder.dir());
        if layout.is_adopted(paths, folder.dir()) {
            format!("{path} (also a {PROFILES} account)")
        } else {
            path
        }
    };
    let windows = identity
        .run_dirs
        .iter()
        .filter(|folder| is_window_dir(paths, folder.dir()))
        .count();
    let mut places: Vec<String> = identity
        .run_dirs
        .iter()
        .filter(|folder| !is_window_dir(paths, folder.dir()))
        .map(shown)
        .collect();
    if windows > 0 {
        places.push(if windows == 1 {
            "1 VS Code workspace".to_owned()
        } else {
            format!("{windows} VS Code workspaces")
        });
    }
    let mut parts = vec![if places.is_empty() {
        "Runs nowhere now".to_owned()
    } else {
        format!("Runs in {}", join_list(&places))
    }];
    let stores: Vec<String> = identity
        .store_dirs
        .iter()
        .map(|folder| paths.abbreviate(folder.dir()))
        .collect();
    match stores.as_slice() {
        [] => {}
        [only] => parts.push(format!("Store ({PROFILES}): {only}")),
        many => parts.push(format!(
            "Stores ({PROFILES}):\n{}",
            many.iter()
                .map(|store| format!("  {store}"))
                .collect::<Vec<_>>()
                .join("\n")
        )),
    }
    parts.join("\n")
}

const PROFILES: &str = "Claude Parallel Profiles";

/// Every folder of an account with what its hooks are.
fn folder_rows(
    identity: &IdentityAccount,
    input: &SettingsInput<'_>,
    hooks_enabled: bool,
) -> Vec<FolderRow> {
    let setup = &input.setup;
    let paths = setup.registry.paths();
    let layout = setup.registry.layout();
    let tracked = !identity.is_hidden;
    let mut rows: Vec<FolderRow> = identity
        .run_dirs
        .iter()
        .map(|folder| {
            let dir = folder.dir();
            let window = is_window_dir(paths, dir);
            let name = window.then(|| setup.window_names.get(dir)).flatten();
            let known = setup.hooks.has_status(&folder.id);
            let status = setup.hooks.folder_status(&folder.id);
            let state = if !tracked {
                "Not tracked"
            } else if !known {
                "Checking…"
            } else if !status.config_dir_exists {
                "Folder missing"
            } else if !status.settings_readable {
                "settings.json unreadable"
            } else if status.hooks_installed {
                if status.status_line_installed {
                    "Hooks and live status line"
                } else {
                    "Hooks installed"
                }
            } else if hooks_enabled {
                "Hooks not installed"
            } else {
                "Hooks off"
            };
            let shown = paths.abbreviate(dir);
            let (title, role) = if window {
                match name {
                    Some(name) => (format!("VS Code · {name}"), shown),
                    None => (shown, "VS Code workspace".to_owned()),
                }
            } else if folder.is_default(paths) {
                (shown, "Default".to_owned())
            } else if layout.is_adopted(paths, dir) {
                (shown, format!("Folder · also a {PROFILES} account"))
            } else {
                (shown, "Folder".to_owned())
            };
            FolderRow {
                id: folder.id.to_string(),
                title,
                role,
                state: state.to_owned(),
                status_line_note: tracked
                    .then(|| status.status_line_left_alone.clone())
                    .flatten(),
                not_hookable: status.not_hookable.clone(),
                codenotch_hooks: status.codenotch_hooks,
            }
        })
        .collect();
    rows.extend(identity.store_dirs.iter().map(|folder| FolderRow {
        id: folder.id.to_string(),
        title: paths.abbreviate(folder.dir()),
        role: "Account store".to_owned(),
        state: "Read only, never changed".to_owned(),
        status_line_note: None,
        not_hookable: None,
        codenotch_hooks: false,
    }));
    rows
}

// ---- usage ----

/// "5-hour 34% · weekly 12% · 4m ago", or why there is no reading
/// (`SettingsUsageLine.text`).
pub fn usage_line(
    reading: Option<&RingReading>,
    ring_shown: bool,
    tracked: bool,
    now: SystemTime,
) -> String {
    // Tracking off takes the ring and the usage checks with it (GUX-7).
    if !tracked {
        return NOT_TRACKED_USAGE.to_owned();
    }
    if !ring_shown {
        return RING_OFF_USAGE.to_owned();
    }
    match reading {
        None => "No reading yet".to_owned(),
        Some(RingReading::Waiting) => "Waiting for the first reading".to_owned(),
        Some(RingReading::SignInNeeded) => "Not signed in to Claude".to_owned(),
        Some(RingReading::Unavailable(message)) => message.clone(),
        Some(RingReading::Failed(message)) => format!("Usage check failed: {message}"),
        Some(RingReading::Reading { usage, status, .. }) => {
            let windows = ring_windows::windows(usage, now);
            let mut parts = Vec::new();
            for (id, name) in [(SESSION_ID, "5-hour"), (WEEKLY_ID, "weekly")] {
                if let Some(window) = windows.iter().find(|window| window.id == id) {
                    parts.push(format!(
                        "{name} {}",
                        percent_text(window.used_fraction * 100.0)
                    ));
                }
            }
            let age = age_text(usage.updated_at, now);
            parts.push(if *status == RingStatus::Stale {
                format!("{age}, stale")
            } else {
                age
            });
            if windows.is_empty() {
                "No limits reported".to_owned()
            } else {
                parts.join(" · ")
            }
        }
    }
}

/// "23%": rounded down so the UI never shows a limit as hit before it is;
/// values over 100 are kept ("110%"), over 999 read "999%", a value that
/// isn't a number "–" (`UsageFormatter.percent`).
fn percent_text(utilization: f64) -> String {
    if !utilization.is_finite() {
        return "–".to_owned();
    }
    // A fraction times 100 can land just under a whole number (0.29 * 100):
    // the nudge keeps 29% from reading 28%.
    let shown = (utilization.clamp(0.0, MAX_DISPLAYED_PERCENT) + 1e-9).floor();
    format!("{shown}%")
}

fn usage_section(input: &SettingsInput<'_>, rows: &[AccountView]) -> UsageSection {
    let settings = input.setup.settings;
    let caption = match settings.probe_interval() {
        Some(interval) => usage_check_caption(interval.as_secs() / 60),
        None => {
            "Off: readings come only from live status lines and Claude Code's own cache.".to_owned()
        }
    };
    let desktop_caption = if input.desktop_format == Some(DesktopCacheFormat::Blockfile) {
        UNSUPPORTED_FORMAT_TEXT
    } else {
        "Claude Desktop keeps the limits it last saw on disk; no token is involved."
    };
    UsageSection {
        interval_minutes: settings.usage_probe_interval_minutes,
        interval_options: INTERVAL_OPTIONS.to_vec(),
        interval_caption: caption,
        desktop_cache: settings.reads_desktop_usage_cache,
        desktop_caption: desktop_caption.to_owned(),
        desktop_format: input.desktop_format.map(|f| f.as_str().to_owned()),
        accounts: rows
            .iter()
            .filter(|view| view.tracked)
            .map(|view| UsageAccountLine {
                ring_id: view.row.ring_id.clone(),
                label: view.row.label.clone(),
                color_index: view.row.color_index,
                line: view.row.usage_line.clone(),
            })
            .collect(),
        refreshing: input.refreshing,
    }
}

/// How the usage check works, said where it can be switched off: it runs
/// Claude Code itself, which may update its own files (`UsageCheckCopy`; the
/// Mac's "To find claude it may ask your login shell once" has no Windows
/// counterpart).
pub fn usage_check_caption(minutes: u64) -> String {
    format!(
        "Every {minutes} min, only when nothing fresher has arrived, this app runs Claude Code's own usage check in each signed-in account (Claude Code may update its own files there); this app never reads your login token."
    )
}

// ---- hooks section ----

fn hooks_section(
    input: &SettingsInput<'_>,
    rows: &[AccountView],
    installs_disabled: bool,
) -> HooksSection {
    let setup = &input.setup;
    let settings = setup.settings;
    let paths = setup.registry.paths();
    let consent = settings.hook_consent == Some(true);
    let (summary, warning) = hooks_summary(settings, installs_disabled, rows);
    let (version, path) = claude_found(settings, input.versions);
    let chosen = settings
        .claude_binary_path
        .as_deref()
        .is_some_and(|path| !path.is_empty());
    HooksSection {
        consent: settings.hook_consent,
        enabled: settings.hooks_active(),
        enabled_locked: !consent || installs_disabled || input.busy,
        summary,
        summary_warning: warning,
        status_line: settings.status_line_integration,
        pipe_name: input.pipe_name.to_owned(),
        claude_version: version,
        claude_path: path.clone(),
        claude_path_chosen: chosen,
        claude_caption: match (&path, chosen) {
            (Some(path), true) => format!("Hooks are written for {path}."),
            _ => "Hooks are written for the oldest claude found, so every version reads them."
                .to_owned(),
        },
        last_change: last_change(input, paths),
        install_allowed: !installs_disabled,
        busy: input.busy,
        footnotes: FOOTNOTES.iter().map(|text| (*text).to_owned()).collect(),
    }
}

/// The Hooks switch's detail: what is true, not what was asked for
/// (`SettingsPaneModel.hooksSummary`), and whether it deserves the amber ink.
fn hooks_summary(
    settings: &ControlSettings,
    installs_disabled: bool,
    rows: &[AccountView],
) -> (String, bool) {
    if settings.hook_consent != Some(true) {
        return ("Turn on Claude Code control first.".to_owned(), false);
    }
    if installs_disabled {
        return (
            "Installing is off for this run (--no-install).".to_owned(),
            false,
        );
    }
    if !settings.hooks_enabled {
        return ("Off: no account has this app's hooks.".to_owned(), false);
    }
    let tracked: Vec<&AccountView> = rows.iter().filter(|view| view.tracked).collect();
    // Tracked accounts with a folder to hook (one only a Claude Parallel
    // Profiles store holds right now has none).
    let total = tracked.iter().filter(|view| view.hook_folders > 0).count();
    let folders: usize = tracked.iter().map(|view| view.hook_folders).sum();
    let hooked_folders: usize = tracked.iter().map(|view| view.hooked_folders).sum();
    let installed = tracked
        .iter()
        .filter(|view| view.state == HookState::Installed)
        .count();
    let accounts = |total: usize| {
        if total == 1 {
            "the tracked account".to_owned()
        } else {
            format!("{total} tracked accounts")
        }
    };
    // An account in several folders (VS Code windows): count folders.
    if folders > total {
        return if hooked_folders == folders {
            (
                format!("Installed in all {folders} folders of {}.", accounts(total)),
                false,
            )
        } else {
            (
                format!(
                    "Installed in {hooked_folders} of {folders} folders of {}.",
                    accounts(total)
                ),
                true,
            )
        };
    }
    if total == 0 {
        return ("No tracked accounts.".to_owned(), false);
    }
    if installed == total {
        (
            if total == 1 {
                "Installed in the tracked account.".to_owned()
            } else {
                format!("Installed in all {total} tracked accounts.")
            },
            false,
        )
    } else {
        (
            format!("Installed in {installed} of {total} tracked accounts."),
            true,
        )
    }
}

/// The version and path Settings shows for Claude Code: the chosen binary's
/// when one was chosen, else the oldest version the app has seen (the one the
/// hooks are written for) with the binary it came from.
fn claude_found(
    settings: &ControlSettings,
    versions: &[VersionSighting],
) -> (Option<String>, Option<String>) {
    let binaries = || {
        versions
            .iter()
            .filter(|sighting| sighting.source == VersionSource::Binary)
    };
    if let Some(chosen) = settings
        .claude_binary_path
        .as_deref()
        .filter(|path| !path.is_empty())
    {
        let version = binaries()
            .find(|sighting| {
                sighting
                    .path
                    .as_deref()
                    .is_some_and(|path| path.to_string_lossy() == chosen)
            })
            .and_then(|sighting| sighting.version.clone());
        return (version, Some(chosen.to_owned()));
    }
    let oldest = effective_version(versions).map(|version| version.to_string());
    let path = binaries()
        .find(|sighting| {
            oldest.is_some() && sighting.version.is_some() && sighting.version == oldest
        })
        .or_else(|| binaries().find(|sighting| sighting.path.is_some()))
        .and_then(|sighting| sighting.path.as_ref())
        .map(|path| path.to_string_lossy().into_owned());
    (oldest, path)
}

/// "Last change: settings.json in ~\.claude-work. The previous version is kept
/// as settings.json.agentnotch-…bak." from the hook manager's last pass
/// (`HooksChangedNotice.text`). Several folders: "…kept beside each file".
fn last_change(input: &SettingsInput<'_>, paths: &Paths) -> Option<String> {
    let setup = &input.setup;
    let folders: Vec<&AccountId> = input
        .changed_folders
        .iter()
        .filter(|id| paths.is_absolute(id.as_str()))
        .collect();
    if folders.is_empty() {
        return None;
    }
    let names: Vec<String> = folders
        .iter()
        .map(|id| {
            let shown = paths.abbreviate(id.as_str());
            match setup.window_names.get(id.as_str()) {
                Some(name) => format!("{shown} (VS Code · {name})"),
                None => shown,
            }
        })
        .collect();
    let backup = match folders.as_slice() {
        [only] => setup
            .hooks
            .folder_status(only)
            .newest_backup
            .as_deref()
            .map(|path| path.to_string_lossy())
            // Both separators: a Windows path read on any host.
            .and_then(|path| path.rsplit(['\\', '/']).next().map(str::to_owned))
            .filter(|name| !name.is_empty()),
        _ => None,
    };
    let note = match backup {
        Some(name) => format!("The previous version is kept as {name}."),
        None => "The previous version is kept beside each file.".to_owned(),
    };
    Some(format!(
        "Last change: settings.json in {}. {note}",
        join_list(&names)
    ))
}

// ---- notifications ----

fn notifications_section(
    settings: &ControlSettings,
    permission: NotifyPermission,
) -> NotificationsSection {
    let (word, text, disabled) = match permission {
        NotifyPermission::Allowed => ("allowed", "Allowed", false),
        NotifyPermission::DisabledForApp => ("disabled_for_app", "Off in Windows Settings", true),
        NotifyPermission::DisabledForUser => ("disabled_for_user", "Off in Windows Settings", true),
        NotifyPermission::DisabledByPolicy => {
            ("disabled_by_policy", "Off in Windows Settings", true)
        }
        NotifyPermission::Unavailable => ("unavailable", "Banners need the installed app", false),
    };
    NotificationsSection {
        notify_needs_input: settings.notify_needs_input,
        notify_ready_for_review: settings.notify_ready_for_review,
        permission: word.to_owned(),
        permission_text: text.to_owned(),
        permission_warning: disabled
            && (settings.notify_needs_input || settings.notify_ready_for_review),
    }
}

// ---- words ----

/// A folder as Settings lists it: "VS Code · project" for a workspace's
/// folder a session named, else the `~\` path.
fn folder_display(paths: &Paths, dir: &str, names: &BTreeMap<String, String>) -> String {
    match names.get(dir).filter(|_| is_window_dir(paths, dir)) {
        Some(name) => format!("VS Code · {name}"),
        None => paths.abbreviate(dir),
    }
}

/// English list formatting: "a", "a and b", "a, b, and c" (Foundation's
/// `ListFormatter` in en-US).
pub fn join_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [init @ .., last] => format!("{}, and {last}", init.join(", ")),
    }
}

/// Where "Turn on" puts the hooks, for the consent card (`ConsentScope`):
/// the default folder, VS Code workspaces' folders (new ones are set up as
/// they appear) and standalone folders; Claude Parallel Profiles' stores
/// never get hooks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsentScope {
    pub includes_default: bool,
    pub window_count: usize,
    pub standalone_folders: Vec<String>,
    pub store_count: usize,
    pub parallel_profiles: bool,
}

impl ConsentScope {
    /// Folders it installs into now.
    pub fn folder_count(&self) -> usize {
        usize::from(self.includes_default) + self.window_count + self.standalone_folders.len()
    }

    /// "Installs into ~\.claude and your VS Code workspaces' folders (3 now;
    /// new ones are set up automatically). Claude Parallel Profiles' account
    /// stores never get hooks." `None` without Claude Parallel Profiles (the
    /// file list says it). A workspace's folder stays after its window
    /// closes, so the count is of workspaces, not open windows.
    pub fn sentence(&self) -> Option<String> {
        if !self.parallel_profiles {
            return None;
        }
        let mut places = Vec::new();
        if self.includes_default {
            places.push("~\\.claude".to_owned());
        }
        places.push(format!(
            "your VS Code workspaces' folders ({} now; new ones are set up automatically)",
            self.window_count
        ));
        places.extend(self.standalone_folders.iter().cloned());
        Some(format!(
            "Installs into {}. Claude Parallel Profiles' account stores never get hooks.",
            join_list(&places)
        ))
    }

    /// What "Turn on" covers now, from the registry's tracked run folders.
    pub fn of(registry: &AccountRegistry, accounts: &[Account], folders: &[RunFolder]) -> Self {
        let paths = registry.paths();
        let targets: Vec<&RunFolder> = folders
            .iter()
            .filter(|folder| is_install_target(accounts, folder))
            .collect();
        let dir = |folder: &&RunFolder| folder.config_dir.to_string_lossy().into_owned();
        ConsentScope {
            includes_default: targets.iter().any(|f| paths.is_default_config_dir(&dir(f))),
            window_count: targets
                .iter()
                .filter(|f| is_window_dir(paths, &dir(f)))
                .count(),
            standalone_folders: targets
                .iter()
                .filter(|f| !paths.is_default_config_dir(&dir(f)) && !is_window_dir(paths, &dir(f)))
                .map(|f| paths.abbreviate(&dir(f)))
                .collect(),
            store_count: folders
                .iter()
                .filter(|folder| folder.kind == FolderKind::Store)
                .count(),
            parallel_profiles: registry.layout().extension_detected,
        }
    }
}

/// The scope notice's message: "This version puts its hooks and status line
/// in 3 VS Code workspaces' folders too, …" (`ConsentCopy.scopeMessage`).
pub fn scope_message(folder_count: usize) -> String {
    let folders = if folder_count == 1 {
        "1 VS Code workspace's folder".to_owned()
    } else {
        format!("{folder_count} VS Code workspaces' folders")
    };
    format!(
        "This version puts its hooks and status line in {folders} too, and in new ones as they appear: {PROFILES} runs Claude Code there. Each settings.json has a backup beside it. Account stores never get hooks."
    )
}
