//! `an_call` (DESIGN-WIN §3.5, §3.7): the pages' one way into the fork.
//!
//! Each call is first checked against the window it came from (the engine's
//! `allowed_from_window`, so the rule lives in one place), then either handled here (the
//! glue-level methods: windows, clipboard, Explorer, logs) or turned into the engine's `Call` by
//! `Call::from_parts` and run by the hub on a blocking thread. The JSON shape `{method, args}` is
//! the contract; the glue never builds a `Call` any other way, so it depends on the table in
//! §3.5, not on how the engine spells its variants.
//!
//! Five glue-level methods reach the Windows shell (`shell_call`): a browser, Settings, Explorer,
//! the folder picker, the clipboard. A page is not trusted with what they are given: a URL passes
//! the rule in `links.rs` or is one the engine made, the Settings page is a fixed one, a folder to
//! show is one the engine resolved from `{kind, id}`, and copied text has a size bound. A sealed
//! run starts no other program, so there all but copying answer `sealed`.

use std::path::{Path, PathBuf};

use agentnotch_engine::hub::{allowed_from_window, Call, CallError, Hub};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::links::{self, Verdict};
use super::panel;

/// Whether the window labelled `label` may make the call `method` (§3.7).
pub(super) fn allowed(label: &str, method: &str) -> bool {
    allowed_from_window(label, method)
}

pub(super) async fn dispatch(
    app: AppHandle,
    label: String,
    method: String,
    args: Value,
) -> Result<Value, CallError> {
    if !allowed(&label, &method) {
        return Err(error(
            "refused",
            format!("{method} isn't available to this window"),
        ));
    }
    if SHELL_METHODS.contains(&method.as_str()) {
        // On the blocking pool: each of these waits for the shell, and the folder picker for
        // the user. Never the main thread, which the picker's owner window needs to stay alive.
        return tauri::async_runtime::spawn_blocking(move || {
            let hub = super::hub();
            let engine = |method: &str, args: Value| match &hub {
                Some(hub) => engine_call(hub, method, args),
                None => Err(not_running()),
            };
            let owner = app
                .get_webview_window(&label)
                .and_then(|window| super::panel_window::hwnd_of(&window));
            shell_call(&method, &args, super::sealed(), owner, &engine, &Windows)
        })
        .await
        .map_err(|e| error("failed", e.to_string()))?;
    }
    if let Some(reply) = glue_method(&app, &label, &method, &args) {
        return reply;
    }
    let hub = super::hub().ok_or_else(not_running)?;
    tauri::async_runtime::spawn_blocking(move || engine_call(&hub, &method, args))
        .await
        .map_err(|e| error("failed", e.to_string()))?
}

/// Runs `{method, args}` on the hub (blocking; at most the hub's call timeout).
pub(super) fn engine_call(hub: &Hub, method: &str, args: Value) -> Result<Value, CallError> {
    hub.call(Call::from_parts(method, args)?)
}

/// `engine_call` on its own thread, waited for at most `wait`: `None` when the hub hasn't answered
/// by then (the call still completes). For callers on the main thread, which must not stall.
pub(super) fn engine_call_within(
    hub: Hub,
    method: &'static str,
    args: Value,
    wait: std::time::Duration,
) -> Option<Result<Value, CallError>> {
    let (reply, answer) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = reply.send(engine_call(&hub, method, args));
    });
    answer.recv_timeout(wait).ok()
}

