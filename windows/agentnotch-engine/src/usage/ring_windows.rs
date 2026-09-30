//! The engine's usage (`AccountUsage`) as the windows a ring draws, and back
//! (UsageRingWindows.swift, AU§8.5). The ids mirror upstream's, so the ring,
//! the hover card and saved preferences treat a Claude ring like upstream's
//! own:
//!
//! - `session`: the 5-hour window (`five_hour`, `limits[kind=session]`);
//! - `weekly_all`: the 7-day window across all models (`seven_day`);
//! - `weekly_<model>`: a model-scoped 7-day window (`weekly_opus`), labelled
//!   with the model's display name;
//! - `extra_usage`: pay-as-you-go credits, metered in money.
//!
//! Upstream's order: session, then weekly_all, then the rest by id. Pure.

use crate::model::{AccountUsage, DesktopWindow, ExtraUsage, IdentityId, UsageSource, UsageWindow};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::time::SystemTime;

pub const SESSION_ID: &str = "session";
pub const WEEKLY_ID: &str = "weekly_all";
pub const EXTRA_USAGE_ID: &str = "extra_usage";
pub const SCOPED_PREFIX: &str = "weekly_";

/// Money a window is metered in (extra usage), in major units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Money {
    pub currency: String,
    pub spent: f64,
    pub remaining: f64,
}

/// One window as a ring reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RingWindow {
    /// `session`, `weekly_all`, `weekly_<model>`, `extra_usage`.
    pub id: String,
    /// `None`: the caller uses the standard label for `id` ([`label_for_id`]).
    pub label: Option<String>,
    /// 0..=1 (can exceed 1).
    pub used_fraction: f64,
    pub resets_at: Option<SystemTime>,
    pub duration_s: Option<u64>,
    pub money: Option<Money>,
}

// MARK: - Engine → ring

/// The windows of `usage` as the ring reads them. A window whose reset time
/// has passed reads 0% (it restarted); money is in major units.
pub fn windows(usage: &AccountUsage, now: SystemTime) -> Vec<RingWindow> {
    let mut windows = Vec::new();
    if let Some(five) = &usage.five_hour {
        windows.push(window(SESSION_ID.to_owned(), None, five, now));
    }
    if let Some(seven) = &usage.seven_day {
        windows.push(window(WEEKLY_ID.to_owned(), None, seven, now));
    }
    let mut seen = vec![SESSION_ID.to_owned(), WEEKLY_ID.to_owned()];
    for (name, scoped) in &usage.scoped {
        let id = scoped_id(name);
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        windows.push(window(id, Some(name.clone()), scoped, now));
    }
    if let Some(extra) = extra_usage_window(usage.extra_usage.as_ref()) {
        windows.push(extra);
    }
    windows.sort_by(display_order);
    windows
}

fn window(id: String, label: Option<String>, window: &UsageWindow, now: SystemTime) -> RingWindow {
    RingWindow {
        id,
        label,
        used_fraction: window.effective_utilization(now) / 100.0,
        resets_at: window.resets_at,
        duration_s: Some(window.duration_s),
        money: None,
    }
}

/// Extra usage when it is switched on and has a limit or spend to show.
/// Claude Code reports credits in minor units (cents for USD).
pub fn extra_usage_window(extra: Option<&ExtraUsage>) -> Option<RingWindow> {
    let extra = extra.filter(|e| e.is_enabled)?;
    let limit = extra.monthly_limit;
    let used = extra.used_credits;
    if limit.is_none() && used.is_none() {
        return None;
    }
    let currency = extra
        .currency
        .as_deref()
        .filter(|c| !c.is_empty())
        .map_or_else(|| "USD".to_owned(), str::to_uppercase);
    let scale = 10f64.powi(fraction_digits(&currency) as i32);
    let fraction = match (extra.utilization, limit, used) {
        (Some(utilization), _, _) => utilization / 100.0,
        (None, Some(limit), Some(used)) if limit > 0.0 => used / limit,
        _ => 0.0,
    };
    let spent = used.unwrap_or(0.0) / scale;
    let remaining = ((limit.or(used).unwrap_or(0.0) - used.unwrap_or(0.0)) / scale).max(0.0);
    Some(RingWindow {
        id: EXTRA_USAGE_ID.to_owned(),
        label: Some("Extra usage".to_owned()),
        used_fraction: fraction,
        resets_at: None,
        duration_s: None,
        money: Some(Money {
            currency,
            spent,
            remaining,
        }),
    })
}

/// `weekly_<slug>` for a model family: lowercased, anything but ASCII
/// letters and digits folded to `_` ("Sonnet 4.5" → `weekly_sonnet_4_5`).
pub fn scoped_id(model: &str) -> String {
    let mut slug = String::new();
    let mut last_was_separator = false;
    for ch in model.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_was_separator = false;
        } else if !last_was_separator && !slug.is_empty() {
            slug.push('_');
            last_was_separator = true;
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }
    format!(
        "{SCOPED_PREFIX}{}",
        if slug.is_empty() { "scoped" } else { &slug }
    )
}

