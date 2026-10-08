//! The app's half of the cross-side check (`Scripts/cloud-contract-e2e.sh`;
//! the Mac's `CloudContractE2ETests`): a sync request built and encoded by
//! the engine's own code, from transcripts, the ledger and the usage
//! outbox, and the website's own answer read back.
//!
//! Without the script's variables both tests still run: the request is
//! checked against the contract's rules, and the contract's response
//! fixture is read. With them, the request is also written to
//! `AGENTNOTCH_CONTRACT_OUT` (the website's integration test posts it
//! through its real sync route into Postgres), and the answer that route
//! gave is read from `AGENTNOTCH_CONTRACT_RESPONSE`. Nothing here reaches
//! the network.
//!
//! The script runs the two tests by exact name, `contract_e2e_request` and
//! `contract_e2e_response`, and requires that each passed.

mod cloud_support;

use agentnotch_engine::cloud::api::CloudApi;
use agentnotch_engine::cloud::auth::{Auth, MemorySessionStore};
use agentnotch_engine::cloud::contract::{self, date, SyncRequest};
use agentnotch_engine::model::{BackfillFolder, LiveSessionObservation, UsageSource};
use agentnotch_engine::platform::{Clock, HttpResponse};
use agentnotch_engine::runtime_types::{CloudAccount, UsageObservation};
use agentnotch_engine::testkit::http::{json_response, path_of, FixtureHttp};
use agentnotch_engine::testkit::FakeClock;
use cloud_support::*;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REQUEST_FILE_VARIABLE: &str = "AGENTNOTCH_CONTRACT_OUT";
const RESPONSE_FILE_VARIABLE: &str = "AGENTNOTCH_CONTRACT_RESPONSE";