/// The methods that never reach the engine. `None` = not one of them.
fn glue_method(
    app: &AppHandle,
    label: &str,
    method: &str,
    args: &Value,
) -> Option<Result<Value, CallError>> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_string);
    let reply = match method {
        "log" => {
            // Pages log their own events only (never prompts, replies or paths); a runaway line
            // is cut so it can't fill run.log.
            let msg: String = text("msg").unwrap_or_default().chars().take(500).collect();
            super::log(&format!("page {label}: {msg}"));
            Ok(json!({}))
        }
        "panel_open" => {
            let route = text("route").unwrap_or_else(|| "sessions".into());
            let reason = text("reason").unwrap_or_else(|| "settings".into());
            panel::open_route(app, route, text("ring_id"), reason);
            Ok(json!({}))
        }
        "panel_toggle" => {
            let reason = text("reason").unwrap_or_else(|| "ring_click".into());
            // A rect that doesn't parse is no rect: the panel then hangs off the middle of the
            // notch instead of off a ring that isn't where the page said.
            panel::toggle(app, text("ring_id"), reason, ring_rect(args.get("rect")));
            Ok(json!({}))
        }
        "panel_close" => {
            panel::close(app);
            Ok(json!({}))
        }
        "panel_route" => {
            if let Some(route) = text("route") {
                panel::set_route(route);
            }
            Ok(json!({}))
        }
        "panel_report_size" => {
            let number = |key: &str| args.get(key).and_then(Value::as_f64).unwrap_or(0.0);
            panel::report_size(app, number("w"), number("h"));
            Ok(json!({}))
        }
        "panel_take_focus" => {
            panel::take_focus(app);
            Ok(json!({}))
        }
        "panel_engaged" => {
            panel::set_engaged(args.get("on").and_then(Value::as_bool).unwrap_or(false));
            Ok(json!({}))
        }
        "open_settings" => {
            // `{tab}` is accepted and ignored: upstream's `settings_window::open` takes no page
            // to land on (its window opens on the page it was last left on), and the fork adds
            // no seam for one.
            crate::settings_window::open(app);
            Ok(json!({}))
        }
        _ => return None,
    };
    Some(reply)
}

/// `panel_toggle`'s `rect`: `[x, y, w, h]`, the clicked ring in physical pixels from the top-left
/// of the calling window's client area. Exactly four finite numbers, no negative size.
fn ring_rect(value: Option<&Value>) -> Option<panel::RingRect> {
    let numbers = value?.as_array()?;
    let [x, y, w, h] = numbers.as_slice() else {
        return None;
    };
    let rect = [x.as_f64()?, y.as_f64()?, w.as_f64()?, h.as_f64()?];
    (rect.iter().all(|n| n.is_finite()) && rect[2] >= 0.0 && rect[3] >= 0.0).then_some(rect)
}

pub(super) fn error(code: &str, message: impl Into<String>) -> CallError {
    CallError {
        code: code.to_string(),
        message: message.into(),
    }
}

fn not_running() -> CallError {
    error(
        "failed",
        format!(
            "{}'s Claude Code control isn't running",
            super::DISPLAY_NAME
        ),
    )
}

// ---- the calls that reach the shell ----

/// The glue-level methods `shell_call` answers.
const SHELL_METHODS: [&str; 5] = [
    "open_url",
    "open_notification_settings",
    "copy_text",
    "reveal",
    "pick_folder",
];

/// Windows Settings › System › Notifications. Fixed: a page never names a Settings page.
const NOTIFICATION_SETTINGS: &str = "ms-settings:notifications";
/// The most text one copy may put on the clipboard (UTF-8 bytes): far above a transcript's
/// longest message, far below what would stall every app that watches the clipboard.
const MAX_COPY_BYTES: usize = 1024 * 1024;
const PICK_FOLDER_TITLE: &str = "Choose a folder";
/// The engine's `cloud_url` targets: the links it makes that a page may have opened.
const CLOUD_URL_TARGETS: [&str; 3] = ["dashboard", "pools", "settings"];

/// What the five calls need from the OS (`agentnotch_win`); a fake in the tests.
trait Os {
    fn open_uri(&self, uri: &str) -> Result<(), String>;
    fn reveal(&self, path: &Path) -> Result<(), String>;
    fn pick_folder(&self, owner: Option<isize>, title: &str) -> Result<Option<PathBuf>, String>;
    fn set_clipboard(&self, text: &str) -> Result<(), String>;
}