/// Upstream's display order: session, then weekly_all, then by id.
pub fn display_order(lhs: &RingWindow, rhs: &RingWindow) -> Ordering {
    order_of_ids(&lhs.id, &rhs.id)
}

/// [`display_order`] over the ids alone.
pub fn order_of_ids(lhs: &str, rhs: &str) -> Ordering {
    fn rank(id: &str) -> u8 {
        match id {
            SESSION_ID => 0,
            WEEKLY_ID => 1,
            _ => 2,
        }
    }
    rank(lhs).cmp(&rank(rhs)).then_with(|| lhs.cmp(rhs))
}

/// The name upstream gives a window id when the reading carries none, for
/// the engine's own text.
pub fn label_for_id(id: &str) -> String {
    match id {
        SESSION_ID => "Current session".to_owned(),
        WEEKLY_ID => "All models".to_owned(),
        "weekly_opus" => "Opus".to_owned(),
        "weekly_sonnet" => "Sonnet".to_owned(),
        "weekly_scoped" | "scoped" => "Scoped".to_owned(),
        EXTRA_USAGE_ID => "Extra usage".to_owned(),
        _ => capitalized(&id.replace(SCOPED_PREFIX, "").replace('_', " ")),
    }
}

/// Every word's first letter in upper case, the rest in lower case.
fn capitalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_word_start = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            at_word_start = true;
            out.push(ch);
        } else if at_word_start {
            out.extend(ch.to_uppercase());
            at_word_start = false;
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

// MARK: - Ring → engine

/// A reading from another client (Claude Desktop's cache) as a full usage
/// snapshot dated when that client saw it. `None` when it has no session or
/// weekly window to show.
pub fn account_usage_from_desktop(
    account_id: IdentityId,
    windows: &[DesktopWindow],
    observed_at: SystemTime,
) -> Option<AccountUsage> {
    let mut usage = AccountUsage::new(account_id, UsageSource::Desktop, observed_at);
    for window in windows.iter().filter(|w| w.utilization.is_finite()) {
        let duration = |default: u64| {
            if window.duration_s > 0 {
                window.duration_s
            } else {
                default
            }
        };
        match window.id.as_str() {
            SESSION_ID => {
                usage.five_hour = Some(UsageWindow::new(
                    window.utilization,
                    window.resets_at,
                    duration(UsageWindow::SESSION_DURATION_S),
                ));
            }
            WEEKLY_ID => {
                usage.seven_day = Some(UsageWindow::new(
                    window.utilization,
                    window.resets_at,
                    duration(UsageWindow::WEEKLY_DURATION_S),
                ));
            }
            id if id.starts_with(SCOPED_PREFIX) => {
                let name = window
                    .label
                    .as_deref()
                    .map(|label| label.trim_matches([' ', '\t']))
                    .filter(|label| !label.is_empty())
                    .map_or_else(|| label_for_id(id), str::to_owned);
                let lower = name.to_lowercase();
                if usage
                    .scoped
                    .iter()
                    .any(|(existing, _)| existing.to_lowercase() == lower)
                {
                    continue;
                }
                usage.scoped.push((
                    name,
                    UsageWindow::new(
                        window.utilization,
                        window.resets_at,
                        duration(UsageWindow::WEEKLY_DURATION_S),
                    ),
                ));
            }
            // Extra usage needs its money, which Claude Desktop's usage
            // windows never carry; anything else is a kind no ring draws.
            _ => {}
        }
    }
    (usage.five_hour.is_some() || usage.seven_day.is_some()).then_some(usage)
}

// MARK: - The usage history's windows

/// `session`, `weekly_all`, `extra_usage` or `weekly_<model>` as the website
/// accepts it: a lowercase letter or digit, then up to 56 of lowercase
/// letters, digits, `_`, `.` and `-` (the website refuses a whole batch over
/// one bad window id).
pub fn is_window_id(id: &str) -> bool {
    if id == SESSION_ID || id == WEEKLY_ID || id == EXTRA_USAGE_ID {
        return true;
    }
    let Some(rest) = id.strip_prefix(SCOPED_PREFIX) else {
        return false;
    };
    let is_alphanumeric = |b: u8| b.is_ascii_digit() || b.is_ascii_lowercase();
    let bytes = rest.as_bytes();
    match bytes.first() {
        Some(&first) if bytes.len() <= 57 && is_alphanumeric(first) => bytes[1..]
            .iter()
            .all(|&b| is_alphanumeric(b) || b == b'_' || b == b'.' || b == b'-'),
        _ => false,
    }
}

/// A window id as the website takes it: as is, or a `weekly_<model>`
/// shortened to fit. `None` for anything else.
pub fn contract_window_id(id: &str) -> Option<String> {
    let id = id.to_lowercase();
    if is_window_id(&id) {
        return Some(id);
    }
    let rest = id.strip_prefix(SCOPED_PREFIX)?;
    let shortened: String = SCOPED_PREFIX.chars().chain(rest.chars().take(57)).collect();
    is_window_id(&shortened).then_some(shortened)
}

/// The windows of a full snapshot as the usage history records them
/// (`UsageObservation.windows(from:)`, CL§8): utilization as read, not reset
/// to 0 for a window whose reset has passed since.
pub fn observation_windows(usage: &AccountUsage) -> Vec<(String, f64, Option<SystemTime>)> {
    let mut windows: Vec<(String, f64, Option<SystemTime>)> = Vec::new();
    let mut add = |id: &str, window: Option<&UsageWindow>| {
        let Some(window) = window.filter(|w| w.utilization.is_finite()) else {
            return;
        };
        if let Some(id) = contract_window_id(id) {
            windows.push((id, window.utilization.max(0.0), window.resets_at));
        }
    };
    add(SESSION_ID, usage.five_hour.as_ref());
    add(WEEKLY_ID, usage.seven_day.as_ref());
    let mut seen = vec![SESSION_ID.to_owned(), WEEKLY_ID.to_owned()];
    for (name, window) in &usage.scoped {
        let id = scoped_id(name);
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        add(&id, Some(window));
    }
    if let Some(extra) = usage.extra_usage.as_ref().filter(|e| e.is_enabled) {
        let utilization = match (extra.utilization, extra.monthly_limit, extra.used_credits) {
            (Some(value), _, _) => Some(value),
            (None, Some(limit), Some(used)) if limit > 0.0 => Some(used / limit * 100.0),
            _ => None,
        };
        if let Some(utilization) = utilization.filter(|u| u.is_finite()) {
            windows.push((EXTRA_USAGE_ID.to_owned(), utilization, None));
        }
    }
    windows
}

// MARK: - Money

/// Minor-unit digits of an ISO 4217 currency (2 for USD, 0 for JPY): the
/// currencies that differ from two digits, as CLDR lists them.
pub fn fraction_digits(currency: &str) -> u32 {
    const NONE: &[&str] = &[
        "ADP", "AFN", "ALL", "BIF", "BYR", "CLP", "DJF", "ESP", "GNF", "IQD", "IRR", "ISK", "ITL",
        "JPY", "KMF", "KPW", "KRW", "LAK", "LBP", "LUF", "MGA", "MGF", "MMK", "MRO", "PYG", "RSD",
        "RWF", "SLL", "SOS", "STD", "SYP", "TMM", "TRL", "UGX", "UYI", "VND", "VUV", "XAF", "XOF",
        "XPF", "YER", "ZMK", "ZWD",
    ];
    const THREE: &[&str] = &["BHD", "JOD", "KWD", "LYD", "OMR", "TND"];
    const FOUR: &[&str] = &["CLF", "UYW"];
    let code = currency.to_uppercase();
    if NONE.contains(&code.as_str()) {
        0
    } else if THREE.contains(&code.as_str()) {
        3
    } else if FOUR.contains(&code.as_str()) {
        4
    } else {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_ids_and_labels() {
        // A3_RingReadingTests.labelsForIdsMatchCodenotchWording
        assert_eq!(label_for_id("session"), "Current session");
        assert_eq!(label_for_id("weekly_all"), "All models");
        assert_eq!(label_for_id("weekly_opus"), "Opus");
        assert_eq!(label_for_id("weekly_scoped"), "Scoped");
        assert_eq!(label_for_id("weekly_fable"), "Fable");
        assert_eq!(label_for_id("weekly_sonnet_4_5"), "Sonnet 4 5");
        assert_eq!(scoped_id("Sonnet 4.5"), "weekly_sonnet_4_5");
        assert_eq!(scoped_id("  "), "weekly_scoped");
        assert_eq!(scoped_id("Opus (beta)!"), "weekly_opus_beta");
    }

    #[test]
    fn contract_ids() {
        assert!(
            is_window_id("session") && is_window_id("weekly_all") && is_window_id("extra_usage")
        );
        assert!(is_window_id("weekly_sonnet_4_5") && is_window_id("weekly_opus-4.1"));
        assert!(!is_window_id("weekly_") && !is_window_id("weekly__x") && !is_window_id("monthly"));
        assert!(!is_window_id("weekly_Opus"));
        assert_eq!(
            contract_window_id("weekly_Opus").as_deref(),
            Some("weekly_opus")
        );
        assert_eq!(contract_window_id("nonsense"), None);
        let long = format!("weekly_{}", "a".repeat(80));
        assert_eq!(contract_window_id(&long).map(|id| id.len()), Some(7 + 57));
    }

    #[test]
    fn currencies() {
        assert_eq!(fraction_digits("USD"), 2);
        assert_eq!(fraction_digits("jpy"), 0);
        assert_eq!(fraction_digits("KWD"), 3);
        assert_eq!(fraction_digits("???"), 2);
    }
}
