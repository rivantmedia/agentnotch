//! The release tool's formats and its command line (DESIGN-WIN §6.4, "Tests (WP11)"): the
//! Appendix B vector (in the unit tests), signing and verifying through the updater's own crate,
//! compatibility with a signature `tauri signer sign` made, every rejection the release gate
//! relies on, the feed round trip, and a seed that only ever travels on stdin.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use agentnotch_release::{
    check_feed, feed_json, key_id_of_feed, key_id_of_pubkey, key_id_of_signature, parse_feed,
    verify, FeedInput, Seed, UpdateKey, FEED_PLATFORMS,
};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// Appendix B's test seed (bytes 0x00..0x1f). Never a real key.
const TEST_SEED: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEI1QTU2MzgzNjFGQkQwMTkKUldRWjBQdGhnMk9sdGN3UFAralJPTnNJUnh6bjFOZUVGamloRlNMUWZTako4Qzk4RlRGZi9qdjQK";
/// A second, unrelated test seed (bytes 0x20..0x3f): a "foreign" key.
const OTHER_SEED: &str = "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=";

const INSTALLER: &str = "AgentNotch-1.1.0-Setup.exe";

fn key(seed: &str) -> UpdateKey {
    UpdateKey::derive(&Seed::from_base64(seed.as_bytes()).unwrap())
}

fn installer_bytes() -> Vec<u8> {
    // Not an installer, just bytes with every value in them.
    (0..=255u8).cycle().take(70_000).collect()
}

/// `tauri-plugin-updater` 2.12.0's `verify_signature`, line for line (updater.rs 1530-1554 and
/// `verify_signed_version`): the check every installed copy runs before it installs an update.
fn updater_accepts(pubkey: &str, data: &[u8], signature: &str, announced: &str) -> bool {
    let decode = |s: &str| {
        STANDARD
            .decode(s)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    };
    let (Some(pk_text), Some(sig_text)) = (decode(pubkey), decode(signature)) else {
        return false;
    };
    let (Ok(pk), Ok(sig)) = (
        minisign_verify::PublicKey::decode(&pk_text),
        minisign_verify::Signature::decode(&sig_text),
    ) else {
        return false;
    };
    if pk.verify(data, &sig, true).is_err() {
        return false;
    }
    sig.trusted_comment()
        .split('\t')
        .find_map(|f| f.strip_prefix("version:"))
        == Some(announced)
}

#[test]
fn a_signature_verifies_with_the_updaters_own_check() {
    let key = key(TEST_SEED);
    assert_eq!(key.tauri_pubkey(), TEST_PUBKEY);
    let data = installer_bytes();
    let sig = key.sign(&data, INSTALLER, "1.1.0", 1_790_000_000).unwrap();
    assert!(!sig.ends_with('\n'), "a .sig value carries no newline");
    assert!(updater_accepts(TEST_PUBKEY, &data, &sig, "1.1.0"));
    assert!(!updater_accepts(TEST_PUBKEY, &data, &sig, "1.1.1"));

    let verified = verify(TEST_PUBKEY, &data, &sig, "1.1.0", INSTALLER).unwrap();
    assert_eq!(verified.key_id.to_string(), "B5A5638361FBD019");
    assert_eq!(
        verified.trusted_comment,
        "timestamp:1790000000\tfile:AgentNotch-1.1.0-Setup.exe\tversion:1.1.0"
    );
    assert_eq!(key_id_of_signature(&sig).unwrap(), key.key_id());

    // The text is minisign's prehashed layout with our untrusted comment.
    let text = String::from_utf8(STANDARD.decode(&sig).unwrap()).unwrap();
    let lines: Vec<&str> = text.split('\n').collect();
    assert_eq!(lines.len(), 5, "four lines and a final newline");
    assert_eq!(
        lines[0],
        "untrusted comment: signature from agentnotch release key"
    );
    assert!(lines[1].starts_with("RUQZ0Pthg2Olt"), "ED ‖ key id");
    assert_eq!(STANDARD.decode(lines[1]).unwrap().len(), 74);
    assert!(lines[2].starts_with("trusted comment: timestamp:1790000000\t"));
    assert_eq!(STANDARD.decode(lines[3]).unwrap().len(), 64);
    assert_eq!(lines[4], "");
}

