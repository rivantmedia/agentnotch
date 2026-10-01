//! The app's side of `web/contract`: keys, the request it encodes, the
//! answers it decodes, dates and limits (the Mac's `CloudContractTests`,
//! plus the Windows path rules and the install secret's privacy).

mod cloud_support;

use agentnotch_engine::cloud::api::ApiError;
use agentnotch_engine::cloud::contract::{
    self, accepts, date, earliest_date, error_code, path as contract_path, ConfigResponse,
    ErrorBody, MeResponse, SessionSource, SyncAccount, SyncDevice, SyncProject, SyncReading,
    SyncRequest, SyncResponse, SyncSession, SyncSummary, SyncTokens, SyncWindow, UsageSourceName,
};
use agentnotch_engine::cloud::files::install_secret;
use agentnotch_engine::cloud::keys;
use agentnotch_engine::cloud::website;
use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::core::paths::PathStyle;
use agentnotch_engine::model::IdentityId;
use agentnotch_engine::platform::SecureFiles;
use cloud_support::{contract_fixture, contract_fixture_json, CloudAccountBuilder, CloudFixture};
use serde_json::Value;
use std::time::{Duration, SystemTime};

const DAY: f64 = 24.0 * 3600.0;

fn d(text: &str) -> SystemTime {
    date::parse(text).unwrap_or_else(|| panic!("not a date: {text}"))
}

fn after(t: SystemTime, seconds: f64) -> SystemTime {
    if seconds >= 0.0 {
        t + Duration::from_secs_f64(seconds)
    } else {
        t - Duration::from_secs_f64(-seconds)
    }
}

// ---- Keys ----

