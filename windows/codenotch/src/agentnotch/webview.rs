//! Looking into a page from outside it, for the sealed self-test and the sealed snapshots
//! (DESIGN-WIN §7.4): run a script and read its result, collect what went wrong while the page
//! loaded, and capture the page as a PNG.
//!
//! **Sealed runs only.** Every entry refuses unless the run is sealed: a script handed to
//! WebView2 from here runs outside the page's content security policy, which a real run must
//! never have happen to it.
//!
//! **Never from the main thread.** WebView2 answers through COM callbacks that run on the main
//! thread's message loop, and the caller waits for the answer on a channel: a caller on the main
//! thread would wait for a callback only it could run. Such a call is refused at once.
//!
//! The scripts this module injects itself are constants; a caller's script is passed as it is.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::WebviewWindow;

/// How often a result that isn't there yet is asked for again.
const POLL: Duration = Duration::from_millis(25);
/// One short question to a page (its scale, what it collected, whether it is ready).
const QUICK: Duration = Duration::from_secs(5);
/// A page's reload, from the request to `readyState == "complete"`. Hosted runners are slow
/// and the first load of a WebView2 profile is the slowest.
const LOAD: Duration = Duration::from_secs(30);

/// The first eight bytes of every PNG file.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// What runs before any script of the page, in every document created after it was added:
/// it keeps the page's content-security-policy violations, uncaught errors (failed resource
/// loads included, hence the capturing listener), unhandled rejections and `console.error`
/// calls in `window.__anSelfTest`, at most 100 entries a list and 300 characters a text.
const COLLECTOR: &str = r#"(function () {
  var s = window.__anSelfTest = window.__anSelfTest || Object.create(null);
  if (s.installed) { return; }
  s.installed = true;
  s.csp = [];
  s.errors = [];
  var CAP = 100, LEN = 300;
  function clip(v) {
    var t;
    try { t = String(v); } catch (e) { t = "[unprintable]"; }
    return t.length > LEN ? t.slice(0, LEN) : t;
  }
  function keep(list, entry) { if (list.length < CAP) { list.push(entry); } }
  window.addEventListener("securitypolicyviolation", function (e) {
    keep(s.csp, {
      blockedURI: clip(e.blockedURI),
      violatedDirective: clip(e.violatedDirective),
      sourceFile: clip(e.sourceFile),
      lineNumber: e.lineNumber | 0
    });
  }, true);
  window.addEventListener("error", function (e) {
    var t = e.target;
    if (t && t !== window && (t.src || t.href)) {
      keep(s.errors, { kind: "resource", message: clip(t.src || t.href) });
    } else {
      keep(s.errors, {
        kind: "error",
        message: clip(e.message),
        source: clip(e.filename || ""),
        line: e.lineno | 0
      });
    }
  }, true);
  window.addEventListener("unhandledrejection", function (e) {
    var r = e.reason;
    keep(s.errors, { kind: "unhandledrejection", message: clip(r && r.message ? r.message : r) });
  });
  var original = console.error;
  console.error = function () {
    try {
      var parts = Array.prototype.map.call(arguments, clip);
      keep(s.errors, { kind: "console.error", message: clip(parts.join(" ")) });
    } catch (e) { /* the call itself must still go through */ }
    return original.apply(console, arguments);
  };
})();"#;

/// Marks the document that is about to be reloaded, so [`READY`] can't mistake it for the new
/// one when the collector was already installed once.
const MARK_STALE: &str =
    "(function () { var s = window.__anSelfTest; if (s) { s.stale = true; } return true; })()";

/// True once the document that loaded with the collector in it has finished loading.
const READY: &str = r#"(function () {
  var s = window.__anSelfTest;
  return !!(s && s.installed && !s.stale && document.readyState === "complete");
})()"#;

/// What the collector kept, or `null` when this document never had it.
const COLLECTED: &str = r#"(function () {
  var s = window.__anSelfTest;
  if (!s || !s.installed) { return null; }
  return { csp: s.csp, errors: s.errors };
})()"#;

