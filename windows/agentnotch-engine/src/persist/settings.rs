//! `control-settings.json` v1 (§4.12): every setting under its key, the
//! whole object kept so a key this build doesn't know survives a write, and
//! a value of the wrong type or out of range falls back to its default
//! without touching the others.
//!
//! Owner after WP0: WP7.

use crate::core::settings::{choices, keys, ControlSettings};
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

    /// Every setting, each from its key when valid, else its default.
    pub fn settings(&self) -> ControlSettings {
        let d = ControlSettings::default();
        let get = |key: &str| self.values.get(key);
        let boolean =
            |key: &str, default: bool| get(key).and_then(Value::as_bool).unwrap_or(default);
        let count = |key: &str| {
            get(key)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        };
        let choice = |key: &str, allowed: &[&str], default: &str| {
            get(key)
                .and_then(Value::as_str)
                .filter(|v| allowed.contains(v))
                .unwrap_or(default)
                .to_owned()
        };
        let text = |key: &str| {
            get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        ControlSettings {
            hook_consent: get(keys::HOOK_CONSENT).and_then(Value::as_bool),
            hook_consent_scope: count(keys::HOOK_CONSENT_SCOPE).unwrap_or(d.hook_consent_scope),
            hooks_enabled: boolean(keys::HOOKS_ENABLED, d.hooks_enabled),
            status_line_integration: boolean(
                keys::STATUS_LINE_INTEGRATION,
                d.status_line_integration,
            ),
            usage_probe_interval_minutes: count(keys::USAGE_PROBE_INTERVAL_MINUTES)
                .unwrap_or(d.usage_probe_interval_minutes),
            reads_desktop_usage_cache: boolean(
                keys::READS_DESKTOP_USAGE_CACHE,
                d.reads_desktop_usage_cache,
            ),
            claude_binary_path: text(keys::CLAUDE_BINARY_PATH),
            notify_needs_input: boolean(keys::NOTIFY_NEEDS_INPUT, d.notify_needs_input),
            notify_ready_for_review: boolean(
                keys::NOTIFY_READY_FOR_REVIEW,
                d.notify_ready_for_review,
            ),
            auto_open: choice(keys::AUTO_OPEN, &choices::AUTO_OPEN, &d.auto_open),
            hold_open_while_needs_you: choice(
                keys::HOLD_OPEN_WHILE_NEEDS_YOU,
                &choices::HOLD_OPEN,
                &d.hold_open_while_needs_you,
            ),
            ring_badges: boolean(keys::RING_BADGES, d.ring_badges),
            resting_marks: boolean(keys::RESTING_MARKS, d.resting_marks),
            tray_badge: boolean(keys::TRAY_BADGE, d.tray_badge),
            ring_click: choice(keys::RING_CLICK, &choices::RING_CLICK, &d.ring_click),
            session_click: choice(
                keys::SESSION_CLICK,
                &choices::SESSION_CLICK,
                &d.session_click,
            ),
            hot_key: choice(keys::HOT_KEY, &choices::HOT_KEY, &d.hot_key),
            panel_pinned: boolean(keys::PANEL_PINNED, d.panel_pinned),
            sound: boolean(keys::SOUND, d.sound),
            peek: boolean(keys::PEEK, d.peek),
            peek_seconds: count(keys::PEEK_SECONDS)
                .filter(|s| choices::PEEK_SECONDS.contains(s))
                .unwrap_or(d.peek_seconds),
            cloud_sync_enabled: boolean(keys::CLOUD_SYNC_ENABLED, d.cloud_sync_enabled),
            cloud_summaries_enabled: boolean(
                keys::CLOUD_SUMMARIES_ENABLED,
                d.cloud_summaries_enabled,
            ),
            cloud_device_id: text(keys::CLOUD_DEVICE_ID),
            type_replies: boolean(keys::TYPE_REPLIES, d.type_replies),
        }
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