#[test]
fn keys_match_the_fixture() {
    let fixture = contract_fixture_json("keys.json");
    let accounts = fixture["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    for account in accounts {
        let uuid = account["accountUuid"].as_str().unwrap();
        let organization = account["organizationUuid"].as_str();
        let key = account["key"].as_str().unwrap();
        assert_eq!(keys::account_key(uuid, organization), key);
        // Case never matters: the key is of the lowercased value.
        assert_eq!(
            keys::account_key(
                &uuid.to_uppercase(),
                organization.map(str::to_uppercase).as_deref()
            ),
            key
        );
        assert!(keys::is_key(key));
    }
    // Project keys: an HMAC with the install's secret, never a plain hash of the path.
    let secret = CloudFixture::install_secret();
    assert_eq!(secret.len(), install_secret::LENGTH);
    let projects = fixture["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 2);
    let other_secret = install_secret::random().unwrap();
    for project in projects {
        let account_key = project["accountKey"].as_str().unwrap();
        let path = project["path"].as_str().unwrap();
        let key = project["key"].as_str().unwrap();
        assert_eq!(keys::project_key(account_key, path, &secret), key);
        assert_ne!(keys::project_key(account_key, path, &other_secret), key);
        assert_ne!(keys::sha256_hex(format!("{account_key}:{path}")), key);
    }
}

fn identity(
    id: &str,
    folders: Vec<agentnotch_engine::runtime_types::CloudFolder>,
) -> agentnotch_engine::runtime_types::CloudAccount {
    let mut builder = CloudAccountBuilder::new(id);
    for folder in folders {
        builder = builder.folder(folder);
    }
    builder.build()
}

/// Regression (review finding 6): the key comes from the account's own UUID
/// and organization, never from how this PC groups identities.
#[test]
fn account_keys_depend_only_on_the_accounts_own_organization() {
    let org = CloudFixture::WORK_ORGANIZATION;
    let folder =
        || CloudAccountBuilder::run_folder("/Users/me/.claude-work", Some(&org.to_uppercase()));
    // Alone on this PC: not split, but its organization is known.
    let alone = identity(CloudFixture::WORK_IDENTITY_ID, vec![folder()]);
    // Beside a folder of the same login in another organization: split.
    let split = identity(
        &format!("{}/{org}", CloudFixture::WORK_IDENTITY_ID),
        vec![folder()],
    );
    assert_eq!(
        keys::account_key_of(&alone).as_deref(),
        Some(CloudFixture::WORK_ACCOUNT_KEY)
    );
    assert_eq!(
        keys::account_key_of(&split).as_deref(),
        Some(CloudFixture::WORK_ACCOUNT_KEY)
    );
    // Organization unknown: the account UUID alone (keys.json's first account).
    let no_organization = identity(
        CloudFixture::IDENTITY_ID,
        vec![CloudAccountBuilder::run_folder("/Users/me/.claude", None)],
    );
    assert_eq!(
        keys::account_key_of(&no_organization).as_deref(),
        Some(CloudFixture::ACCOUNT_KEY)
    );
    // A mirrored copy's (stale) organization is never used.
    let mirrored = identity(
        CloudFixture::IDENTITY_ID,
        vec![CloudAccountBuilder::mirrored_folder(
            "/Users/me/.claude",
            "stale-org",
        )],
    );
    assert_eq!(
        keys::account_key_of(&mirrored).as_deref(),
        Some(CloudFixture::ACCOUNT_KEY)
    );
    // No account UUID: nothing the same on every computer.
    assert_eq!(
        keys::account_key_of(&identity("email:me@example.com", vec![])),
        None
    );
    assert_eq!(
        keys::account_key_of(&identity(r"dir:C:\Users\me\.claude-work", vec![])),
        None
    );
    // The Mac checks the same through its account list; here the identity
    // alone gives the account UUID the list would carry.
    assert_eq!(
        IdentityId::from(CloudFixture::WORK_IDENTITY_ID).account_uuid(),
        Some(CloudFixture::WORK_UUID)
    );
    assert_eq!(
        keys::account_key_for(&IdentityId::from(CloudFixture::WORK_IDENTITY_ID), Some(org))
            .as_deref(),
        Some(CloudFixture::WORK_ACCOUNT_KEY)
    );
}

/// Regression (review findings 18 and 2): the install secret is made once,
/// kept private, and read back unchanged.
#[test]
fn the_install_secret_is_made_once_and_kept_privately() {
    let root = tempfile::tempdir().unwrap();
    let files = StdSecureFiles;
    let first = install_secret::load(root.path(), &files).expect("made");
    assert_eq!(first.len(), 32);
    let file = root.path().join(install_secret::FILE_NAME);
    if cfg!(unix) {
        // Plain std can't read an ACL; agentnotch-win's files prove it there.
        assert!(files.is_private(&file).unwrap());
    }
    assert_eq!(
        install_secret::load(root.path(), &files),
        Some(first.clone())
    );
    assert_eq!(std::fs::read(&file).unwrap(), first);
    // Only the file itself: no temporary file left beside it.
    let names: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        vec![std::ffi::OsString::from(install_secret::FILE_NAME)]
    );
    // Created only if absent: a second maker loses and reads the first's.
    let other = install_secret::random().unwrap();
    assert!(!files.create_exclusive(&file, &other).unwrap());
    assert_eq!(
        install_secret::load(root.path(), &files),
        Some(first.clone())
    );
    // A damaged one is replaced, and stays private.
    std::fs::write(&file, [1u8, 2, 3]).unwrap();
    let replaced = install_secret::load(root.path(), &files).expect("replaced");
    assert!(replaced.len() == 32 && replaced != first);
    assert_eq!(std::fs::read(&file).unwrap(), replaced);
    if cfg!(unix) {
        assert!(files.is_private(&file).unwrap());
    }
}

#[test]
fn the_install_secret_makes_its_folder_and_gives_up_quietly() {
    let root = tempfile::tempdir().unwrap();
    let files = StdSecureFiles;
    // A folder that doesn't exist yet is made (private) first.
    let support = root.path().join("Agent Notch").join("Claude");
    let made = install_secret::load(&support, &files).expect("made");
    if cfg!(unix) {
        assert!(files
            .is_private(&support.join(install_secret::FILE_NAME))
            .unwrap());
    }
    assert_eq!(install_secret::load(&support, &files), Some(made));
    // A folder that can't be written: no secret, so the caller keeps one in memory.
    let blocker = root.path().join("a-file");
    std::fs::write(&blocker, b"x").unwrap();
    assert_eq!(install_secret::load(&blocker.join("support"), &files), None);
}