#[test]
fn signing_is_deterministic_and_pinned() {
    // Ed25519 signatures are deterministic, so the exact bytes can be pinned: any change to the
    // derivation, the prehash, the comment layout or the encoding shows up here. (The value
    // was checked with the updater's own verifier above before it was pinned.)
    let data = b"Agent Notch update signature fixture: a stand-in for an installer.\n";
    let sig = key(TEST_SEED)
        .sign(data, "AgentNotch-0.9.0-Setup.exe", "0.9.0", 1_790_594_405)
        .unwrap();
    assert!(updater_accepts(TEST_PUBKEY, data, &sig, "0.9.0"));
    assert_eq!(sig, include_str!("fixtures/golden-0.9.0.sig"));
}

#[test]
fn a_signature_the_tauri_cli_made_is_accepted() {
    // Made once with `tauri signer sign --app-version 0.9.0` (@tauri-apps/cli 2.12.0) and a
    // throwaway key generated for this purpose alone; only public material is committed.
    let dir = fixtures().join("tauri-signer");
    let data = std::fs::read(dir.join("AgentNotch-0.9.0-Setup.exe")).unwrap();
    let sig = std::fs::read_to_string(dir.join("AgentNotch-0.9.0-Setup.exe.sig")).unwrap();
    let pubkey = std::fs::read_to_string(dir.join("throwaway-key.pub")).unwrap();
    assert!(updater_accepts(&pubkey, &data, &sig, "0.9.0"));
    let verified = verify(&pubkey, &data, &sig, "0.9.0", "AgentNotch-0.9.0-Setup.exe").unwrap();
    assert_eq!(verified.key_id.to_string(), "BBAE346681FA028D");
    assert_eq!(key_id_of_pubkey(&pubkey).unwrap(), verified.key_id);
    assert_eq!(key_id_of_signature(&sig).unwrap(), verified.key_id);
    // Its text has the same layout as ours (only the untrusted comment differs).
    let ours = key(TEST_SEED)
        .sign(&data, "AgentNotch-0.9.0-Setup.exe", "0.9.0", 1)
        .unwrap();
    let layout = |s: &str| {
        let text = String::from_utf8(STANDARD.decode(s).unwrap()).unwrap();
        text.split('\n')
            .skip(1)
            .map(|l| l.len().min(17))
            .collect::<Vec<_>>()
    };
    assert_eq!(layout(&sig), layout(&ours));
    // And our key is not the throwaway one.
    assert!(verify(
        TEST_PUBKEY,
        &data,
        &sig,
        "0.9.0",
        "AgentNotch-0.9.0-Setup.exe"
    )
    .is_err());
}

