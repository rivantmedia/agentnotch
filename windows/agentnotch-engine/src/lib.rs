//! All of Agent Notch's Claude Code logic for Windows: accounts and rings,
//! the session pipeline, usage, hook installation, answering prompts,
//! jumping to terminals, notifications and cloud sync, ported from the Mac's
//! `ClaudeControl` package (DESIGN-WIN §2).
//!
//! The engine names no OS API. Every platform service (processes, the hook
//! pipe, files with their security, terminals, toasts, HTTP, …) is a trait in
//! [`platform`], implemented for real by `agentnotch-win` and by fakes in
//! [`testkit`], so everything here builds and is tested the same on macOS,
//! Linux and Windows. Paths are handled as strings through
//! [`core::paths`], whose [`core::paths::PathStyle`] is explicit so both the
//! Windows and the POSIX rules are tested on every OS.
//!
//! The Tauri app talks to it only through [`hub::Hub`]: calls in
//! ([`hub::Call`]), events out ([`hub::HubEvent`]), snapshots for the pages
//! ([`model::ui`]).
//!
//! Module owners are listed in DESIGN-WIN §2.1; `runtime_types` and
//! `hub::api` fix the types that cross between them.

#![forbid(unsafe_code)]

pub mod accounts;
pub mod attention;
pub mod cloud;
pub mod control;
pub mod core;
pub mod geometry;
pub mod hooks;
pub mod hub;
pub mod ingress;
pub mod model;
pub mod persist;
pub mod platform;
pub mod review;
pub mod runtime_types;
pub mod sessions;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod usage;

pub use hub::{Call, CallError, DeepLinkOutcome, DoctorExtras, Hub, HubConfig, HubEvent};
