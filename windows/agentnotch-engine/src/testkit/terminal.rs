//! Terminals, console typing, toasts and sounds that only record: tests
//! never type into a real terminal or post a real notification.
//!
//! Owner after WP0: WP6.

use super::lock;
use crate::platform::{
    Chime, ConsoleInfo, ConsoleInput, ConsoleTarget, FocusOutcome, FocusStep, Foreground, HostApp,
    HostKind, Notifier, NotifyPermission, ProcessTable, Sounds, Terminals, Toast, TypeOutcome,
};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct FakeTerminals {
    hosts: Mutex<HashMap<u32, HostApp>>,
    consoles: Mutex<HashMap<u32, ConsoleInfo>>,
    focus_steps: Mutex<Vec<FocusStep>>,
    focus_outcome: Mutex<Option<FocusOutcome>>,
    foreground: Mutex<Option<Foreground>>,
    titles: Mutex<HashMap<u64, String>>,
    visible: Mutex<bool>,
}

impl FakeTerminals {
    pub fn set_host(&self, claude_pid: u32, host: HostApp) {
        lock(&self.hosts).insert(claude_pid, host);
    }

    pub fn set_console(&self, claude_pid: u32, info: ConsoleInfo) {
        lock(&self.consoles).insert(claude_pid, info);
    }

    pub fn set_focus_outcome(&self, outcome: FocusOutcome) {
        *lock(&self.focus_outcome) = Some(outcome);
    }

    pub fn set_foreground(&self, foreground: Option<Foreground>) {
        *lock(&self.foreground) = foreground;
    }

    pub fn set_visible(&self, visible: bool) {
        *lock(&self.visible) = visible;
    }

    /// Every focus step run, in order.
    pub fn focus_steps(&self) -> Vec<FocusStep> {
        lock(&self.focus_steps).clone()
    }
}

impl Terminals for FakeTerminals {
    fn classify_host(&self, claude_pid: u32, _table: &ProcessTable) -> HostApp {
        lock(&self.hosts)
            .get(&claude_pid)
            .cloned()
            .unwrap_or(HostApp {
                kind: HostKind::Unknown,
                window: None,
                host_pid: None,
                exe_path: None,
            })
    }

    fn console_info(&self, claude_pid: u32) -> ConsoleInfo {
        lock(&self.consoles)
            .get(&claude_pid)
            .cloned()
            .unwrap_or_else(|| ConsoleInfo {
                error: Some("No console in the test.".into()),
                ..ConsoleInfo::default()
            })
    }

    fn run_focus(&self, step: &FocusStep) -> FocusOutcome {
        lock(&self.focus_steps).push(step.clone());
        lock(&self.focus_outcome)
            .clone()
            .unwrap_or(FocusOutcome::NotFound)
    }

    fn foreground(&self) -> Option<Foreground> {
        lock(&self.foreground).clone()
    }

    fn window_title(&self, window: u64) -> Option<String> {
        lock(&self.titles).get(&window).cloned()
    }

    fn wt_tab_titles(&self, _window: u64) -> Option<Vec<(String, bool)>> {
        None
    }

    fn any_terminal_visible(&self) -> bool {
        *lock(&self.visible)
    }

    fn watch_foreground(&self, _sink: crossbeam_channel::Sender<crate::platform::Foreground>) {}
}

/// Records what would have been typed; answers with a set outcome.
#[derive(Default)]
pub struct FakeConsole {
    typed: Mutex<Vec<(ConsoleTarget, String, bool)>>,
    outcome: Mutex<Option<TypeOutcome>>,
}

impl FakeConsole {
    pub fn set_outcome(&self, outcome: TypeOutcome) {
        *lock(&self.outcome) = Some(outcome);
    }

    /// (target, text, whether Return was pressed).
    pub fn typed(&self) -> Vec<(ConsoleTarget, String, bool)> {
        lock(&self.typed).clone()
    }
}

impl ConsoleInput for FakeConsole {
    fn type_text(
        &self,
        target: &ConsoleTarget,
        text: &str,
        recheck: &mut dyn FnMut() -> bool,
    ) -> TypeOutcome {
        let outcome = lock(&self.outcome).clone();
        match outcome {
            Some(TypeOutcome::Delivered) | None => {
                let submitted = recheck();
                lock(&self.typed).push((target.clone(), text.to_owned(), submitted));
                if submitted {
                    TypeOutcome::Delivered
                } else {
                    TypeOutcome::TypedNotSubmitted("Claude asked for something while your reply was typed; it's in the terminal, not sent.".into())
                }
            }
            Some(other) => other,
        }
    }
}

#[derive(Default)]
pub struct RecordingNotifier {
    posted: Mutex<Vec<Toast>>,
    withdrawn: Mutex<Vec<(String, String)>>,
    permission: Mutex<Option<NotifyPermission>>,
}

impl RecordingNotifier {
    pub fn posted(&self) -> Vec<Toast> {
        lock(&self.posted).clone()
    }

    pub fn withdrawn(&self) -> Vec<(String, String)> {
        lock(&self.withdrawn).clone()
    }

    pub fn set_permission(&self, permission: NotifyPermission) {
        *lock(&self.permission) = Some(permission);
    }
}

impl Notifier for RecordingNotifier {
    fn post(&self, t: &Toast) {
        lock(&self.posted).push(t.clone());
    }

    fn withdraw(&self, tag: &str, group: &str) {
        lock(&self.withdrawn).push((tag.to_owned(), group.to_owned()));
    }

    fn permission(&self) -> NotifyPermission {
        lock(&self.permission).unwrap_or(NotifyPermission::Allowed)
    }
}

#[derive(Default)]
pub struct RecordingSounds {
    played: Mutex<Vec<Chime>>,
}

impl RecordingSounds {
    pub fn played(&self) -> Vec<Chime> {
        lock(&self.played).clone()
    }
}

impl Sounds for RecordingSounds {
    fn play(&self, c: Chime) {
        lock(&self.played).push(c);
    }
}