#[test]
fn the_release_gate_refuses() {
    let key = key(TEST_SEED);
    let data = installer_bytes();
    let sig = key.sign(&data, INSTALLER, "1.1.0", 1_790_000_000).unwrap();

    let reason = |r: Result<_, agentnotch_release::Error>| r.expect_err("refused").0;
    // Another version than the one signed.
    assert!(reason(verify(TEST_PUBKEY, &data, &sig, "1.1.1", INSTALLER)).contains("1.1.0"));
    // Another file name than the one signed.
    assert!(reason(verify(
        TEST_PUBKEY,
        &data,
        &sig,
        "1.1.0",
        "AgentNotch-1.1.0-Setup (1).exe"
    ))
    .contains("file"));
    // One flipped byte in the installer.
    let mut flipped = data.clone();
    flipped[12_345] ^= 0x01;
    assert!(verify(TEST_PUBKEY, &flipped, &sig, "1.1.0", INSTALLER).is_err());
    assert!(!updater_accepts(TEST_PUBKEY, &flipped, &sig, "1.1.0"));
    // A trusted comment edited after signing (the global signature no longer covers it).
    let text = String::from_utf8(STANDARD.decode(&sig).unwrap()).unwrap();
    let edited = STANDARD.encode(text.replace("version:1.1.0", "version:9.9.9"));
    assert!(verify(TEST_PUBKEY, &data, &edited, "9.9.9", INSTALLER).is_err());
    assert!(!updater_accepts(TEST_PUBKEY, &data, &edited, "9.9.9"));
    // A foreign key: its id differs, and so does the key.
    let foreign = crate::key(OTHER_SEED)
        .sign(&data, INSTALLER, "1.1.0", 1_790_000_000)
        .unwrap();
    assert_ne!(key_id_of_signature(&foreign).unwrap(), key.key_id());
    assert!(reason(verify(TEST_PUBKEY, &data, &foreign, "1.1.0", INSTALLER)).contains("key"));
    // A foreign signature relabelled with our key id still fails the Ed25519 check.
    let relabelled = relabel(&foreign, &key.key_id().0);
    assert!(verify(TEST_PUBKEY, &data, &relabelled, "1.1.0", INSTALLER).is_err());
    // A value with a trailing newline, which the updater's strict decoder would refuse.
    assert!(reason(verify(
        TEST_PUBKEY,
        &data,
        &format!("{sig}\n"),
        "1.1.0",
        INSTALLER
    ))
    .contains("base64"));
    // minisign's legacy (not prehashed) kind: the updater allows it, the release gate does not.
    let legacy = legacy_signature(&data, "1.1.0");
    assert!(updater_accepts(TEST_PUBKEY, &data, &legacy, "1.1.0"));
    assert!(reason(verify(TEST_PUBKEY, &data, &legacy, "1.1.0", INSTALLER)).contains("legacy"));
    // A signature without a version field is refused outright.
    let unversioned = unversioned_signature(&data);
    assert!(
        reason(verify(TEST_PUBKEY, &data, &unversioned, "1.1.0", INSTALLER)).contains("version")
    );
    // Bad inputs are refused before any signing.
    assert!(key.sign(&data, INSTALLER, "1.1", 1).is_err());
    assert!(key.sign(&data, "has space.exe", "1.1.0", 1).is_err());
    assert!(key.sign(&data, "tab\tname.exe", "1.1.0", 1).is_err());
}

fn feed_input(sig: &str) -> FeedInput {
    FeedInput {
        version: "1.1.0".into(),
        tag: "agentnotch-v1.1.0".into(),
        repo: "rivantmedia/agentnotch".into(),
        installer: INSTALLER.into(),
        signature: sig.into(),
        notes_url: "https://github.com/rivantmedia/agentnotch/releases/tag/agentnotch-v1.1.0"
            .into(),
        pub_date: "2026-10-01T12:00:00Z".into(),
    }
}

#[test]
fn the_feed_round_trips() {
    let key = key(TEST_SEED);
    let data = installer_bytes();
    let sig = key.sign(&data, INSTALLER, "1.1.0", 1_790_000_000).unwrap();
    let input = feed_input(&sig);
    let json = feed_json(&input).unwrap();

    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["version"], "1.1.0");
    assert_eq!(
        value["notes"],
        "https://github.com/rivantmedia/agentnotch/releases/tag/agentnotch-v1.1.0"
    );
    assert_eq!(value["pub_date"], "2026-10-01T12:00:00Z");
    let url = "https://github.com/rivantmedia/agentnotch/releases/download/agentnotch-v1.1.0/AgentNotch-1.1.0-Setup.exe";
    for platform in FEED_PLATFORMS {
        assert_eq!(value["platforms"][platform]["signature"], sig.as_str());
        assert_eq!(value["platforms"][platform]["url"], url);
    }
    assert_eq!(value["platforms"].as_object().unwrap().len(), 2);
    assert!(!json.contains("latest/download"));
    // The keys come in the documented order.
    let order: Vec<usize> = ["\"version\"", "\"notes\"", "\"pub_date\"", "\"platforms\""]
        .iter()
        .map(|k| json.find(k).unwrap())
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]));

    let feed = parse_feed(&json).unwrap();
    assert_eq!(feed.entries.len(), 2);
    assert_eq!(
        key_id_of_feed(&json).unwrap().to_string(),
        "B5A5638361FBD019"
    );
    let verified = check_feed(&json, &input, TEST_PUBKEY, &data).unwrap();
    assert_eq!(verified.version, "1.1.0");
    assert!(updater_accepts(
        TEST_PUBKEY,
        &data,
        value["platforms"]["windows-x86_64-nsis"]["signature"]
            .as_str()
            .unwrap(),
        "1.1.0"
    ));
}