const DEVICE_SCALE: &str = "window.devicePixelRatio";

/// `ExecuteScript` does not wait for a promise, so the caller's expression is settled in the
/// page and its outcome parked under a key; [`ASYNC_TAKE`] fetches it in a later call. The
/// caller's text stands between the head and the middle, the key (ours: letters, digits, `_`)
/// between the middle and the tail. The line breaks keep a `//` comment at the end of the
/// caller's text from swallowing the wrapper.
const ASYNC_HEAD: &str = "(function (k) {\n  var s = window.__anSelfTest = window.__anSelfTest || Object.create(null);\n  function done(r) { s[k] = r; }\n  function why(e) { return String(e && e.message ? e.message : e); }\n  try {\n    Promise.resolve((function () { return (\n";
const ASYNC_MID: &str = "\n    ); })()).then(\n      function (v) { done({ ok: true, value: v === undefined ? null : v }); },\n      function (e) { done({ ok: false, error: why(e) }); });\n  } catch (e) { done({ ok: false, error: why(e) }); }\n  return true;\n})(\"";
const ASYNC_TAIL: &str = "\")";

/// The parked outcome, taken out of the page; `null` while the promise is still pending.
const ASYNC_TAKE_HEAD: &str = "(function (k) {\n  var s = window.__anSelfTest;\n  if (!s || !Object.prototype.hasOwnProperty.call(s, k)) { return null; }\n  var r = s[k];\n  delete s[k];\n  return r;\n})(\"";

/// What a page's collector kept since the page loaded.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Collected {
    /// `{blockedURI, violatedDirective, sourceFile, lineNumber}` per violation.
    pub csp: Vec<Value>,
    /// `{kind, message, …}`; kind is `error`, `resource`, `unhandledrejection` or
    /// `console.error`.
    pub errors: Vec<Value>,
}

// ---- the entries ----

/// Runs `js` in the window's page and gives the value of its last expression, as JSON.
/// `undefined`, a value JSON can't hold and a script that threw all come back as `null`
/// (WebView2's rule); a promise comes back as `{}`: use [`eval_async`] for one.
pub(super) fn eval(window: &WebviewWindow, js: &str, timeout: Duration) -> Result<Value, String> {
    entry()?;
    if js.is_empty() {
        return Err("no script to run".into());
    }
    decode(&os::execute(window, js, timeout)?)
}

/// Runs an expression that gives a promise (or a plain value) and waits until it has settled:
/// its value, or `Err` with the rejection's message. A reload of the page in between loses the
/// outcome, which then reads as a timeout; so does a syntax error in `js`.
pub(super) fn eval_async(
    window: &WebviewWindow,
    js: &str,
    timeout: Duration,
) -> Result<Value, String> {
    entry()?;
    let deadline = Instant::now() + timeout;
    let key = poll_key();
    eval(window, &async_start(js, &key), timeout)?;
    let take = async_take(&key);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!(
                "the page's promise did not settle within {} ms",
                timeout.as_millis()
            ));
        }
        if let Some(outcome) = settled(eval(window, &take, left)?) {
            return outcome;
        }
        std::thread::sleep(POLL.min(left));
    }
}

/// Makes `js` run before the page's own scripts in every document the window loads from now
/// on (the current one is not touched: reload for that).
pub(super) fn add_document_script(window: &WebviewWindow, js: &str) -> Result<(), String> {
    entry()?;
    if js.is_empty() {
        return Err("no script to add".into());
    }
    os::add_script(window, js, QUICK)
}

/// Puts the collector into the window's page and reloads it, so the collector sees the whole
/// load; returns once the reloaded document is complete.
pub(super) fn install_collector(window: &WebviewWindow) -> Result<(), String> {
    entry()?;
    add_document_script(window, COLLECTOR)?;
    // Best effort: a page that can't answer now is still reloaded and asked again below.
    let _ = eval(window, MARK_STALE, QUICK);
    window.reload().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + LOAD;
    let mut last = String::from("the page never answered");
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!(
                "{} did not finish reloading within {} s ({last})",
                window.label(),
                LOAD.as_secs()
            ));
        }
        // A script can fail while the navigation is under way: that is "not yet", not an error.
        match eval(window, READY, left.min(QUICK)) {
            Ok(Value::Bool(true)) => return Ok(()),
            Ok(_) => last = "still loading".into(),
            Err(e) => last = e,
        }
        std::thread::sleep(POLL.min(left));
    }
}

