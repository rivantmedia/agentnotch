//! Cloud sync (CL): the contract, sign-in with PKCE, the sync service, the
//! ledger, the token scanner, the backfill, folder logins, the usage outbox
//! and session summaries, on its own thread (`an-cloud`).
//!
//! Owner: WP8. WP0 stub: the §3.4 signatures; it never signs in or sends.

use crate::hub::DeepLinkOutcome;
use crate::model::{CloudAuthState, CloudState};
use crate::platform::Platform;
use crate::runtime_types::{CloudCall, CloudConfig, CloudDeps, LiveBatch, UsageObservation};
use std::sync::Arc;

pub struct CloudService;

impl CloudService {
    pub fn start(cfg: CloudConfig, deps: Arc<dyn CloudDeps>, platform: &Platform) -> CloudHandle {
        let _ = (deps, platform);
        CloudHandle { cfg }
    }
}

pub struct CloudHandle {
    cfg: CloudConfig,
}

impl CloudHandle {
    pub fn observe_live(&self, o: LiveBatch) {
        let _ = o;
    }

    pub fn record_usage(&self, o: UsageObservation) {
        let _ = o;
    }

    pub fn call(&self, c: CloudCall) -> Result<(), String> {
        let _ = c;
        Err("Cloud sync isn't in this build yet.".into())
    }

    pub fn deep_link(&self, url: &str) -> DeepLinkOutcome {
        let _ = url;
        DeepLinkOutcome::SignInIgnored("Cloud sync isn't in this build yet.".into())
    }

    pub fn state(&self) -> CloudState {
        CloudState {
            website_url: self.cfg.website.clone(),
            website_is_overridden: self.cfg.website_is_overridden,
            auth: CloudAuthState::SignedOut,
            ..CloudState::default()
        }
    }

    pub fn stop(&self) {}
}
