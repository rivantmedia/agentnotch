//! The engine's settings (§4.12), one value per `claudeControl.*` switch of
//! the Mac plus the Windows additions. `an-core` is their only writer
//! (§1.2); `persist::settings` reads and writes `control-settings.json`
//! (unknown keys kept, a bad value falls back to its default).
//!
//! Owner after WP0: WP7. One rule per key decides what a value may be, for
//! the file (`persist::settings`: a bad value falls back to the default of
//! its key alone) and for `Call::SetSetting` ([`ControlSettings::validated`]:
//! the same value is refused with `invalid`), so the two cannot drift.

use crate::hub::CallError;
use crate::model::UiSettings;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// The file's keys.
pub mod keys {
    pub const HOOK_CONSENT: &str = "hookConsent";
    pub const HOOK_CONSENT_SCOPE: &str = "hookConsentScope";
    pub const HOOKS_ENABLED: &str = "hooksEnabled";
    pub const STATUS_LINE_INTEGRATION: &str = "statusLineIntegration";
    pub const USAGE_PROBE_INTERVAL_MINUTES: &str = "usageProbeIntervalMinutes";
    pub const READS_DESKTOP_USAGE_CACHE: &str = "readsDesktopUsageCache";
    pub const CLAUDE_BINARY_PATH: &str = "claudeBinaryPath";
    pub const NOTIFY_NEEDS_INPUT: &str = "notifyNeedsInput";
    pub const NOTIFY_READY_FOR_REVIEW: &str = "notifyReadyForReview";
    pub const AUTO_OPEN: &str = "autoOpen";
    pub const HOLD_OPEN_WHILE_NEEDS_YOU: &str = "holdOpenWhileNeedsYou";
    pub const RING_BADGES: &str = "ringBadges";
    pub const RESTING_MARKS: &str = "restingMarks";
    pub const TRAY_BADGE: &str = "trayBadge";
    pub const RING_CLICK: &str = "ringClick";
    pub const SESSION_CLICK: &str = "sessionClick";
    pub const HOT_KEY: &str = "hotKey";
    pub const PANEL_PINNED: &str = "panelPinned";
    pub const SOUND: &str = "sound";
    pub const PEEK: &str = "peek";
    pub const PEEK_SECONDS: &str = "peekSeconds";
    pub const CLOUD_SYNC_ENABLED: &str = "cloudSyncEnabled";
    pub const CLOUD_SUMMARIES_ENABLED: &str = "cloudSummariesEnabled";
    pub const CLOUD_DEVICE_ID: &str = "cloudDeviceId";
    pub const TYPE_REPLIES: &str = "typeReplies";

    /// Every key of the file, in the order the file lists them.
    pub const ALL: [&str; 25] = [
        HOOK_CONSENT,
        HOOK_CONSENT_SCOPE,
        HOOKS_ENABLED,
        STATUS_LINE_INTEGRATION,
        USAGE_PROBE_INTERVAL_MINUTES,
        READS_DESKTOP_USAGE_CACHE,
        CLAUDE_BINARY_PATH,
        NOTIFY_NEEDS_INPUT,
        NOTIFY_READY_FOR_REVIEW,
        AUTO_OPEN,
        HOLD_OPEN_WHILE_NEEDS_YOU,
        RING_BADGES,
        RESTING_MARKS,
        TRAY_BADGE,
        RING_CLICK,
        SESSION_CLICK,
        HOT_KEY,
        PANEL_PINNED,
        SOUND,
        PEEK,
        PEEK_SECONDS,
        CLOUD_SYNC_ENABLED,
        CLOUD_SUMMARIES_ENABLED,
        CLOUD_DEVICE_ID,
        TYPE_REPLIES,
    ];
}

/// The choices of each multiple-choice setting, default first.
pub mod choices {
    pub const AUTO_OPEN: [&str; 3] = ["never", "needsInput", "needsInputOrDone"];
    pub const HOLD_OPEN: [&str; 2] = ["never", "always"];
    pub const RING_CLICK: [&str; 2] = ["openPanel", "refreshUsage"];
    pub const SESSION_CLICK: [&str; 3] = ["smart", "panel", "terminal"];
    pub const HOT_KEY: [&str; 3] = ["off", "ctrlAltSpace", "ctrlAltJ"];
    pub const PEEK_SECONDS: [u32; 3] = [5, 3, 10];
}