/// What the page's collector kept so far. `Err` when the page has no collector
/// ([`install_collector`] was never run on it).
pub(super) fn collected(window: &WebviewWindow) -> Result<Collected, String> {
    entry()?;
    collected_from(eval(window, COLLECTED, QUICK)?)
}

/// The page as a PNG, as WebView2 draws it.
///
/// A hidden or zero-size WebView gives an empty image: show the window first (without
/// activating it) and let it paint (two animation frames) before asking. The image is the
/// WebView's client area in physical pixels; a transparent page keeps whatever background
/// WebView2 gives it.
pub(super) fn capture_png(window: &WebviewWindow, timeout: Duration) -> Result<Vec<u8>, String> {
    entry()?;
    checked_png(os::capture(window, timeout)?)
}

/// Makes the window's page draw at `scale` whatever its monitor's scale is, and gives the scale
/// WebView2 then reports. The window keeps its size in physical pixels, so the page has
/// 1/`scale` of them to lay itself out in: what a monitor set to that scale does to a page.
///
/// WebView2 takes a page's scale from its host window and ignores Chromium's
/// `--force-device-scale-factor` (run 36716764131: `devicePixelRatio` stayed 1 under 1.25 and
/// 1.5), so the scaled self-test runs set it here. The page is laid out again at once; reload
/// it for a load that happens at the new scale from the start.
pub(super) fn set_scale(window: &WebviewWindow, scale: f64) -> Result<f64, String> {
    entry()?;
    if !scale_allowed(scale) {
        return Err(format!("{scale} is not a scale a page can be drawn at"));
    }
    os::set_scale(window, scale, QUICK)
}

/// The page's `devicePixelRatio`: the scale it really draws at, which [`set_scale`] changes
/// without changing the monitor's.
pub(super) fn device_scale(window: &WebviewWindow) -> Result<f64, String> {
    entry()?;
    match eval(window, DEVICE_SCALE, QUICK)?.as_f64() {
        Some(scale) if scale.is_finite() && scale > 0.0 => Ok(scale),
        _ => Err("the page gave no device scale".into()),
    }
}

// ---- pure parts ----

/// The scales Windows itself offers for a monitor lie between 100 % and 500 %; below 1 is a
/// zoomed-out page, which nothing here needs.
fn scale_allowed(scale: f64) -> bool {
    scale.is_finite() && (1.0..=5.0).contains(&scale)
}

/// Width and height from a PNG's header (its first chunk, IHDR).
pub(super) fn png_size(bytes: &[u8]) -> Result<(u32, u32), String> {
    if !bytes.starts_with(&PNG_SIGNATURE) {
        return Err("not a PNG".into());
    }
    // Signature 8, chunk length 4, "IHDR" 4, width 4, height 4: big-endian.
    let (Some(kind), Some(width), Some(height)) =
        (bytes.get(12..16), bytes.get(16..20), bytes.get(20..24))
    else {
        return Err("the PNG is cut short".into());
    };
    if kind != b"IHDR" {
        return Err("the PNG does not start with its header".into());
    }
    let number = |four: &[u8]| u32::from_be_bytes([four[0], four[1], four[2], four[3]]);
    match (number(width), number(height)) {
        (0, _) | (_, 0) => Err("the PNG has no pixels".into()),
        size => Ok(size),
    }
}

/// A capture is kept only when it is a PNG with pixels in it.
fn checked_png(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.is_empty() {
        return Err("the capture is empty (is the window shown?)".into());
    }
    png_size(&bytes).map_err(|e| format!("the capture is unusable: {e}"))?;
    Ok(bytes)
}

