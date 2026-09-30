//! `IdentityPrefs.ring_hidden` (`ringHidden`): the Windows app's "Ring in
//! notch" off, kept with the rest of an identity's choices. It is written
//! only when set, so an `accounts.json` the Mac wrote reads and writes back
//! unchanged.

use agentnotch_engine::persist::accounts::{AccountsFile, IdentityPrefs};
use std::collections::BTreeMap;

const KEY: &str = "uuid:5f0c3a1e-0000-4000-8000-000000000001";

fn file_with(ring_hidden: bool) -> AccountsFile {
    let mut identities = BTreeMap::new();
    identities.insert(
        KEY.to_owned(),
        IdentityPrefs {
            custom_label: Some("Work".to_owned()),
            color_index: 2,
            is_hidden: false,
            ring_hidden,
        },
    );
    AccountsFile {
        version: 2,
        accounts: Vec::new(),
        removed_ids: Vec::new(),
        identities: Some(identities),
        forgotten_identities: None,
        default_identity_timeline: None,
    }
}

fn encoded(file: &AccountsFile) -> String {
    String::from_utf8(file.encode()).expect("utf-8")
}

#[test]
fn ring_hidden_is_left_out_when_false() {
    let text = encoded(&file_with(false));
    assert!(!text.contains("ringHidden"), "{text}");
    let back = AccountsFile::parse(text.as_bytes()).expect("parses");
    assert!(!back.identities.unwrap()[KEY].ring_hidden);
}

#[test]
fn ring_hidden_is_written_when_true_and_read_back() {
    let text = encoded(&file_with(true));
    assert!(
        text.contains(r#""ringHidden" : true"#) || text.contains(r#""ringHidden": true"#),
        "{text}"
    );
    let back = AccountsFile::parse(text.as_bytes()).expect("parses");
    let prefs = &back.identities.unwrap()[KEY];
    assert!(prefs.ring_hidden);
    assert_eq!(prefs.custom_label.as_deref(), Some("Work"));
    assert_eq!(prefs.color_index, 2);
}

#[test]
fn the_mac_fixture_has_no_ring_hidden_and_keeps_none() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mac-files/accounts.json"
    );
    let bytes = std::fs::read(path).expect("fixture");
    let file = AccountsFile::parse(&bytes).expect("parses");
    let identities = file
        .identities
        .as_ref()
        .expect("the fixture has identities");
    assert!(!identities.is_empty());
    assert!(identities.values().all(|prefs| !prefs.ring_hidden));
    let text = encoded(&file);
    assert!(!text.contains("ringHidden"), "{text}");
}