/// The settings the pages' `set_setting` may write (the pages' contract,
/// `lib/contract.cjs` `SETTINGS`). Consent, hooks, the status line, the
/// binary choice and the cloud switches have their own calls, so a page never
/// flips them here; the cloud thread does it through `Input::SetSetting`,
/// which takes any key.
pub const PAGE_KEYS: [&str; 17] = [
    keys::USAGE_PROBE_INTERVAL_MINUTES,
    keys::READS_DESKTOP_USAGE_CACHE,
    keys::NOTIFY_NEEDS_INPUT,
    keys::NOTIFY_READY_FOR_REVIEW,
    keys::AUTO_OPEN,
    keys::HOLD_OPEN_WHILE_NEEDS_YOU,
    keys::RING_BADGES,
    keys::RESTING_MARKS,
    keys::TRAY_BADGE,
    keys::RING_CLICK,
    keys::SESSION_CLICK,
    keys::HOT_KEY,
    keys::PANEL_PINNED,
    keys::SOUND,
    keys::PEEK,
    keys::PEEK_SECONDS,
    keys::TYPE_REPLIES,
];

/// The longest probe interval a setting may hold: a day (the pages offer up
/// to 30 minutes; a hand-edited file may ask for more, within reason).
pub const MAX_PROBE_INTERVAL_MINUTES: u32 = 24 * 60;

/// The longest `claudeBinaryPath` (a Windows path can't be longer).
const MAX_BINARY_PATH_CHARS: usize = 32_767;

/// Why a value was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingError {
    /// No such key.
    Unknown,
    /// The key's rule, in words ("must be true or false").
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlSettings {
    /// `None` until "Turn on" / "Not now". Nothing is written to any
    /// settings.json until it is `Some(true)`.
    pub hook_consent: Option<bool>,
    /// What the yes covered.
    pub hook_consent_scope: u32,
    /// Counts only with consent.
    pub hooks_enabled: bool,
    pub status_line_integration: bool,
    /// 0 turns the probe off; the effective interval is at least 300 s.
    pub usage_probe_interval_minutes: u32,
    pub reads_desktop_usage_cache: bool,
    pub claude_binary_path: Option<String>,
    pub notify_needs_input: bool,
    pub notify_ready_for_review: bool,
    /// `never` (the Windows default until R6 is settled) | `needsInput` |
    /// `needsInputOrDone`.
    pub auto_open: String,
    /// `never` | `always` (no camera on Windows).
    pub hold_open_while_needs_you: String,
    pub ring_badges: bool,
    pub resting_marks: bool,
    pub tray_badge: bool,
    /// `openPanel` | `refreshUsage`.
    pub ring_click: String,
    /// `smart` | `panel` | `terminal`.
    pub session_click: String,
    /// `off` | `ctrlAltSpace` | `ctrlAltJ`.
    pub hot_key: String,
    pub panel_pinned: bool,
    pub sound: bool,
    pub peek: bool,
    /// 3, 5 or 10.
    pub peek_seconds: u32,
    pub cloud_sync_enabled: bool,
    pub cloud_summaries_enabled: bool,
    /// Made on demand (an uppercase UUID v4).
    pub cloud_device_id: Option<String>,
    /// Typing replies into a terminal is opt-in on Windows (§4.8).
    pub type_replies: bool,
}

impl Default for ControlSettings {
    fn default() -> Self {
        ControlSettings {
            hook_consent: None,
            hook_consent_scope: Self::CURRENT_CONSENT_SCOPE,
            hooks_enabled: true,
            status_line_integration: true,
            usage_probe_interval_minutes: 5,
            reads_desktop_usage_cache: true,
            claude_binary_path: None,
            notify_needs_input: true,
            notify_ready_for_review: true,
            auto_open: choices::AUTO_OPEN[0].into(),
            hold_open_while_needs_you: choices::HOLD_OPEN[0].into(),
            ring_badges: true,
            resting_marks: true,
            tray_badge: true,
            ring_click: choices::RING_CLICK[0].into(),
            session_click: choices::SESSION_CLICK[0].into(),
            hot_key: choices::HOT_KEY[0].into(),
            panel_pinned: false,
            sound: true,
            peek: true,
            peek_seconds: choices::PEEK_SECONDS[0],
            cloud_sync_enabled: false,
            cloud_summaries_enabled: false,
            cloud_device_id: None,
            type_replies: false,
        }
    }
}

