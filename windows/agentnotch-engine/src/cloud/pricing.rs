//! What a response cost at Anthropic's list prices, worked out the way
//! Claude Code works out its own `total_cost_usd` (the status line's cost,
//! `/cost`), for the sessions whose figure from Claude Code is missing or
//! can't be used: the VS Code extension's chat panel, Claude Desktop and the
//! SDK never run the status line, a session found only on disk was never
//! seen running, a session more than one account ran can't divide the one
//! it reported, and one continued where no status line runs outgrows it
//! (the Mac's `ModelPricing.swift`).
//!
//! Per response: input, output, cache reads and cache writes (the 1-hour
//! ones at their own price) per million tokens, 10% more when inference was
//! kept in the US (`inference_geo` "us"), a cent per web search, and the
//! advisor tool's calls at the advisor's model. Fast mode has its own
//! prices. Claude Code 2.1.282's, read from its bundle.
//!
//! The table exists twice, here and in Swift: one shared vector file
//! (`Packages/ClaudeControl/Tests/ClaudeControlTests/Fixtures/model-pricing-vectors.json`)
//! is read by both test suites, so the two cannot drift. A price change
//! bumps both scanner state versions and both payload versions together: a
//! response is priced when it is read, so only a rescan and a rebuild give
//! sessions already sent the new prices.
//!
//! A model the table doesn't know is priced at nothing rather than guessed:
//! its session's cost is then unknown. On a subscription the figure is what
//! the usage would have cost through the API, not what was paid.

use serde_json::Value;

/// A cost in billionths of a US dollar: a session's responses add up
/// exactly, and a response written again with new usage is taken back
/// exactly.
pub type NanoUsd = i64;

/// US dollars per million tokens, and per web search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
    pub web_search: f64,
}

const fn rates(
    input: f64,
    output: f64,
    cache_write_5m: f64,
    cache_write_1h: f64,
    cache_read: f64,
) -> Rates {
    Rates {
        input,
        output,
        cache_write_5m,
        cache_write_1h,
        cache_read,
        web_search: 0.01,
    }
}

// Claude Code's pricing tiers.
const TIER_2X10: Rates = rates(2.0, 10.0, 2.5, 4.0, 0.2);
const TIER_3X15: Rates = rates(3.0, 15.0, 3.75, 6.0, 0.3);
const TIER_4X20: Rates = rates(4.0, 20.0, 5.0, 8.0, 0.2);
const TIER_5X25: Rates = rates(5.0, 25.0, 6.25, 10.0, 0.5);
const TIER_10X50: Rates = rates(10.0, 50.0, 12.5, 20.0, 1.0);
const TIER_10X50_CHEAP_READS: Rates = rates(10.0, 50.0, 12.5, 20.0, 0.25);
const TIER_15X75: Rates = rates(15.0, 75.0, 18.75, 30.0, 1.5);
const HAIKU_35: Rates = rates(0.8, 4.0, 1.0, 1.6, 0.08);
const HAIKU_45: Rates = rates(1.0, 5.0, 1.25, 2.0, 0.1);
/// Fast mode on Opus 4.6 and 4.7.
const TIER_30X150: Rates = rates(30.0, 150.0, 37.5, 60.0, 3.0);
/// Fast mode on Opus 5.5.
const TIER_8X40: Rates = rates(8.0, 40.0, 10.0, 16.0, 0.4);

/// By the model's short id (Claude Code's canonical name).
pub const RATES: &[(&str, Rates)] = &[
    ("claude-3-5-haiku", HAIKU_35),
    ("claude-haiku-4-5", HAIKU_45),
    ("claude-3-5-sonnet", TIER_3X15),
    ("claude-3-7-sonnet", TIER_3X15),
    ("claude-sonnet-4-0", TIER_3X15),
    ("claude-sonnet-4-5", TIER_3X15),
    ("claude-sonnet-4-6", TIER_3X15),
    ("claude-sonnet-5", TIER_2X10),
    ("claude-opus-4-0", TIER_15X75),
    ("claude-opus-4-1", TIER_15X75),
    ("claude-opus-4-5", TIER_5X25),
    ("claude-opus-4-6", TIER_5X25),
    ("claude-opus-4-7", TIER_5X25),
    ("claude-opus-4-8", TIER_5X25),
    ("claude-opus-5", TIER_5X25),
    ("claude-opus-5-5", TIER_4X20),
    ("claude-fable-5", TIER_10X50),
    ("claude-fable-5-1", TIER_10X50_CHEAP_READS),
    ("claude-mythos-5", TIER_10X50),
    ("claude-mythos-5-1", TIER_10X50_CHEAP_READS),
];

/// Fast mode's prices (`usage.speed` "fast"); other models keep theirs.
pub const FAST_RATES: &[(&str, Rates)] = &[
    ("claude-opus-4-6", TIER_30X150),
    ("claude-opus-4-7", TIER_30X150),
    ("claude-opus-4-8", TIER_10X50),
    ("claude-opus-5", TIER_10X50),
    ("claude-opus-5-5", TIER_8X40),
];

fn lookup(table: &[(&str, Rates)], short_id: &str) -> Option<Rates> {
    table
        .iter()
        .find(|(id, _)| *id == short_id)
        .map(|(_, r)| *r)
}

/// The standard prices of a short id.
pub fn rates_for(short_id: &str) -> Option<Rates> {
    lookup(RATES, short_id)
}

/// The names a model id may carry, longest first (then alphabetically), and
/// the short id each stands for: "claude-opus-4" and "claude-sonnet-4"
/// followed by a date are the first Claude 4 models.
fn names() -> Vec<(&'static str, &'static str)> {
    let mut names: Vec<(&str, &str)> = RATES.iter().map(|(id, _)| (*id, *id)).collect();
    names.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(b.0)));
    names.push(("claude-opus-4", "claude-opus-4-0"));
    names.push(("claude-sonnet-4", "claude-sonnet-4-0"));
    names
}

