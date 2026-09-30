//! The order- and spelling-keeping JSON the installer edits settings.json
//! with, and the splicing document over it. Ports of the Mac's
//! `A2_SettingsDocumentTests.swift` (`OrderedJSONTests`,
//! `SettingsDocumentTests`), plus what Windows adds: CRLF line endings and a
//! UTF-8 byte order mark are kept.

use agentnotch_engine::core::settings_doc::{
    is_blank, newline_style, parse_top_level_object, Json, Member, SettingsDocument, BOM,
};

fn parse(text: &str) -> Json {
    Json::parse(text.as_bytes()).expect("valid JSON")
}

fn document(text: &str) -> SettingsDocument {
    SettingsDocument::new(Some(text.as_bytes())).expect("a JSON object")
}

fn text(document: &SettingsDocument) -> String {
    String::from_utf8(document.data()).expect("UTF-8")
}

fn keys(value: &Json) -> Vec<&str> {
    value
        .members()
        .unwrap_or_default()
        .iter()
        .map(|member| member.key.as_str())
        .collect()
}

// ---- OrderedJSONTests ----

/// `parsesAndWritesBackJavaScriptsFormat`
#[test]
fn parses_and_writes_back_javascripts_format() {
    let text = r#"{
  "b": 1,
  "a": [
    0.1,
    1e-7,
    -0,
    true,
    null
  ],
  "s": "tab\tquote\"slash/é\u0001",
  "o": {},
  "l": []
}"#;
    let value = parse(text);
    assert_eq!(keys(&value), ["b", "a", "s", "o", "l"]);
    assert_eq!(value.serialized(), text);
    assert_eq!(
        value.get("s").and_then(Json::as_str),
        Some("tab\tquote\"slash/é\u{01}")
    );
}

