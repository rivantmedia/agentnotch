//! What each answer in the panel makes the hook print (HS§1.7, HS§6): the
//! engine's `permission_response` encoded as the response frame, fed with
//! Claude Code's stdin to the hook's own `permission_output_for_frames`, and
//! compared with the Mac hook script's exact stdout (`json.dumps` spacing,
//! the original input's keys in document order, every non-ASCII character
//! escaped). Where `agentnotch-proto/tests/fixtures/v1-responses` holds the
//! same case, its `.stdout` file (made by the Mac script) is the expectation.
//!
//! Ported from `ClaudeSessionMonitor.approvePermission/denyPermission/
//! answerQuestion` (190-263). Not ported: which request an answer lands on
//! (`pendingPermission`, the tool_use_id lookup) is the hub's (WP7), and the
//! socket-failure outcome is the transport's (WP1).

mod control_support;

use agentnotch_engine::control::answers::permission_response;
use agentnotch_engine::model::{Answer, PendingRequest, RequestKind};
use agentnotch_proto::{
    permission_output_for_frames, PermissionResponse, DEFAULT_DENY_MESSAGE, KEEP_PLANNING_REASON,
};
use control_support::{pending, request};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

const HEAD: &str = r#"{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": "#;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../agentnotch-proto/tests/fixtures")
}

fn stdin_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(format!("stdin/{name}.json"))).unwrap()
}

/// The Mac script's stdout for a v1-responses case.
fn mac_stdout(name: &str) -> String {
    String::from_utf8(
        std::fs::read(fixtures().join(format!("v1-responses/{name}.stdout"))).unwrap(),
    )
    .unwrap()
}

fn tool_input(stdin: &[u8]) -> Value {
    serde_json::from_slice::<Value>(stdin).unwrap()["tool_input"].clone()
}

/// A held request for the tool and input on `stdin`, the way the session
/// store builds it (kind from the tool name, the first suggestion kept).
fn held(stdin: &[u8]) -> PendingRequest {
    let data: Value = serde_json::from_slice(stdin).unwrap();
    let suggestions = data["permission_suggestions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    request(
        "s1",
        data["tool_name"].as_str().unwrap(),
        data["tool_input"].clone(),
        &suggestions,
    )
}

/// The frame the app sends for `answer`; the UI never sets `interrupt`.
fn frame(req: &PendingRequest, answer: &Answer) -> Vec<u8> {
    let response = permission_response(req, answer).unwrap();
    assert_eq!(response.interrupt, None);
    let bytes = response.to_json();
    let sent: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(sent.get("interrupt").is_none(), "{sent}");
    // What the hook reads back is what was meant.
    assert_eq!(PermissionResponse::from_frame(&bytes).unwrap(), response);
    bytes
}

/// What the hook prints for `answer` to the request on `stdin`.
fn printed(stdin: &[u8], req: &PendingRequest, answer: &Answer) -> String {
    let out = permission_output_for_frames(stdin, &frame(req, answer)).unwrap();
    assert!(!out.contains("interrupt"), "{out}");
    out
}

fn decision(json: &str) -> String {
    format!("{HEAD}{json}}}}}")
}

fn allow(always: bool) -> Answer {
    Answer::Allow { always }
}

fn questions(pairs: &[(&str, &str)]) -> Answer {
    Answer::Questions {
        answers: pairs
            .iter()
            .map(|(q, a)| (q.to_string(), a.to_string()))
            .collect(),
    }
}

// MARK: - Approve and always