impl ControlSettings {
    /// The scope a yes to "Turn on" covers today (HS§3.1).
    pub const CURRENT_CONSENT_SCOPE: u32 = 2;
    /// The shortest probe interval, whatever the setting says.
    pub const MINIMUM_PROBE_INTERVAL: Duration = Duration::from_secs(300);

    /// Hooks are wanted: consent given and not turned off.
    pub fn hooks_active(&self) -> bool {
        self.hook_consent == Some(true) && self.hooks_enabled
    }

    /// How often to probe; `None` when probes are off.
    pub fn probe_interval(&self) -> Option<Duration> {
        (self.usage_probe_interval_minutes > 0).then(|| {
            Duration::from_secs(u64::from(self.usage_probe_interval_minutes) * 60)
                .max(Self::MINIMUM_PROBE_INTERVAL)
        })
    }

    /// Sets `key` from a JSON value by the key's own rule. Nothing changes
    /// when the value is refused.
    pub fn set_value(&mut self, key: &str, value: &Value) -> Result<(), SettingError> {
        fn invalid<T>(rule: &str) -> Result<T, SettingError> {
            Err(SettingError::Invalid(rule.to_owned()))
        }
        fn boolean(value: &Value) -> Result<bool, SettingError> {
            value
                .as_bool()
                .map_or_else(|| invalid("must be true or false"), Ok)
        }
        fn choice(value: &Value, allowed: &[&str]) -> Result<String, SettingError> {
            match value.as_str() {
                Some(word) if allowed.contains(&word) => Ok(word.to_owned()),
                _ => Err(SettingError::Invalid(format!(
                    "must be one of {}",
                    allowed.join(", ")
                ))),
            }
        }
        fn count(value: &Value) -> Option<u32> {
            value.as_u64().and_then(|n| u32::try_from(n).ok())
        }
        match key {
            keys::HOOK_CONSENT => {
                self.hook_consent = match value {
                    Value::Null => None,
                    Value::Bool(answer) => Some(*answer),
                    _ => return invalid("must be true, false or null"),
                }
            }
            keys::HOOK_CONSENT_SCOPE => {
                self.hook_consent_scope = count(value)
                    .map_or_else(|| invalid("must be a whole number"), Ok)?;
            }
            keys::HOOKS_ENABLED => self.hooks_enabled = boolean(value)?,
            keys::STATUS_LINE_INTEGRATION => self.status_line_integration = boolean(value)?,
            keys::USAGE_PROBE_INTERVAL_MINUTES => {
                self.usage_probe_interval_minutes = match count(value) {
                    Some(minutes) if minutes <= MAX_PROBE_INTERVAL_MINUTES => minutes,
                    _ => {
                        return invalid(&format!(
                            "must be 0 (off) or a whole number of minutes up to {MAX_PROBE_INTERVAL_MINUTES}"
                        ))
                    }
                }
            }
            keys::READS_DESKTOP_USAGE_CACHE => self.reads_desktop_usage_cache = boolean(value)?,
            keys::CLAUDE_BINARY_PATH => {
                self.claude_binary_path = match value {
                    Value::Null => None,
                    Value::String(path)
                        if path.chars().count() <= MAX_BINARY_PATH_CHARS && !path.contains('\0') =>
                    {
                        (!path.is_empty()).then(|| path.clone())
                    }
                    _ => return invalid("must be a path or null"),
                }
            }
            keys::NOTIFY_NEEDS_INPUT => self.notify_needs_input = boolean(value)?,
            keys::NOTIFY_READY_FOR_REVIEW => self.notify_ready_for_review = boolean(value)?,
            keys::AUTO_OPEN => self.auto_open = choice(value, &choices::AUTO_OPEN)?,
            keys::HOLD_OPEN_WHILE_NEEDS_YOU => {
                self.hold_open_while_needs_you = choice(value, &choices::HOLD_OPEN)?
            }
            keys::RING_BADGES => self.ring_badges = boolean(value)?,
            keys::RESTING_MARKS => self.resting_marks = boolean(value)?,
            keys::TRAY_BADGE => self.tray_badge = boolean(value)?,
            keys::RING_CLICK => self.ring_click = choice(value, &choices::RING_CLICK)?,
            keys::SESSION_CLICK => self.session_click = choice(value, &choices::SESSION_CLICK)?,
            keys::HOT_KEY => self.hot_key = choice(value, &choices::HOT_KEY)?,
            keys::PANEL_PINNED => self.panel_pinned = boolean(value)?,
            keys::SOUND => self.sound = boolean(value)?,
            keys::PEEK => self.peek = boolean(value)?,
            keys::PEEK_SECONDS => {
                self.peek_seconds = match count(value) {
                    Some(seconds) if choices::PEEK_SECONDS.contains(&seconds) => seconds,
                    _ => return invalid("must be 3, 5 or 10"),
                }
            }
            keys::CLOUD_SYNC_ENABLED => self.cloud_sync_enabled = boolean(value)?,
            keys::CLOUD_SUMMARIES_ENABLED => self.cloud_summaries_enabled = boolean(value)?,
            keys::CLOUD_DEVICE_ID => {
                self.cloud_device_id = match value {
                    Value::Null => None,
                    Value::String(id) if id.len() == 36 => match uuid::Uuid::parse_str(id) {
                        Ok(parsed) => Some(parsed.hyphenated().to_string().to_uppercase()),
                        Err(_) => return invalid("must be a UUID"),
                    },
                    _ => return invalid("must be a UUID or null"),
                }
            }
            keys::TYPE_REPLIES => self.type_replies = boolean(value)?,
            _ => return Err(SettingError::Unknown),
        }
        Ok(())
    }

