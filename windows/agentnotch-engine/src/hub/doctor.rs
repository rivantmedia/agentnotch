//! The doctor's report (design §4.14): one line per fact, stable prefixes
//! the smoke test and support greps rely on. Read-only and synchronous: it
//! works on a hub that was never started (the command line's `doctor`),
//! starts no thread, writes no file, never runs `claude` and never prints a
//! token, a prompt or a file's body: only names, counts and paths the user
//! already sees in Settings.
//!
//! [`render`] is pure, over [`DoctorFacts`], so the golden test fixes every
//! fact. [`Core::doctor_facts`] gathers them from the engine; a hub that is
//! running answers from what its pages were last shown
//! ([`facts_from_projection`]) and says "?" for what only the core holds.
//!
//! Owner: WP7.

use super::api::{DoctorExtras, HubConfig};
use super::core_state::{Core, Projection};
use crate::core::roots::IDENTIFIER;
use crate::hooks::manager::is_install_target;
use crate::platform::Platform;
use crate::runtime_types::{FolderHookStatus, VersionSighting, VersionSource};
use crate::usage::{desktop, locator, versions};
use std::path::PathBuf;
use std::time::SystemTime;

/// One account's line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountFact {
    pub ring_id: String,
    pub label: String,
    pub signed_in: bool,
    /// The run folders, as Settings titles them (`~\.claude`).
    pub folders: Vec<String>,
}

/// Everything the report says, already decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorFacts {
    pub version: String,
    pub exe: PathBuf,
    pub sealed: bool,
    /// This app's own token.
    pub elevated_app: bool,
    /// A running instance's token, when one answered.
    pub elevated_running: Option<bool>,
    pub sessions_elevated: u32,
    /// `off` | `on` | `evaluation` | `unknown`.
    pub smart_app_control: String,
    pub data: PathBuf,
    pub support: PathBuf,
    /// `Some(true)` private, `Some(false)` not, `None` not made yet.
    pub support_private: Option<bool>,
    /// The `updates:` line, whole.
    pub updates: String,
    pub pipe_name: String,
    /// A running instance's `(sessions, accounts)`.
    pub running: Option<(u32, u32)>,
    pub hook_exe: PathBuf,
    pub hook_exe_present: bool,
    pub accounts: Vec<AccountFact>,
    /// `granted` | `declined` | `unasked`.
    pub consent: String,
    /// Run folders holding this app's hooks, of those that get them; `None`
    /// when this report can't say.
    pub hooks_installed: Option<usize>,
    pub hook_targets: Option<usize>,
    /// `exec` | `string` | `mixed` | `none`, or `?`.
    pub form: String,
    pub exec_form_min: Option<String>,
    /// `<version or unknown> (<where>)`.
    pub versions: Vec<String>,
    pub status_lines_wrapped: usize,
    pub status_lines_left_alone: usize,
    pub left_alone_reason: Option<String>,
    pub claude: Option<PathBuf>,
    /// `simple` | `blockfile` | `absent` | `unknown`.
    pub desktop_cache: String,
    pub deep_link: String,
    pub autostart: bool,
    pub shortcut_present: bool,
    pub providers: Vec<String>,
}

/// One line's text: no line break from a user's label can split a line.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn count(value: Option<usize>) -> String {
    value.map_or_else(|| "?".to_owned(), |n| n.to_string())
}