/// A path the script passed, if any.
fn file(variable: &str) -> Option<PathBuf> {
    let value = std::env::var(variable).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

// ---- The scenario ----

fn personal() -> CloudAccount {
    CloudAccountBuilder::new(CloudFixture::IDENTITY_ID)
        .email("me@example.com")
        .plan("Max 20x")
        .label("Personal ☕")
        .build()
}

fn work() -> CloudAccount {
    CloudAccountBuilder::new(CloudFixture::WORK_IDENTITY_ID)
        .email("me@company.com")
        .organization_name("Société Exemple")
        .plan("Team")
        .folder(CloudAccountBuilder::run_folder(
            "/Users/me/.claude-work",
            Some(CloudFixture::WORK_ORGANIZATION),
        ))
        .build()
}

/// Run by the personal account, ended, summarised; one subagent.
const SESSION_A: &str = CloudFixture::SESSION_A;
/// Run by the work account in VS Code, still running.
const SESSION_B: &str = CloudFixture::SESSION_B;
/// Begun as the personal account, resumed as the work one: two rows.
const SESSION_C: &str = CloudFixture::SESSION_C;
/// Found on disk in a folder only the personal account uses (backfilled).
const SESSION_D: &str = "5e6f7a8b-9c0d-4e1f-8a2b-3c4d5e6f7a8b";

const CWD_A: &str = "/Users/me/code/agentnotch";
const CWD_B: &str = "/Users/me/work/billing-service";
const CWD_C: &str = "/Users/me/code/données-client";
const CWD_D: &str = "/Users/me/work/reports";

/// The scenario's own clock: its seconds count from a few hours before the
/// real now, because the website checks dates against its own clock (and
/// keeps readings for 90 days). The transcript builders count from the
/// fixture's base, so [`rel`](Self::rel) shifts a scenario second there.
struct Morning {
    base: SystemTime,
    offset: f64,
}

impl Morning {
    fn new() -> Morning {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_secs_f64();
        let base_seconds = (now - 4.0 * 3600.0).floor();
        let fixture = CloudFixture::base()
            .duration_since(UNIX_EPOCH)
            .expect("a base after 1970")
            .as_secs_f64();
        Morning {
            base: UNIX_EPOCH + Duration::from_secs_f64(base_seconds),
            offset: base_seconds - fixture,
        }
    }

    /// The moment `seconds` after the scenario's base.
    fn at(&self, seconds: f64) -> SystemTime {
        if seconds >= 0.0 {
            self.base + Duration::from_secs_f64(seconds)
        } else {
            self.base - Duration::from_secs_f64(-seconds)
        }
    }

    fn stamp(&self, seconds: f64) -> String {
        date::to_string(self.at(seconds))
    }

    /// `seconds` of the scenario as the line builders count them.
    fn rel(&self, seconds: f64) -> f64 {
        self.offset + seconds
    }
}

/// The summary Claude Code's `-p` answers with in the scenario.
fn summary_answer() -> String {
    json!({
        "type": "result", "subtype": "success", "is_error": false, "num_turns": 1,
        "result": "Réparé le rafraîchissement du jeton ✓ — rotating refresh tokens are kept, with tests.",
        "total_cost_usd": 0.002,
        "modelUsage": {"claude-haiku-4-5-20251001": {"outputTokens": 12}}
    })
    .to_string()
}

fn usage(
    account: &CloudAccount,
    source: UsageSource,
    at: SystemTime,
    windows: &[(&str, f64, Option<SystemTime>)],
) -> UsageObservation {
    UsageObservation {
        identity: account.identity_id.clone(),
        source,
        observed_at: at,
        windows: windows
            .iter()
            .map(|(id, utilization, resets)| ((*id).to_owned(), *utilization, *resets))
            .collect(),
    }
}

/// A morning on two accounts, as the sync service captures and sends it:
/// live sessions the hub attributed, a session resumed under the other
/// account, a folder's own history backfilled, usage readings from four
/// sources, and a summary. The website is down for the first pass, so the
/// next one carries everything in one request. Everything is dated a few
/// hours before now (the real clock's, as the Mac's test does).
fn morning() -> (Harness, Morning) {
    let m = Morning::new();
    let (personal, work) = (personal(), work());
    let (a, b, c, d) = (SESSION_A, SESSION_B, SESSION_C, SESSION_D);

    let h = Harness::with(HarnessOptions {
        summaries_on: true,
        ..HarnessOptions::default()
    });
    *h.deps.accounts.lock().unwrap() = vec![personal.clone(), work.clone()];
    h.answer_summary(&summary_answer());
    h.handles.clock.set(m.at(-120.0));
    // The sign-in the harness saved was made at its own clock: make a fresh one.
    h.save_session(AuthFixture::session(7 * 24 * 3600, h.now()));
    h.start();

    // The backfilled folder, seen signed in as the personal account before
    // its session began (with sync off, so nothing is sent yet).
    let own = h.handles.roots.home.join(".claude-own");
    let own_text = own.to_string_lossy().into_owned();
    let reports = own.join("projects").join("-Users-me-work-reports");
    Lines::write(
        &[
            Lines::user_in(
                "MY SECRET PROMPT about the report",
                d,
                m.rel(60.0),
                CWD_D,
                "sdk-ts",
            ),
            Lines::ai_title("Monthly usage report", d),
            Lines::assistant("d1", "rd1", d)
                .model("claude-sonnet-4-5")
                .usage(1200, 340)
                .cache(5000, 0)
                .at(m.rel(90.0))
                .cwd(CWD_D)
                .line(),
            Lines::assistant("d2", "rd2", d)
                .model("claude-sonnet-4-5")
                .usage(30, 900)
                .cache(0, 5000)
                .at(m.rel(120.0))
                .cwd(CWD_D)
                .line(),
        ],
        &reports.join(format!("{d}.jsonl")),
        false,
    );
    *h.deps.backfill_folders.lock().unwrap() = vec![BackfillFolder {
        config_dir: own_text.clone(),
        identity_id: Some(personal.identity_id.clone()),
        account_key: Some(key_of(&personal)),
        signed_in_since: None,
    }];
    *h.deps.folder_logins.lock().unwrap() =
        Some(BTreeMap::from([(own_text, "login-personal".to_owned())]));
    h.handles.clock.set(m.at(-60.0));
    h.service.set_sync(false, h.now());
    h.tick();
    h.service.set_sync(true, h.now());

    // The live sessions' transcripts (and A's subagent).
    let config_dir = h.handles.roots.home.join(".claude");
    let projects = config_dir.join("projects");
    let transcript = |slug: &str, id: &str| projects.join(slug).join(format!("{id}.jsonl"));
    let path_a = transcript("-Users-me-code-agentnotch", a);
    let path_b = transcript("-Users-me-work-billing-service", b);
    let path_c = transcript("-Users-me-code-donn-es-client", c);
    Lines::write(
        &[
            Lines::user_in(
                "MY SECRET PROMPT about the login bug",
                a,
                m.rel(300.0),
                CWD_A,
                "cli",
            ),
            Lines::assistant("a1", "ra1", a)
                .usage(1800, 420)
                .cache(24000, 120_000)
                .at(m.rel(330.0))
                .text("Looking into it.")
                .tool_use()
                .cwd(CWD_A)
                .line(),
            Lines::tool_result("TOOL OUTPUT WITH A PATH /Users/me/secret", a, m.rel(331.0)),
            Lines::assistant("a2", "ra2", a)
                .model("claude-haiku-4-5")
                .usage(60, 35)
                .cache(0, 2000)
                .at(m.rel(360.0))
                .text("Fixed it.")
                .cwd(CWD_A)
                .line(),
            Lines::assistant("a3", "ra3", a)
                .usage(12, 880)
                .cache(3000, 144_000)
                .at(m.rel(600.0))
                .cwd(CWD_A)
                .line(),
        ],
        &path_a,
        false,
    );
    Lines::write(
        &[Lines::assistant("a-sub", "ra-sub", a)
            .model("claude-sonnet-4-5")
            .usage(900, 150)
            .cache(8000, 0)
            .at(m.rel(400.0))
            .sidechain()
            .cwd(CWD_A)
            .line()],
        &path_a
            .with_extension("")
            .join("subagents")
            .join("agent-review.jsonl"),
        false,
    );
    // B reads billions of cached tokens: more than a 32-bit integer holds.
    Lines::write(
        &[
            Lines::user_in(
                "MY SECRET PROMPT about invoices",
                b,
                m.rel(900.0),
                CWD_B,
                "claude-vscode",
            ),
            Lines::assistant("b1", "rb1", b)
                .model("claude-sonnet-4-5")
                .usage(5120, 8840)
                .cache(64000, 3_210_987_654)
                .at(m.rel(960.0))
                .cwd(CWD_B)
                .line(),
            Lines::assistant("b2", "rb2", b)
                .model("claude-sonnet-4-5")
                .usage(44, 1210)
                .cache(0, 65000)
                .at(m.rel(1100.0))
                .cwd(CWD_B)
                .line(),
        ],
        &path_b,
        false,
    );
    Lines::write(
        &[
            Lines::user_in(
                "MY SECRET PROMPT about imports",
                c,
                m.rel(1200.0),
                CWD_C,
                "cli",
            ),
            Lines::assistant("c1", "rc1", c)
                .usage(400, 90)
                .cache(1500, 9000)
                .at(m.rel(1230.0))
                .cwd(CWD_C)
                .line(),
            Lines::user_in("go on", c, m.rel(1560.0), CWD_C, "cli"),
            Lines::assistant("c2", "rc2", c)
                .model("claude-sonnet-4-5")
                .usage(70, 30)
                .cache(0, 11000)
                .at(m.rel(1590.0))
                .cwd(CWD_C)
                .line(),
            Lines::assistant("c3", "rc3", c)
                .model("claude-sonnet-4-5")
                .usage(5, 400)
                .cache(0, 12000)
                .at(m.rel(1620.0))
                .cwd(CWD_C)
                .line(),
        ],
        &path_c,
        false,
    );

    let config_text = config_dir.to_string_lossy().into_owned();
    #[allow(clippy::too_many_arguments)]
    let live = |id: &str,
                account: &CloudAccount,
                cwd: &str,
                path: &std::path::Path,
                entrypoint: &str,
                start: f64,
                last: f64,
                cost: Option<f64>,
                title: Option<&str>,
                process: Option<f64>|
     -> LiveSessionObservation {
        LiveSessionObservation {
            session_id: id.to_owned(),
            identity_id: account.identity_id.clone(),
            account_key: key_of(account),
            cwd: cwd.to_owned(),
            transcript_path: Some(path.to_string_lossy().into_owned()),
            config_dir: Some(config_text.clone()),
            entrypoint: Some(entrypoint.to_owned()),
            started_at: m.at(start),
            last_activity_at: m.at(last),
            model: Some("claude-opus-4-5".to_owned()),
            cost_usd: cost,
            title: title.map(str::to_owned),
            process_started_at: process.map(|p| m.at(p)),
            in_shared_history: None,
        }
    };
    let live_a = live(
        a,
        &personal,
        CWD_A,
        &path_a,
        "cli",
        300.0,
        600.0,
        Some(2.4617),
        Some("Fix the login bug 🐛"),
        None,
    );
    let live_b = live(
        b,
        &work,
        CWD_B,
        &path_b,
        "claude-vscode",
        900.0,
        1100.0,
        Some(1.25),
        None,
        None,
    );
    let title_c = "Tidy the données-client imports";
    let live_c = live(
        c,
        &personal,
        CWD_C,
        &path_c,
        "cli",
        1200.0,
        1230.0,
        Some(0.4),
        Some(title_c),
        None,
    );
    let resumed_c = live(
        c,
        &work,
        CWD_C,
        &path_c,
        "claude-vscode",
        1500.0,
        1620.0,
        Some(0.9),
        Some(title_c),
        Some(1500.0),
    );
    let personal_reset = m.at(4.0 * 3600.0);
    let work_reset = m.at(5.0 * 3600.0);
    let personal_week = m.at(4.0 * 24.0 * 3600.0);
    let work_week = m.at(6.0 * 24.0 * 3600.0);

    h.handles.clock.set(m.at(620.0));
    h.observe(vec![live_a.clone()], &[], &[a]);
    h.service.record_usage(usage(
        &personal,
        UsageSource::Desktop,
        m.at(610.0),
        &[
            ("session", 42.0, Some(personal_reset)),
            ("weekly_all", 61.5, Some(personal_week)),
            ("weekly_opus", 12.0, None),
            ("extra_usage", 3.25, None),
        ],
    ));
    h.handles.clock.set(m.at(1110.0));
    h.observe(vec![live_a.clone(), live_b.clone()], &[], &[a, b]);
    h.handles.clock.set(m.at(1240.0));
    h.observe(
        vec![live_a.clone(), live_b.clone(), live_c],
        &[],
        &[a, b, c],
    );
    h.service.record_usage(usage(
        &personal,
        UsageSource::StatusLine,
        m.at(1235.0),
        &[
            ("session", 44.5, Some(personal_reset)),
            ("weekly_all", 62.0, Some(personal_week)),
        ],
    ));
    h.handles.clock.set(m.at(1630.0));
    h.observe(
        vec![live_a, live_b.clone(), resumed_c.clone()],
        &[],
        &[a, b, c],
    );
    h.service.record_usage(usage(
        &work,
        UsageSource::Probe,
        m.at(1625.0),
        &[
            ("session", 8.0, Some(work_reset)),
            ("weekly_all", 0.0, Some(work_week)),
            ("weekly_sonnet", 17.25, Some(work_week)),
        ],
    ));
    h.service.record_usage(usage(
        &work,
        UsageSource::Cache,
        m.at(1640.0),
        &[("session", 9.0, Some(work_reset))],
    ));

    // The website is down for the first pass: nothing is marked sent (the
    // pass still backfills and reads the transcripts).
    h.handles.clock.set(m.at(1700.0));
    h.handles.http.set_handler(|request| {
        if path_of(request) != "/api/app/v1/sync" {
            return Ok(website_answer(request));
        }
        let mut response = json_response(
            503,
            r#"{"error":{"code":"INTERNAL","message":"The website is restarting."}}"#,
        );
        response
            .headers
            .push(("Retry-After".to_owned(), "120".to_owned()));
        Ok(response)
    });
    h.sync_now();
    assert_eq!(
        h.service.state().last_error.as_deref(),
        Some("The website is restarting.")
    );
    h.handles
        .http
        .set_handler(|request| Ok(website_answer(request)));

    // A ends (gone for a minute), and is summarised once it has been quiet.
    h.handles.clock.set(m.at(1800.0));
    h.observe(vec![live_b.clone(), resumed_c.clone()], &[], &[b, c]);
    h.handles.clock.set(m.at(1870.0));
    h.observe(vec![live_b, resumed_c], &[], &[b, c]);
    h.handles.clock.set(m.at(2460.0));
    h.summarize_next();
    assert_eq!(h.handles.runner.spawned().len(), 1);
    let stdin = String::from_utf8(h.handles.runner.stdin_of(0).expect("stdin")).unwrap();
    assert!(stdin.contains("MY SECRET PROMPT about the login bug"));

    // "Sync now": the rest of the morning in one request.
    h.handles.clock.set(m.at(2480.0));
    h.sync_now();
    (h, m)
}

// ---- The request ----

/// The request the engine sends keeps every rule of the contract it can
/// check, carries what the transcripts and readings say, and leaks nothing.
/// With `AGENTNOTCH_CONTRACT_OUT` set, its exact bytes are written there
/// for the website's side.
#[test]
fn contract_e2e_request() {
    let (h, m) = morning();
    let at = |seconds: f64| m.stamp(seconds);
    assert_eq!(h.sync_requests().len(), 2);
    let request = h.sync_requests().last().cloned().expect("a request");
    let body = request.body.clone().expect("a body");
    let state = h.service.state();
    assert_eq!(state.last_error, None);
    assert_eq!((state.pending_usage, state.pending_sessions), (0, 0));

    let json: Value = serde_json::from_slice(&body).expect("JSON");
    let problems = violations(&json, SystemTime::now());
    assert!(problems.is_empty(), "{problems:?}");
    // The engine reads back what it wrote, and it is already within the limits.
    let decoded: SyncRequest = serde_json::from_slice(&body).expect("a sync request");
    assert_eq!(decoded.clamped(SystemTime::now()), decoded);

    let text = String::from_utf8(body.clone()).expect("UTF-8");
    let secret = h.service.stores().unwrap().secret();
    let secret_hex: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    let root = h.root.path().to_string_lossy().into_owned();
    let real_root = std::fs::canonicalize(h.root.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    for leaked in [
        "MY SECRET PROMPT",
        "TOOL OUTPUT",
        "Looking into it",
        "Fixed it.",
        "go on",
        "/Users/me",
        // What a Windows machine would have written.
        "C:\\\\Users",
        "C:\\Users",
        &root,
        &root.replace('\\', "\\\\"),
        &real_root,
        &real_root.replace('\\', "\\\\"),
        ".jsonl",
        CloudFixture::ACCOUNT_UUID,
        CloudFixture::WORK_UUID,
        CloudFixture::WORK_ORGANIZATION,
        &secret_hex,
    ] {
        assert!(!text.contains(leaked), "{leaked} was sent");
    }

    let (personal, work) = (personal(), work());
    let accounts = json["accounts"].as_array().expect("accounts");
    let mut expected_keys = vec![key_of(&personal), key_of(&work)];
    expected_keys.sort();
    let sent_keys: Vec<&str> = accounts.iter().filter_map(|a| a["key"].as_str()).collect();
    assert_eq!(sent_keys, expected_keys);
    let find = |key: &str| {
        accounts
            .iter()
            .find(|a| a["key"] == json!(key))
            .expect("an account")
    };
    let sent_personal = find(&key_of(&personal));
    assert_eq!(sent_personal["label"], json!("Personal ☕"));
    assert_eq!(sent_personal["organizationName"], Value::Null);
    let sent_work = find(&key_of(&work));
    assert_eq!(sent_work["organizationName"], json!("Société Exemple"));
    assert_eq!(sent_work["label"], Value::Null);

    let sessions = json["sessions"].as_array().expect("sessions");
    assert_eq!(sessions.len(), 5);
    let rows: BTreeMap<String, &Value> = sessions
        .iter()
        .map(|s| {
            (
                format!(
                    "{}|{}",
                    s["sessionId"].as_str().unwrap_or(""),
                    s["accountKey"].as_str().unwrap_or("")
                ),
                s,
            )
        })
        .collect();
    assert_eq!(rows.len(), 5);
    let row = |id: &str, account: &CloudAccount| -> &Value {
        rows.get(&format!("{id}|{}", key_of(account)))
            .unwrap_or_else(|| panic!("no row for {id} on {:?}", account.email))
    };
    let tokens = |row: &Value| row["tokens"].clone();
    let project = |row: &Value| row["project"]["name"].clone();

    let a = row(SESSION_A, &personal);
    assert_eq!(
        tokens(a),
        json!({"input": 2772, "output": 1485, "cacheCreation": 35000, "cacheRead": 266_000})
    );
    assert_eq!(a["messageCount"], json!(4));
    assert_eq!(a["source"], json!("cli"));
    assert_eq!(project(a), json!("agentnotch"));
    assert_eq!(a["models"][0], json!("claude-opus-4-5"));
    assert_eq!(a["models"].as_array().unwrap().len(), 3);
    assert_eq!(a["title"], json!("Fix the login bug 🐛"));
    assert_eq!(a["costUsd"], json!(2.4617));
    assert_eq!(a["startedAt"], json!(at(300.0)));
    assert_eq!(a["lastActivityAt"], json!(at(600.0)));
    assert_eq!(a["endedAt"], json!(at(1800.0)));
    let summary = &a["summary"];
    assert!(summary["text"]
        .as_str()
        .unwrap()
        .starts_with("Réparé le rafraîchissement du jeton ✓"));
    assert_eq!(summary["model"], json!("claude-haiku-4-5-20251001"));
    assert_eq!(summary["generatedAt"], json!(at(2460.0)));

    let b = row(SESSION_B, &work);
    assert_eq!(
        tokens(b),
        json!({"input": 5164, "output": 10050, "cacheCreation": 64000, "cacheRead": 3_211_052_654u64})
    );
    assert_eq!(b["messageCount"], json!(2));
    assert_eq!(b["source"], json!("vscode"));
    assert_eq!(project(b), json!("billing-service"));
    assert_eq!(b["title"], Value::Null);
    assert_eq!(b["endedAt"], Value::Null);
    assert!(b.get("summary").is_none());
    // Its billions of cache reads at Sonnet 4.5's list prices come to more
    // than the $1.25 reported: the larger goes.
    assert_eq!(b["costUsd"], json!(963.722038));

    // Resumed under the other account: each part only its own responses,
    // and not Claude Code's cost (it can't be split) but those responses at
    // list prices.
    let (mine, theirs) = (row(SESSION_C, &personal), row(SESSION_C, &work));
    assert_eq!(
        tokens(mine),
        json!({"input": 400, "output": 90, "cacheCreation": 1500, "cacheRead": 9000})
    );
    assert_eq!(
        tokens(theirs),
        json!({"input": 75, "output": 430, "cacheCreation": 0, "cacheRead": 23000})
    );
    assert_eq!(mine["messageCount"], json!(1));
    assert_eq!(theirs["messageCount"], json!(2));
    assert_eq!(mine["endedAt"], json!(at(1500.0)));
    assert_eq!(theirs["endedAt"], Value::Null);
    assert_eq!(mine["costUsd"], json!(0.018125));
    assert_eq!(theirs["costUsd"], json!(0.013575));
    assert_eq!(project(mine), json!("données-client"));
    assert_eq!(project(theirs), json!("données-client"));
    assert_ne!(mine["project"]["key"], theirs["project"]["key"]);

    let d = row(SESSION_D, &personal);
    assert_eq!(
        tokens(d),
        json!({"input": 1230, "output": 1240, "cacheCreation": 5000, "cacheRead": 5000})
    );
    assert_eq!(d["source"], json!("sdk"));
    assert_eq!(d["title"], json!("Monthly usage report"));
    assert_eq!(project(d), json!("reports"));
    assert_eq!(d["startedAt"], json!(at(60.0)));
    assert_eq!(d["endedAt"], json!(at(120.0)));
    // Found only on disk: no status line reported a cost, so its responses
    // at list prices.
    assert_eq!(d["costUsd"], json!(0.04254));

    let usage = json["usage"].as_array().expect("usage");
    let mut sources: Vec<&str> = usage.iter().filter_map(|u| u["source"].as_str()).collect();
    sources.sort_unstable();
    assert_eq!(sources, ["claudeJson", "desktop", "probe", "statusLine"]);
    let windows: usize = usage
        .iter()
        .map(|u| u["windows"].as_array().map_or(0, Vec::len))
        .sum();
    assert_eq!(windows, 10);

    if let Some(out) = file(REQUEST_FILE_VARIABLE) {
        std::fs::write(&out, &body).expect("the request written");
    }
}

/// The rules are real: the contract's own fixture keeps them, and each
/// thing the website's schemas refuse breaks one.
#[test]
fn the_contract_rules_catch_what_the_website_refuses() {
    let now = date::parse("2026-09-25T12:00:00Z").expect("a date");
    let fixture = String::from_utf8(contract_fixture("sync-request.json")).expect("UTF-8");
    let problems = |text: &str| -> Vec<String> {
        match serde_json::from_str::<Value>(text) {
            Ok(json) => violations(&json, now),
            Err(_) => vec!["not JSON".to_owned()],
        }
    };
    let fixture_problems = problems(&fixture);
    assert!(fixture_problems.is_empty(), "{fixture_problems:?}");

    let personal = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
    let long_title = "x".repeat(201);
    let breakages: Vec<(&str, String, String)> = vec![
        (
            "another schema version",
            "\"schemaVersion\": 1".into(),
            "\"schemaVersion\": 2".into(),
        ),
        (
            "a field the contract doesn't have",
            "\"schemaVersion\": 1,".into(),
            "\"schemaVersion\": 1, \"extra\": true,".into(),
        ),
        (
            "a device id that isn't a UUID",
            "0E6F0B4C-2F7A-4E53-9D1B-6A2C7F9E1D35".into(),
            "studio".into(),
        ),
        (
            "an uppercase account key",
            format!("\"key\": \"{personal}\""),
            format!("\"key\": \"{}\"", personal.to_uppercase()),
        ),
        (
            "a session of an account not listed",
            "\"key\": \"d8485d82".into(),
            "\"key\": \"e8485d82".into(),
        ),
        (
            "a null left out",
            "\"endedAt\": null,".into(),
            String::new(),
        ),
        (
            "an unknown source",
            "\"source\": \"cli\"".into(),
            "\"source\": \"terminal\"".into(),
        ),
        (
            "a date with an offset",
            "\"startedAt\": \"2026-09-25T11:00:00Z\"".into(),
            "\"startedAt\": \"2026-09-25T12:00:00+01:00\"".into(),
        ),
        (
            "a date before 2023",
            "\"startedAt\": \"2026-09-25T11:00:00Z\"".into(),
            "\"startedAt\": \"2022-12-31T23:59:59Z\"".into(),
        ),
        (
            "a date two days ahead",
            "\"endedAt\": \"2026-09-25T09:48:00Z\"".into(),
            "\"endedAt\": \"2026-09-27T12:00:01Z\"".into(),
        ),
        (
            "a reset time 33 days ahead",
            "\"resetsAt\": \"2026-09-29T08:00:00Z\"".into(),
            "\"resetsAt\": \"2026-10-28T12:00:01Z\"".into(),
        ),
        (
            "a fractional count",
            "\"messageCount\": 37".into(),
            "\"messageCount\": 37.5".into(),
        ),
        (
            "a negative token count",
            "\"cacheRead\": 910000".into(),
            "\"cacheRead\": -5".into(),
        ),
        (
            "a token count as text",
            "\"cacheRead\": 910000".into(),
            "\"cacheRead\": \"910000\"".into(),
        ),
        (
            "a negative cost",
            "\"costUsd\": 14.82".into(),
            "\"costUsd\": -1".into(),
        ),
        (
            "a window id the website doesn't know",
            "\"id\": \"weekly_opus\"".into(),
            "\"id\": \"monthly\"".into(),
        ),
        (
            "a title that is too long",
            "\"title\": null".into(),
            format!("\"title\": \"{long_title}\""),
        ),
        (
            "a summary without its model",
            "\"model\": \"claude-haiku-4-5-20251001\",".into(),
            String::new(),
        ),
        (
            "a list where a number goes",
            "\"costUsd\": 14.82".into(),
            "\"costUsd\": [14.82]".into(),
        ),
        (
            "a boolean where a number goes",
            "\"messageCount\": 212".into(),
            "\"messageCount\": true".into(),
        ),
    ];
    assert_eq!(breakages.len(), 20);
    for (what, original, replacement) in &breakages {
        assert!(
            fixture.contains(original.as_str()),
            "{what}: the fixture no longer has {original}"
        );
        let broken = fixture.replacen(original.as_str(), replacement, 1);
        let found = problems(&broken);
        assert!(
            !found.is_empty() && found != ["not JSON"],
            "{what} wasn't caught"
        );
    }
}

// ---- The answer ----

/// The website's answer to the request, read by the engine's real client
/// ([`CloudApi::sync`], the one the service calls) and taken by the sync
/// service as success. With `AGENTNOTCH_CONTRACT_RESPONSE` set, the answer
/// is the one the website's real route gave to the request in
/// `AGENTNOTCH_CONTRACT_OUT`; otherwise the contract's fixture.
#[test]
fn contract_e2e_response() {
    let response_file = file(RESPONSE_FILE_VARIABLE);
    let answer = match &response_file {
        Some(path) => std::fs::read(path).expect("the website's answer"),
        None => contract_fixture("sync-response.json"),
    };
    let request_data = if response_file.is_some() {
        let path = file(REQUEST_FILE_VARIABLE)
            .expect("the website's answer needs the request it answered");
        std::fs::read(path).expect("the request")
    } else {
        contract_fixture("sync-request.json")
    };
    let request: SyncRequest = serde_json::from_slice(&request_data).expect("a sync request");

    let http = Arc::new(FixtureHttp::default());
    let body = answer.clone();
    http.set_handler(move |request| {
        if path_of(request) == "/api/app/v1/sync" {
            Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: body.clone(),
            })
        } else {
            Ok(json_response(404, "{}"))
        }
    });
    let clock = Arc::new(FakeClock::new(SystemTime::now()));
    let store = Arc::new(MemorySessionStore::new(Some(AuthFixture::session(
        3600,
        clock.now(),
    ))));
    let auth = Arc::new(Auth::new(http.clone(), store, clock.clone()));
    let api = CloudApi::new(AuthFixture::WEBSITE, http.clone(), Some(auth), "9.9")
        .with_clock(clock.clone());
    let response = api.sync(&request).expect("the answer is read");
    assert_eq!(http.requests_to("/api/app/v1/sync").len(), 1);
    if response_file.is_some() {
        // Everything was new to the website, so everything was stored.
        assert_eq!(response.accepted.sessions, request.sessions.len() as i64);
        assert_eq!(response.accepted.usage, request.usage.len() as i64);
        let distance = match SystemTime::now().duration_since(response.server_time) {
            Ok(d) => d,
            Err(e) => e.duration(),
        };
        assert!(distance < Duration::from_secs(24 * 3600));
    } else {
        assert_eq!(response.accepted.sessions, 2);
        assert_eq!(response.accepted.usage, 2);
        assert_eq!(
            Some(response.server_time),
            date::parse("2026-09-25T11:21:00Z")
        );
    }

    // The service takes the same answer as success: what it sent is marked sent.
    let h = Harness::new();
    h.handles.http.set_handler(move |request| {
        if path_of(request) == "/api/app/v1/sync" {
            return Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: answer.clone(),
            });
        }
        Ok(website_answer(request))
    });
    h.start();
    h.service.record_usage(h.usage());
    h.sync_now();
    assert_eq!(h.sync_requests().len(), 1);
    let state = h.service.state();
    assert_eq!(state.last_error, None);
    assert!(state.last_sync_at_ms.is_some());
    assert_eq!(state.pending_usage, 0);
}

