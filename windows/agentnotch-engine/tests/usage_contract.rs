//! `UsageSource::contract_name` against the cloud contract: the names the
//! website accepts for `usage[].source` (web/contract/README.md) and the
//! values of the contract's own request fixture.

use agentnotch_engine::model::UsageSource;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

const ALL: [UsageSource; 4] = [
    UsageSource::Probe,
    UsageSource::StatusLine,
    UsageSource::Cache,
    UsageSource::Desktop,
];

fn contract_file(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/contract")
        .join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn names() -> BTreeSet<&'static str> {
    ALL.iter().map(|s| s.contract_name()).collect()
}

#[test]
fn contract_names_are_the_wire_names() {
    assert_eq!(UsageSource::Probe.contract_name(), "probe");
    assert_eq!(UsageSource::StatusLine.contract_name(), "statusLine");
    assert_eq!(UsageSource::Cache.contract_name(), "claudeJson");
    assert_eq!(UsageSource::Desktop.contract_name(), "desktop");
    assert_eq!(names().len(), 4, "every source has its own name");
}

#[test]
fn every_usage_source_of_the_fixture_is_a_contract_name() {
    let fixture: Value = serde_json::from_str(&contract_file("fixtures/sync-request.json"))
        .expect("the fixture is JSON");
    let usage = fixture["usage"].as_array().expect("usage[]");
    assert!(!usage.is_empty());
    for reading in usage {
        let source = reading["source"].as_str().expect("usage[].source");
        assert!(
            names().contains(source),
            "{source:?} is not a name the app can send"
        );
    }
}

#[test]
fn the_names_equal_the_readmes_list() {
    let readme = contract_file("README.md");
    let row = readme
        .lines()
        .find(|line| line.starts_with("| `usage[].source` |"))
        .expect("the README row for usage[].source");
    // The row's second cell: `"probe"`, `"statusLine"`, `"claudeJson"` or `"desktop"`.
    let cell = row.split('|').nth(2).expect("the description cell");
    let listed: BTreeSet<&str> = cell
        .split('"')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, name)| name)
        .collect();
    assert_eq!(listed, names());
}