/// The report: one line per fact, a line break at the end of each.
pub fn render(facts: &DoctorFacts) -> String {
    let mut lines = vec![
        format!("Agent Notch doctor v{} ({IDENTIFIER})", facts.version),
        format!("exe: {}", facts.exe.display()),
        format!("sealed: {}", yes_no(facts.sealed)),
        format!(
            "elevated: app={} running={} sessions-elevated={}",
            yes_no(facts.elevated_app),
            facts
                .elevated_running
                .map_or("none", |running| yes_no(running)),
            facts.sessions_elevated
        ),
        format!(
            "smart-app-control: {}{}",
            facts.smart_app_control,
            if facts.smart_app_control == "on" {
                "   (unsigned builds and hook copies are blocked; see the release notes)"
            } else {
                ""
            }
        ),
        format!("data: {}", facts.data.display()),
        format!(
            "support: {} (private: {})",
            facts.support.display(),
            match facts.support_private {
                Some(true) => "yes",
                Some(false) => "no",
                None => "not made yet",
            }
        ),
        one_line(&facts.updates),
    ];
    lines.push(match facts.running {
        _ if facts.sealed => format!("pipe: {} sealed (no server)", facts.pipe_name),
        Some((sessions, accounts)) => format!(
            "pipe: {} running (sessions {sessions}, accounts {accounts})",
            facts.pipe_name
        ),
        None => format!("pipe: {} no instance running", facts.pipe_name),
    });
    lines.push(format!(
        "hook exe: {} ({})",
        facts.hook_exe.display(),
        if facts.hook_exe_present {
            "present"
        } else {
            "missing"
        }
    ));
    lines.push(format!("accounts: {}", facts.accounts.len()));
    for account in &facts.accounts {
        lines.push(one_line(&format!(
            "account: {} \"{}\" signed-in={} folders={}",
            account.ring_id,
            account.label,
            yes_no(account.signed_in),
            if account.folders.is_empty() {
                "none".to_owned()
            } else {
                account.folders.join(",")
            }
        )));
    }
    lines.push(format!(
        "hooks: consent={} installed={}/{} form={} exec-form-min={}",
        facts.consent,
        count(facts.hooks_installed),
        count(facts.hook_targets),
        facts.form,
        facts.exec_form_min.as_deref().unwrap_or("unset")
    ));
    lines.push(format!(
        "claude-versions: {}",
        if facts.sealed {
            "none (sealed)".to_owned()
        } else if facts.versions.is_empty() {
            "none known (the doctor never runs claude)".to_owned()
        } else {
            facts.versions.join(", ")
        }
    ));
    lines.push(format!(
        "status-line: wrapped={} left-alone={}{}",
        facts.status_lines_wrapped,
        facts.status_lines_left_alone,
        facts
            .left_alone_reason
            .as_deref()
            .map(|reason| format!(" (reason: {reason})"))
            .unwrap_or_default()
    ));
    lines.push(match &facts.claude {
        _ if facts.sealed => "claude: not looked for (sealed)".to_owned(),
        Some(path) => format!("claude: {} (not run by the doctor)", path.display()),
        None => "claude: not found".to_owned(),
    });
    lines.push(format!("desktop-cache: {}", facts.desktop_cache));
    lines.push(one_line(&format!("deep-link: {}", facts.deep_link)));
    lines.push(format!(
        "autostart: {}",
        if facts.autostart { "on" } else { "off" }
    ));
    lines.push(format!(
        "notifications: {}",
        if facts.shortcut_present {
            format!("shortcut present (AUMID {IDENTIFIER})")
        } else {
            "shortcut missing".to_owned()
        }
    ));
    lines.push(one_line(&format!(
        "providers: {}",
        facts.providers.join("; ")
    )));
    lines.join("\n") + "\n"
}

// ---- gathering ----