#[test]
fn a_tampered_feed_is_refused() {
    let key = key(TEST_SEED);
    let data = installer_bytes();
    let sig = key.sign(&data, INSTALLER, "1.1.0", 1_790_000_000).unwrap();
    let input = feed_input(&sig);
    let json = feed_json(&input).unwrap();

    let newer = json.replace("\"version\": \"1.1.0\"", "\"version\": \"1.2.0\"");
    assert!(check_feed(&newer, &input, TEST_PUBKEY, &data).is_err());
    let moved = json.replace(
        "/download/agentnotch-v1.1.0/",
        "/download/agentnotch-v1.0.0/",
    );
    assert!(check_feed(&moved, &input, TEST_PUBKEY, &data).is_err());
    let one_platform = {
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["platforms"]
            .as_object_mut()
            .unwrap()
            .remove("windows-x86_64");
        v.to_string()
    };
    assert!(check_feed(&one_platform, &input, TEST_PUBKEY, &data).is_err());
    let undated = {
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v.as_object_mut().unwrap().remove("pub_date");
        v.to_string()
    };
    assert!(check_feed(&undated, &input, TEST_PUBKEY, &data).is_err());
    // Two keys in one feed.
    let foreign = crate::key(OTHER_SEED)
        .sign(&data, INSTALLER, "1.1.0", 1)
        .unwrap();
    let mixed = {
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["platforms"]["windows-x86_64"]["signature"] = foreign.clone().into();
        v.to_string()
    };
    assert!(key_id_of_feed(&mixed).is_err());
    assert!(check_feed(&mixed, &input, TEST_PUBKEY, &data).is_err());

    // Inputs a feed is never written from.
    let mut bad = input.clone();
    bad.tag = "agentnotch-v1.0.0".into();
    assert!(feed_json(&bad).is_err());
    let mut bad = input.clone();
    bad.repo = "rivantmedia/agentnotch/extra".into();
    assert!(feed_json(&bad).is_err());
    let mut bad = input.clone();
    bad.notes_url = "http://example.com".into();
    assert!(feed_json(&bad).is_err());
    let mut bad = input.clone();
    bad.pub_date = "yesterday".into();
    assert!(feed_json(&bad).is_err());
    let mut bad = input;
    bad.signature = format!("{sig}\n");
    assert!(feed_json(&bad).is_err());
}

// ---- the command line ------------------------------------------------------------------

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agentnotch-release-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn arg(&self, name: &str) -> String {
        self.path(name).to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tool(args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentnotch-release"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut pipe = child.stdin.take().unwrap();
        if let Some(input) = stdin {
            // A command that refuses before reading may have closed its stdin already.
            let _ = pipe.write_all(input.as_bytes());
        }
    }
    child.wait_with_output().unwrap()
}

