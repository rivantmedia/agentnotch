//! The engine's settings (§4.12), one value per `claudeControl.*` switch of
//! the Mac plus the Windows additions. `an-core` is their only writer
//! (§1.2); `persist::settings` reads and writes `control-settings.json`
//! (unknown keys kept, a bad value falls back to its default).
//!
//! Owner after WP0: WP7 (validation of `Call::SetSetting`, the snapshot).

use crate::model::UiSettings;
use serde::{Deserialize, Serialize};
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
        }
    }
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
