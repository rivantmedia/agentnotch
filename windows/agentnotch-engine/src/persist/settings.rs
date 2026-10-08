//! `control-settings.json` v1 (§4.12): every setting under its key, the
//! whole object kept so a key this build doesn't know survives a write, and
//! a value of the wrong type or out of range falls back to its default
//! without touching the others.
//!
//! Owner after WP0: WP7.

use crate::core::settings::{keys, ControlSettings};
use serde_json::{Map, Value};

pub const FILE_NAME: &str = "control-settings.json";
pub const VERSION: u32 = 1;

/// The file as read: its object, unknown keys included.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SettingsFile {
    pub values: Map<String, Value>,
}

impl SettingsFile {
    /// `None` when the bytes aren't a JSON object (the defaults then apply,
    /// and the next write replaces the file).
    pub fn parse(bytes: &[u8]) -> Option<SettingsFile> {
        match serde_json::from_slice::<Value>(bytes).ok()? {
            Value::Object(values) => Some(SettingsFile { values }),
            _ => None,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        super::encode_pretty_sorted(&self.values)
    }

    /// Every setting, each from its key when valid, else its default. The
    /// rule of each key is `ControlSettings::set_value`'s, the one a
    /// `set_setting` call is held to.
    pub fn settings(&self) -> ControlSettings {
        let mut settings = ControlSettings::default();
        for key in keys::ALL {
            if let Some(value) = self.values.get(key) {
                // A refused value leaves the key's default.
                let _ = settings.set_value(key, value);
            }
        }
        settings
    }

    /// Writes every setting under its key (and the version), keeping the
    /// keys this build doesn't know.
    pub fn apply(&mut self, s: &ControlSettings) {
        let mut set = |key: &str, value: Value| {
            self.values.insert(key.to_owned(), value);
        };
        set("version", Value::from(VERSION));
        set(
            keys::HOOK_CONSENT,
            s.hook_consent.map_or(Value::Null, Value::Bool),
        );
        set(keys::HOOK_CONSENT_SCOPE, s.hook_consent_scope.into());
        set(keys::HOOKS_ENABLED, s.hooks_enabled.into());
        set(
            keys::STATUS_LINE_INTEGRATION,
            s.status_line_integration.into(),
        );
        set(
            keys::USAGE_PROBE_INTERVAL_MINUTES,
            s.usage_probe_interval_minutes.into(),
        );
        set(
            keys::READS_DESKTOP_USAGE_CACHE,
            s.reads_desktop_usage_cache.into(),
        );
        set(
            keys::CLAUDE_BINARY_PATH,
            s.claude_binary_path
                .clone()
                .map_or(Value::Null, Value::String),
        );
        set(keys::NOTIFY_NEEDS_INPUT, s.notify_needs_input.into());
        set(
            keys::NOTIFY_READY_FOR_REVIEW,
            s.notify_ready_for_review.into(),
        );
        set(keys::AUTO_OPEN, s.auto_open.clone().into());
        set(
            keys::HOLD_OPEN_WHILE_NEEDS_YOU,
            s.hold_open_while_needs_you.clone().into(),
        );
        set(keys::RING_BADGES, s.ring_badges.into());
        set(keys::RESTING_MARKS, s.resting_marks.into());
        set(keys::TRAY_BADGE, s.tray_badge.into());
        set(keys::RING_CLICK, s.ring_click.clone().into());
        set(keys::SESSION_CLICK, s.session_click.clone().into());
        set(keys::HOT_KEY, s.hot_key.clone().into());
        set(keys::PANEL_PINNED, s.panel_pinned.into());
        set(keys::SOUND, s.sound.into());
        set(keys::PEEK, s.peek.into());
        set(keys::PEEK_SECONDS, s.peek_seconds.into());
        set(keys::CLOUD_SYNC_ENABLED, s.cloud_sync_enabled.into());
        set(
            keys::CLOUD_SUMMARIES_ENABLED,
            s.cloud_summaries_enabled.into(),
        );
        set(
            keys::CLOUD_DEVICE_ID,
            s.cloud_device_id.clone().map_or(Value::Null, Value::String),
        );
        set(keys::TYPE_REPLIES, s.type_replies.into());
    }
}
