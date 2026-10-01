//! Prices per response (the Mac's `ModelPricingTests`, all 8, and the shared
//! vector file the Swift suite reads too: the two tables cannot drift apart
//! without one of the suites failing).

use agentnotch_engine::cloud::pricing::{cost, dollars, short_id, FAST_RATES, RATES};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../Packages/ClaudeControl/Tests/ClaudeControlTests/Fixtures/model-pricing-vectors.json"
);

fn vectors() -> Vec<Value> {
    let text = std::fs::read_to_string(VECTORS).expect("the shared pricing vectors");
    let root: Value = serde_json::from_str(&text).expect("the vectors are JSON");
    root["vectors"].as_array().expect("a vectors list").clone()
}

#[test]
fn every_vector_prices_as_the_swift_table_does() {
    let all = vectors();
    assert!(all.len() >= RATES.len() + FAST_RATES.len());
    for v in &all {
        let name = v["name"].as_str().unwrap_or("?");
        let model = v["model"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: model"));
        let expected = match &v["expectedNanoUsd"] {
            Value::Null => None,
            n => Some(n.as_i64().unwrap_or_else(|| panic!("{name}: expected"))),
        };
        assert_eq!(cost(model, &v["usage"]), expected, "{name}");
    }
}

#[test]
fn every_table_entry_has_a_vector() {
    let names: Vec<String> = vectors()
        .iter()
        .filter_map(|v| v["name"].as_str().map(str::to_owned))
        .collect();
    for (id, _) in RATES {
        assert!(names.contains(&format!("standard {id}")), "standard {id}");
    }
    for (id, _) in FAST_RATES {
        assert!(names.contains(&format!("fast {id}")), "fast {id}");
    }
}

#[test]
fn the_vectors_cover_unknown_and_continued_ids() {
    let all = vectors();
    for model in [
        "claude-opus-4-10",
        "claude-opus-5-6",
        "gpt-5",
        "<synthetic>",
        "",
    ] {
        let vector = all
            .iter()
            .find(|v| v["model"] == model)
            .unwrap_or_else(|| panic!("a vector for {model:?}"));
        assert!(vector["expectedNanoUsd"].is_null(), "{model:?}");
    }
    assert!(all
        .iter()
        .any(|v| v["model"] == "us.anthropic.claude-opus-4-1-20250805-v1:0"));
}

#[test]
fn model_ids_as_each_provider_names_them() {
    let expected: BTreeMap<&str, Option<&str>> = BTreeMap::from([
        ("claude-opus-4-5-20251101", Some("claude-opus-4-5")),
        ("claude-opus-4-5", Some("claude-opus-4-5")),
        ("claude-opus-4-5@20251101", Some("claude-opus-4-5")),
        (
            "us.anthropic.claude-opus-4-1-20250805-v1:0",
            Some("claude-opus-4-1"),
        ),
        (
            "eu.anthropic.claude-sonnet-4-5-20250929-v1:0",
            Some("claude-sonnet-4-5"),
        ),
        ("claude-opus-4-20250514", Some("claude-opus-4-0")),
        ("claude-sonnet-4-20250514", Some("claude-sonnet-4-0")),
        ("claude-opus-4-0", Some("claude-opus-4-0")),
        ("claude-haiku-4-5-20251001", Some("claude-haiku-4-5")),
        ("claude-3-7-sonnet-20250219", Some("claude-3-7-sonnet")),
        ("claude-3-5-haiku-20241022", Some("claude-3-5-haiku")),
        ("claude-opus-5", Some("claude-opus-5")),
        ("claude-opus-5-5", Some("claude-opus-5-5")),
        ("Claude-Opus-5-5", Some("claude-opus-5-5")),
        ("claude-fable-5-1", Some("claude-fable-5-1")),
        ("claude-sonnet-5", Some("claude-sonnet-5")),
        // Newer models of a family are not their predecessors.
        ("claude-opus-5-6", None),
        ("claude-sonnet-5-1", None),
        ("claude-opus-4-10", None),
        // Older than Claude Code's catalog, not Claude, or no model.
        ("claude-3-opus-20240229", None),
        ("gpt-5", None),
        ("<synthetic>", None),
        ("", None),
    ]);
    for (model, short) in expected {
        assert_eq!(short_id(model), short, "{model}");
    }
    // Every model the table prices is found by its own name.
    for (id, _) in RATES {
        assert_eq!(short_id(id), Some(*id));
    }
}

#[test]
fn each_kind_of_token_at_its_price() {
    // Opus 4.5: $5 in, $25 out, $6.25 and $10 cache writes, $0.50 reads.
    let usage = json!({
        "input_tokens": 1000, "output_tokens": 2000, "cache_read_input_tokens": 10000,
        "cache_creation_input_tokens": 3000,
        "cache_creation": {"ephemeral_5m_input_tokens": 2000, "ephemeral_1h_input_tokens": 1000},
    });
    assert_eq!(cost("claude-opus-4-5-20251101", &usage), Some(82_500_000));
    // Kept in the US: 10% more; each web search a cent.
    let mut us = usage.clone();
    us["inference_geo"] = json!("us");
    assert_eq!(cost("claude-opus-4-5", &us), Some(90_750_000));
    us["server_tool_use"] = json!({"web_search_requests": 2});
    assert_eq!(cost("claude-opus-4-5", &us), Some(110_750_000));
    // Elsewhere, or unknown: list price.
    let mut elsewhere = usage.clone();
    elsewhere["inference_geo"] = json!("not_available");
    assert_eq!(cost("claude-opus-4-5", &elsewhere), Some(82_500_000));
    // No 1-hour breakdown: every write at the 5-minute price.
    assert_eq!(
        cost(
            "claude-opus-4-5",
            &json!({"cache_creation_input_tokens": 1_000_000})
        ),
        Some(6_250_000_000)
    );
    // A 1-hour count larger than the writes counts no more than them.
    assert_eq!(
        cost(
            "claude-opus-4-5",
            &json!({
                "cache_creation_input_tokens": 1000,
                "cache_creation": {"ephemeral_1h_input_tokens": 5000},
            })
        ),
        Some(10_000_000)
    );
}

#[test]
fn each_model_at_its_own_prices() {
    let million = json!({
        "input_tokens": 1_000_000, "output_tokens": 1_000_000, "cache_read_input_tokens": 1_000_000,
    });
    let prices: [(&str, f64); 11] = [
        ("claude-haiku-4-5-20251001", 1.0 + 5.0 + 0.1),
        ("claude-3-5-haiku-20241022", 0.8 + 4.0 + 0.08),
        ("claude-sonnet-4-5-20250929", 3.0 + 15.0 + 0.3),
        ("claude-sonnet-5", 2.0 + 10.0 + 0.2),
        ("claude-opus-4-1-20250805", 15.0 + 75.0 + 1.5),
        ("claude-opus-4-8", 5.0 + 25.0 + 0.5),
        ("claude-opus-5", 5.0 + 25.0 + 0.5),
        ("claude-opus-5-5", 4.0 + 20.0 + 0.2),
        ("claude-fable-5", 10.0 + 50.0 + 1.0),
        ("claude-fable-5-1", 10.0 + 50.0 + 0.25),
        ("claude-mythos-5-1", 10.0 + 50.0 + 0.25),
    ];
    for (model, price) in prices {
        assert_eq!(
            cost(model, &million),
            Some((price * 1_000_000_000.0).round() as i64),
            "{model}"
        );
    }
}

#[test]
fn fast_mode_has_its_own_prices() {
    let fast = json!({"input_tokens": 1000, "output_tokens": 1000, "speed": "fast"});
    assert_eq!(cost("claude-opus-5", &fast), Some(60_000_000));
    assert_eq!(cost("claude-opus-4-8", &fast), Some(60_000_000));
    assert_eq!(cost("claude-opus-5-5", &fast), Some(48_000_000));
    assert_eq!(cost("claude-opus-4-6", &fast), Some(180_000_000));
    // Standard speed, and a model without fast prices: the usual ones.
    assert_eq!(
        cost(
            "claude-opus-5",
            &json!({"input_tokens": 1000, "output_tokens": 1000})
        ),
        Some(30_000_000)
    );
    assert_eq!(cost("claude-sonnet-4-5", &fast), Some(18_000_000));
}

#[test]
fn the_advisors_calls_at_the_advisors_prices() {
    let advisor = json!({
        "type": "advisor_message", "model": "claude-opus-4-5-20251101", "input_tokens": 2000,
        "output_tokens": 300, "cache_read_input_tokens": 10000, "cache_creation_input_tokens": 1000,
        "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 1000},
    });
    // Other kinds of iteration are already in the response's own counts.
    let compaction =
        json!({"type": "compaction", "model": "claude-opus-4-5", "input_tokens": 99999});
    let message = json!({"type": "message", "input_tokens": 1000, "output_tokens": 500});
    let mut usage = json!({
        "input_tokens": 1000, "output_tokens": 500, "inference_geo": "us", "speed": "fast",
        "iterations": [message, advisor.clone(), compaction],
    });
    // Per million: Sonnet 4.5 1000x3 + 500x15 = 10500, the Opus 4.5 advisor 2000x5 + 300x25 +
    // 10000x0.5 + 1000x10 = 32500 (never at fast prices); both kept in the US.
    assert_eq!(cost("claude-sonnet-4-5", &usage), Some(47_300_000));
    // An advisor model with no known price: the response's cost is unknown.
    let mut unknown = advisor;
    unknown["model"] = json!("claude-opus-9");
    usage["iterations"] = json!([unknown]);
    assert_eq!(cost("claude-sonnet-4-5", &usage), None);
}

/// A damaged line's absurd counts leave the cost unknown instead of
/// overflowing the conversion.
#[test]
fn a_figure_no_response_reaches_is_unknown() {
    assert_eq!(
        cost(
            "claude-opus-4-5",
            &json!({"output_tokens": 1_000_000_000_000_000_i64})
        ),
        None
    );
    assert_eq!(
        cost(
            "claude-opus-4-5",
            &json!({"server_tool_use": {"web_search_requests": 1_000_000_000_000_i64}})
        ),
        None
    );
    // Beyond i64 (the Mac's Int(_:) would trap; the Rust parse reads a float).
    let huge: Value = serde_json::from_str(r#"{"output_tokens": 9000000000000000000}"#).unwrap();
    assert_eq!(cost("claude-opus-4-5", &huge), None);
}

#[test]
fn an_unknown_model_has_no_price() {
    let usage = json!({"input_tokens": 10, "output_tokens": 10});
    assert_eq!(cost("claude-opus-9", &usage), None);
    assert_eq!(cost("", &usage), None);
    // A known model with no tokens costs nothing (known), not unknown.
    assert_eq!(cost("claude-haiku-4-5", &json!({})), Some(0));
    // Nonsense counts count as none.
    assert_eq!(
        cost(
            "claude-haiku-4-5",
            &json!({"input_tokens": -5, "output_tokens": "x"})
        ),
        Some(0)
    );
}

#[test]
fn dollars_to_the_millionth_the_website_keeps() {
    assert_eq!(dollars(10_565_000), 0.010565);
    assert_eq!(dollars(1_234_567), 0.001235);
    assert_eq!(dollars(400), 0.0);
    assert_eq!(dollars(-1), 0.0);
}
