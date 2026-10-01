//! Chime, peek and auto-open for a burst of attention transitions (HS§7,
//! UI§3.8): `attention::policy` decides, this turns its decisions into what
//! the hub carries out (`Sounds::play`, `HubEvent::Peek`, `HubEvent::Panel`).
//!
//! The panel never opens by itself over an open panel, over a full-screen
//! app, while a terminal is on screen or while the user is in that session's
//! terminal, and never at all unless the user chose an auto-open policy
//! (Windows defaults to Never, §4.10).

use super::notifications;
use super::panel::{self, REASON_AUTO};
use crate::attention::policy::{self, AutoOpenPolicy, Decision, PolicyContext, Sound};
use crate::model::{AttentionTransition, RingId};
use crate::platform::Chime;
use crate::runtime_types::{ReactionContext, Reactions};

/// What the policy weighs, from the hub's context.
pub fn policy_context(ctx: &ReactionContext) -> PolicyContext {
    PolicyContext {
        auto_open: AutoOpenPolicy::from_setting(&ctx.auto_open),
        chimes: ctx.sound,
        peeks: ctx.peek,
        // Only a session-precise yes counts; "can't tell" is not looking.
        terminal_focused: ctx.looking_at == Some(true),
        any_terminal_visible: ctx.any_terminal_visible,
        full_screen: ctx.full_screen,
        panel_open: ctx.panel.open,
        // A hidden notch shows no ring to unfold on.
        ring_shown: ctx.ring_shown && !ctx.notch_hidden,
    }
}

/// One transition's decisions under its context (`Burst::finish` takes them).
pub fn decide(tr: &AttentionTransition, ctx: &ReactionContext) -> Vec<Decision> {
    policy::decide(tr, &policy_context(ctx))
}

/// A closed burst's merged decisions as reactions. `default_ring` is where
/// a session whose account isn't known yet shows (its rows go there); with
/// none given, such a session peeks nothing.
pub fn carry_out(
    merged: &[Decision],
    peek_seconds: u32,
    default_ring: Option<&RingId>,
) -> Reactions {
    let mut reactions = Reactions::default();
    for decision in merged {
        match decision {
            Decision::Chime(Sound::NeedsInput) => reactions.chime = Some(Chime::Blocked),
            Decision::Chime(Sound::Finished) => reactions.chime = Some(Chime::Finished),
            Decision::AutoOpen { session, .. } => {
                reactions.open_panel = Some(panel::lands_on_list(session, REASON_AUTO));
            }
            Decision::Peek { ring, .. } => {
                reactions.peek = ring
                    .as_ref()
                    .or(default_ring)
                    .map(|ring| (ring.clone(), peek_seconds));
            }
        }
    }
    reactions
}

/// Chime, peek and auto-open for a burst whose transitions share one
/// context, plus the banners the burst's changes withdraw. `toasts` is left
/// empty: a banner's text needs each session's own [`ToastContext`]
/// (`toast_for`).
///
/// [`ToastContext`]: crate::runtime_types::ToastContext
pub fn reactions(burst: &[AttentionTransition], ctx: &ReactionContext) -> Reactions {
    let each: Vec<(&AttentionTransition, &ReactionContext)> =
        burst.iter().map(|tr| (tr, ctx)).collect();
    reactions_each(&each, None)
}

/// [`reactions`] for transitions that each have their own context (whether
/// the user is looking at a session, and whether its ring is shown, differ
/// per session). The peek lasts as long as the last context says.
pub fn reactions_each(
    burst: &[(&AttentionTransition, &ReactionContext)],
    default_ring: Option<&RingId>,
) -> Reactions {
    let decisions: Vec<Decision> = burst.iter().flat_map(|(tr, ctx)| decide(tr, ctx)).collect();
    let peek_seconds = burst.last().map_or(0, |(_, ctx)| ctx.peek_seconds);
    let mut reactions = carry_out(&policy::merge(&decisions), peek_seconds, default_ring);
    for (tr, _) in burst {
        for withdrawal in notifications::withdrawals(tr) {
            if !reactions.withdraw.contains(&withdrawal) {
                reactions.withdraw.push(withdrawal);
            }
        }
    }
    reactions
}
