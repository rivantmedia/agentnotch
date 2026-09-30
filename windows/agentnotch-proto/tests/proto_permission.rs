//! What the hook prints for each answer (HS§1.7). The expected bytes in
//! `tests/fixtures/v1-responses/*.stdout` were produced by the Mac hook
//! script's own `permission_output` + `json.dumps` for the same stdin and
//! response, so Windows prints exactly what the Mac prints.

use agentnotch_proto::*;
use serde_json::{json, Value};
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

struct Case {
    name: String,
    stdin: Vec<u8>,
    response_frame: Vec<u8>,
    expected: Option<String>,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for entry in std::fs::read_dir(fixtures().join("v1-responses")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let stdin_name = doc["stdin"].as_str().unwrap();
        let stdin = std::fs::read(fixtures().join(format!("stdin/{stdin_name}.json"))).unwrap();
        // The response frame's bytes as the fixture holds them (key order kept).
        let raw = std::fs::read_to_string(&path).unwrap();
        let response_frame = extract_response(&raw);
        let stdout = std::fs::read(path.with_extension("stdout")).unwrap();
        let expected = (!stdout.is_empty()).then(|| String::from_utf8(stdout).unwrap());
        cases.push(Case {
            name,
            stdin,
            response_frame,
            expected,
        });
    }
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    assert!(cases.len() >= 8);
    cases
}

/// The `"response": {…}` object's text, without re-serialising it.
fn extract_response(doc: &str) -> Vec<u8> {
    let start = doc.find("\"response\":").unwrap() + "\"response\":".len();
    let bytes = doc.as_bytes();
    let open = start + doc[start..].find('{').unwrap();
    let (mut depth, mut in_string, mut escaped) = (0i32, false, false);
    for (offset, &b) in bytes[open..].iter().enumerate() {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return bytes[open..=open + offset].to_vec();
                }
            }
            _ => {}
        }
    }
    panic!("no response object")
}