/// `ExecuteScript` answers with the result as JSON text.
fn decode(raw: &str) -> Result<Value, String> {
    if raw.is_empty() {
        return Err("the page gave no result".into());
    }
    serde_json::from_str(raw).map_err(|e| format!("the page's result is not JSON: {e}"))
}

/// A key no other call of this process uses.
fn poll_key() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("result_{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn async_start(js: &str, key: &str) -> String {
    [ASYNC_HEAD, js, ASYNC_MID, key, ASYNC_TAIL].concat()
}

fn async_take(key: &str) -> String {
    [ASYNC_TAKE_HEAD, key, ASYNC_TAIL].concat()
}

/// What [`ASYNC_TAKE_HEAD`]'s script answered: `None` while pending.
fn settled(answer: Value) -> Option<Result<Value, String>> {
    let Value::Object(mut outcome) = answer else {
        return None;
    };
    Some(match outcome.get("ok") {
        Some(Value::Bool(true)) => Ok(outcome.remove("value").unwrap_or(Value::Null)),
        _ => Err(match outcome.get("error") {
            Some(Value::String(why)) => format!("the page's promise was rejected: {why}"),
            _ => "the page's promise was rejected".into(),
        }),
    })
}

fn collected_from(answer: Value) -> Result<Collected, String> {
    let Value::Object(mut kept) = answer else {
        return Err("the page has no collector".into());
    };
    let mut list = |name: &str| match kept.remove(name) {
        Some(Value::Array(entries)) => entries,
        _ => Vec::new(),
    };
    Ok(Collected {
        csp: list("csp"),
        errors: list("errors"),
    })
}

/// The check every entry starts with.
fn entry() -> Result<(), String> {
    admit(super::sealed(), on_main_thread())
}

fn admit(sealed: bool, on_main: bool) -> Result<(), String> {
    if !sealed {
        return Err("page probes run only in a sealed run".into());
    }
    if on_main {
        return Err("a page probe can't be awaited on the main thread".into());
    }
    Ok(())
}

/// The thread `main` runs on, which is where Tauri's event loop (and so every WebView2
/// callback) runs, is the only one the standard library names "main".
fn on_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

// ---- WebView2 ----

#[cfg(windows)]
mod os {
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::time::Duration;