#[test]
fn project_paths_expand_the_home_and_resolve_links() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real").join("app");
    std::fs::create_dir_all(&real).unwrap();
    let files = StdSecureFiles;
    let home = root.path();
    let resolved_real = files
        .canonical(&real)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    #[cfg(unix)]
    {
        let link = root.path().join("link");
        std::os::unix::fs::symlink(root.path().join("real"), &link).unwrap();
        let through_link = format!("{}/app/", link.display());
        let resolved = keys::project_path(&through_link, home, &files);
        assert_eq!(resolved, resolved_real);
        assert_eq!(keys::project_path("~/real/app", home, &files), resolved);
    }
    // Without a link: the folder's own canonical path, a trailing separator not counted.
    let plain = format!("{}{}", real.display(), std::path::MAIN_SEPARATOR);
    assert_eq!(keys::project_path(&plain, home, &files), resolved_real);
    // A folder that is gone is kept as written, normalized.
    // (the host's own separators: `\nowhere\at\all` on Windows, which
    // normalizes the way the engine's `Paths` does)
    let gone = keys::project_path("/nowhere/at/all/", home, &files);
    assert_eq!(
        gone,
        if cfg!(windows) {
            r"\nowhere\at\all"
        } else {
            "/nowhere/at/all"
        }
    );
    assert_eq!(
        keys::project_name("/Users/me/code/agentnotch/"),
        "agentnotch"
    );
    assert_eq!(
        keys::project_name("/Users/me/work/billing-service"),
        "billing-service"
    );
}