#[test]
fn approve_is_a_plain_allow() {
    let stdin = stdin_fixture("permission_request_bash");
    let req = held(&stdin);
    let out = printed(&stdin, &req, &allow(false));
    assert_eq!(out, decision(r#"{"behavior": "allow"}"#));
    assert_eq!(out, mac_stdout("allow"));
}

#[test]
fn always_allow_sends_the_first_suggestion_verbatim() {
    let stdin = stdin_fixture("permission_request_bash");
    let first = json!({
        "type": "addRules",
        "rules": [{"toolName": "Bash", "ruleContent": "npm run test:*"}],
        "behavior": "allow",
        "destination": "localSettings"
    });
    let second = json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"});
    let req = request("s1", "Bash", tool_input(&stdin), &[first.clone(), second]);
    let out = printed(&stdin, &req, &allow(true));

    // Only the first suggestion, unchanged: never widened to the others.
    let parsed: Value = serde_json::from_str(&out).unwrap();
    let decision_value = &parsed["hookSpecificOutput"]["decision"];
    assert_eq!(decision_value["updatedPermissions"], json!([first]));
    assert!(decision_value.get("updatedInput").is_none());
    // The Mac's content exactly. Its key order inside the suggestion is
    // Swift Dictionary order (AnyCodable re-encoded, unspecified per run);
    // here it is the suggestion Value's own order, printed as it stands.
    assert_eq!(
        parsed,
        serde_json::from_str::<Value>(&mac_stdout("always")).unwrap()
    );
    assert_eq!(
        out,
        decision(
            r#"{"behavior": "allow", "updatedPermissions": [{"behavior": "allow", "destination": "localSettings", "rules": [{"ruleContent": "npm run test:*", "toolName": "Bash"}], "type": "addRules"}]}"#
        )
    );
}

#[test]
fn always_without_a_suggestion_is_a_plain_allow() {
    let stdin = stdin_fixture("permission_request_bash");
    let req = pending("Bash", RequestKind::Permission, tool_input(&stdin), None);
    let response = permission_response(&req, &allow(true)).unwrap();
    assert_eq!(response.updated_permissions, None);
    assert_eq!(printed(&stdin, &req, &allow(true)), mac_stdout("allow"));
}

#[test]
fn approve_without_always_never_sends_the_suggestion() {
    let stdin = stdin_fixture("permission_request_bash");
    let req = held(&stdin);
    assert!(req.always.is_some());
    assert_eq!(printed(&stdin, &req, &allow(false)), mac_stdout("allow"));
}

// MARK: - Plans

#[test]
fn approving_a_plan_echoes_the_original_input() {
    let stdin = stdin_fixture("permission_request_plan");
    let req = held(&stdin);
    let expected = decision(
        r#"{"behavior": "allow", "updatedInput": {"plan": "1. Add models\n2. Migrate\n3. Remove the old store"}}"#,
    );
    assert_eq!(expected, mac_stdout("plan"));
    for answer in [Answer::ApprovePlan, allow(false), allow(true)] {
        let response = permission_response(&req, &answer).unwrap();
        assert_eq!(response.updated_input, Some(serde_json::Map::new()));
        assert_eq!(printed(&stdin, &req, &answer), expected, "{answer:?}");
    }
}

/// Claude Code's own key order, nested objects and non-ASCII text survive
/// the echo exactly as `json.dumps` prints them.
#[test]
fn the_plan_echo_keeps_key_order_and_escapes() {
    let stdin = br#"{"session_id":"s1","hook_event_name":"PermissionRequest","tool_name":"ExitPlanMode","tool_input":{"plan":"\u00c9tape 1 \u2014 \u56f3\u8868 \ud83d\ude80\n\"quoted\" \\ tab\there","zeta":{"b":1,"a":[true,null,2.5,"caf\u00e9"]},"alpha":"x"}}"#;
    let req = held(stdin);
    let out = printed(stdin, &req, &Answer::ApprovePlan);
    assert_eq!(
        out,
        decision(
            r#"{"behavior": "allow", "updatedInput": {"plan": "\u00c9tape 1 \u2014 \u56f3\u8868 \ud83d\ude80\n\"quoted\" \\ tab\there", "zeta": {"b": 1, "a": [true, null, 2.5, "caf\u00e9"]}, "alpha": "x"}}"#
        )
    );
}

#[test]
fn keep_planning_denies_with_the_reason_claude_reads() {
    let stdin = stdin_fixture("permission_request_plan");
    let req = held(&stdin);
    let out = printed(&stdin, &req, &Answer::KeepPlanning);
    assert_eq!(
        out,
        decision(&format!(
            r#"{{"behavior": "deny", "message": "{KEEP_PLANNING_REASON}"}}"#
        ))
    );
    assert_eq!(out, mac_stdout("keep_planning"));
}

// MARK: - Questions

const CHARTS: &str = "Which charting library should the dashboard use?";

#[test]
fn a_single_answer_is_merged_onto_the_original_input() {
    let stdin = stdin_fixture("permission_request_question");
    let req = held(&stdin);
    let out = printed(&stdin, &req, &questions(&[(CHARTS, "Recharts")]));
    assert_eq!(
        out,
        decision(&format!(
            r#"{{"behavior": "allow", "updatedInput": {{"questions": [{{"question": "{CHARTS}", "header": "Charts", "multiSelect": false, "options": [{{"label": "Recharts", "description": "Composable React components"}}, {{"label": "Chart.js", "description": "Canvas, small bundle"}}]}}], "answers": {{"{CHARTS}": "Recharts"}}}}}}"#
        ))
    );
    assert_eq!(out, mac_stdout("question"));
}

#[test]
fn a_non_ascii_answer_is_escaped_like_the_mac() {
    let stdin = stdin_fixture("permission_request_question");
    let req = held(&stdin);
    let out = printed(
        &stdin,
        &req,
        &questions(&[(CHARTS, "Chart.js \u{2713} caf\u{e9} \u{1F600}")]),
    );
    assert_eq!(out, mac_stdout("unicode"));
}

/// Multi-select: the labels in option order joined ", ", then the Other
/// text, as one answer (the panel joins them; the engine sends it as is).
#[test]
fn a_multi_select_answer_with_other_text() {
    let stdin = stdin_fixture("permission_request_question");
    let req = held(&stdin);
    let out = printed(
        &stdin,
        &req,
        &questions(&[(CHARTS, "Recharts, Chart.js, D3 if it fits")]),
    );
    assert!(
        out.ends_with(&format!(
            r#"}}]}}], "answers": {{"{CHARTS}": "Recharts, Chart.js, D3 if it fits"}}}}}}}}}}"#
        )),
        "{out}"
    );
    let parsed: Value = serde_json::from_str(&out).unwrap();
    let echoed = &parsed["hookSpecificOutput"]["decision"]["updatedInput"];
    assert_eq!(echoed["questions"], tool_input(&stdin)["questions"]);
}

/// Two questions: each keyed by its text exactly as sent (non-ASCII and
/// trailing punctuation included), after the original input's keys.
#[test]
fn two_questions_each_keyed_by_their_exact_text() {
    let stdin = br#"{"session_id":"s1","hook_event_name":"PermissionRequest","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"\u00bfQu\u00e9 base de datos?","header":"DB","options":[{"label":"Postgres"},{"label":"SQLite"}],"multiSelect":false},{"question":"Which charts? ","header":"Charts","multiSelect":true,"options":["Recharts","D3"]}],"metadata":{"source":"plan"}}}"#;
    let req = held(stdin);
    let out = printed(
        stdin,
        &req,
        &questions(&[
            ("\u{bf}Qu\u{e9} base de datos?", "Postgres"),
            ("Which charts? ", "Recharts, D3"),
        ]),
    );
    assert_eq!(
        out,
        decision(
            r#"{"behavior": "allow", "updatedInput": {"questions": [{"question": "\u00bfQu\u00e9 base de datos?", "header": "DB", "options": [{"label": "Postgres"}, {"label": "SQLite"}], "multiSelect": false}, {"question": "Which charts? ", "header": "Charts", "multiSelect": true, "options": ["Recharts", "D3"]}], "metadata": {"source": "plan"}, "answers": {"Which charts? ": "Recharts, D3", "\u00bfQu\u00e9 base de datos?": "Postgres"}}}"#
        )
    );
}

// MARK: - Deny

#[test]
fn deny_without_a_reason_says_the_default_message() {
    let stdin = stdin_fixture("permission_request_bash");
    let req = held(&stdin);
    let expected = decision(&format!(
        r#"{{"behavior": "deny", "message": "{DEFAULT_DENY_MESSAGE}"}}"#
    ));
    assert_eq!(expected, mac_stdout("deny"));
    for reason in [None, Some(""), Some("   "), Some(" \n\t ")] {
        let answer = Answer::Deny {
            reason: reason.map(str::to_owned),
        };
        assert_eq!(printed(&stdin, &req, &answer), expected, "{reason:?}");
    }
}

#[test]
fn deny_with_a_reason_says_it() {
    let stdin = stdin_fixture("permission_request_bash");
    let req = held(&stdin);
    let deny = |reason: &str| Answer::Deny {
        reason: Some(reason.into()),
    };
    assert_eq!(
        printed(&stdin, &req, &deny("Not on main")),
        decision(r#"{"behavior": "deny", "message": "Not on main"}"#)
    );
    assert_eq!(
        printed(&stdin, &req, &deny("Pas sur \u{ab} main \u{bb}")),
        decision(r#"{"behavior": "deny", "message": "Pas sur \u00ab main \u00bb"}"#)
    );
}

/// Any request can be denied, a question or a plan included.
#[test]
fn every_kind_of_request_can_be_denied() {
    for name in ["permission_request_question", "permission_request_plan"] {
        let stdin = stdin_fixture(name);
        let req = held(&stdin);
        assert_eq!(
            printed(&stdin, &req, &Answer::Deny { reason: None }),
            mac_stdout("deny"),
            "{name}"
        );
    }
}

// MARK: - Refusals (nothing is sent; the request stays held)

#[test]
fn a_question_is_never_plainly_allowed() {
    let stdin = stdin_fixture("permission_request_question");
    let req = held(&stdin);
    assert!(permission_response(&req, &allow(false)).is_err());
    assert!(permission_response(&req, &allow(true)).is_err());
    // Nor a request shown as a question, whatever its tool says.
    let shown = pending("Bash", RequestKind::Question, json!({}), None);
    assert!(permission_response(&shown, &allow(false)).is_err());
}

#[test]
fn answers_go_only_to_ask_user_question() {
    let answer = questions(&[(CHARTS, "Recharts")]);
    let bash = held(&stdin_fixture("permission_request_bash"));
    let plan = held(&stdin_fixture("permission_request_plan"));
    assert!(permission_response(&bash, &answer).is_err());
    assert!(permission_response(&plan, &answer).is_err());
    // The Mac keys on the tool name: a kind that says "question" doesn't
    // make another tool take answers into its input.
    let mislabelled = pending(
        "Bash",
        RequestKind::Question,
        json!({"command": "ls"}),
        None,
    );
    assert!(permission_response(&mislabelled, &answer).is_err());
}

#[test]
fn a_question_without_answers_is_refused() {
    let req = held(&stdin_fixture("permission_request_question"));
    let none = Answer::Questions {
        answers: BTreeMap::new(),
    };
    assert!(permission_response(&req, &none).is_err());
}

#[test]
fn plan_answers_go_only_to_exit_plan_mode() {
    let bash = held(&stdin_fixture("permission_request_bash"));
    let question = held(&stdin_fixture("permission_request_question"));
    let mislabelled = pending("Bash", RequestKind::Plan, json!({"command": "ls"}), None);
    for req in [&bash, &question, &mislabelled] {
        assert!(permission_response(req, &Answer::ApprovePlan).is_err());
        assert!(permission_response(req, &Answer::KeepPlanning).is_err());
    }
}

/// Only ExitPlanMode's input is echoed: a request whose kind says "plan"
/// for another tool gets a plain allow.
#[test]
fn no_other_tool_echoes_its_input() {
    let stdin = stdin_fixture("permission_request_bash");
    let mislabelled = pending("Bash", RequestKind::Plan, tool_input(&stdin), None);
    let response = permission_response(&mislabelled, &allow(false)).unwrap();
    assert_eq!(response.updated_input, None);
    assert_eq!(
        printed(&stdin, &mislabelled, &allow(false)),
        mac_stdout("allow")
    );
}
