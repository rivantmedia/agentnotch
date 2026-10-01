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