    use agentnotch_win::capture::{memory_stream, stream_bytes, ComInterface};
    use tauri::WebviewWindow;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2Controller3, COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
    };
    use webview2_com::{
        AddScriptToExecuteOnDocumentCreatedCompletedHandler, CapturePreviewCompletedHandler,
        CoTaskMemPWSTR, ExecuteScriptCompletedHandler,
    };

    type Reply<T> = Sender<Result<T, String>>;

    /// Starts `work` on the main thread with the window's WebView2 and waits here for what it
    /// (or the completion handler it made) sends back. The handler may fire after the wait
    /// gave up; its send then goes nowhere.
    fn ask<T, W>(window: &WebviewWindow, timeout: Duration, work: W) -> Result<T, String>
    where
        T: Send + 'static,
        W: FnOnce(&ICoreWebView2, Reply<T>) -> Result<(), String> + Send + 'static,
    {
        let (reply, answer) = mpsc::channel();
        window
            .with_webview(move |webview| {
                // SAFETY: a COM call on the window's own controller, on the thread that owns
                // it (`with_webview` runs this closure there); its HRESULT is checked.
                let started = unsafe { webview.controller().CoreWebView2() }
                    .map_err(|e| e.to_string())
                    .and_then(|core| work(&core, reply.clone()));
                if let Err(e) = started {
                    let _ = reply.send(Err(e));
                }
            })
            .map_err(|e| e.to_string())?;
        wait(window, &answer, timeout)
    }

    fn wait<T>(
        window: &WebviewWindow,
        answer: &Receiver<Result<T, String>>,
        timeout: Duration,
    ) -> Result<T, String> {
        match answer.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(format!(
                "{} did not answer within {} ms",
                window.label(),
                timeout.as_millis()
            )),
            // Every sender was dropped unused: the WebView went away with its handler.
            Err(RecvTimeoutError::Disconnected) => {
                Err(format!("{} closed before it answered", window.label()))
            }
        }
    }

    /// `RasterizationScale`, with WebView2 told to stop following the monitor's scale (or it
    /// would put the monitor's back at the next move); the scale it reports afterwards.
    pub(super) fn set_scale(
        window: &WebviewWindow,
        scale: f64,
        timeout: Duration,
    ) -> Result<f64, String> {
        let (reply, answer) = mpsc::channel();
        window
            .with_webview(move |webview| {
                // SAFETY: COM calls on the window's own controller, on the thread that owns it
                // (`with_webview` runs this closure there); each HRESULT is checked, and `now`
                // is a valid out pointer for the last call.
                let set = unsafe {
                    // Controller3 came with WebView2 runtime 88; an older one refuses the cast.
                    webview
                        .controller()
                        .cast::<ICoreWebView2Controller3>()
                        .and_then(|controller| {
                            controller.SetShouldDetectMonitorScaleChanges(false)?;
                            controller.SetRasterizationScale(scale)?;
                            let mut now = 0.0f64;
                            controller.RasterizationScale(&mut now)?;
                            Ok(now)
                        })
                };
                let _ = reply.send(set.map_err(|e| e.to_string()));
            })
            .map_err(|e| e.to_string())?;
        wait(window, &answer, timeout)
    }

    /// `ExecuteScript`: the script's result as JSON text.
    pub(super) fn execute(
        window: &WebviewWindow,
        js: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let js = js.to_owned();
        ask(window, timeout, move |core, reply| {
            let text = CoTaskMemPWSTR::from(js.as_str());
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |status, json| {
                let _ = reply.send(status.map(|()| json).map_err(|e| e.to_string()));
                Ok(())
            }));
            // SAFETY: `text` is a NUL-terminated copy that outlives the call (WebView2 copies
            // the script before returning); the handler is reference-counted by WebView2.
            unsafe { core.ExecuteScript(*text.as_ref().as_pcwstr(), &handler) }
                .map_err(|e| e.to_string())
        })
    }

    /// `AddScriptToExecuteOnDocumentCreated`.
    pub(super) fn add_script(
        window: &WebviewWindow,
        js: &str,
        timeout: Duration,
    ) -> Result<(), String> {
        let js = js.to_owned();
        ask(window, timeout, move |core, reply| {
            let text = CoTaskMemPWSTR::from(js.as_str());
            let handler = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                move |status, _id| {
                    let _ = reply.send(status.map_err(|e| e.to_string()));
                    Ok(())
                },
            ));
            // SAFETY: as in `execute`.
            unsafe {
                core.AddScriptToExecuteOnDocumentCreated(*text.as_ref().as_pcwstr(), &handler)
            }
            .map_err(|e| e.to_string())
        })
    }

    /// `CapturePreview` as a PNG into a stream in memory; the stream's bytes.
    pub(super) fn capture(window: &WebviewWindow, timeout: Duration) -> Result<Vec<u8>, String> {
        ask(window, timeout, move |core, reply| {
            let stream = memory_stream()?;
            let written = stream.clone();
            let handler = CapturePreviewCompletedHandler::create(Box::new(move |status| {
                let bytes = status
                    .map_err(|e| e.to_string())
                    .and_then(|()| stream_bytes(&written));
                let _ = reply.send(bytes);
                Ok(())
            }));
            // SAFETY: the stream and the handler are reference-counted COM objects WebView2
            // keeps alive until the capture is done.
            unsafe {
                core.CapturePreview(
                    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                    &stream,
                    &handler,
                )
            }
            .map_err(|e| e.to_string())
        })
    }
}

/// Off Windows (the host build that runs the glue's tests) there is no WebView2: every probe
/// fails honestly, after the same checks.
#[cfg(not(windows))]
mod os {
    use std::time::Duration;

    use tauri::WebviewWindow;

    const ONLY_WINDOWS: &str = "page probes need WebView2 (Windows only)";