#[test]
fn byte_for_byte_like_the_mac_script() {
    for case in cases() {
        assert_eq!(
            permission_output_for_frames(&case.stdin, &case.response_frame),
            case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn the_typed_response_gives_the_same_json() {
    for case in cases() {
        let response = PermissionResponse::from_frame(&case.response_frame).unwrap();
        let from_stdin = permission_output_for_stdin(&case.stdin, &response);
        let stdin: Value = serde_json::from_slice(&case.stdin).unwrap();
        let from_value = permission_output(&stdin["tool_input"], &response);
        match &case.expected {
            None => {
                assert_eq!(from_stdin, None, "{}", case.name);
                assert_eq!(from_value, None, "{}", case.name);
            }
            Some(expected) => {
                // Same JSON; only key order may differ.
                let expected: Value = serde_json::from_str(expected).unwrap();
                for text in [from_stdin.unwrap(), from_value.unwrap()] {
                    assert_eq!(
                        serde_json::from_str::<Value>(&text).unwrap(),
                        expected,
                        "{}",
                        case.name
                    );
                }
            }
        }
    }
}

#[test]
fn the_documented_answers() {
    let bash = br#"{"tool_input": {"command": "npm test"}}"#;
    let allow = PermissionResponse::allow();
    assert_eq!(
        permission_output_for_stdin(bash, &allow).unwrap(),
        r#"{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}}}"#
    );
    assert_eq!(
        permission_output_for_stdin(bash, &PermissionResponse::deny(None)).unwrap(),
        r#"{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "deny", "message": "Denied by user via Agent Notch"}}}"#
    );
    // An empty reason is no reason.
    assert_eq!(
        permission_output_for_stdin(bash, &PermissionResponse::deny(Some(String::new()))).unwrap(),
        permission_output_for_stdin(bash, &PermissionResponse::deny(None)).unwrap()
    );
    let keep = PermissionResponse::deny(Some(KEEP_PLANNING_REASON.into()));
    assert!(
        permission_output_for_stdin(br#"{"tool_input": {"plan": "p"}}"#, &keep)
            .unwrap()
            .contains(KEEP_PLANNING_REASON)
    );

    // Plan approval: an empty update echoes the original input verbatim.
    let plan = PermissionResponse {
        updated_input: Some(Default::default()),
        ..PermissionResponse::allow()
    };
    assert_eq!(
        permission_output_for_stdin(br#"{"tool_input": {"plan": "1. a\n2. b"}}"#, &plan).unwrap(),
        r#"{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow", "updatedInput": {"plan": "1. a\n2. b"}}}}"#
    );
    // An empty permissions list is left out; a missing original input merges onto {}.
    let odd = PermissionResponse {
        updated_input: Some(json!({"answers": {"Q": "A"}}).as_object().unwrap().clone()),
        updated_permissions: Some(vec![]),
        ..PermissionResponse::allow()
    };
    assert_eq!(
        permission_output_for_stdin(b"not json", &odd).unwrap(),
        r#"{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow", "updatedInput": {"answers": {"Q": "A"}}}}}"#
    );
}

#[test]
fn ask_unknown_and_broken_responses_print_nothing() {
    let stdin = br#"{"tool_input": {"command": "ls"}}"#;
    for frame in [
        &br#"{"decision":"ask"}"#[..],
        br#"{"decision":"maybe"}"#,
        br#"{}"#,
        b"[1]",
        b"",
        b"not json",
    ] {
        assert_eq!(
            permission_output_for_frames(stdin, frame),
            None,
            "{}",
            String::from_utf8_lossy(frame)
        );
    }
    // Wrongly typed fields are ignored, not fatal (the Mac's isinstance checks).
    assert_eq!(
        permission_output_for_frames(
            stdin,
            br#"{"decision":"allow","updated_input":"x","updated_permissions":{}}"#
        )
        .unwrap(),
        r#"{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}}}"#
    );
    assert_eq!(
        permission_output_for_frames(
            stdin,
            br#"{"decision":"deny","reason":5,"interrupt":"yes"}"#
        )
        .unwrap(),
        r#"{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "deny", "message": "Denied by user via Agent Notch"}}}"#
    );
}

#[test]
fn the_response_frame_shape() {
    let r = PermissionResponse {
        reason: Some("No".into()),
        interrupt: Some(true),
        ..PermissionResponse::deny(None)
    };
    assert_eq!(
        String::from_utf8(r.to_json()).unwrap(),
        r#"{"decision":"deny","reason":"No","interrupt":true}"#
    );
    let parsed: PermissionResponse =
        serde_json::from_str(r#"{"decision":"allow","future":1}"#).unwrap();
    assert_eq!(parsed, PermissionResponse::allow());
}

/// Each answer the app can give has a constructor; what the hook prints for
/// it is the fixture's JSON (the constructors go through `Value`, so only
/// the order of keys may differ from the Mac's bytes).
#[test]
fn the_constructors_give_the_documented_answers() {
    let stdin = |name: &str| std::fs::read(fixtures().join(format!("stdin/{name}.json"))).unwrap();
    let expected = |name: &str| -> Option<Value> {
        let text = std::fs::read(fixtures().join(format!("v1-responses/{name}.stdout"))).unwrap();
        (!text.is_empty()).then(|| serde_json::from_slice(&text).unwrap())
    };
    let printed = |stdin: &[u8], response: &PermissionResponse| -> Option<Value> {
        permission_output_for_stdin(stdin, response)
            .map(|text| serde_json::from_str(&text).unwrap())
    };

    let bash = stdin("permission_request_bash");
    let bash_json: Value = serde_json::from_slice(&bash).unwrap();
    assert_eq!(
        printed(&bash, &PermissionResponse::allow()),
        expected("allow")
    );
    assert_eq!(
        printed(
            &bash,
            &PermissionResponse::always_allow(bash_json["permission_suggestions"][0].clone())
        ),
        expected("always")
    );
    assert_eq!(
        printed(&bash, &PermissionResponse::deny(None)),
        expected("deny")
    );
    assert_eq!(printed(&bash, &PermissionResponse::ask()), expected("ask"));
    assert_eq!(expected("ask"), None);

    let plan = stdin("permission_request_plan");
    assert_eq!(
        printed(&plan, &PermissionResponse::approve_plan()),
        expected("plan")
    );
    assert_eq!(
        printed(&plan, &PermissionResponse::keep_planning()),
        expected("keep_planning")
    );

    let question = stdin("permission_request_question");
    assert_eq!(
        printed(
            &question,
            &PermissionResponse::answers([(
                "Which charting library should the dashboard use?",
                "Recharts"
            )])
        ),
        expected("question")
    );
    // The frame the app writes for it.
    assert_eq!(
        String::from_utf8(PermissionResponse::answers([("Q", "A")]).to_json()).unwrap(),
        r#"{"decision":"allow","updated_input":{"answers":{"Q":"A"}}}"#
    );
    assert_eq!(
        String::from_utf8(PermissionResponse::approve_plan().to_json()).unwrap(),
        r#"{"decision":"allow","updated_input":{}}"#
    );
}
