//! The panel shortcut (DESIGN-WIN §2.4 `hotkey.rs`, §5.3): registers the `hotKey` setting
//! (off, Ctrl+Alt+Space, Ctrl+Alt+J) through `agentnotch_win::hotkey` and reports each attempt to
//! the hub as `hotkey_status`, so Settings can say "That shortcut is taken by another app."
//!
//! The port of the Mac's `ClaudeHotKey`: the setting is read at start and on every settings
//! event; one registration is held at a time and dropped when the setting changes or goes off.
//! A press opens the panel for every account, or closes / takes the keyboard for one that is
//! already open (`panel::hotkey`, the Mac's `hotKeyPressed`).
//!
//! Never registered when sealed: a self-test must not grab a global chord on the runner's
//! desktop, and a sealed run reports nothing.

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agentnotch_engine::hub::Hub;
use agentnotch_win::hotkey::{Chord, HotkeyError};
use serde_json::{json, Value};
use tauri::AppHandle;

use super::panel;

/// The copy Settings shows when Windows refuses the chord because another app holds it.
const TAKEN: &str = "That shortcut is taken by another app.";

/// The chord a `hotKey` setting names; `off` and any value this build doesn't know register
/// nothing (the engine only stores the three it offers, so an unknown one is a newer settings
/// file, and off is the safe reading).
fn chord_for(setting: &str) -> Option<Chord> {
    match setting {
        "ctrlAltSpace" => Some(Chord::CtrlAltSpace),
        "ctrlAltJ" => Some(Chord::CtrlAltJ),
        _ => None,
    }
}

/// The arguments of `hotkey_status` for one attempt.
fn status_args(outcome: &Result<(), HotkeyError>) -> Value {
    match outcome {
        Ok(()) => json!({ "ok": true }),
        Err(HotkeyError::Taken) => json!({ "ok": false, "message": TAKEN }),
        Err(HotkeyError::Failed(text)) => json!({ "ok": false, "message": text }),
    }
}

/// What makes a registration: `agentnotch_win::hotkey::register`, or a fake in the tests.
trait Registrar {
    /// Dropping it gives the chord back.
    type Held: Send;
    fn register(
        &self,
        chord: Chord,
        on_press: Box<dyn Fn() + Send>,
    ) -> Result<Self::Held, HotkeyError>;
}

/// The real thing.
struct Windows;

impl Registrar for Windows {
    type Held = agentnotch_win::hotkey::Registration;

    fn register(
        &self,
        chord: Chord,
        on_press: Box<dyn Fn() + Send>,
    ) -> Result<Self::Held, HotkeyError> {
        agentnotch_win::hotkey::register(chord, on_press)
    }
}

/// The one registration a process holds, and what decides when to make or drop it.
struct Manager<R: Registrar> {
    registrar: R,
    /// Runs on a press; it must return at once (the hotkey's thread calls it).
    on_press: Arc<dyn Fn() + Send + Sync>,
    /// The setting last acted on (`None` = never yet). A failed attempt counts: the same setting
    /// arriving again is not tried again, or the report of its failure (which the hub may answer
    /// with a settings event) would start the attempt over, for ever. Off and on again retries.
    applied: Option<Option<Chord>>,
    held: Option<R::Held>,
    /// The last thing reported was a failure, which switching off must clear.
    failed: bool,
}

impl<R: Registrar> Manager<R> {
    fn new(registrar: R, on_press: Arc<dyn Fn() + Send + Sync>) -> Self {
        Manager {
            registrar,
            on_press,
            applied: None,
            held: None,
            failed: false,
        }
    }

    /// Makes the registration match `setting`. The arguments of the `hotkey_status` to report,
    /// or `None` when there is nothing to say (no change, or off with no failure on show).
    fn apply(&mut self, setting: &str) -> Option<Value> {
        let chord = chord_for(setting);
        if self.applied == Some(chord) {
            return None;
        }
        self.applied = Some(chord);
        // The old chord goes back before the new one is asked for: they may be the same key.
        self.held = None;
        let Some(chord) = chord else {
            // Settings shows the last report; a "taken" line under a shortcut that is now off
            // would be stale.
            return std::mem::take(&mut self.failed).then(|| status_args(&Ok(())));
        };
        let press = Arc::clone(&self.on_press);
        let outcome = match self.registrar.register(chord, Box::new(move || press())) {
            Ok(held) => {
                self.held = Some(held);
                Ok(())
            }
            Err(e) => Err(e),
        };
        self.failed = outcome.is_err();
        Some(status_args(&outcome))
    }
}