struct Windows;

impl Os for Windows {
    fn open_uri(&self, uri: &str) -> Result<(), String> {
        agentnotch_win::shell::open_uri(uri)
    }
    fn reveal(&self, path: &Path) -> Result<(), String> {
        agentnotch_win::shell::reveal(path)
    }
    fn pick_folder(&self, owner: Option<isize>, title: &str) -> Result<Option<PathBuf>, String> {
        agentnotch_win::shell::pick_folder(owner, title)
    }
    fn set_clipboard(&self, text: &str) -> Result<(), String> {
        agentnotch_win::clipboard::set_text(text)
    }
}

/// One of `SHELL_METHODS`. Blocks (the shell, the picker, the engine): run it off the main
/// thread. `engine` runs a `{method, args}` call on the hub; `owner` is the calling window.
fn shell_call(
    method: &str,
    args: &Value,
    sealed: bool,
    owner: Option<isize>,
    engine: &dyn Fn(&str, Value) -> Result<Value, CallError>,
    os: &dyn Os,
) -> Result<Value, CallError> {
    let failed = |message: String| error("failed", message);
    if sealed && method != "copy_text" {
        // Before anything is looked at: a sealed run starts no browser, Settings, Explorer or
        // dialog, whatever it was asked to open.
        return Err(CallError::sealed(format!(
            "Sealed: {method} does nothing in a sealed run."
        )));
    }
    match method {
        "open_url" => {
            let url = args.get("url").and_then(Value::as_str).unwrap_or_default();
            let open = match links::verdict(url) {
                Verdict::Open => true,
                Verdict::IfTheEngines => is_cloud_url(url, engine),
                Verdict::Refused => false,
            };
            if !open {
                // The URL itself stays out of the log: it is page data of any size and content.
                super::log("open_url refused: not an https link or one of the website's");
                return Err(error("refused", "Only https links can be opened."));
            }
            os.open_uri(url).map_err(failed)?;
            Ok(json!({}))
        }
        "open_notification_settings" => {
            os.open_uri(NOTIFICATION_SETTINGS).map_err(failed)?;
            Ok(json!({}))
        }
        "copy_text" => {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| error("invalid", "copy_text: text is missing"))?;
            if text.len() > MAX_COPY_BYTES {
                return Err(error("refused", "That is too much text to copy at once."));
            }
            os.set_clipboard(text).map_err(failed)?;
            Ok(json!({}))
        }
        "reveal" => {
            // Only `kind` and `id` go on: whatever else the page sent (a `path`, say) is
            // dropped here, and the folder shown is the one the engine knows by that id.
            let ask = json!({
                "kind": args.get("kind").cloned().unwrap_or(Value::Null),
                "id": args.get("id").cloned().unwrap_or(Value::Null),
            });
            let reply = engine("reveal_target", ask)?;
            let path = reply
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| error("not_found", "There is no folder to show."))?;
            os.reveal(Path::new(path)).map_err(failed)?;
            Ok(json!({}))
        }
        "pick_folder" => {
            let picked = os.pick_folder(owner, PICK_FOLDER_TITLE).map_err(failed)?;
            Ok(json!({ "path": picked.map(|path| path.to_string_lossy().into_owned()) }))
        }
        _ => Err(error("invalid", format!("{method} isn't a shell call"))),
    }
}