/// `oneLineFormat`
#[test]
fn one_line_format() {
    let value = parse(r#"{"a": [1, {"b": null}], "c": "d"}"#);
    assert_eq!(
        value.serialized_with("", "", "\n"),
        r#"{"a":[1,{"b":null}],"c":"d"}"#
    );
}

/// `rejectsWhatIsNotJSON`
#[test]
fn rejects_what_is_not_json() {
    for text in [
        "",
        "{",
        "[1,]x",
        "{\"a\" 1}",
        "01",
        "1.",
        "\"\\x\"",
        "\"\\ud800\"",
        "{\"a\":1}}",
        "nul",
    ] {
        assert!(Json::parse(text.as_bytes()).is_err(), "{text:?}");
    }
}

/// `acceptsWhatFoundationAccepts`: a BOM, trailing commas, surrogate pairs.
#[test]
fn accepts_a_bom_trailing_commas_and_surrogate_pairs() {
    let mut bytes = BOM.to_vec();
    bytes.extend_from_slice(br#"{"a": [1, 2,], "e": "\ud83d\ude00",}"#);
    let value = Json::parse(&bytes).expect("accepted");
    assert_eq!(value.get("e").and_then(Json::as_str), Some("😀"));
    assert_eq!(
        value.get("a").and_then(Json::items).map(<[Json]>::len),
        Some(2)
    );
}

/// `equivalenceIgnoresKeyOrderAndNumberSpelling`
#[test]
fn equivalence_ignores_key_order_and_number_spelling() {
    let lhs = parse(r#"{"a": 1.0, "b": [1e2, "x"]}"#);
    let rhs = parse(r#"{"b": [100, "x"], "a": 1}"#);
    assert!(lhs.is_equivalent(&rhs));
    assert!(!lhs.is_equivalent(&parse(r#"{"a": 1, "b": ["x", 100]}"#)));
    // A repeated key: the last one counts, as JavaScript reads it.
    let repeated = parse(r#"{"a": 1, "a": 2}"#);
    assert!(Json::equivalent(repeated.get("a"), Some(&Json::int(2))));
    assert!(Json::equivalent(None, None));
    assert!(!Json::equivalent(repeated.get("a"), None));
}

/// `setKeepsPositionAndAppendsNewKeys`
#[test]
fn set_keeps_position_and_appends_new_keys() {
    let mut value = parse(r#"{"a": 1, "b": 2, "a": 3}"#);
    value.set("a", Some(Json::int(9)));
    value.set("c", Some(Json::Bool(false)));
    value.set("b", None);
    assert_eq!(value.serialized_with("", "", "\n"), r#"{"a":9,"c":false}"#);
}

#[test]
fn nesting_deeper_than_the_limit_is_refused() {
    let depth = agentnotch_engine::core::settings_doc::MAX_DEPTH;
    let ok = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    assert!(Json::parse(ok.as_bytes()).is_ok());
    let deep = format!("{}{}", "[".repeat(depth + 1), "]".repeat(depth + 1));
    assert!(Json::parse(deep.as_bytes()).is_err());
}

// ---- SettingsDocumentTests ----

/// `untouchedDocumentsKeepTheirBytes`
#[test]
fn untouched_documents_keep_their_bytes() {
    let source = "{\n    \"z\" : 0.10,\n  \"a\":[ ]\n}\n";
    assert_eq!(text(&document(source)), source);
}

/// `replacingAValueKeepsEverythingAroundIt`
#[test]
fn replacing_a_value_keeps_everything_around_it() {
    let mut document =
        document("{\n  \"model\": \"opus\",\n  \"hooks\": {\"x\": 1},\n  \"tiny\": 1e-7\n}\n");
    document.set(
        "hooks",
        Some(Json::object([("Stop", Json::Array(Vec::new()))])),
    );
    assert_eq!(
        text(&document),
        "{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"Stop\": []\n  },\n  \"tiny\": 1e-7\n}\n"
    );
}

/// `addingAndRemovingAMemberRoundTrips`
#[test]
fn adding_and_removing_a_member_round_trips() {
    let source = "{\n  \"model\": \"opus\",\n  \"env\": {\n    \"B\": \"1\"\n  }\n}";
    let mut first = document(source);
    first.set(
        "statusLine",
        Some(Json::object([("type", Json::string("command"))])),
    );
    let added = text(&first);
    assert!(
        added.ends_with("  },\n  \"statusLine\": {\n    \"type\": \"command\"\n  }\n}"),
        "{added}"
    );

    let mut again = document(&added);
    again.set("statusLine", None);
    assert_eq!(text(&again), source);
}

/// `removingTheFirstMemberAndEveryMember`
#[test]
fn removing_the_first_member_and_every_member() {
    let mut document = document(r#"{"a": 1, "b": 2}"#);
    document.set("a", None);
    assert_eq!(text(&document), r#"{"b": 2}"#);
    document.set("b", None);
    assert_eq!(text(&document), "{}");
}

/// `aMissingOrBlankFileIsEmpty`
#[test]
fn a_missing_or_blank_file_is_empty() {
    for data in [None, Some(&b""[..]), Some(&b" \n"[..])] {
        let document = SettingsDocument::new(data).expect("blank is the empty object");
        assert_eq!(document.value().members().map(<[Member]>::len), Some(0));
    }
    let mut fresh = SettingsDocument::new(None).expect("no file is the empty object");
    fresh.set("hooks", Some(Json::Object(Vec::new())));
    assert_eq!(text(&fresh), "{\n  \"hooks\": {}\n}\n");
}

/// `refusesAnythingButAnObject`
#[test]
fn refuses_anything_but_an_object() {
    for text in ["[1]", "\"s\"", "{", "{} {}", "\u{FEFF}[]"] {
        assert!(
            SettingsDocument::new(Some(text.as_bytes())).is_none(),
            "{text:?}"
        );
    }
    // Bytes that aren't UTF-8 are not blank, and not JSON.
    assert!(SettingsDocument::new(Some(&[0xFF, 0xFE, 0x00, 0x7B])).is_none());
}

#[test]
fn a_repeated_key_keeps_only_its_last_occurrence_when_set() {
    let mut document = document("{\n  \"hooks\": 1,\n  \"model\": \"opus\",\n  \"hooks\": 2\n}\n");
    document.set("hooks", Some(Json::int(3)));
    assert_eq!(
        text(&document),
        "{\n  \"model\": \"opus\",\n  \"hooks\": 3\n}\n"
    );
}

#[test]
fn a_trailing_comma_survives_a_splice() {
    // Strict parsers refuse the file, so only ours is asked; the comma stays.
    let mut document = document("{\n  \"model\": \"opus\",\n}\n");
    document.set("hooks", Some(Json::Object(Vec::new())));
    assert_eq!(
        text(&document),
        "{\n  \"model\": \"opus\",\n  \"hooks\": {},\n}\n"
    );
}

// ---- What Windows adds: CRLF and the byte order mark ----

#[test]
fn crlf_files_are_spliced_with_crlf() {
    let source =
        "{\r\n  \"model\": \"opus\",\r\n  \"hooks\": {\"x\": 1},\r\n  \"tiny\": 1e-7\r\n}\r\n";
    let mut document = document(source);
    document.set(
        "hooks",
        Some(Json::object([("Stop", Json::Array(Vec::new()))])),
    );
    assert_eq!(
        text(&document),
        "{\r\n  \"model\": \"opus\",\r\n  \"hooks\": {\r\n    \"Stop\": []\r\n  },\r\n  \"tiny\": 1e-7\r\n}\r\n"
    );
}

#[test]
fn crlf_additions_and_removals_round_trip() {
    let source = "{\r\n\t\"model\": \"opus\",\r\n\t\"env\": {\r\n\t\t\"B\": \"1\"\r\n\t}\r\n}\r\n";
    let mut first = document(source);
    first.set(
        "statusLine",
        Some(Json::object([
            ("type", Json::string("command")),
            ("command", Json::string("x")),
        ])),
    );
    let added = text(&first);
    // The addition uses the file's own indentation (tabs) and newline.
    assert!(
        added.ends_with(
            "\t},\r\n\t\"statusLine\": {\r\n\t\t\"type\": \"command\",\r\n\t\t\"command\": \"x\"\r\n\t}\r\n}\r\n"
        ),
        "{added:?}"
    );
    assert!(
        !added.replace("\r\n", "").contains('\n'),
        "a bare LF was written"
    );

    let mut again = document(&added);
    again.set("statusLine", None);
    assert_eq!(text(&again), source);
}

#[test]
fn the_byte_order_mark_is_kept() {
    let mut source = BOM.to_vec();
    source.extend_from_slice(b"{\r\n  \"model\": \"opus\"\r\n}\r\n");
    let mut document = SettingsDocument::new(Some(&source)).expect("a BOM is not an error");
    assert_eq!(document.data(), source, "untouched bytes");
    assert_eq!(document.get("model").and_then(Json::as_str), Some("opus"));

    document.set("hooks", Some(Json::Object(Vec::new())));
    let mut expected = BOM.to_vec();
    expected.extend_from_slice(b"{\r\n  \"model\": \"opus\",\r\n  \"hooks\": {}\r\n}\r\n");
    assert_eq!(document.data(), expected);

    // Every member removed: the object is written afresh, the BOM still first.
    let mut emptied = SettingsDocument::new(Some(&source)).expect("parses");
    emptied.set("model", None);
    let mut expected = BOM.to_vec();
    expected.extend_from_slice(b"{}\r\n");
    assert_eq!(emptied.data(), expected);
}

#[test]
fn a_blank_file_keeps_its_bom_and_newline_when_first_written() {
    // What "New text document, saved as UTF-8 with BOM" leaves.
    let mut blank = BOM.to_vec();
    blank.extend_from_slice(b"\r\n");
    assert!(is_blank(&blank));
    let mut document = SettingsDocument::new(Some(&blank)).expect("blank is the empty object");
    document.set("hooks", Some(Json::Object(Vec::new())));
    let mut expected = BOM.to_vec();
    expected.extend_from_slice(b"{\r\n  \"hooks\": {}\r\n}\r\n");
    assert_eq!(document.data(), expected);
}

#[test]
fn a_file_mixing_newlines_gets_the_majority_style_inside_the_splice_only() {
    // Two CRLF, one LF: the splice writes CRLF, the odd LF stays where it was.
    let source = "{\r\n  \"a\": 1,\n  \"b\": 2\r\n}";
    assert_eq!(newline_style(source.as_bytes()), "\r\n");
    let mut crlf = document(source);
    crlf.set("hooks", Some(Json::object([("x", Json::int(1))])));
    assert_eq!(
        text(&crlf),
        "{\r\n  \"a\": 1,\n  \"b\": 2,\r\n  \"hooks\": {\r\n    \"x\": 1\r\n  }\r\n}"
    );

    // Two LF, one CRLF: LF.
    let source = "{\n  \"a\": 1,\r\n  \"b\": 2\n}";
    assert_eq!(newline_style(source.as_bytes()), "\n");
    let mut lf = document(source);
    lf.set("hooks", Some(Json::object([("x", Json::int(1))])));
    assert_eq!(
        text(&lf),
        "{\n  \"a\": 1,\r\n  \"b\": 2,\n  \"hooks\": {\n    \"x\": 1\n  }\n}"
    );
}

#[test]
fn spans_are_byte_offsets_past_the_bom() {
    let mut source = BOM.to_vec();
    source.extend_from_slice(b"  {\"a\": 1}");
    let top = parse_top_level_object(&source)
        .expect("parses")
        .expect("an object");
    assert_eq!(top.object_start, BOM.len() + 2);
    assert_eq!(top.object_end, source.len());
    assert_eq!(top.spans.len(), 1);
    assert_eq!(
        &source[top.spans[0].value_start..top.spans[0].value_end],
        b"1"
    );
    assert!(!top.trailing_comma);
}