// ---- The contract's rules ----

/// The contract's rules (`web/contract/README.md`) over a sync request as
/// JSON, checked on the encoded bytes rather than on the Rust values: every
/// field there and no other, explicit nulls, types, limits, key and id
/// formats, dates as ISO 8601 UTC with a `Z` from 2023-01-01 to a day after
/// `now` (a window's reset time to 32 days after), and every session and
/// reading naming a listed account. Pure.
fn violations(json: &Value, now: SystemTime) -> Vec<String> {
    let mut check = Checker::new(now);
    check.request(json);
    check.problems
}

const SESSION_SOURCES: [&str; 5] = ["cli", "vscode", "desktop", "sdk", "other"];
const USAGE_SOURCES: [&str; 4] = ["probe", "statusLine", "claudeJson", "desktop"];

struct Checker {
    now: SystemTime,
    problems: Vec<String>,
    date: Regex,
    key: Regex,
    uuid: Regex,
    window: Regex,
}

impl Checker {
    fn new(now: SystemTime) -> Checker {
        let pattern = |text: &str| Regex::new(text).expect("a valid pattern");
        Checker {
            now,
            problems: Vec::new(),
            date: pattern(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?Z$"),
            key: pattern("^[0-9a-f]{64}$"),
            uuid: pattern(
                "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
            ),
            window: pattern("^(session|weekly_all|extra_usage|weekly_[a-z0-9._-]{1,60})$"),
        }
    }

    fn fail(&mut self, path: &str, message: &str) {
        self.problems.push(format!("{path}: {message}"));
    }

    fn request(&mut self, json: &Value) {
        let Some(root) = self.object(
            Some(json),
            "$",
            &["schemaVersion", "device", "accounts", "sessions", "usage"],
            &[],
        ) else {
            return;
        };
        if let Some(version) = self.number(
            root.get("schemaVersion"),
            "$.schemaVersion",
            true,
            1.0,
            false,
            false,
        ) {
            if version != 1.0 {
                self.fail("$.schemaVersion", "must be 1");
            }
        }
        if let Some(device) = self.object(
            root.get("device"),
            "$.device",
            &["id", "name", "appVersion"],
            &[],
        ) {
            let uuid = self.uuid.clone();
            self.string(device.get("id"), "$.device.id", 36, false, Some(&uuid));
            self.string(device.get("name"), "$.device.name", 120, false, None);
            self.string(
                device.get("appVersion"),
                "$.device.appVersion",
                40,
                false,
                None,
            );
        }
        let mut account_keys: Vec<String> = Vec::new();
        for (index, value) in self
            .array(root.get("accounts"), "$.accounts", 50)
            .iter()
            .enumerate()
        {
            let path = format!("$.accounts[{index}]");
            let Some(account) = self.object(
                Some(value),
                &path,
                &["key", "email", "organizationName", "plan", "label"],
                &[],
            ) else {
                continue;
            };
            let key = self.key.clone();
            if let Some(key) = self.string(
                account.get("key"),
                &format!("{path}.key"),
                64,
                false,
                Some(&key),
            ) {
                account_keys.push(key);
            }
            self.string(
                account.get("email"),
                &format!("{path}.email"),
                320,
                true,
                None,
            );
            self.string(
                account.get("organizationName"),
                &format!("{path}.organizationName"),
                200,
                true,
                None,
            );
            self.string(account.get("plan"), &format!("{path}.plan"), 60, true, None);
            self.string(
                account.get("label"),
                &format!("{path}.label"),
                80,
                true,
                None,
            );
        }
        for (index, value) in self
            .array(root.get("sessions"), "$.sessions", 200)
            .iter()
            .enumerate()
        {
            self.session(value, &format!("$.sessions[{index}]"), &account_keys);
        }
        for (index, value) in self
            .array(root.get("usage"), "$.usage", 500)
            .iter()
            .enumerate()
        {
            self.reading(value, &format!("$.usage[{index}]"), &account_keys);
        }
    }

    fn session(&mut self, value: &Value, path: &str, accounts: &[String]) {
        let Some(session) = self.object(
            Some(value),
            path,
            &[
                "accountKey",
                "sessionId",
                "project",
                "title",
                "source",
                "models",
                "startedAt",
                "lastActivityAt",
                "endedAt",
                "messageCount",
                "tokens",
                "costUsd",
            ],
            &["summary"],
        ) else {
            return;
        };
        self.account_key(
            session.get("accountKey"),
            &format!("{path}.accountKey"),
            accounts,
        );
        let uuid = self.uuid.clone();
        self.string(
            session.get("sessionId"),
            &format!("{path}.sessionId"),
            36,
            false,
            Some(&uuid),
        );
        if let Some(project) = self.object(
            session.get("project"),
            &format!("{path}.project"),
            &["key", "name"],
            &[],
        ) {
            let key = self.key.clone();
            self.string(
                project.get("key"),
                &format!("{path}.project.key"),
                64,
                false,
                Some(&key),
            );
            self.string(
                project.get("name"),
                &format!("{path}.project.name"),
                120,
                false,
                None,
            );
        }
        self.string(
            session.get("title"),
            &format!("{path}.title"),
            200,
            true,
            None,
        );
        if let Some(source) = self.string(
            session.get("source"),
            &format!("{path}.source"),
            20,
            false,
            None,
        ) {
            if !SESSION_SOURCES.contains(&source.as_str()) {
                self.fail(
                    &format!("{path}.source"),
                    &format!("{source} isn't a session source"),
                );
            }
        }
        for (index, model) in self
            .array(session.get("models"), &format!("{path}.models"), 10)
            .iter()
            .enumerate()
        {
            self.string(
                Some(model),
                &format!("{path}.models[{index}]"),
                200,
                false,
                None,
            );
        }
        self.date(
            session.get("startedAt"),
            &format!("{path}.startedAt"),
            false,
            24 * 3600,
        );
        self.date(
            session.get("lastActivityAt"),
            &format!("{path}.lastActivityAt"),
            false,
            24 * 3600,
        );
        self.date(
            session.get("endedAt"),
            &format!("{path}.endedAt"),
            true,
            24 * 3600,
        );
        self.number(
            session.get("messageCount"),
            &format!("{path}.messageCount"),
            true,
            2_147_483_647.0,
            false,
            false,
        );
        if let Some(tokens) = self.object(
            session.get("tokens"),
            &format!("{path}.tokens"),
            &["input", "output", "cacheCreation", "cacheRead"],
            &[],
        ) {
            for field in ["input", "output", "cacheCreation", "cacheRead"] {
                self.number(
                    tokens.get(field),
                    &format!("{path}.tokens.{field}"),
                    true,
                    9_007_199_254_740_991.0,
                    false,
                    false,
                );
            }
        }
        self.number(
            session.get("costUsd"),
            &format!("{path}.costUsd"),
            false,
            100_000_000.0,
            true,
            true,
        );
        if session.contains_key("summary") {
            if let Some(summary) = self.object(
                session.get("summary"),
                &format!("{path}.summary"),
                &["text", "model", "generatedAt"],
                &[],
            ) {
                self.string(
                    summary.get("text"),
                    &format!("{path}.summary.text"),
                    2000,
                    false,
                    None,
                );
                self.string(
                    summary.get("model"),
                    &format!("{path}.summary.model"),
                    80,
                    false,
                    None,
                );
                self.date(
                    summary.get("generatedAt"),
                    &format!("{path}.summary.generatedAt"),
                    false,
                    24 * 3600,
                );
            }
        }
    }

    fn reading(&mut self, value: &Value, path: &str, accounts: &[String]) {
        let Some(reading) = self.object(
            Some(value),
            path,
            &["accountKey", "source", "observedAt", "windows"],
            &[],
        ) else {
            return;
        };
        self.account_key(
            reading.get("accountKey"),
            &format!("{path}.accountKey"),
            accounts,
        );
        if let Some(source) = self.string(
            reading.get("source"),
            &format!("{path}.source"),
            20,
            false,
            None,
        ) {
            if !USAGE_SOURCES.contains(&source.as_str()) {
                self.fail(
                    &format!("{path}.source"),
                    &format!("{source} isn't a usage source"),
                );
            }
        }
        self.date(
            reading.get("observedAt"),
            &format!("{path}.observedAt"),
            false,
            24 * 3600,
        );
        for (index, value) in self
            .array(reading.get("windows"), &format!("{path}.windows"), 20)
            .iter()
            .enumerate()
        {
            let window_path = format!("{path}.windows[{index}]");
            let Some(window) = self.object(
                Some(value),
                &window_path,
                &["id", "utilization", "resetsAt"],
                &[],
            ) else {
                continue;
            };
            let pattern = self.window.clone();
            self.string(
                window.get("id"),
                &format!("{window_path}.id"),
                67,
                false,
                Some(&pattern),
            );
            self.number(
                window.get("utilization"),
                &format!("{window_path}.utilization"),
                false,
                f64::MAX,
                false,
                false,
            );
            self.date(
                window.get("resetsAt"),
                &format!("{window_path}.resetsAt"),
                true,
                32 * 24 * 3600,
            );
        }
    }

    fn account_key(&mut self, value: Option<&Value>, path: &str, accounts: &[String]) {
        let key = self.key.clone();
        if let Some(found) = self.string(value, path, 64, false, Some(&key)) {
            if !accounts.contains(&found) {
                self.fail(path, "must appear in accounts[]");
            }
        }
    }

    /// Every required field present (an explicit null counts), and no other.
    fn object<'a>(
        &mut self,
        value: Option<&'a Value>,
        path: &str,
        required: &[&str],
        optional: &[&str],
    ) -> Option<&'a Map<String, Value>> {
        let Some(object) = value.and_then(Value::as_object) else {
            self.fail(path, "must be an object");
            return None;
        };
        let missing: Vec<&&str> = required
            .iter()
            .filter(|k| !object.contains_key(**k))
            .collect();
        if !missing.is_empty() {
            self.fail(path, &format!("missing {missing:?}"));
        }
        let extra: Vec<&String> = object
            .keys()
            .filter(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
            .collect();
        if !extra.is_empty() {
            self.fail(path, &format!("not in the contract: {extra:?}"));
        }
        Some(object)
    }