    /// The settings after `key` is set to `value`, or why it can't be. This is
    /// `Input::SetSetting`'s rule (any key; the cloud thread's switches too).
    pub fn validated(&self, key: &str, value: &Value) -> Result<ControlSettings, CallError> {
        let mut next = self.clone();
        match next.set_value(key, value) {
            Ok(()) => Ok(next),
            Err(SettingError::Unknown) => Err(CallError::invalid(format!(
                "Unknown setting {}.",
                shown(key)
            ))),
            Err(SettingError::Invalid(rule)) => {
                Err(CallError::invalid(format!("{} {rule}.", shown(key))))
            }
        }
    }

    /// `Call::SetSetting` from a page: only the keys of [`PAGE_KEYS`].
    pub fn validated_from_page(
        &self,
        key: &str,
        value: &Value,
    ) -> Result<ControlSettings, CallError> {
        if !PAGE_KEYS.contains(&key) {
            return Err(CallError::invalid(format!(
                "Unknown setting {}.",
                shown(key)
            )));
        }
        self.validated(key, value)
    }

    /// This install's id on the website: the saved one, else a new uppercase
    /// UUID v4 (the caller saves the settings when it changed).
    pub fn device_id(&mut self) -> String {
        self.cloud_device_id
            .get_or_insert_with(|| uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase())
            .clone()
    }

    /// The part the pages need.
    pub fn ui(&self) -> UiSettings {
        UiSettings {
            panel_open_mode: self.auto_open.clone(),
            hold_open: self.hold_open_while_needs_you.clone(),
            ring_badges: self.ring_badges,
            resting_marks: self.resting_marks,
            ring_click: self.ring_click.clone(),
            hover_click: self.session_click.clone(),
            hotkey: self.hot_key.clone(),
            panel_pinned: self.panel_pinned,
            peek: self.peek,
            peek_seconds: self.peek_seconds,
            type_replies: self.type_replies,
            sound: self.sound,
            tray_badge: self.tray_badge,
            // The hub fills in what the hot key service reports.
            hotkey_ok: true,
            hotkey_message: None,
        }
    }
}

/// A key from a page, short enough for a message.
fn shown(key: &str) -> String {
    let mut text: String = key.chars().take(48).collect();
    if key.chars().count() > 48 {
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_defaults() {
        let s = ControlSettings::default();
        assert_eq!(s.hook_consent, None);
        assert!(!s.hooks_active());
        assert_eq!(s.auto_open, "never");
        assert!(!s.type_replies);
        assert_eq!(s.probe_interval(), Some(Duration::from_secs(300)));
        let off = ControlSettings {
            usage_probe_interval_minutes: 0,
            ..s.clone()
        };
        assert_eq!(off.probe_interval(), None);
        let long = ControlSettings {
            usage_probe_interval_minutes: 30,
            ..s
        };
        assert_eq!(long.probe_interval(), Some(Duration::from_secs(1800)));
    }
}