/// What the extras and the platform alone say.
fn base_facts(
    cfg: &HubConfig,
    platform: &Platform,
    extra: &DoctorExtras,
    sealed: bool,
) -> DoctorFacts {
    DoctorFacts {
        version: cfg.app_version.clone(),
        exe: extra.exe.clone(),
        sealed,
        elevated_app: platform.device.elevated(),
        elevated_running: extra.running.as_ref().map(|status| status.elevated),
        sessions_elevated: 0,
        smart_app_control: platform
            .device
            .smart_app_control()
            .filter(|state| matches!(state.as_str(), "on" | "off" | "evaluation"))
            .unwrap_or_else(|| "unknown".to_owned()),
        data: cfg.roots.data.clone(),
        support: cfg.roots.support.clone(),
        support_private: platform.files.is_private(&cfg.roots.support).ok(),
        updates: extra.updates.clone(),
        pipe_name: cfg.pipe_name.clone(),
        running: extra
            .running
            .as_ref()
            .map(|status| (status.sessions, status.accounts)),
        hook_exe: cfg.hook_exe.clone(),
        hook_exe_present: cfg.hook_exe.is_file(),
        accounts: Vec::new(),
        consent: "unasked".to_owned(),
        hooks_installed: None,
        hook_targets: None,
        form: "?".to_owned(),
        exec_form_min: None,
        versions: Vec::new(),
        status_lines_wrapped: 0,
        status_lines_left_alone: 0,
        left_alone_reason: None,
        claude: None,
        desktop_cache: "absent".to_owned(),
        deep_link: extra.deep_link.clone(),
        autostart: extra.autostart,
        shortcut_present: extra.shortcut_present,
        providers: extra.providers.clone(),
    }
}

/// A sealed run's facts: nothing is read from the PC.
pub(crate) fn sealed_facts(cfg: &HubConfig, extra: &DoctorExtras) -> DoctorFacts {
    DoctorFacts {
        version: cfg.app_version.clone(),
        exe: extra.exe.clone(),
        sealed: true,
        elevated_app: false,
        elevated_running: None,
        sessions_elevated: 0,
        smart_app_control: "unknown".to_owned(),
        data: cfg.roots.data.clone(),
        support: cfg.roots.support.clone(),
        support_private: None,
        updates: extra.updates.clone(),
        pipe_name: cfg.pipe_name.clone(),
        running: None,
        hook_exe: cfg.hook_exe.clone(),
        hook_exe_present: false,
        accounts: Vec::new(),
        consent: "unasked".to_owned(),
        hooks_installed: Some(0),
        hook_targets: Some(0),
        form: "none".to_owned(),
        exec_form_min: None,
        versions: Vec::new(),
        status_lines_wrapped: 0,
        status_lines_left_alone: 0,
        left_alone_reason: None,
        claude: None,
        desktop_cache: "absent".to_owned(),
        deep_link: extra.deep_link.clone(),
        autostart: extra.autostart,
        shortcut_present: extra.shortcut_present,
        providers: extra.providers.clone(),
    }
}

fn account_facts(projection: &Projection) -> Vec<AccountFact> {
    projection
        .settings
        .accounts
        .iter()
        .map(|account| AccountFact {
            ring_id: account.ring_id.clone(),
            label: account.label.clone(),
            signed_in: account.is_signed_in,
            folders: account
                .folders
                .iter()
                .map(|folder| folder.title.clone())
                .collect(),
        })
        .collect()
}

fn source_name(source: VersionSource) -> &'static str {
    match source {
        VersionSource::Binary => "binary",
        VersionSource::BundledVsCode => "vscode extension",
        VersionSource::BundledDesktop => "desktop",
        VersionSource::Registry => "session",
        VersionSource::StatusLine => "status line",
    }
}

/// The versions known, then the ones the disk names without running
/// anything: bundled folders' names, and installed binaries not asked.
fn version_lines(known: &[VersionSighting], sightings: &[VersionSighting]) -> Vec<String> {
    let mut all: Vec<VersionSighting> = known.to_vec();
    for sighting in sightings {
        let seen = all
            .iter()
            .any(|other| other.source == sighting.source && other.path == sighting.path);
        if !seen {
            all.push(sighting.clone());
        }
    }
    all.iter()
        .map(|sighting| {
            let version = sighting.version.as_deref().unwrap_or("unknown");
            format!("{version} ({})", source_name(sighting.source))
        })
        .collect()
}