#[test]
fn windows_project_paths_resolve_junctions_and_keep_the_case_on_disk() {
    let style = PathStyle::Windows;
    let home = r"C:\Users\Me";
    let none = |_: &str| None;
    // Typed in any case and any separator, a folder that is gone is normalized.
    assert_eq!(
        keys::project_path_in(style, "c:/code/app/", home, &none),
        r"C:\code\app"
    );
    assert_eq!(
        keys::project_path_in(style, r"~\code\app", home, &none),
        r"C:\Users\Me\code\app"
    );
    assert_eq!(
        keys::project_path_in(style, "~/code/app", home, &none),
        r"C:\Users\Me\code\app"
    );
    // A drive root keeps its backslash; the name of one is `C:`.
    assert_eq!(keys::project_path_in(style, r"C:\", home, &none), r"C:\");
    assert_eq!(keys::project_name(r"C:\"), "C:");
    // The case on disk decides, so `c:\code` and `C:\CODE` share a key.
    let on_disk = |p: &str| {
        p.eq_ignore_ascii_case(r"c:\code\app")
            .then(|| r"C:\Code\App".to_owned())
    };
    let secret = CloudFixture::install_secret();
    let typed_lower = keys::project_path_in(style, r"c:\code\app", home, &on_disk);
    let typed_upper = keys::project_path_in(style, r"C:\CODE\APP\", home, &on_disk);
    assert_eq!(typed_lower, r"C:\Code\App");
    assert_eq!(typed_lower, typed_upper);
    assert_eq!(
        keys::project_key(CloudFixture::ACCOUNT_KEY, &typed_lower, &secret),
        keys::project_key(CloudFixture::ACCOUNT_KEY, &typed_upper, &secret)
    );
    // A junction resolves to its target, and the `\\?\` the system adds is stripped.
    let junction = |p: &str| {
        p.eq_ignore_ascii_case(r"C:\link\app")
            .then(|| r"\\?\D:\real\app".to_owned())
    };
    assert_eq!(
        keys::project_path_in(style, r"C:\link\app", home, &junction),
        r"D:\real\app"
    );
    assert_eq!(
        keys::project_path_in(style, r"\\?\C:\code\.\x\..\app", home, &none),
        r"C:\code\app"
    );
}

// ---- The request ----

/// The fixture's request, built from Rust values.
fn fixture_request() -> SyncRequest {
    let key_a = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
    let key_b = "d8485d82cdbceb2582311953e97b1666022b76dcbb98ef283fece56b1b5b8874";
    let secret = CloudFixture::install_secret();
    SyncRequest {
        schema_version: 1,
        device: SyncDevice {
            id: "0E6F0B4C-2F7A-4E53-9D1B-6A2C7F9E1D35".into(),
            name: "Studio MacBook Pro".into(),
            app_version: "1.18.0".into(),
        },
        accounts: vec![
            SyncAccount {
                key: key_a.into(),
                email: Some("me@example.com".into()),
                organization_name: None,
                plan: Some("Max 20x".into()),
                label: Some("Personal".into()),
            },
            SyncAccount {
                key: key_b.into(),
                email: Some("me@company.com".into()),
                organization_name: Some("Company".into()),
                plan: Some("Team".into()),
                label: None,
            },
        ],
        sessions: vec![
            SyncSession {
                account_key: key_a.into(),
                session_id: "a1b2c3d4-e5f6-4789-8abc-def012345678".into(),
                project: SyncProject {
                    key: keys::project_key(key_a, "/Users/me/code/agentnotch", &secret),
                    name: "agentnotch".into(),
                },
                title: Some("Keep sessions working while background agents run".into()),
                source: SessionSource::Vscode,
                models: vec!["claude-opus-4-5-20251101".into(), "claude-haiku-4-5-20251001".into()],
                started_at: d("2026-09-25T08:02:11.482Z"),
                last_activity_at: d("2026-09-25T09:47:03Z"),
                ended_at: Some(d("2026-09-25T09:48:00Z")),
                message_count: 212,
                tokens: SyncTokens { input: 18234, output: 96512, cache_creation: 402118, cache_read: 12873120 },
                cost_usd: Some(14.82),
                summary: Some(SyncSummary {
                    text: "Changed the notch so a Claude Code session stays 'working' while workflows it started are still running, with tests.".into(),
                    model: "claude-haiku-4-5-20251001".into(),
                    generated_at: d("2026-09-25T10:05:00Z"),
                }),
            },
            SyncSession {
                account_key: key_b.into(),
                session_id: "0f9e8d7c-6b5a-4493-8271-605f4e3d2c1b".into(),
                project: SyncProject {
                    key: keys::project_key(key_b, "/Users/me/work/billing-service", &secret),
                    name: "billing-service".into(),
                },
                title: None,
                source: SessionSource::Cli,
                models: vec!["claude-sonnet-4-5-20250929".into()],
                started_at: d("2026-09-25T11:00:00Z"),
                last_activity_at: d("2026-09-25T11:20:42Z"),
                ended_at: None,
                message_count: 37,
                tokens: SyncTokens { input: 5120, output: 8840, cache_creation: 64000, cache_read: 910000 },
                cost_usd: None,
                summary: None,
            },
        ],
        usage: vec![
            SyncReading {
                account_key: key_a.into(),
                source: UsageSourceName::Desktop,
                observed_at: d("2026-09-25T09:50:00Z"),
                windows: vec![
                    SyncWindow { id: "session".into(), utilization: 42.0, resets_at: Some(d("2026-09-25T13:00:00Z")) },
                    SyncWindow { id: "weekly_all".into(), utilization: 61.5, resets_at: Some(d("2026-09-29T08:00:00Z")) },
                    SyncWindow { id: "weekly_opus".into(), utilization: 12.0, resets_at: None },
                ],
            },
            SyncReading {
                account_key: key_b.into(),
                source: UsageSourceName::Probe,
                observed_at: d("2026-09-25T11:15:00Z"),
                windows: vec![SyncWindow {
                    id: "session".into(),
                    utilization: 8.0,
                    resets_at: Some(d("2026-09-25T15:00:00Z")),
                }],
            },
        ],
    }
}

fn fixture_now() -> SystemTime {
    d("2026-09-25T12:00:00Z")
}

/// Same keys at every level (an explicit null counts as present), same
/// values; dates compared as instants (the app writes milliseconds).
fn compare(ours: &Value, theirs: &Value, path: &str, differences: &mut Vec<String>) {
    match (ours, theirs) {
        (Value::Object(left), Value::Object(right)) => {
            let left_keys: std::collections::BTreeSet<_> = left.keys().collect();
            let right_keys: std::collections::BTreeSet<_> = right.keys().collect();
            if left_keys != right_keys {
                differences.push(format!("{path}: keys {left_keys:?} vs {right_keys:?}"));
            }
            for key in left_keys.intersection(&right_keys) {
                compare(
                    &left[*key],
                    &right[*key],
                    &format!("{path}.{key}"),
                    differences,
                );
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            if left.len() != right.len() {
                differences.push(format!("{path}: {} items vs {}", left.len(), right.len()));
                return;
            }
            for (index, (a, b)) in left.iter().zip(right).enumerate() {
                compare(a, b, &format!("{path}[{index}]"), differences);
            }
        }
        (Value::String(left), Value::String(right)) => {
            if left == right {
                return;
            }
            if let (Some(a), Some(b)) = (date::parse(left), date::parse(right)) {
                if date::rounded_ms(a).abs_diff(date::rounded_ms(b)) < 1 {
                    return;
                }
            }
            differences.push(format!("{path}: {left} vs {right}"));
        }
        (Value::Number(left), Value::Number(right)) => {
            if left.as_f64() != right.as_f64() {
                differences.push(format!("{path}: {left} vs {right}"));
            }
        }
        (Value::Null, Value::Null) => {}
        (Value::Bool(left), Value::Bool(right)) if left == right => {}
        _ => differences.push(format!("{path}: {ours} vs {theirs}")),
    }
}

#[test]
fn the_shape_comparison_notices_what_differs() {
    let mut differences = Vec::new();
    compare(
        &serde_json::json!({"a": null, "b": [1, "2026-09-25T09:47:03.000Z"], "c": 1}),
        &serde_json::json!({"a": 0, "b": [1, "2026-09-25T09:47:03Z"], "d": 1}),
        "$",
        &mut differences,
    );
    assert_eq!(differences.len(), 2, "{differences:?}");
    // An explicit null is present; leaving it out is a difference.
    let mut differences = Vec::new();
    compare(
        &serde_json::json!({"a": null}),
        &serde_json::json!({}),
        "$",
        &mut differences,
    );
    assert_eq!(differences.len(), 1);
}

#[test]
fn encoded_request_has_the_fixtures_shape() {
    let fixture = contract_fixture_json("sync-request.json");
    let clamped = fixture_request().clamped(fixture_now());
    let ours: Value = serde_json::from_slice(&contract::to_json(&clamped)).unwrap();
    let mut differences = Vec::new();
    compare(&ours, &fixture, "$", &mut differences);
    assert!(differences.is_empty(), "{differences:?}");
    // The explicit nulls the website's schemas need, and no summary where there is none.
    assert!(ours["accounts"][0]["organizationName"].is_null());
    assert!(ours["sessions"][1]["title"].is_null() && ours["sessions"][1]["endedAt"].is_null());
    assert!(ours["sessions"][1]["costUsd"].is_null());
    assert!(ours["sessions"][1].get("summary").is_none());
    assert!(ours["usage"][0]["windows"][2]["resetsAt"].is_null());
}

#[test]
fn fixture_request_decodes_and_reencodes_to_itself() {
    let data = contract_fixture("sync-request.json");
    let decoded: SyncRequest = serde_json::from_slice(&data).unwrap();
    assert_eq!(decoded, fixture_request());
    assert_eq!(decoded.clamped(fixture_now()), decoded);
    // The bytes sorted, and the same however they are written out.
    let again: SyncRequest = serde_json::from_slice(&contract::to_json(&decoded)).unwrap();
    assert_eq!(again, decoded);
    let text = String::from_utf8(contract::to_json(&decoded)).unwrap();
    assert!(text.find("\"accounts\"").unwrap() < text.find("\"device\"").unwrap());
    assert!(text.find("\"device\"").unwrap() < text.find("\"schemaVersion\"").unwrap());
}

/// The contract: every date between 2023-01-01 and a day after now. A
/// session or reading dated outside is left out (the website would refuse
/// the whole request); a summary dated outside goes alone.
#[test]
fn dates_outside_the_contracts_range_are_left_out() {
    let now = fixture_now();
    let mut request = fixture_request();
    assert_eq!(request.clamped(now), request);
    let mut ancient = request.sessions[1].clone();
    ancient.session_id = uuid::Uuid::new_v4().to_string();
    ancient.started_at = d("2019-05-01T00:00:00Z");
    let mut future = request.sessions[1].clone();
    future.session_id = uuid::Uuid::new_v4().to_string();
    future.last_activity_at = after(now, 2.0 * DAY);
    let mut ends_later = request.sessions[1].clone();
    ends_later.session_id = uuid::Uuid::new_v4().to_string();
    ends_later.ended_at = Some(after(now, 3.0 * DAY));
    request.sessions.extend([ancient, future, ends_later]);
    request.sessions[0].summary.as_mut().unwrap().generated_at = d("2022-12-31T23:59:59Z");
    let mut old_reading = request.usage[1].clone();
    old_reading.observed_at = d("2020-01-01T00:00:00Z");
    request.usage.push(old_reading);
    // A weekly window resets days ahead: that date stays.
    request.usage[0].windows[1].resets_at = Some(after(now, 6.0 * DAY));

    let clamped = request.clamped(now);
    let ids: Vec<_> = clamped
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            request.sessions[0].session_id.as_str(),
            request.sessions[1].session_id.as_str()
        ]
    );
    assert!(clamped.sessions[0].summary.is_none());
    assert_eq!(clamped.usage.len(), 2);
    assert_eq!(
        clamped.usage[0].windows[1].resets_at,
        Some(after(now, 6.0 * DAY))
    );
    assert!(accepts(earliest_date(), now));
    assert!(!accepts(after(earliest_date(), -1.0), now));
    assert!(accepts(after(now, DAY), now));
    assert!(!accepts(after(now, DAY + 1.0), now));
}

/// Regression (fix check): the website takes a window's reset time from
/// 2023-01-01 to 32 days after its clock and refuses the whole request over
/// one outside. Such a time is sent as null; the reading, and the window's
/// utilization, still go.
#[test]
fn reset_times_the_website_would_refuse_are_sent_as_null() {
    let now = fixture_now();
    let times: [Option<SystemTime>; 8] = [
        Some(std::time::UNIX_EPOCH), // a zero
        Some(after(earliest_date(), -1.0)),
        Some(earliest_date()),
        Some(after(now, 7.0 * DAY)), // next week's reset
        Some(after(now, 32.0 * DAY)),
        Some(after(now, 32.0 * DAY + 1.0)),
        Some(after(now, 400.0 * DAY)),
        None,
    ];
    let ids = [
        "session",
        "weekly_all",
        "extra_usage",
        "weekly_opus",
        "weekly_sonnet",
        "weekly_haiku",
        "weekly_x",
        "weekly_y",
    ];
    let mut request = fixture_request();
    request.usage[0].windows = ids
        .iter()
        .zip(times)
        .map(|(id, resets_at)| SyncWindow {
            id: (*id).into(),
            utilization: 12.5,
            resets_at,
        })
        .collect();
    let clamped = request.clamped(now);
    let windows = &clamped.usage[0].windows;
    assert_eq!(
        windows.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        ids
    );
    assert!(windows.iter().all(|w| w.utilization == 12.5));
    let resets: Vec<_> = windows.iter().map(|w| w.resets_at).collect();
    assert_eq!(
        resets,
        [
            None,
            None,
            Some(earliest_date()),
            Some(after(now, 7.0 * DAY)),
            Some(after(now, 32.0 * DAY)),
            None,
            None,
            None
        ]
    );
    assert_eq!(clamped.clamped(now), clamped);
    // Written as an explicit null, as the contract wants.
    let json: Value = serde_json::from_slice(&contract::to_json(&clamped)).unwrap();
    assert!(json["usage"][0]["windows"][0]["resetsAt"].is_null());
}

// ---- Answers ----

#[test]
fn every_response_fixture_decodes() {
    let config: ConfigResponse = serde_json::from_slice(&contract_fixture("config.json")).unwrap();
    assert_eq!(config.redirect_url, contract::REDIRECT_URL);
    assert_eq!(config.supabase_url, "https://abcdefghijklmnop.supabase.co");
    assert!(config
        .supabase_publishable_key
        .starts_with("sb_publishable_"));
    assert!(website::validated_link(Some(&config.dashboard_url)).is_some());

    let me: MeResponse = serde_json::from_slice(&contract_fixture("me.json")).unwrap();
    assert_eq!(me.user.email.as_deref(), Some("me@example.com"));
    assert_eq!(me.user.name.as_deref(), Some("Me Example"));

    let sync: SyncResponse =
        serde_json::from_slice(&contract_fixture("sync-response.json")).unwrap();
    assert_eq!((sync.accepted.sessions, sync.accepted.usage), (2, 2));
    assert_eq!(sync.server_time, d("2026-09-25T11:21:00Z"));

    let error_bytes = contract_fixture("error.json");
    let error: ErrorBody = serde_json::from_slice(&error_bytes).unwrap();
    assert_eq!(error.error.code, error_code::UNAUTHORIZED);
    let mapped = ApiError::from_response(401, &error_bytes, None);
    assert_eq!(
        mapped,
        ApiError::Server {
            status: 401,
            code: Some("UNAUTHORIZED".into()),
            message: Some("Sign in again.".into()),
            retry_after: None
        }
    );
    assert!(mapped.is_unauthorized());
    assert_eq!(mapped.to_string(), "Sign in again.");
    assert_eq!(
        ApiError::from_response(429, b"", Some("12")).retry_after(),
        Some(12.0)
    );
    assert!(ApiError::from_response(429, b"", Some("12")).is_rate_limited());
}

#[test]
fn dates_are_utc_with_z_and_read_either_way() {
    let t = d("2026-09-25T08:02:11.482Z");
    assert_eq!(date::to_string(t), "2026-09-25T08:02:11.482Z");
    assert!(date::parse("2026-09-25T09:47:03Z").is_some());
    assert_eq!(
        date::to_string(d("2026-09-25T09:47:03Z")),
        "2026-09-25T09:47:03.000Z"
    );
    assert_eq!(date::parse("yesterday"), None);
}

// ---- Limits ----

#[test]
fn clamping_leaves_out_what_the_website_would_refuse() {
    let mut request = fixture_request();
    let key = request.accounts[0].key.clone();
    request.accounts.push(SyncAccount {
        key: "not-a-key".into(),
        email: None,
        organization_name: None,
        plan: None,
        label: None,
    });
    let mut bad = request.sessions[1].clone();
    bad.session_id = "not-a-uuid".into();
    request.sessions.push(bad);
    let mut stranger = request.sessions[1].clone();
    stranger.session_id = uuid::Uuid::new_v4().to_string();
    stranger.account_key = "a".repeat(64);
    request.sessions.push(stranger);
    request.sessions[0].title = Some("t".repeat(500));
    request.sessions[0].models = ["", "m", "m"]
        .iter()
        .map(|m| m.to_string())
        .chain((0..20).map(|i| format!("model-{i}")))
        .collect();
    request.sessions[0].cost_usd = Some(f64::NAN);
    request.sessions[0].tokens.input = -5;
    let window = |id: String, utilization: f64| SyncWindow {
        id,
        utilization,
        resets_at: None,
    };
    request.usage[0].windows.push(window("bogus".into(), 3.0));
    request.usage[0]
        .windows
        .push(window(format!("weekly_{}", "x".repeat(80)), 3.0));
    request.usage[0]
        .windows
        .push(window("weekly_neg".into(), -2.0));
    request.usage.push(SyncReading {
        account_key: "b".repeat(64),
        source: UsageSourceName::Probe,
        observed_at: CloudFixture::base(),
        windows: vec![window("session".into(), 1.0)],
    });

    let clamped = request.clamped(fixture_now());
    assert_eq!(
        clamped
            .accounts
            .iter()
            .map(|a| a.key.as_str())
            .collect::<Vec<_>>(),
        [key.as_str(), request.accounts[1].key.as_str()]
    );
    assert_eq!(clamped.sessions.len(), 2);
    assert_eq!(
        clamped.sessions[0]
            .title
            .as_deref()
            .map(|t| t.chars().count()),
        Some(200)
    );
    assert_eq!(clamped.sessions[0].models.len(), 10);
    assert_eq!(clamped.sessions[0].models[0], "m");
    assert_eq!(clamped.sessions[0].cost_usd, None);
    assert_eq!(clamped.sessions[0].tokens.input, 0);
    assert_eq!(clamped.usage.len(), 2);
    let ids: Vec<_> = clamped.usage[0]
        .windows
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    assert_eq!(ids, ["session", "weekly_all", "weekly_opus", "weekly_neg"]);
    assert_eq!(clamped.usage[0].windows.last().unwrap().utilization, 0.0);
}

#[test]
fn window_ids_follow_the_websites_rule() {
    assert!(
        keys::is_window_id("session")
            && keys::is_window_id("weekly_all")
            && keys::is_window_id("extra_usage")
    );
    assert!(keys::is_window_id("weekly_sonnet_4_5") && keys::is_window_id("weekly_opus-4.1"));
    assert!(
        !keys::is_window_id("weekly_")
            && !keys::is_window_id("weekly__x")
            && !keys::is_window_id("monthly")
    );
    let long = format!("weekly_{}", "a".repeat(90));
    assert_eq!(
        keys::contract_window_id(&long).map(|id| id.len()),
        Some(7 + 57)
    );
    assert_eq!(keys::contract_window_id("nonsense"), None);
    // The website takes lowercase only.
    assert_eq!(
        keys::contract_window_id("weekly_Opus").as_deref(),
        Some("weekly_opus")
    );
    assert!(!keys::is_window_id("weekly_Opus"));
}

#[test]
fn session_sources_from_entrypoints() {
    let source = SessionSource::from_entrypoint;
    assert_eq!(source(Some("cli")), SessionSource::Cli);
    assert_eq!(source(Some("claude-vscode")), SessionSource::Vscode);
    assert_eq!(source(Some("claude-desktop")), SessionSource::Desktop);
    assert_eq!(source(Some("claude-desktop-3p")), SessionSource::Desktop);
    assert_eq!(source(Some("local-agent")), SessionSource::Desktop);
    assert_eq!(source(Some("sdk-cli")), SessionSource::Sdk);
    assert_eq!(source(Some("sdk-ts")), SessionSource::Sdk);
    assert_eq!(source(None), SessionSource::Other);
    assert_eq!(source(Some("something-new")), SessionSource::Other);
}

// ---- The website's address ----

#[test]
fn only_https_or_this_pc_are_accepted() {
    let valid = |text: &str| website::validated(Some(text));
    assert_eq!(
        valid("https://agentnotch.example.com/").as_deref(),
        Some("https://agentnotch.example.com")
    );
    assert_eq!(
        valid("agentnotch.example.com").as_deref(),
        Some("https://agentnotch.example.com")
    );
    assert_eq!(
        valid("HTTPS://Example.com/app/").as_deref(),
        Some("https://example.com/app")
    );
    assert_eq!(
        valid("http://localhost:3000").as_deref(),
        Some("http://localhost:3000")
    );
    assert_eq!(
        valid("http://127.0.0.1:3000").as_deref(),
        Some("http://127.0.0.1:3000")
    );
    assert_eq!(
        valid("http://[::1]:3000").as_deref(),
        Some("http://[::1]:3000")
    );
    assert_eq!(valid("http://example.com"), None);
    assert_eq!(valid("ftp://example.com"), None);
    assert_eq!(valid("https://example.com/?next=x"), None);
    assert_eq!(valid("https://user:pw@example.com"), None);
    assert_eq!(valid("   "), None);
    assert_eq!(website::validated(None), None);
    assert_eq!(
        website::endpoint("https://example.com/app", contract_path::SYNC),
        "https://example.com/app/api/app/v1/sync"
    );
}

#[test]
fn the_repositorys_app_config_names_a_website_the_app_accepts() {
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app-config.json");
    let text = std::fs::read_to_string(file).unwrap();
    let config: Value = serde_json::from_str(&text).unwrap();
    let url = config["websiteURL"].as_str().expect("a websiteURL string");
    assert!(url.is_empty() || website::validated(Some(url)).as_deref() == Some(url));
    // The build's own rules accept it, and name the same website.
    let checked = website::check_app_config(&text).unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(checked.as_deref(), (!url.is_empty()).then_some(url));
    assert_eq!(website::from_app_config(&text), checked);
}
