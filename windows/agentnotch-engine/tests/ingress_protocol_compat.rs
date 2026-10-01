//! Hook copies in run folders can be older (or, after a downgrade, newer)
//! than the app (DESIGN-WIN §1.4 "Versions"). Every frame of protocol 1
//! committed under `agentnotch-proto/tests/fixtures/v1` is kept forever, and
//! every later server must accept it: decode it to the right kind, and let
//! it through ingress (a hook event arrives, a PermissionRequest is held, a
//! status line and a control request are read).

use agentnotch_engine::ingress::{decode, Decoded, HookIngress};
use agentnotch_engine::runtime_types::{IngressConfig, IngressOut};
use agentnotch_engine::testkit::MemoryTransport;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

fn fixtures() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../agentnotch-proto/tests/fixtures/v1"
    ));
    let mut frames: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir)
        .expect("the v1 fixtures")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .map(|path| {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&path).unwrap())
        })
        .collect();
    frames.sort();
    frames
}

fn at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000)
}

#[test]
fn every_v1_frame_is_accepted() {
    let frames = fixtures();
    assert!(frames.len() >= 20, "the v1 fixtures are all there");
    for (name, bytes) in &frames {
        let raw: Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(raw["protocol"], 1, "{name} is a protocol 1 frame");
        match decode(bytes, at()) {
            Decoded::Control(_) => assert!(name.starts_with("control_"), "{name}"),
            Decoded::StatusLine(message) => {
                assert_eq!(name, "status_line");
                assert_eq!(message.session_id.as_str(), raw["session_id"]);
            }
            Decoded::Hook(event) => {
                assert_eq!(event.event, raw["event"], "{name}");
                assert_eq!(event.session_id.as_str(), raw["session_id"], "{name}");
                assert_eq!(event.protocol, 1);
                assert_eq!(event.hook_pid, raw["hook_pid"].as_u64().map(|p| p as u32));
                assert_eq!(event.pid, raw["pid"].as_u64().map(|p| p as u32), "{name}");
            }
            Decoded::Unreadable(why) => panic!("{name} was refused: {why}"),
        }
    }
}

#[test]
fn every_v1_frame_passes_ingress() {
    let transport = Arc::new(MemoryTransport::default());
    let mut ingress =
        HookIngress::with_transport(IngressConfig::new(r"\\.\pipe\compat"), transport.clone());
    for (name, bytes) in fixtures() {
        let raw: Value = serde_json::from_slice(&bytes).unwrap();
        let conn = transport.inject(bytes.clone(), at());
        let frame = agentnotch_engine::platform::TransportEvent::Frame(
            agentnotch_engine::platform::IncomingFrame {
                conn,
                bytes,
                received_at: at(),
                peer_pid: None,
            },
        );
        let outs = ingress.on_transport(frame, at());
        match raw["event"].as_str().unwrap() {
            "AgentNotchControl" => assert!(
                matches!(&outs[..], [IngressOut::Control { .. }]),
                "{name}: {outs:?}"
            ),
            "StatusLine" => assert!(
                matches!(&outs[..], [IngressOut::StatusLine(_)]),
                "{name}: {outs:?}"
            ),
            "PermissionRequest" => assert!(
                matches!(&outs[..], [IngressOut::PermissionHeld(h)] if h.conn == conn),
                "{name}: {outs:?}"
            ),
            event => assert!(
                matches!(&outs[..], [IngressOut::Hook(e)] if e.event == event),
                "{name}: {outs:?}"
            ),
        }
    }
}

/// A newer hook's frame: a higher protocol, fields this server has never
/// heard of, and a changed shape of an optional field. All read.
#[test]
fn a_newer_frame_is_read_leniently() {
    let frame = json!({
        "protocol": 9,
        "event": "PreToolUse",
        "session_id": "s",
        "pid": 4242,
        "tool": "Bash",
        "tool_input": {"command": "ls"},
        "tool_use_id": "toolu_1",
        "terminal": {"wt_session": "w", "term_program": null, "new_thing": [1, 2]},
        "hook_pid": 1,
        "something_new": {"deep": true},
    });
    match decode(&serde_json::to_vec(&frame).unwrap(), at()) {
        Decoded::Hook(event) => {
            assert_eq!(event.protocol, 9);
            assert_eq!(event.tool_use_id.as_deref(), Some("toolu_1"));
            assert_eq!(
                event.terminal.and_then(|t| t.wt_session).as_deref(),
                Some("w")
            );
        }
        other => panic!("{other:?}"),
    }
}