impl Core {
    /// Every fact of the report from the engine's own stores and a
    /// read-only look at each folder's `settings.json`.
    pub(crate) fn doctor_facts(&mut self, now: SystemTime, extra: &DoctorExtras) -> DoctorFacts {
        self.ensure_discovered(now);
        let projection = self.project(now);
        let mut facts = base_facts(&self.cfg, &self.platform, extra, false);
        facts.accounts = account_facts(&projection);
        facts.consent = projection.status.hook_consent.clone();
        facts.exec_form_min = self.hooks_w.facts().exec_form_min.clone();
        facts.desktop_cache = desktop::desktop_cache_format(&self.cfg.roots)
            .as_str()
            .to_owned();
        facts.sessions_elevated = self
            .session_views()
            .iter()
            .filter(|view| {
                view.pid
                    .is_some_and(|pid| self.platform.processes.elevated(pid) == Some(true))
            })
            .count() as u32;

        // Hooks: only the folders that get them, read the way the pass
        // reads them back (read-only, whatever the consent).
        let accounts = super::wire_hooks::hook_accounts(&self.registry);
        let setup = crate::hooks::apply::Setup::detect();
        let statuses: Vec<FolderHookStatus> = self
            .registry
            .folders()
            .iter()
            .filter(|folder| is_install_target(&accounts, folder))
            .map(|folder| {
                crate::hooks::apply::read_status(
                    &folder.config_dir,
                    self.platform.files.as_ref(),
                    &setup,
                )
            })
            .collect();
        facts.hook_targets = Some(statuses.len());
        facts.hooks_installed = Some(statuses.iter().filter(|s| s.hooks_installed).count());
        facts.form = form_of(&statuses);
        facts.status_lines_wrapped = statuses.iter().filter(|s| s.status_line_installed).count();
        let left_alone: Vec<&String> = statuses
            .iter()
            .filter_map(|s| s.status_line_left_alone.as_ref())
            .collect();
        facts.status_lines_left_alone = left_alone.len();
        facts.left_alone_reason = left_alone.first().map(|reason| (*reason).clone());

        // Versions: what the hub knows, plus names on disk. Nothing is run.
        let mut seen = versions::bundled_versions(&versions::bundled_roots(&self.cfg.roots));
        let env_path = std::env::var_os("PATH").unwrap_or_default();
        let choice = self
            .settings
            .claude_binary_path
            .as_deref()
            .map(std::path::Path::new);
        seen.extend(
            locator::installed(&self.cfg.roots, choice, &env_path, &|path| path.is_file())
                .into_iter()
                .map(|path| VersionSighting {
                    source: VersionSource::Binary,
                    path: Some(path),
                    version: None,
                }),
        );
        facts.versions = version_lines(&self.versions, &seen);
        facts.claude = locator::locate_claude_from(
            &self.cfg.roots,
            choice,
            self.usage.remembered_binary().map(std::path::Path::new),
            &env_path,
            &|path| path.is_file(),
        )
        .map(|binary| binary.program);
        facts
    }
}

/// `exec` | `string` | `mixed` | `none` over the forms the folders hold.
fn form_of(statuses: &[FolderHookStatus]) -> String {
    let mut forms: Vec<&str> = statuses
        .iter()
        .filter_map(|status| status.form.as_deref())
        .collect();
    forms.dedup();
    forms.sort_unstable();
    forms.dedup();
    match forms.as_slice() {
        [] => "none".to_owned(),
        [one] => (*one).to_owned(),
        _ => "mixed".to_owned(),
    }
}

/// A running hub's report: the accounts, consent and Claude Code's path as
/// its pages were last shown; what only the core holds reads "?".
pub(crate) fn facts_from_projection(
    cfg: &HubConfig,
    platform: &Platform,
    extra: &DoctorExtras,
    projection: &Projection,
) -> DoctorFacts {
    let mut facts = base_facts(cfg, platform, extra, false);
    facts.accounts = account_facts(projection);
    facts.consent = projection.status.hook_consent.clone();
    facts.desktop_cache = desktop::desktop_cache_format(&cfg.roots)
        .as_str()
        .to_owned();
    facts.claude = projection
        .settings
        .hooks
        .claude_path
        .as_ref()
        .map(PathBuf::from);
    facts.exec_form_min = crate::hooks::facts::ClaudeCodeFacts::compiled_in().exec_form_min;
    facts
}