/// The short id of a model as the API, Bedrock or Vertex name it
/// ("claude-sonnet-4-5-20250929", "us.anthropic.claude-opus-4-1-20250805-v1:0",
/// "claude-opus-4-5@20251101"); `None` for a model the table doesn't know. A
/// name whose version number goes on ("claude-opus-5" in "claude-opus-5-6",
/// "claude-opus-4-1" in "claude-opus-4-10") is another model, not this one: a
/// new model is never priced as its predecessor.
pub fn short_id(model: &str) -> Option<&'static str> {
    let id = model.to_lowercase();
    for (name, short) in names() {
        let mut from = 0;
        while let Some(found) = id[from..].find(name) {
            let end = from + found + name.len();
            if !continues_version(&id[end..]) {
                return Some(short);
            }
            from = end;
        }
    }
    None
}

/// What follows a name goes on with its version number: a digit, or "-6" /
/// "-10-…" (fewer than 8 digits: not a date like "-20250929").
pub fn continues_version(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) if c.is_ascii_digit() => true,
        Some('-') => {
            let digits = chars.take_while(char::is_ascii_digit).count();
            (1..=7).contains(&digits)
        }
        _ => false,
    }
}

/// A JSON number, or a number written as text; never a boolean, NaN or an
/// infinity (the Mac's `UsageParser.number`).
pub fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64().filter(|v| v.is_finite()),
        Value::String(s) => s
            .trim_matches(|c: char| c == ' ' || c == '\t')
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite()),
        _ => None,
    }
}

/// [`number`] as a whole number, truncated, within `i64`'s range (the Mac's
/// `JSONValue.int`: strictly below 2^63, which `Int(_:)` would trap on).
pub fn integer(value: Option<&Value>) -> Option<i64> {
    let v = number(value)?;
    (v >= i64::MIN as f64 && v < i64::MAX as f64).then_some(v as i64)
}

/// A token or request count; anything else counts as none.
pub fn count(value: Option<&Value>) -> f64 {
    integer(value).unwrap_or(0).max(0) as f64
}

/// What one response cost, from its `message.model` and `message.usage`;
/// `None` when a model's prices aren't known (the response's or its
/// advisor's), or for a figure no response reaches (the website's limit,
/// from a damaged line).
pub fn cost(model: &str, usage: &Value) -> Option<NanoUsd> {
    let short = short_id(model)?;
    let is_fast = usage.get("speed").and_then(Value::as_str) == Some("fast");
    let rates = (if is_fast {
        lookup(FAST_RATES, short)
    } else {
        None
    })
    .or_else(|| rates_for(short))?;
    let geo = usage.get("inference_geo");
    let mut dollars = token_dollars(usage, &rates, geo)
        + count(
            usage
                .get("server_tool_use")
                .and_then(|s| s.get("web_search_requests")),
        ) * rates.web_search;
    // The advisor tool's calls, at the advisor's standard prices and the
    // response's `inference_geo`, as Claude Code adds them.
    if let Some(iterations) = usage.get("iterations").and_then(Value::as_array) {
        for call in iterations
            .iter()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some("advisor_message"))
        {
            let advisor = call
                .get("model")
                .and_then(Value::as_str)
                .and_then(short_id)?;
            let advisor_rates = rates_for(advisor)?;
            dollars += token_dollars(call, &advisor_rates, geo);
        }
    }
    if !dollars.is_finite() || dollars < 0.0 || dollars >= super::contract::limit::COST_USD {
        return None;
    }
    Some((dollars * 1_000_000_000.0).round() as NanoUsd)
}

/// The tokens of a usage object in dollars.
pub fn token_dollars(usage: &Value, rates: &Rates, geo: Option<&Value>) -> f64 {
    let written = count(usage.get("cache_creation_input_tokens"));
    let written_for_1h = count(
        usage
            .get("cache_creation")
            .and_then(|c| c.get("ephemeral_1h_input_tokens")),
    )
    .min(written);
    let per_million = count(usage.get("input_tokens")) * rates.input
        + count(usage.get("output_tokens")) * rates.output
        + count(usage.get("cache_read_input_tokens")) * rates.cache_read
        + (written - written_for_1h) * rates.cache_write_5m
        + written_for_1h * rates.cache_write_1h;
    let dollars = per_million / 1_000_000.0;
    if geo.and_then(Value::as_str) == Some("us") {
        dollars * 1.1
    } else {
        dollars
    }
}

/// A cost in dollars, to the millionth the website keeps (so what is sent
/// is what it stores).
pub fn dollars(cost: NanoUsd) -> f64 {
    (cost.max(0) as f64 / 1000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_priced_model_is_found_by_its_own_name() {
        for (id, _) in RATES {
            assert_eq!(short_id(id), Some(*id));
        }
        assert_eq!(short_id("claude-opus-5-6"), None);
        assert_eq!(short_id("claude-opus-4-20250514"), Some("claude-opus-4-0"));
    }

    #[test]
    fn opus_four_five() {
        let usage = json!({
            "input_tokens": 1000, "output_tokens": 2000, "cache_read_input_tokens": 10000,
            "cache_creation_input_tokens": 3000,
            "cache_creation": {"ephemeral_5m_input_tokens": 2000, "ephemeral_1h_input_tokens": 1000},
        });
        assert_eq!(cost("claude-opus-4-5-20251101", &usage), Some(82_500_000));
        assert_eq!(dollars(10_565_000), 0.010565);
        assert_eq!(dollars(1_234_567), 0.001235);
    }
}