    fn array<'a>(&mut self, value: Option<&'a Value>, path: &str, max: usize) -> &'a [Value] {
        let Some(items) = value.and_then(Value::as_array) else {
            self.fail(path, "must be a list");
            return &[];
        };
        if items.len() > max {
            self.fail(path, &format!("more than {max} items"));
        }
        items
    }

    /// Lengths in UTF-16 units, as the website's schemas count them.
    fn string(
        &mut self,
        value: Option<&Value>,
        path: &str,
        max: usize,
        nullable: bool,
        pattern: Option<&Regex>,
    ) -> Option<String> {
        if matches!(value, Some(Value::Null)) {
            if !nullable {
                self.fail(path, "must not be null");
            }
            return None;
        }
        let Some(text) = value.and_then(Value::as_str) else {
            self.fail(path, "must be a string");
            return None;
        };
        if text.encode_utf16().count() > max {
            self.fail(path, &format!("longer than {max}"));
        }
        if let Some(pattern) = pattern {
            if !pattern.is_match(text) {
                self.fail(path, &format!("{text} doesn't match {}", pattern.as_str()));
            }
        }
        Some(text.to_owned())
    }

    /// A JSON number (never a boolean), finite and at least 0.
    fn number(
        &mut self,
        value: Option<&Value>,
        path: &str,
        integer: bool,
        max: f64,
        nullable: bool,
        exclusive: bool,
    ) -> Option<f64> {
        if matches!(value, Some(Value::Null)) {
            if !nullable {
                self.fail(path, "must not be null");
            }
            return None;
        }
        let Some(number) = value.and_then(Value::as_f64) else {
            self.fail(path, "must be a number");
            return None;
        };
        if !number.is_finite() || number < 0.0 {
            self.fail(path, "must be finite and not negative");
        }
        if integer && number.round() != number {
            self.fail(path, "must be a whole number");
        }
        if if exclusive {
            number >= max
        } else {
            number > max
        } {
            self.fail(path, "too large");
        }
        Some(number)
    }

    fn date(&mut self, value: Option<&Value>, path: &str, nullable: bool, ahead: u64) {
        let pattern = self.date.clone();
        let Some(text) = self.string(value, path, 40, nullable, Some(&pattern)) else {
            return;
        };
        let Some(parsed) = date::parse(&text) else {
            self.fail(path, &format!("{text} isn't a date"));
            return;
        };
        if parsed < contract::earliest_date() {
            self.fail(path, "before 2023-01-01");
        }
        if parsed > self.now + Duration::from_secs(ahead) {
            self.fail(path, "too far after the clock");
        }
    }
}