/// Whether `url` is, character for character, a link the engine makes for the website right now
/// (its dashboard, pools or settings page). In a dev run against a local website those are
/// `http://localhost…`, which the https rule refuses; nothing a page made up can equal one.
fn is_cloud_url(url: &str, engine: &dyn Fn(&str, Value) -> Result<Value, CallError>) -> bool {
    CLOUD_URL_TARGETS.iter().any(|target| {
        engine("cloud_url", json!({ "target": target }))
            .ok()
            .and_then(|reply| reply.get("url").and_then(Value::as_str).map(|u| u == url))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    use agentnotch_engine::hub::CallError;
    use serde_json::{json, Value};

    use super::{allowed, ring_rect, shell_call, Os, MAX_COPY_BYTES, SHELL_METHODS};

    /// Records what would have reached Windows.
    #[derive(Default)]
    struct FakeOs {
        did: RefCell<Vec<String>>,
        picks: Option<PathBuf>,
    }

    impl FakeOs {
        fn did(&self) -> Vec<String> {
            self.did.borrow().clone()
        }
    }

    impl Os for FakeOs {
        fn open_uri(&self, uri: &str) -> Result<(), String> {
            self.did.borrow_mut().push(format!("open {uri}"));
            Ok(())
        }
        fn reveal(&self, path: &Path) -> Result<(), String> {
            self.did
                .borrow_mut()
                .push(format!("reveal {}", path.display()));
            Ok(())
        }
        fn pick_folder(
            &self,
            owner: Option<isize>,
            _title: &str,
        ) -> Result<Option<PathBuf>, String> {
            self.did.borrow_mut().push(format!("pick {owner:?}"));
            Ok(self.picks.clone())
        }
        fn set_clipboard(&self, text: &str) -> Result<(), String> {
            self.did
                .borrow_mut()
                .push(format!("copy {} bytes", text.len()));
            Ok(())
        }
    }

    /// An engine whose website is a local dev one and whose one known folder is `acct-1`.
    /// Every call it is asked is recorded.
    struct FakeEngine {
        asked: RefCell<Vec<(String, Value)>>,
    }

    impl FakeEngine {
        fn new() -> Self {
            FakeEngine {
                asked: RefCell::new(Vec::new()),
            }
        }

        fn call(&self, method: &str, args: Value) -> Result<Value, CallError> {
            self.asked
                .borrow_mut()
                .push((method.to_string(), args.clone()));
            match method {
                "cloud_url" => Ok(json!({
                    "url": format!("http://localhost:3000/{}", args["target"].as_str().unwrap_or("?"))
                })),
                "reveal_target" if args["kind"] == "config_dir" && args["id"] == "acct-1" => {
                    Ok(json!({ "path": "C:\\Users\\me\\.claude" }))
                }
                "reveal_target" => Err(CallError::not_found("no such folder")),
                other => Err(CallError::invalid(other.to_string())),
            }
        }
    }

    fn run(
        method: &str,
        args: Value,
        sealed: bool,
        engine: &FakeEngine,
        os: &FakeOs,
    ) -> Result<Value, CallError> {
        shell_call(
            method,
            &args,
            sealed,
            Some(77),
            &|m, a| engine.call(m, a),
            os,
        )
    }

    #[test]
    fn open_url_opens_https_links_and_refuses_the_rest() {
        let (engine, os) = (FakeEngine::new(), FakeOs::default());
        for url in ["https://example.com/a?b=c", "HTTPS://EXAMPLE.COM"] {
            assert_eq!(
                run("open_url", json!({ "url": url }), false, &engine, &os),
                Ok(json!({})),
                "{url}"
            );
        }
        assert_eq!(
            os.did(),
            ["open https://example.com/a?b=c", "open HTTPS://EXAMPLE.COM"]
        );
        // An https link needs no word from the engine.
        assert!(engine.asked.borrow().is_empty());

        let os = FakeOs::default();
        let long = format!("https://example.com/{}", "a".repeat(2980));
        for url in [
            "http://example.com",
            "file:///C:/x",
            "javascript:alert(1)",
            "ms-settings:notifications",
            "agentnotch://open",
            "\\\\server\\share",
            "C:\\Windows\\notepad.exe",
            "https://",
            "https://example.com/a\nb",
            "https://example.com/a\0b",
            " https://example.com",
            "http://localhost:3000/dashboard/../x",
            "http://localhost:3000/dashboard ",
            "HTTP://LOCALHOST:3000/dashboard",
            long.as_str(),
            "",
        ] {
            let reply = run("open_url", json!({ "url": url }), false, &engine, &os);
            assert_eq!(reply.unwrap_err().code, "refused", "{url:?}");
        }
        for args in [
            json!({}),
            json!(null),
            json!({ "url": 7 }),
            json!({ "url": ["https://example.com"] }),
        ] {
            let reply = run("open_url", args.clone(), false, &engine, &os);
            assert_eq!(reply.unwrap_err().code, "refused", "{args}");
        }
        assert_eq!(os.did(), Vec::<String>::new());
    }

    #[test]
    fn open_url_opens_a_link_the_engine_made_even_when_it_isnt_https() {
        let (engine, os) = (FakeEngine::new(), FakeOs::default());
        for target in ["dashboard", "pools", "settings"] {
            let url = format!("http://localhost:3000/{target}");
            assert_eq!(
                run("open_url", json!({ "url": url }), false, &engine, &os),
                Ok(json!({}))
            );
        }
        assert_eq!(
            os.did(),
            [
                "open http://localhost:3000/dashboard",
                "open http://localhost:3000/pools",
                "open http://localhost:3000/settings"
            ]
        );
        // The engine was only ever asked for its own links, never handed the page's string.
        for (method, args) in engine.asked.borrow().iter() {
            assert_eq!(method, "cloud_url");
            assert!(!args.to_string().contains("localhost"), "{args}");
        }
    }

    #[test]
    fn open_url_without_an_engine_still_refuses() {
        let os = FakeOs::default();
        let reply = shell_call(
            "open_url",
            &json!({ "url": "http://localhost:3000/dashboard" }),
            false,
            None,
            &|_, _| Err(super::not_running()),
            &os,
        );
        assert_eq!(reply.unwrap_err().code, "refused");
        assert_eq!(os.did(), Vec::<String>::new());
    }

    #[test]
    fn notification_settings_is_one_fixed_page_whatever_the_page_sends() {
        let (engine, os) = (FakeEngine::new(), FakeOs::default());
        for args in [
            json!(null),
            json!({ "url": "ms-settings:privacy" }),
            json!({ "page": "C:\\x.exe" }),
        ] {
            assert_eq!(
                run("open_notification_settings", args, false, &engine, &os),
                Ok(json!({}))
            );
        }
        assert_eq!(os.did(), ["open ms-settings:notifications"; 3]);
    }

    #[test]
    fn reveal_shows_the_engines_folder_and_never_a_path_from_the_page() {
        let (engine, os) = (FakeEngine::new(), FakeOs::default());
        let args = json!({
            "kind": "config_dir",
            "id": "acct-1",
            "path": "C:\\Windows\\System32",
            "target": "\\\\server\\share",
        });
        assert_eq!(run("reveal", args, false, &engine, &os), Ok(json!({})));
        assert_eq!(
            *engine.asked.borrow(),
            [(
                "reveal_target".to_string(),
                json!({ "kind": "config_dir", "id": "acct-1" })
            )]
        );
        assert_eq!(os.did(), ["reveal C:\\Users\\me\\.claude"]);

        // A path and nothing the engine knows: its error comes back, nothing is shown.
        let (engine, os) = (FakeEngine::new(), FakeOs::default());
        let reply = run(
            "reveal",
            json!({ "path": "C:\\Windows" }),
            false,
            &engine,
            &os,
        );
        assert_eq!(reply.unwrap_err().code, "not_found");
        assert_eq!(
            *engine.asked.borrow(),
            [(
                "reveal_target".to_string(),
                json!({ "kind": null, "id": null })
            )]
        );
        assert_eq!(os.did(), Vec::<String>::new());
    }

    #[test]
    fn reveal_passes_on_the_engines_error_and_refuses_an_empty_path() {
        let os = FakeOs::default();
        let args = json!({ "kind": "backup", "id": "b" });
        let sealed_hub = |_: &str, _: Value| Err(CallError::sealed("Sealed: there is no folder."));
        let reply = shell_call("reveal", &args, false, None, &sealed_hub, &os);
        assert_eq!(reply.unwrap_err().code, "sealed");
        for answer in [json!({ "path": "" }), json!({}), json!({ "path": 3 })] {
            let engine = move |_: &str, _: Value| Ok(answer.clone());
            let reply = shell_call("reveal", &args, false, None, &engine, &os);
            assert_eq!(reply.unwrap_err().code, "not_found");
        }
        assert_eq!(os.did(), Vec::<String>::new());
    }

    #[test]
    fn copy_text_has_a_size_bound() {
        let (engine, os) = (FakeEngine::new(), FakeOs::default());
        let most = "a".repeat(MAX_COPY_BYTES);
        assert_eq!(
            run("copy_text", json!({ "text": most }), false, &engine, &os),
            Ok(json!({}))
        );
        assert_eq!(
            run("copy_text", json!({ "text": "" }), false, &engine, &os),
            Ok(json!({}))
        );
        let over = "a".repeat(MAX_COPY_BYTES + 1);
        let reply = run("copy_text", json!({ "text": over }), false, &engine, &os);
        assert_eq!(reply.unwrap_err().code, "refused");
        // Bytes, not characters: the bound is on what is handed to Windows.
        let wide = "\u{e9}".repeat(MAX_COPY_BYTES / 2 + 1);
        let reply = run("copy_text", json!({ "text": wide }), false, &engine, &os);
        assert_eq!(reply.unwrap_err().code, "refused");
        for args in [json!(null), json!({}), json!({ "text": 5 })] {
            let reply = run("copy_text", args, false, &engine, &os);
            assert_eq!(reply.unwrap_err().code, "invalid");
        }
        assert_eq!(
            os.did(),
            [
                format!("copy {MAX_COPY_BYTES} bytes"),
                "copy 0 bytes".to_string()
            ]
        );
    }

    #[test]
    fn pick_folder_is_owned_by_the_calling_window_and_answers_a_path_or_null() {
        let engine = FakeEngine::new();
        let os = FakeOs {
            picks: Some(PathBuf::from("C:\\Users\\me\\.claude-work")),
            ..FakeOs::default()
        };
        assert_eq!(
            run(
                "pick_folder",
                json!({ "path": "C:\\x" }),
                false,
                &engine,
                &os
            ),
            Ok(json!({ "path": "C:\\Users\\me\\.claude-work" }))
        );
        assert_eq!(os.did(), ["pick Some(77)"]);
        // Cancelled.
        let os = FakeOs::default();
        assert_eq!(
            run("pick_folder", json!(null), false, &engine, &os),
            Ok(json!({ "path": null }))
        );
        assert!(engine.asked.borrow().is_empty());
    }

    #[test]
    fn a_sealed_run_starts_no_other_program_but_still_copies() {
        let (engine, os) = (FakeEngine::new(), FakeOs::default());
        for (method, args) in [
            ("open_url", json!({ "url": "https://example.com" })),
            (
                "open_url",
                json!({ "url": "http://localhost:3000/dashboard" }),
            ),
            ("open_url", json!({ "url": "file:///C:/x" })),
            ("open_notification_settings", json!(null)),
            ("reveal", json!({ "kind": "config_dir", "id": "acct-1" })),
            ("pick_folder", json!(null)),
        ] {
            let reply = run(method, args, true, &engine, &os);
            assert_eq!(reply.unwrap_err().code, "sealed", "{method}");
        }
        assert_eq!(os.did(), Vec::<String>::new());
        assert!(engine.asked.borrow().is_empty());

        assert_eq!(
            run("copy_text", json!({ "text": "abc" }), true, &engine, &os),
            Ok(json!({}))
        );
        assert_eq!(os.did(), ["copy 3 bytes"]);
    }

    #[test]
    fn the_shell_calls_are_glue_methods_the_engine_never_sees() {
        for method in SHELL_METHODS {
            assert!(
                agentnotch_engine::hub::GLUE_METHODS.contains(&method),
                "{method}"
            );
            assert!(allowed("agentnotch-panel", method), "{method}");
            assert!(allowed("settings", method), "{method}");
            assert!(!allowed("notch", method), "{method}");
            assert!(!allowed("dropzones", method), "{method}");
        }
    }

    #[test]
    fn panel_toggles_rect_is_four_finite_numbers_with_a_size_that_isnt_negative() {
        let parse = |value: Value| ring_rect(Some(&value));
        assert_eq!(
            parse(json!([12, 300.5, 44, 44])),
            Some([12.0, 300.5, 44.0, 44.0])
        );
        // A point, and a ring left of or above the window's corner, are rects too.
        assert_eq!(parse(json!([0, 0, 0, 0])), Some([0.0; 4]));
        assert_eq!(
            parse(json!([-8, -4.5, 44, 44])),
            Some([-8.0, -4.5, 44.0, 44.0])
        );

        assert_eq!(ring_rect(None), None);
        for wrong in [
            json!(null),
            json!([]),
            json!([1, 2, 3]),
            json!([1, 2, 3, 4, 5]),
            json!([1, 2, -3, 4]),
            json!([1, 2, 3, -0.5]),
            json!([1, 2, "3", 4]),
            json!([1, 2, null, 4]),
            json!([[1, 2, 3, 4]]),
            json!({ "x": 1, "y": 2, "w": 3, "h": 4 }),
            json!("1,2,3,4"),
            // Too large for a float: JSON has no NaN or infinity of its own, and a number that
            // can't be read as one is refused rather than rounded to infinity.
            serde_json::from_str::<Value>("[1, 2, 1e999, 4]").unwrap_or(json!([1, 2, "inf", 4])),
        ] {
            assert_eq!(parse(wrong.clone()), None, "{wrong}");
        }
    }

    #[test]
    fn glue_only_calls_are_refused_to_every_page() {
        for label in ["agentnotch-panel", "notch", "settings", "dropzones"] {
            assert!(!allowed(label, "panel_state"), "{label}");
            assert!(!allowed(label, "hotkey_status"), "{label}");
        }
    }

    #[test]
    fn the_panel_may_call_the_rest() {
        for method in [
            "answer",
            "send_message",
            "snapshot",
            "chat_open",
            "panel_close",
            "log",
        ] {
            assert!(allowed("agentnotch-panel", method), "{method}");
        }
    }

    #[test]
    fn the_notch_calls_only_its_own_list() {
        // `open_settings`: the notch's "Turn on in Settings" chip (§7.5 step 7).
        for method in [
            "snapshot",
            "refresh_usage",
            "focus",
            "mark_reviewed",
            "panel_toggle",
            "open_settings",
            "log",
        ] {
            assert!(allowed("notch", method), "{method}");
        }
        for method in [
            "answer",
            "send_message",
            "cloud",
            "hook_consent",
            "set_setting",
            "open_url",
        ] {
            assert!(!allowed("notch", method), "{method}");
        }
    }

    #[test]
    fn settings_never_answers_or_types() {
        assert!(!allowed("settings", "answer"));
        assert!(!allowed("settings", "send_message"));
        for method in [
            "settings",
            "hook_consent",
            "cloud",
            "set_setting",
            "panel_open",
            "open_url",
        ] {
            assert!(allowed("settings", method), "{method}");
        }
    }

    #[test]
    fn other_windows_call_nothing() {
        assert!(!allowed("dropzones", "snapshot"));
        assert!(!allowed("", "snapshot"));
        assert!(!allowed("agentnotch-panel-2", "answer"));
    }
}
