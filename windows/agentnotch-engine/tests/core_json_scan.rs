//! `core::json_scan`: the Mac's PP_JSONFieldScannerTests vectors.

use agentnotch_engine::core::json_scan::{objects, values};

#[test]
fn picks_top_level_values_and_skips_the_rest() {
    let text = r#"{"numStartups": 3, "projects": {"/a": {"history": [{"display": "say \"oauthAccount\": {\"x\": 1}"}], "n": [1, [2, {"k": "}"}]]}},
         "tips": "\\\" }", "oauthAccount": {"accountUuid": "u-1", "emailAddress": "me@x.dev"},
         "flag": true, "nothing": null, "count": -12.5e3,
         "cachedUsageUtilization": {"accountUuid": "u-1", "fetchedAtMs": 1790000000000}}"#;
    let found = values(
        text.as_bytes(),
        &["oauthAccount", "cachedUsageUtilization", "count"],
    )
    .unwrap();
    let mut keys: Vec<&String> = found.keys().collect();
    keys.sort();
    assert_eq!(keys, ["cachedUsageUtilization", "count", "oauthAccount"]);
    assert_eq!(found["count"], b"-12.5e3");
    let parsed = objects(text.as_bytes(), &["oauthAccount"]).unwrap();
    assert_eq!(parsed["oauthAccount"]["emailAddress"], "me@x.dev");
    // A key only mentioned inside a string or nested deeper is not a top-level key.
    assert!(values(text.as_bytes(), &["x", "k", "display"])
        .unwrap()
        .is_empty());
}

#[test]
fn refuses_what_is_not_an_object() {
    assert!(values(b"[1, 2]", &["a"]).is_none());
    assert!(
        values(br#"{"a": {"b": 1}"#, &["a"]).is_none(),
        "cut off mid-write"
    );
    assert!(values(br#"{"a" 1}"#, &["a"]).is_none());
    assert!(values(b"", &["a"]).is_none());
    assert!(values(br#"{"a": "unterminated}"#, &["a"]).is_none());
    assert_eq!(values(b"{}", &["a"]).unwrap().len(), 0);
    assert_eq!(values(b" \n{ } ", &["a"]).unwrap().len(), 0);
    let bom = [&[0xEF, 0xBB, 0xBF][..], br#"{"a":1}"#].concat();
    assert_eq!(
        values(&bom, &["a"]).unwrap().keys().collect::<Vec<_>>(),
        ["a"]
    );
}

#[test]
fn escaped_keys_and_duplicates_behave_like_a_parser() {
    let parsed = objects(
        br#"{"oauthAccount": {"emailAddress": "a"}, "k": 1, "k": 2}"#,
        &["oauthAccount", "k"],
    )
    .unwrap();
    assert_eq!(parsed["oauthAccount"]["emailAddress"], "a");
    assert_eq!(parsed["k"], 2);
    // A key written with an escape is still that key.
    let escaped = objects(
        br#"{"oauth\u0041ccount": {"emailAddress": "b"}}"#,
        &["oauthAccount"],
    )
    .unwrap();
    assert_eq!(escaped["oauthAccount"]["emailAddress"], "b");
}

#[test]
fn a_value_that_does_not_parse_is_left_out() {
    let parsed = objects(br#"{"a": tru, "b": 1}"#, &["a", "b"]).unwrap();
    assert!(!parsed.contains_key("a"));
    assert_eq!(parsed["b"], 1);
}
