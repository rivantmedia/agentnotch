//! Control and OS integration (HS§6-9): answers to requests, typing replies
//! (message safety), jumping to terminals, "is the user looking at it",
//! banners, chimes, peek, auto-open and the panel's auto-close.
//!
//! Everything here is a pure rule over plain data. The hub gathers the data
//! (the session, its host and console from the `an-ui` lane, the panel the
//! glue reports, the settings) and carries the results out through the
//! platform services; `agentnotch-win` does the OS work and decides nothing.
//!
//! The functions re-exported below are the interface of DESIGN-WIN §3.4.
//! Where a rule needs more than those signatures carry, a fuller form sits
//! beside it in its module:
//!
//! | §3.4 | Fuller form | What it adds |
//! |---|---|---|
//! | [`message_safety`] | [`messaging::typing_check`] | the tools in flight, so a reply waits for a tool rather than the whole turn, and whether a refusal is only "busy" |
//! | [`focus_plan`] | [`focus::focus_plan_with`] | the workspace root an editor has open, and a running editor for an extension session |
//! | [`looking_at`] | [`looking::looking_at_console`] | the console's title, which tells one Windows Terminal tab from another |
//! | [`reactions`] | [`reactions::reactions_each`] | a context per transition, and the ring a session without an account shows on |
//! | [`auto_close_deadline`] | [`panel::AutoOpenWatch`] | that the user touched the panel, once it no longer shows |
//!
//! Owner: WP6.

pub mod answers;
pub mod focus;
pub mod hosts;
pub mod looking;
pub mod messaging;
pub mod notifications;
pub mod panel;
pub mod reactions;
pub mod text;

pub use answers::permission_response;
pub use focus::focus_plan;
pub use looking::looking_at;
pub use messaging::message_safety;
pub use notifications::toast_for;
pub use panel::auto_close_deadline;
pub use reactions::reactions;
