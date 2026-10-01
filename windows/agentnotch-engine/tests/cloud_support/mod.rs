//! Shared by the cloud's integration tests (CL§10; the Mac's
//! `Cloud_TestSupport.swift`): the contract's fixtures, the ids, keys and
//! times the suites agree on, and a builder for the accounts the engine
//! hands the cloud. Nothing here reaches the network, the real home or
//! `claude`. Later sub-tasks add the fake website and the stand-in engine.
#![allow(dead_code)]

use agentnotch_engine::cloud::auth::AuthSession;
use agentnotch_engine::cloud::contract::{date, ConfigResponse, REDIRECT_URL};
use agentnotch_engine::model::{FolderKind, IdentityId};
use agentnotch_engine::platform::HttpResponse;
use agentnotch_engine::runtime_types::{CloudAccount, CloudFolder};
use agentnotch_engine::testkit::http::json_response;
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// `web/contract/fixtures`: the JSON both the app and the website test
/// against. Read, never copied.
pub fn contract_fixtures() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../web/contract/fixtures/"
    ))
}

pub fn contract_fixture(name: &str) -> Vec<u8> {
    let path = contract_fixtures().join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn contract_fixture_json(name: &str) -> Value {
    serde_json::from_slice(&contract_fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Session ids, account keys and times used across the cloud suites.
pub struct CloudFixture;

impl CloudFixture {
    pub const ACCOUNT_UUID: &'static str = "3f1f0a3e-8a7b-4c1d-9e2f-5a6b7c8d9e0f";
    pub const IDENTITY_ID: &'static str = "uuid:3f1f0a3e-8a7b-4c1d-9e2f-5a6b7c8d9e0f";
    pub const ACCOUNT_KEY: &'static str =
        "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
    /// keys.json's second account: one in an organization.
    pub const WORK_UUID: &'static str = "9d2c7b1a-0000-4e5f-8a9b-1c2d3e4f5a6b";
    pub const WORK_ORGANIZATION: &'static str = "7a6b5c4d-3e2f-4a1b-9c8d-0e1f2a3b4c5d";
    pub const WORK_IDENTITY_ID: &'static str = "uuid:9d2c7b1a-0000-4e5f-8a9b-1c2d3e4f5a6b";
    pub const WORK_ACCOUNT_KEY: &'static str =
        "d8485d82cdbceb2582311953e97b1666022b76dcbb98ef283fece56b1b5b8874";
    pub const SESSION_A: &'static str = "a1b2c3d4-e5f6-4789-8abc-def012345678";
    pub const SESSION_B: &'static str = "0f9e8d7c-6b5a-4493-8271-605f4e3d2c1b";
    pub const SESSION_C: &'static str = "11111111-2222-4333-8444-555555555555";

    /// The first account the engine hands the cloud (`me@example.com`).
    pub fn account() -> CloudAccount {
        CloudAccountBuilder::new(Self::IDENTITY_ID)
            .email("me@example.com")
            .plan("Max 20x")
            .label("Personal")
            .build()
    }

    /// The second: a Team account in an organization.
    pub fn work_account() -> CloudAccount {
        CloudAccountBuilder::new(Self::WORK_IDENTITY_ID)
            .email("me@company.com")
            .organization_name("Company")
            .plan("Team")
            .folder(CloudAccountBuilder::run_folder(
                "/Users/me/.claude-work",
                Some(Self::WORK_ORGANIZATION),
            ))
            .build()
    }

    /// keys.json's `installSecretHex`: the install secret its project keys
    /// were made with.
    pub fn install_secret() -> Vec<u8> {
        let fixture = contract_fixture_json("keys.json");
        let hex = fixture["installSecretHex"]
            .as_str()
            .expect("installSecretHex");
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// 2026-09-25T08:00:00Z.
    pub fn base() -> SystemTime {
        date::parse("2026-09-25T08:00:00Z").expect("base")
    }

    /// The moment `seconds` after [`Self::base`].
    pub fn at(seconds: f64) -> SystemTime {
        let delta = Duration::from_secs_f64(seconds.abs());
        if seconds >= 0.0 {
            Self::base() + delta
        } else {
            Self::base() - delta
        }
    }

    /// ISO 8601 for a moment `seconds` after 2026-09-25T08:00:00Z.
    pub fn stamp(seconds: f64) -> String {
        date::to_string(Self::at(seconds))
    }
}

/// A [`CloudAccount`] as the engine would hand it over.
pub struct CloudAccountBuilder(CloudAccount);

impl CloudAccountBuilder {
    pub fn new(identity_id: &str) -> Self {
        CloudAccountBuilder(CloudAccount {
            identity_id: IdentityId::from(identity_id),
            email: None,
            organization_name: None,
            plan: None,
            label: None,
            folders: Vec::new(),
        })
    }

    pub fn email(mut self, value: &str) -> Self {
        self.0.email = Some(value.to_owned());
        self
    }

    pub fn organization_name(mut self, value: &str) -> Self {
        self.0.organization_name = Some(value.to_owned());
        self
    }

    pub fn plan(mut self, value: &str) -> Self {
        self.0.plan = Some(value.to_owned());
        self
    }

    pub fn label(mut self, value: &str) -> Self {
        self.0.label = Some(value.to_owned());
        self
    }

    pub fn folder(mut self, folder: CloudFolder) -> Self {
        self.0.folders.push(folder);
        self
    }

    pub fn build(self) -> CloudAccount {
        self.0
    }

    /// A folder Claude Code runs in, signed in to `organization`.
    pub fn run_folder(config_dir: &str, organization: Option<&str>) -> CloudFolder {
        CloudFolder {
            config_dir: PathBuf::from(config_dir),
            config_dir_env: Some(config_dir.to_owned()),
            organization_uuid: organization.map(str::to_owned),
            corrected: false,
            kind: FolderKind::Run,
            is_default: false,
            mirrored_default: false,
        }
    }

    /// A mirrored folder whose organization is stale (never used).
    pub fn mirrored_folder(config_dir: &str, stale_organization: &str) -> CloudFolder {
        CloudFolder {
            corrected: true,
            ..Self::run_folder(config_dir, Some(stale_organization))
        }
    }
}

/// The website sign-in's stand-ins (the Mac's `CloudAuthTests` statics):
/// a made-up Supabase project, its config, sessions and token answers.
pub struct AuthFixture;

impl AuthFixture {
    pub const SUPABASE: &'static str = "https://abcdefghijklmnop.supabase.co";
    pub const PUBLISHABLE_KEY: &'static str = "sb_publishable_test";
    pub const WEBSITE: &'static str = "https://agentnotch.example.com";

    pub fn config() -> ConfigResponse {
        ConfigResponse {
            supabase_url: Self::SUPABASE.into(),
            supabase_publishable_key: Self::PUBLISHABLE_KEY.into(),
            redirect_url: REDIRECT_URL.into(),
            dashboard_url: format!("{}/dashboard", Self::WEBSITE),
        }
    }

    /// `access-1`/`refresh-1` for `user-1` (`me@example.com`) through
    /// [`Self::WEBSITE`], expiring `seconds` after `now`.
    pub fn session(seconds: i64, now: SystemTime) -> AuthSession {
        Self::session_with(seconds, now, "access-1", "refresh-1")
    }

    pub fn session_with(seconds: i64, now: SystemTime, access: &str, refresh: &str) -> AuthSession {
        let delta = Duration::from_secs(seconds.unsigned_abs());
        AuthSession {
            access_token: access.into(),
            refresh_token: refresh.into(),
            expires_at: if seconds >= 0 {
                now + delta
            } else {
                now - delta
            },
            user_id: Some("user-1".into()),
            email: Some("me@example.com".into()),
            supabase_url: Self::SUPABASE.into(),
            publishable_key: Self::PUBLISHABLE_KEY.into(),
            website_url: Self::WEBSITE.into(),
        }
    }

    /// Supabase's answer to a token request.
    pub fn token_answer(access: &str, refresh: &str) -> HttpResponse {
        json_response(
            200,
            serde_json::json!({
                "access_token": access,
                "refresh_token": refresh,
                "expires_in": 3600,
                "token_type": "bearer",
                "user": {"id": "user-1", "email": "me@example.com"},
            })
            .to_string(),
        )
    }
}

// ---- The ledger's stand-ins (the Mac's `SessionLedgerTests.observation`) ----

use agentnotch_engine::cloud::keys;
use agentnotch_engine::cloud::ledger::CloudLedgerAccount;
use agentnotch_engine::model::{
    AccountId, Attribution, BackgroundWait, LiveSessionObservation, Phase, SessionId, SessionState,
    SessionView,
};
use std::collections::BTreeSet;

/// A set of session ids.
pub fn ids(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

/// The contract key of an account the engine hands the cloud.
pub fn key_of(account: &CloudAccount) -> String {
    keys::account_key_of(account).expect("an account with an account UUID")
}

pub fn ledger_account(account: &CloudAccount) -> CloudLedgerAccount {
    CloudLedgerAccount::from_account(account)
}

/// A running session as the hub attributed it: `me` in `/Users/me/code/app`,
/// in VS Code, last active at the fixture's base. Times are seconds after
/// [`CloudFixture::base`].
pub struct Live(pub LiveSessionObservation);

pub fn live(session_id: &str) -> Live {
    live_for(session_id, &CloudFixture::account())
}

pub fn live_for(session_id: &str, account: &CloudAccount) -> Live {
    Live(LiveSessionObservation {
        session_id: session_id.to_owned(),
        identity_id: account.identity_id.clone(),
        account_key: key_of(account),
        cwd: "/Users/me/code/app".to_owned(),
        transcript_path: Some(format!(
            "/Users/me/.claude/projects/-Users-me-code-app/{session_id}.jsonl"
        )),
        config_dir: Some("/Users/me/.claude".to_owned()),
        entrypoint: Some("claude-vscode".to_owned()),
        started_at: CloudFixture::base(),
        last_activity_at: CloudFixture::base(),
        model: Some("claude-opus-4-5".to_owned()),
        cost_usd: None,
        title: None,
        process_started_at: None,
    })
}

impl Live {
    pub fn cwd(mut self, cwd: &str) -> Self {
        self.0.cwd = cwd.to_owned();
        self
    }

    pub fn entrypoint(mut self, entrypoint: Option<&str>) -> Self {
        self.0.entrypoint = entrypoint.map(str::to_owned);
        self
    }

    pub fn active(mut self, seconds: f64) -> Self {
        self.0.last_activity_at = CloudFixture::at(seconds);
        self
    }

    pub fn started(mut self, seconds: f64) -> Self {
        self.0.started_at = CloudFixture::at(seconds);
        self
    }

    pub fn process(mut self, seconds: f64) -> Self {
        self.0.process_started_at = Some(CloudFixture::at(seconds));
        self
    }

    pub fn cost(mut self, cost: f64) -> Self {
        self.0.cost_usd = Some(cost);
        self
    }

    pub fn title(mut self, title: &str) -> Self {
        self.0.title = Some(title.to_owned());
        self
    }

    pub fn build(self) -> LiveSessionObservation {
        self.0
    }
}

/// A session as the hub's projection hands it over: certain as the fixture's
/// account, in `/Users/me/code/app`, first seen at the fixture's base.
pub fn session_view(session_id: &str) -> SessionView {
    SessionView {
        id: SessionId::from(session_id),
        account: Some(AccountId::from("/Users/me/.claude")),
        ring: None,
        attribution: Attribution::Known(Some(IdentityId::from(CloudFixture::IDENTITY_ID))),
        attribution_since: CloudFixture::base(),
        cwd: PathBuf::from("/Users/me/code/app"),
        project_name: "app".to_owned(),
        title: "app".to_owned(),
        title_from_folder: true,
        state: SessionState::Working,
        phase: Phase::Processing,
        pid: Some(4242),
        pid_started: None,
        entrypoint: Some("cli".to_owned()),
        config_dir_env: None,
        host_session_id: None,
        registry_status: None,
        first_seen_at: CloudFixture::base(),
        model: None,
        context_pct: None,
        tasks: None,
        background: BackgroundWait::default(),
        last_activity: CloudFixture::base(),
        turn_started_at: None,
        completed_at: None,
        reviewed_at: None,
        last_assistant_message: None,
        pending: Vec::new(),
        cost_usd: None,
        transcript_path: None,
    }
}

// ---- Transcript lines (the Mac's `CloudTranscriptLines`) ----

/// JSON lines as Claude Code writes them in a transcript.
pub struct Lines;

static LINE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn line_uuid() -> String {
    let n = LINE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("00000000-0000-4000-8000-{n:012x}")
}

/// An assistant line under construction: `Lines::assistant(id, request,
/// session)`, then the usage and the time.
pub struct AssistantLine {
    id: String,
    request: String,
    session: String,
    model: String,
    input: i64,
    output: i64,
    cache_creation: i64,
    cache_read: i64,
    stamp: String,
    tool_use: bool,
    sidechain: bool,
    cwd: String,
}

impl AssistantLine {
    pub fn model(mut self, model: &str) -> Self {
        self.model = model.to_owned();
        self
    }

    pub fn usage(mut self, input: i64, output: i64) -> Self {
        self.input = input;
        self.output = output;
        self
    }

    pub fn cache(mut self, creation: i64, read: i64) -> Self {
        self.cache_creation = creation;
        self.cache_read = read;
        self
    }

    pub fn at(mut self, seconds: f64) -> Self {
        self.stamp = CloudFixture::stamp(seconds);
        self
    }

    pub fn tool_use(mut self) -> Self {
        self.tool_use = true;
        self
    }

    pub fn sidechain(mut self) -> Self {
        self.sidechain = true;
        self
    }

    pub fn line(self) -> String {
        let mut content = vec![serde_json::json!({"type": "text", "text": "Done."})];
        if self.tool_use {
            content.push(serde_json::json!({
                "type": "tool_use", "id": format!("toolu_{}", self.id), "name": "Bash",
                "input": {"command": "cat secrets.txt"}
            }));
        }
        serde_json::json!({
            "type": "assistant", "sessionId": self.session, "timestamp": self.stamp,
            "requestId": self.request, "cwd": self.cwd, "isSidechain": self.sidechain,
            "uuid": line_uuid(),
            "message": {
                "id": self.id, "role": "assistant", "model": self.model, "content": content,
                "usage": {
                    "input_tokens": self.input, "output_tokens": self.output,
                    "cache_creation_input_tokens": self.cache_creation,
                    "cache_read_input_tokens": self.cache_read
                }
            }
        })
        .to_string()
    }
}

impl Lines {
    pub fn user(text: &str, session: &str, seconds: f64) -> String {
        Self::user_in(text, session, seconds, "/Users/me/code/app", "cli")
    }

    pub fn user_in(text: &str, session: &str, seconds: f64, cwd: &str, entrypoint: &str) -> String {
        serde_json::json!({
            "type": "user", "sessionId": session, "timestamp": CloudFixture::stamp(seconds),
            "cwd": cwd, "entrypoint": entrypoint, "uuid": line_uuid(),
            "message": {"role": "user", "content": text}
        })
        .to_string()
    }

    pub fn assistant(id: &str, request: &str, session: &str) -> AssistantLine {
        AssistantLine {
            id: id.to_owned(),
            request: request.to_owned(),
            session: session.to_owned(),
            model: "claude-opus-4-5".to_owned(),
            input: 0,
            output: 0,
            cache_creation: 0,
            cache_read: 0,
            stamp: CloudFixture::stamp(0.0),
            tool_use: false,
            sidechain: false,
            cwd: "/Users/me/code/app".to_owned(),
        }
    }

    pub fn ai_title(title: &str, session: &str) -> String {
        serde_json::json!({"type": "ai-title", "sessionId": session, "aiTitle": title}).to_string()
    }

    /// `line` as `/branch` copies it into a fork: the fork's session id, and
    /// where it came from.
    pub fn forked(line: &str, into: &str, from: &str) -> String {
        let mut object: Value = serde_json::from_str(line).expect("a JSON line");
        let uuid = object
            .get("uuid")
            .cloned()
            .unwrap_or_else(|| Value::String(line_uuid()));
        object["sessionId"] = Value::String(into.to_owned());
        object["forkedFrom"] = serde_json::json!({"sessionId": from, "messageUuid": uuid});
        object.to_string()
    }

    /// Writes (or appends) the lines, making the folders.
    pub fn write(lines: &[String], path: &std::path::Path, append: bool) {
        use std::io::Write;
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("folders made");
        if append {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)
                .expect("opened");
            file.write_all(text.as_bytes()).expect("appended");
        } else {
            std::fs::write(path, text).expect("written");
        }
    }
}
