//! What the hub tells the ledger (CL§6.1; the Mac's
//! `ClaudeControlHub+Cloud.swift` "Feeding the ledger"): pure helpers the hub
//! calls on every projection to turn its [`SessionView`]s into a
//! [`LiveBatch`], so the rules live beside the ledger that depends on them.
//!
//! - [`placement`]: a session attributed to an identity for certain is
//!   `Certain`. Otherwise only real uncertainty is `Unsure`: a session the
//!   hub simply hasn't placed yet waits, for at most [`PLACEMENT_GRACE`]
//!   after its attribution began. That is a folder the registry hasn't
//!   grouped yet (`Known(None)`), a session the hub itself calls `Waiting`,
//!   or one Claude Desktop hosts whose registry entry hasn't been read (no
//!   host session id, no registry status: its state was just made by a
//!   hook).
//! - [`observation`]: a running session as the ledger captures it. The title
//!   is the session's own title, never the prompt, and only when it was not
//!   derived from the folder name.
//! - [`live_batch`]: the three lists and every running id.

use super::contract::is_desktop_hosted;
use crate::model::{Attribution, IdentityId, LiveSessionObservation, SessionView, PLACEMENT_GRACE};
use crate::runtime_types::LiveBatch;
use std::collections::BTreeSet;
use std::time::SystemTime;

/// How the session ledger hears of a running session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Its account is known for certain.
    Certain,
    /// Its account can't be told: its new responses count for no one.
    Unsure,
    /// Not placed yet: neither counted nor paused.
    Waiting,
}

/// Where the session ledger puts a running session. Pure.
pub fn placement(view: &SessionView, now: SystemTime) -> Placement {
    if matches!(view.attribution, Attribution::Known(Some(_))) {
        return Placement::Certain;
    }
    // A clock that went back is still within the grace.
    let waited = now
        .duration_since(view.attribution_since)
        .unwrap_or_default();
    if waited >= PLACEMENT_GRACE {
        return Placement::Unsure;
    }
    match view.attribution {
        Attribution::Known(_) | Attribution::Waiting => Placement::Waiting,
        Attribution::Unsure(_)
            if is_desktop_hosted(view.entrypoint.as_deref())
                && view.host_session_id.is_none()
                && view.registry_status.is_none() =>
        {
            Placement::Waiting
        }
        Attribution::Unsure(_) => Placement::Unsure,
    }
}

/// A running session as the ledger captures it: its account (the identity and
/// the contract key of its own account UUID and organization), where it
/// started, its transcript, where it runs, its times, when its process
/// started, model, cost and title (never the prompt). `None` for a session
/// with no working directory. Pure.
pub fn observation(
    view: &SessionView,
    identity: &IdentityId,
    account_key: &str,
) -> Option<LiveSessionObservation> {
    let cwd = view.cwd.to_string_lossy().into_owned();
    if cwd.is_empty() {
        return None;
    }
    let title = if view.title_from_folder {
        None
    } else {
        Some(view.title.trim().to_owned()).filter(|text| !text.is_empty())
    };
    Some(LiveSessionObservation {
        session_id: view.id.as_str().to_owned(),
        identity_id: identity.clone(),
        account_key: account_key.to_owned(),
        cwd,
        transcript_path: view
            .transcript_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        config_dir: view.account.as_ref().map(|a| a.as_str().to_owned()),
        entrypoint: view.entrypoint.clone(),
        // When the app first saw it: one Claude Code process can outlive a
        // `/clear` into a new session, so its start isn't the session's.
        // The transcript's first line corrects this later.
        started_at: view.first_seen_at,
        last_activity_at: view.last_activity,
        model: view.model.clone(),
        cost_usd: view.cost_usd,
        title,
        process_started_at: view.pid_started,
    })
}

/// The batch the ledger is fed: sessions placed for certain with an account
/// the website may hear of (`account_key_of` knows the identity: not hidden or
/// forgotten, with an account UUID), those whose account can't be told now,
/// those not placed yet, and every running session.
pub fn live_batch(
    sessions: &[SessionView],
    now: SystemTime,
    account_key_of: &dyn Fn(&IdentityId) -> Option<String>,
) -> LiveBatch {
    let mut batch = LiveBatch {
        attributed: Vec::new(),
        unsure: BTreeSet::new(),
        waiting: BTreeSet::new(),
        live_ids: BTreeSet::new(),
        at: now,
    };
    for view in sessions {
        let id = view.id.as_str().to_owned();
        batch.live_ids.insert(id.clone());
        match placement(view, now) {
            Placement::Certain => {
                let Attribution::Known(Some(identity)) = &view.attribution else {
                    continue;
                };
                if let Some(observed) =
                    account_key_of(identity).and_then(|key| observation(view, identity, &key))
                {
                    batch.attributed.push(observed);
                }
            }
            Placement::Waiting => {
                batch.waiting.insert(id);
            }
            Placement::Unsure => {
                batch.unsure.insert(id);
            }
        }
    }
    batch
}