fn stdout_json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn derive_public_reads_the_seed_from_stdin_only() {
    let out = tool(&["derive-public"], Some(&format!("{TEST_SEED}\n")));
    let json = stdout_json(&out);
    assert_eq!(json["pubkey"], TEST_PUBKEY);
    assert_eq!(json["key_id"], "B5A5638361FBD019");
    assert_eq!(
        json["minisign_public_key"],
        "RWQZ0Pthg2OltcwPP+jRONsIRxzn1NeEFjihFSLQfSjJ8C98FTFf/jv4"
    );
    assert_eq!(
        json["sparkle_public_key"],
        "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg="
    );
    let everything = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!everything.contains(TEST_SEED));

    // A seed as an argument is a usage error, and is never echoed.
    let out = tool(&["derive-public", TEST_SEED], Some(""));
    assert_eq!(out.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(TEST_SEED));
    // Nothing, or not a seed, on stdin: refused without echoing it.
    let out = tool(&["derive-public"], Some(""));
    assert_eq!(out.status.code(), Some(1));
    let not_a_seed = "c2VjcmV0LXNlZWQtbWF0ZXJpYWwtdGhhdC1pcy13cm9uZw==";
    let out = tool(&["derive-public"], Some(not_a_seed));
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(not_a_seed));
    // More than a seed.
    let out = tool(&["derive-public"], Some(&"A".repeat(10_000)));
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn sign_verify_and_feed_from_the_command_line() {
    let dir = Scratch::new();
    std::fs::write(dir.path(INSTALLER), installer_bytes()).unwrap();
    let installer = dir.arg(INSTALLER);
    let sig = dir.arg("AgentNotch-1.1.0-Setup.exe.sig");

    let out = tool(
        &[
            "sign",
            "--file",
            &installer,
            "--version",
            "1.1.0",
            "--out",
            &sig,
            "--timestamp",
            "1790000000",
        ],
        Some(TEST_SEED),
    );
    let json = stdout_json(&out);
    assert_eq!(json["key_id"], "B5A5638361FBD019");
    assert_eq!(
        json["trusted_comment"],
        "timestamp:1790000000\tfile:AgentNotch-1.1.0-Setup.exe\tversion:1.1.0"
    );
    let written = std::fs::read_to_string(&sig).unwrap();
    assert!(!written.ends_with('\n'));
    assert!(updater_accepts(
        TEST_PUBKEY,
        &installer_bytes(),
        &written,
        "1.1.0"
    ));

    let verify_args = |version: &str, pubkey: &str| {
        tool(
            &[
                "verify",
                "--pubkey",
                pubkey,
                "--file",
                &installer,
                "--sig",
                &sig,
                "--version",
                version,
            ],
            None,
        )
    };
    assert_eq!(
        stdout_json(&verify_args("1.1.0", TEST_PUBKEY))["key_id"],
        "B5A5638361FBD019"
    );
    let wrong = verify_args("1.1.1", TEST_PUBKEY);
    assert_eq!(wrong.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("1.1.1"));
    let foreign_pubkey = key(OTHER_SEED).tauri_pubkey();
    assert_eq!(verify_args("1.1.0", &foreign_pubkey).status.code(), Some(1));
    // The bare minisign key line works too.
    assert!(verify_args(
        "1.1.0",
        "RWQZ0Pthg2OltcwPP+jRONsIRxzn1NeEFjihFSLQfSjJ8C98FTFf/jv4"
    )
    .status
    .success());

    let feed = dir.arg("latest.json");
    let feed_args = |installer_name: &str| {
        tool(
            &[
                "feed",
                "--version",
                "1.1.0",
                "--tag",
                "agentnotch-v1.1.0",
                "--repo",
                "rivantmedia/agentnotch",
                "--installer",
                installer_name,
                "--sig",
                &sig,
                "--pubkey",
                TEST_PUBKEY,
                "--file",
                &installer,
                "--notes-url",
                "https://github.com/rivantmedia/agentnotch/releases/tag/agentnotch-v1.1.0",
                "--out",
                &feed,
                "--pub-date",
                "2026-10-01T12:00:00Z",
            ],
            None,
        )
    };
    let json = stdout_json(&feed_args(INSTALLER));
    assert_eq!(json["key_id"], "B5A5638361FBD019");
    let latest = std::fs::read_to_string(&feed).unwrap();
    assert_eq!(parse_feed(&latest).unwrap().version, "1.1.0");
    assert_eq!(
        feed_args("AgentNotch-1.1.1-Setup.exe").status.code(),
        Some(1)
    );

    let out = tool(&["key-id-of-feed", &feed], None);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "B5A5638361FBD019"
    );
    let out = tool(&["key-id-of-pubkey", TEST_PUBKEY], None);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "B5A5638361FBD019"
    );

    // A default pub_date is now, in RFC 3339.
    std::fs::remove_file(&feed).unwrap();
    let out = tool(
        &[
            "feed",
            "--version",
            "1.1.0",
            "--tag",
            "agentnotch-v1.1.0",
            "--repo",
            "rivantmedia/agentnotch",
            "--installer",
            INSTALLER,
            "--sig",
            &sig,
            "--pubkey",
            TEST_PUBKEY,
            "--file",
            &installer,
            "--notes-url",
            "https://github.com/rivantmedia/agentnotch/releases/tag/agentnotch-v1.1.0",
            "--out",
            &feed,
        ],
        None,
    );
    assert!(out.status.success());
    let date = parse_feed(&std::fs::read_to_string(&feed).unwrap())
        .unwrap()
        .pub_date
        .unwrap();
    assert!(agentnotch_release::check_rfc3339(&date).is_ok(), "{date}");
}