    pub(super) fn execute(
        _window: &WebviewWindow,
        _js: &str,
        _timeout: Duration,
    ) -> Result<String, String> {
        Err(ONLY_WINDOWS.into())
    }

    pub(super) fn add_script(
        _window: &WebviewWindow,
        _js: &str,
        _timeout: Duration,
    ) -> Result<(), String> {
        Err(ONLY_WINDOWS.into())
    }

    pub(super) fn capture(_window: &WebviewWindow, _timeout: Duration) -> Result<Vec<u8>, String> {
        Err(ONLY_WINDOWS.into())
    }

    pub(super) fn set_scale(
        _window: &WebviewWindow,
        _scale: f64,
        _timeout: Duration,
    ) -> Result<f64, String> {
        Err(ONLY_WINDOWS.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Signature, an IHDR chunk's length and name, then width and height.
    fn header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes
    }

    #[test]
    fn a_page_is_drawn_only_at_a_scale_a_monitor_can_have() {
        for scale in [1.0, 1.25, 1.5, 2.0, 5.0] {
            assert!(scale_allowed(scale), "{scale}");
        }
        for scale in [0.0, -1.25, 0.99, 5.01, f64::NAN, f64::INFINITY] {
            assert!(!scale_allowed(scale), "{scale}");
        }
    }

    #[test]
    fn png_size_reads_the_header_and_refuses_what_is_not_a_whole_one() {
        assert_eq!(png_size(&header(440, 680)), Ok((440, 680)));
        assert_eq!(png_size(&header(1, 70_000)), Ok((1, 70_000)));

        let whole = header(440, 680);
        for cut in 0..whole.len() {
            assert!(png_size(&whole[..cut]).is_err(), "cut at {cut}");
        }
        assert!(png_size(b"GIF89a, and then enough bytes to be long").is_err());
        let mut other_chunk = header(440, 680);
        other_chunk[12..16].copy_from_slice(b"IDAT");
        assert!(png_size(&other_chunk).is_err());
        assert!(png_size(&header(0, 680)).is_err());
        assert!(png_size(&header(440, 0)).is_err());

        // A capture is refused when empty or not a PNG, and kept as it is otherwise.
        assert!(checked_png(Vec::new()).is_err());
        assert!(checked_png(b"<html>".to_vec()).is_err());
        assert_eq!(checked_png(header(2, 3)), Ok(header(2, 3)));
    }

    #[test]
    fn a_script_result_is_decoded_and_a_bad_one_is_an_error() {
        assert_eq!(decode("null"), Ok(Value::Null));
        assert_eq!(decode("\"text\""), Ok(json!("text")));
        assert_eq!(decode("{\"a\":1}"), Ok(json!({"a": 1})));
        assert_eq!(decode("1.25"), Ok(json!(1.25)));
        assert!(decode("").is_err());
        assert!(decode("{\"a\":").is_err());
        assert!(decode("undefined").is_err());
    }

    /// Brackets pair up outside string literals: enough to catch a constant cut short or a
    /// placeholder left in it, without a JavaScript parser.
    fn balanced(script: &str) -> bool {
        let mut open = Vec::new();
        let mut quote = None;
        let mut escaped = false;
        for c in script.chars() {
            if let Some(q) = quote {
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    _ if c == q => quote = None,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' | '\'' => quote = Some(c),
                '(' | '[' | '{' => open.push(c),
                ')' | ']' | '}' => {
                    let wanted = match c {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    if open.pop() != Some(wanted) {
                        return false;
                    }
                }
                _ => {}
            }
        }
        open.is_empty() && quote.is_none()
    }

    #[test]
    fn the_injected_scripts_are_constants_and_poll_keys_differ() {
        // Constants by type: nothing of a caller's can reach them.
        let fixed: [&'static str; 5] = [COLLECTOR, MARK_STALE, READY, COLLECTED, DEVICE_SCALE];
        for script in fixed {
            assert!(balanced(script), "{script}");
            // No placeholder of a formatting macro was left in.
            assert!(
                !script.contains("{}") && !script.contains("{0}"),
                "{script}"
            );
        }
        for wanted in [
            "\"securitypolicyviolation\"",
            "\"error\"",
            "\"unhandledrejection\"",
            "console.error = function",
            "blockedURI",
            "violatedDirective",
            "sourceFile",
            "lineNumber",
            "CAP = 100",
            "LEN = 300",
            "__anSelfTest",
        ] {
            assert!(COLLECTOR.contains(wanted), "{wanted}");
        }

        let first = poll_key();
        let second = poll_key();
        assert_ne!(first, second);
        for key in [&first, &second] {
            assert!(key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        }

        // The caller's text is passed as it is, once, between the fixed parts; a trailing
        // line comment can't swallow the wrapper.
        let js = "fetchThings({ a: [1, 2] }) // done";
        let start = async_start(js, &first);
        assert!(balanced(&start.replace("// done", "")), "{start}");
        assert_eq!(start.matches(js).count(), 1);
        assert!(start.starts_with(ASYNC_HEAD) && start.ends_with(&format!("(\"{first}\")")));
        assert!(start.contains(&format!("{js}\n")));
        let take = async_take(&first);
        assert!(balanced(&take), "{take}");
        assert!(take.ends_with(&format!("(\"{first}\")")));
    }

    #[test]
    fn a_parked_outcome_is_pending_settled_or_rejected() {
        assert_eq!(settled(Value::Null), None);
        assert_eq!(
            settled(json!({"ok": true, "value": {"rings": 2}})),
            Some(Ok(json!({"rings": 2})))
        );
        assert_eq!(
            settled(json!({"ok": true, "value": null})),
            Some(Ok(Value::Null))
        );
        let rejected = settled(json!({"ok": false, "error": "no such call"}));
        assert!(matches!(rejected, Some(Err(why)) if why.contains("no such call")));
        assert!(matches!(settled(json!({})), Some(Err(_))));
    }

    #[test]
    fn what_the_collector_kept_is_read_and_a_page_without_one_is_an_error() {
        assert!(collected_from(Value::Null).is_err());
        let kept = collected_from(json!({
            "csp": [{"blockedURI": "inline", "violatedDirective": "script-src",
                     "sourceFile": "", "lineNumber": 3}],
            "errors": [],
        }))
        .expect("a collector's answer");
        assert_eq!(kept.csp.len(), 1);
        assert_eq!(kept.csp[0]["violatedDirective"], "script-src");
        assert!(kept.errors.is_empty());
        assert_eq!(collected_from(json!({})), Ok(Collected::default()));
    }

    #[test]
    fn nothing_runs_unless_sealed_and_never_on_the_main_thread() {
        assert!(admit(false, false).is_err());
        assert!(admit(false, true).is_err());
        assert!(admit(true, true).is_err());
        assert_eq!(admit(true, false), Ok(()));
        // A test's thread is not the main one, so here only the seal decides.
        assert!(!on_main_thread());
        assert_eq!(entry().is_ok(), crate::agentnotch::sealed());

        // Every entry that takes a window starts with that check, on every OS (the WebView2
        // half and its twin sit below the entries, behind them).
        let source = include_str!("webview.rs");
        let entries = source
            .split("\n// ---- pure parts ----")
            .next()
            .expect("the entries");
        let marker = ["pub(super) ", "fn "].concat();
        let mut seen = Vec::new();
        for item in entries.split(marker.as_str()).skip(1) {
            let name = item.split('(').next().expect("a name");
            let (signature, body) = item.split_once(" {\n").expect("a body");
            assert!(signature.contains("window: &WebviewWindow"), "{name}");
            assert!(body.trim_start().starts_with("entry()?;"), "{name}");
            seen.push(name);
        }
        assert_eq!(
            seen,
            [
                "eval",
                "eval_async",
                "add_document_script",
                "install_collector",
                "collected",
                "capture_png",
                "set_scale",
                "device_scale"
            ]
        );
        // The platform halves are reachable only through those entries.
        assert!(!source.contains(&["pub(super) ", "mod os"].concat()));
        assert!(!source.contains(&["pub ", "mod os"].concat()));
    }
}