type Slot = Mutex<Manager<Windows>>;

fn slot(app: &AppHandle) -> &'static Slot {
    static SLOT: OnceLock<Slot> = OnceLock::new();
    SLOT.get_or_init(|| {
        let app = app.clone();
        Mutex::new(Manager::new(
            Windows,
            Arc::new(move || {
                // On the hotkey's own thread: hand the work over at once. The panel's rules take
                // their own lock and post the window work to the main thread.
                let app = app.clone();
                let _ = std::thread::Builder::new()
                    .name("an-hotkey-press".into())
                    .spawn(move || panel::hotkey(&app));
            }),
        ))
    })
}

/// Registers what the settings say now (from the hub's snapshot), once the hub is up.
pub(super) fn start(app: &AppHandle, hub: &Hub) {
    if super::sealed() {
        return;
    }
    apply_setting(app, &hub.settings_snapshot().attention.hotkey);
}

/// A settings event: registers, changes or drops the shortcut. Called on the hub's event thread:
/// registering is quick, and the report goes out through [`report`]'s own thread.
pub(super) fn apply_setting(app: &AppHandle, setting: &str) {
    if super::sealed() {
        return;
    }
    let args = slot(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .apply(setting);
    if let Some(args) = args {
        super::log(&format!("hotkey {setting:?}: {args}"));
        report(args);
    }
}

/// `hotkey_status` to the hub, on a thread of its own and in order: the event that led here
/// comes from the hub's thread, where a call back into the hub would wait on itself.
fn report(args: Value) {
    static REPORTER: OnceLock<Mutex<Sender<Value>>> = OnceLock::new();
    let sender = REPORTER.get_or_init(|| {
        let (sender, receiver) = channel::<Value>();
        let _ = std::thread::Builder::new()
            .name("an-hotkey-report".into())
            .spawn(move || {
                for args in receiver {
                    let Some(hub) = published_hub() else { continue };
                    if let Err(e) = super::calls::engine_call(&hub, "hotkey_status", args) {
                        super::log(&format!("hotkey_status: {}", e.message));
                    }
                }
            });
        Mutex::new(sender)
    });
    let _ = sender.lock().unwrap_or_else(|e| e.into_inner()).send(args);
}

/// The hub, waiting a little for it: the first settings event can come before `setup` has
/// published it (the sink is attached first so that nothing is lost).
fn published_hub() -> Option<Hub> {
    for _ in 0..100 {
        if let Some(hub) = super::hub() {
            return Some(hub);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

#[cfg(test)]
mod tests {
    use agentnotch_engine::hub::Call;

    use super::*;

    #[test]
    fn the_setting_names_the_chord() {
        assert_eq!(chord_for("ctrlAltSpace"), Some(Chord::CtrlAltSpace));
        assert_eq!(chord_for("ctrlAltJ"), Some(Chord::CtrlAltJ));
        assert_eq!(chord_for("off"), None);
        assert_eq!(chord_for(""), None);
        assert_eq!(chord_for("optionCommandJ"), None);
        // Case matters: the settings file holds exactly the engine's spelling.
        assert_eq!(chord_for("ctrlaltj"), None);
    }

    #[test]
    fn an_outcome_becomes_the_hubs_status() {
        assert_eq!(status_args(&Ok(())), json!({ "ok": true }));
        assert_eq!(
            status_args(&Err(HotkeyError::Taken)),
            json!({ "ok": false, "message": "That shortcut is taken by another app." })
        );
        assert_eq!(
            status_args(&Err(HotkeyError::Failed("no message queue".into()))),
            json!({ "ok": false, "message": "no message queue" })
        );
    }

    #[test]
    fn the_hub_accepts_what_the_glue_sends() {
        for args in [status_args(&Ok(())), status_args(&Err(HotkeyError::Taken))] {
            let call = Call::from_parts("hotkey_status", args.clone()).expect("a hotkey_status");
            let Call::HotkeyStatus { ok, message } = call else {
                panic!("not a hotkey_status");
            };
            assert_eq!(json!(ok), args["ok"]);
            assert_eq!(message.map(Value::from), args.get("message").cloned());
        }
    }

    /// What the fake registrar saw, in order.
    type Log = Arc<Mutex<Vec<String>>>;

    struct Fake {
        log: Log,
        /// Chords another app holds.
        taken: Vec<Chord>,
    }

    struct FakeHeld {
        chord: Chord,
        log: Log,
    }

    impl Drop for FakeHeld {
        fn drop(&mut self) {
            self.log
                .lock()
                .unwrap()
                .push(format!("drop {:?}", self.chord));
        }
    }

    impl Registrar for Fake {
        type Held = FakeHeld;

        fn register(
            &self,
            chord: Chord,
            _on_press: Box<dyn Fn() + Send>,
        ) -> Result<FakeHeld, HotkeyError> {
            self.log.lock().unwrap().push(format!("register {chord:?}"));
            if self.taken.contains(&chord) {
                return Err(HotkeyError::Taken);
            }
            Ok(FakeHeld {
                chord,
                log: Arc::clone(&self.log),
            })
        }
    }

    fn manager(taken: Vec<Chord>) -> (Manager<Fake>, Log) {
        let log: Log = Arc::default();
        let fake = Fake {
            log: Arc::clone(&log),
            taken,
        };
        (Manager::new(fake, Arc::new(|| {})), log)
    }

    fn seen(log: &Log) -> Vec<String> {
        log.lock().unwrap().clone()
    }

    #[test]
    fn off_and_unknown_settings_register_nothing() {
        let (mut manager, log) = manager(Vec::new());
        assert_eq!(manager.apply("off"), None);
        assert_eq!(manager.apply("something else"), None);
        assert!(seen(&log).is_empty());
    }

    #[test]
    fn a_changed_setting_drops_the_old_registration_before_making_the_new() {
        let (mut manager, log) = manager(Vec::new());
        assert_eq!(manager.apply("ctrlAltSpace"), Some(json!({ "ok": true })));
        assert_eq!(manager.apply("ctrlAltJ"), Some(json!({ "ok": true })));
        assert_eq!(
            seen(&log),
            [
                "register CtrlAltSpace",
                "drop CtrlAltSpace",
                "register CtrlAltJ"
            ]
        );
        // Off gives the last one back and says nothing (no failure was on show).
        assert_eq!(manager.apply("off"), None);
        assert_eq!(seen(&log).last().map(String::as_str), Some("drop CtrlAltJ"));
    }

    #[test]
    fn the_same_setting_twice_registers_once() {
        let (mut manager, log) = manager(Vec::new());
        assert!(manager.apply("ctrlAltJ").is_some());
        assert_eq!(manager.apply("ctrlAltJ"), None);
        assert_eq!(seen(&log), ["register CtrlAltJ"]);
    }

    #[test]
    fn a_taken_chord_is_reported_and_not_retried_until_the_setting_changes() {
        let (mut manager, log) = manager(vec![Chord::CtrlAltSpace]);
        assert_eq!(
            manager.apply("ctrlAltSpace"),
            Some(json!({ "ok": false, "message": "That shortcut is taken by another app." }))
        );
        // The same setting again (say, the settings event the report caused): no new attempt.
        assert_eq!(manager.apply("ctrlAltSpace"), None);
        assert_eq!(seen(&log), ["register CtrlAltSpace"]);
        // Another chord works, and its ok clears the failure.
        assert_eq!(manager.apply("ctrlAltJ"), Some(json!({ "ok": true })));
    }

    #[test]
    fn switching_off_clears_a_failure_once() {
        let (mut manager, _log) = manager(vec![Chord::CtrlAltJ]);
        assert!(manager.apply("ctrlAltJ").is_some());
        assert_eq!(manager.apply("off"), Some(json!({ "ok": true })));
        assert_eq!(manager.apply("off"), None);
    }
}