#[test]
fn sign_refuses_before_reading_the_seed() {
    let dir = Scratch::new();
    std::fs::write(dir.path(INSTALLER), b"x").unwrap();
    let installer = dir.arg(INSTALLER);
    let sig = dir.arg("out.sig");
    for (version, file) in [
        ("1.1", installer.as_str()),
        ("1.1.0", "/definitely/not/here.exe"),
    ] {
        let out = tool(
            &["sign", "--file", file, "--version", version, "--out", &sig],
            Some(TEST_SEED),
        );
        assert_eq!(out.status.code(), Some(1), "{version} {file}");
        assert!(!dir.path("out.sig").exists());
    }
    // A wrong seed writes nothing either.
    let out = tool(
        &[
            "sign",
            "--file",
            &installer,
            "--version",
            "1.1.0",
            "--out",
            &sig,
        ],
        Some("not base64"),
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(!dir.path("out.sig").exists());
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        vec![],
        vec!["bogus"],
        vec!["sign", "--file", "x"],
        vec!["verify", "--pubkey", "p", "--file", "f", "--sig", "s"],
        vec![
            "verify",
            "--pubkey",
            "p",
            "--pubkey",
            "p",
            "--file",
            "f",
            "--sig",
            "s",
            "--version",
            "1.0.0",
        ],
        vec![
            "sign",
            "--file",
            "f",
            "--version",
            "1.0.0",
            "--out",
            "o",
            "--seed",
            "x",
        ],
        vec![
            "sign",
            "--file",
            "f",
            "--version",
            "1.0.0",
            "--out",
            "o",
            "--timestamp",
            "-1",
        ],
        vec!["key-id-of-feed"],
        vec!["key-id-of-pubkey", "a", "b"],
    ] {
        let out = tool(&args, Some(""));
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
    let out = tool(&["--help"], None);
    assert!(out.status.success());
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Swaps the key id inside a signature's blob (the rest stays the foreign key's).
fn relabel(signature: &str, id: &[u8; 8]) -> String {
    let text = String::from_utf8(STANDARD.decode(signature).unwrap()).unwrap();
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let mut blob = STANDARD.decode(&lines[1]).unwrap();
    blob[2..10].copy_from_slice(id);
    lines[1] = STANDARD.encode(blob);
    STANDARD.encode(lines.join("\n"))
}

/// A minisign legacy signature ("Ed", the whole file signed, not its BLAKE2b hash), made with
/// the test key's own derived secret, so only the algorithm differs.
fn legacy_signature(data: &[u8], version: &str) -> String {
    raw_signature(
        data,
        b"Ed",
        &format!("timestamp:1\tfile:{INSTALLER}\tversion:{version}"),
    )
}

fn unversioned_signature(data: &[u8]) -> String {
    use blake2::{Blake2b512, Digest};
    raw_signature(
        &Blake2b512::digest(data),
        b"ED",
        &format!("timestamp:1\tfile:{INSTALLER}"),
    )
}

fn raw_signature(signed: &[u8], algorithm: &[u8; 2], trusted: &str) -> String {
    use ed25519_dalek::{Signer, SigningKey};
    use hkdf::Hkdf;
    use sha2::Sha256;
    let seed = STANDARD.decode(TEST_SEED).unwrap();
    let hk = Hkdf::<Sha256>::new(Some(agentnotch_release::HKDF_SALT), &seed);
    let mut okm = [0u8; 40];
    hk.expand(agentnotch_release::HKDF_INFO, &mut okm).unwrap();
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&okm[..32]);
    let signing = SigningKey::from_bytes(&secret);
    let sig = signing.sign(signed);
    let mut global = sig.to_bytes().to_vec();
    global.extend_from_slice(trusted.as_bytes());
    let global = signing.sign(&global);
    let mut blob = algorithm.to_vec();
    blob.extend_from_slice(&okm[32..40]);
    blob.extend_from_slice(&sig.to_bytes());
    STANDARD.encode(format!(
        "untrusted comment: test\n{}\ntrusted comment: {trusted}\n{}\n",
        STANDARD.encode(blob),
        STANDARD.encode(global.to_bytes())
    ))
}
