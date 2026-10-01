import Foundation
import Testing
@testable import ClaudeControl

/// A response's cost at list prices, worked out as Claude Code works out
/// its own.
struct ModelPricingTests {
    @Test func modelIdsAsEachProviderNamesThem() {
        let expected: [String: String?] = [
            "claude-opus-4-5-20251101": "claude-opus-4-5",
            "claude-opus-4-5": "claude-opus-4-5",
            "claude-opus-4-5@20251101": "claude-opus-4-5",
            "us.anthropic.claude-opus-4-1-20250805-v1:0": "claude-opus-4-1",
            "eu.anthropic.claude-sonnet-4-5-20250929-v1:0": "claude-sonnet-4-5",
            "claude-opus-4-20250514": "claude-opus-4-0",
            "claude-sonnet-4-20250514": "claude-sonnet-4-0",
            "claude-opus-4-0": "claude-opus-4-0",
            "claude-haiku-4-5-20251001": "claude-haiku-4-5",
            "claude-3-7-sonnet-20250219": "claude-3-7-sonnet",
            "claude-3-5-haiku-20241022": "claude-3-5-haiku",
            "claude-opus-5": "claude-opus-5",
            "claude-opus-5-5": "claude-opus-5-5",
            "Claude-Opus-5-5": "claude-opus-5-5",
            "claude-fable-5-1": "claude-fable-5-1",
            "claude-sonnet-5": "claude-sonnet-5",
            // Newer models of a family are not their predecessors.
            "claude-opus-5-6": nil,
            "claude-sonnet-5-1": nil,
            "claude-opus-4-10": nil,
            // Older than Claude Code's catalog, not Claude, or no model.
            "claude-3-opus-20240229": nil,
            "gpt-5": nil,
            "<synthetic>": nil,
            "": nil,
        ]
        for (model, shortId) in expected {
            #expect(ModelPricing.shortId(of: model) == shortId, "\(model)")
        }
        // Every model the table prices is found by its own name.
        for model in ModelPricing.rates.keys {
            #expect(ModelPricing.shortId(of: model) == model)
        }
    }

    @Test func eachKindOfTokenAtItsPrice() {
        // Opus 4.5: $5 in, $25 out, $6.25 and $10 cache writes, $0.50 reads.
        let usage: [String: Any] = [
            "input_tokens": 1000, "output_tokens": 2000, "cache_read_input_tokens": 10000,
            "cache_creation_input_tokens": 3000,
            "cache_creation": ["ephemeral_5m_input_tokens": 2000, "ephemeral_1h_input_tokens": 1000],
        ]
        #expect(ModelPricing.cost(model: "claude-opus-4-5-20251101", usage: usage) == 82_500_000)
        // Kept in the US: 10% more; each web search a cent.
        var us = usage
        us["inference_geo"] = "us"
        #expect(ModelPricing.cost(model: "claude-opus-4-5", usage: us) == 90_750_000)
        us["server_tool_use"] = ["web_search_requests": 2]
        #expect(ModelPricing.cost(model: "claude-opus-4-5", usage: us) == 110_750_000)
        // Elsewhere, or unknown: list price.
        var elsewhere = usage
        elsewhere["inference_geo"] = "not_available"
        #expect(ModelPricing.cost(model: "claude-opus-4-5", usage: elsewhere) == 82_500_000)
        // No 1-hour breakdown: every write at the 5-minute price.
        #expect(ModelPricing.cost(model: "claude-opus-4-5", usage: ["cache_creation_input_tokens": 1_000_000]) == 6_250_000_000)
        // A 1-hour count larger than the writes counts no more than them.
        #expect(ModelPricing.cost(model: "claude-opus-4-5", usage: [
            "cache_creation_input_tokens": 1000, "cache_creation": ["ephemeral_1h_input_tokens": 5000],
        ]) == 10_000_000)
    }

    @Test func eachModelAtItsOwnPrices() {
        let million: [String: Any] = ["input_tokens": 1_000_000, "output_tokens": 1_000_000,
                                      "cache_read_input_tokens": 1_000_000]
        let dollars: [String: Double] = [
            "claude-haiku-4-5-20251001": 1 + 5 + 0.1,
            "claude-3-5-haiku-20241022": 0.8 + 4 + 0.08,
            "claude-sonnet-4-5-20250929": 3 + 15 + 0.3,
            "claude-sonnet-5": 2 + 10 + 0.2,
            "claude-opus-4-1-20250805": 15 + 75 + 1.5,
            "claude-opus-4-8": 5 + 25 + 0.5,
            "claude-opus-5": 5 + 25 + 0.5,
            "claude-opus-5-5": 4 + 20 + 0.2,
            "claude-fable-5": 10 + 50 + 1,
            "claude-fable-5-1": 10 + 50 + 0.25,
            "claude-mythos-5-1": 10 + 50 + 0.25,
        ]
        for (model, price) in dollars {
            #expect(ModelPricing.cost(model: model, usage: million) == NanoUSD((price * 1_000_000_000).rounded()), "\(model)")
        }
    }

    @Test func fastModeHasItsOwnPrices() {
        let fast: [String: Any] = ["input_tokens": 1000, "output_tokens": 1000, "speed": "fast"]
        #expect(ModelPricing.cost(model: "claude-opus-5", usage: fast) == 60_000_000)
        #expect(ModelPricing.cost(model: "claude-opus-4-8", usage: fast) == 60_000_000)
        #expect(ModelPricing.cost(model: "claude-opus-5-5", usage: fast) == 48_000_000)
        #expect(ModelPricing.cost(model: "claude-opus-4-6", usage: fast) == 180_000_000)
        // Standard speed, and a model without fast prices: the usual ones.
        #expect(ModelPricing.cost(model: "claude-opus-5", usage: ["input_tokens": 1000, "output_tokens": 1000]) == 30_000_000)
        #expect(ModelPricing.cost(model: "claude-sonnet-4-5", usage: fast) == 18_000_000)
    }

    @Test func theAdvisorsCallsAtTheAdvisorsPrices() {
        let advisor: [String: Any] = [
            "type": "advisor_message", "model": "claude-opus-4-5-20251101", "input_tokens": 2000, "output_tokens": 300,
            "cache_read_input_tokens": 10000, "cache_creation_input_tokens": 1000,
            "cache_creation": ["ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 1000],
        ]
        // Other kinds of iteration are already in the response's own counts.
        let compaction: [String: Any] = ["type": "compaction", "model": "claude-opus-4-5", "input_tokens": 99999]
        let message: [String: Any] = ["type": "message", "input_tokens": 1000, "output_tokens": 500]
        var usage: [String: Any] = ["input_tokens": 1000, "output_tokens": 500, "inference_geo": "us", "speed": "fast",
                                    "iterations": [message, advisor, compaction]]
        // Per million: Sonnet 4.5 1000×3 + 500×15 = 10500, the Opus 4.5 advisor 2000×5 + 300×25 +
        // 10000×0.5 + 1000×10 = 32500 (never at fast prices); both kept in the US.
        #expect(ModelPricing.cost(model: "claude-sonnet-4-5", usage: usage) == 47_300_000)
        // An advisor model with no known price: the response's cost is unknown.
        var unknown = advisor
        unknown["model"] = "claude-opus-9"
        usage["iterations"] = [unknown]
        #expect(ModelPricing.cost(model: "claude-sonnet-4-5", usage: usage) == nil)
    }

    /// A damaged line's absurd counts leave the cost unknown instead of
    /// trapping on the conversion.
    @Test func aFigureNoResponseReachesIsUnknown() {
        #expect(ModelPricing.cost(model: "claude-opus-4-5", usage: ["output_tokens": 1_000_000_000_000_000]) == nil)
        #expect(ModelPricing.cost(model: "claude-opus-4-5", usage: ["server_tool_use": ["web_search_requests": 1_000_000_000_000]]) == nil)
        #expect(ModelPricing.cost(model: "claude-opus-4-5", usage: ["output_tokens": 9_000_000_000_000_000_000]) == nil)
    }

    @Test func anUnknownModelHasNoPrice() {
        let usage: [String: Any] = ["input_tokens": 10, "output_tokens": 10]
        #expect(ModelPricing.cost(model: "claude-opus-9", usage: usage) == nil)
        #expect(ModelPricing.cost(model: "", usage: usage) == nil)
        // A known model with no tokens costs nothing (known), not unknown.
        #expect(ModelPricing.cost(model: "claude-haiku-4-5", usage: [:]) == 0)
        // Nonsense counts count as none.
        #expect(ModelPricing.cost(model: "claude-haiku-4-5", usage: ["input_tokens": -5, "output_tokens": "x"]) == 0)
    }

    @Test func dollarsToTheMillionthTheWebsiteKeeps() {
        #expect(ModelPricing.dollars(10_565_000) == 0.010565)
        #expect(ModelPricing.dollars(1_234_567) == 0.001235)
        #expect(ModelPricing.dollars(400) == 0)
        #expect(ModelPricing.dollars(-1) == 0)
    }

    /// The vectors the Rust engine's `cloud_pricing` test reads too: the two
    /// tables cannot drift apart without one of the suites failing.
    @Test func sharedVectorsMatch() throws {
        let file = TestPaths.packageRoot
            .appendingPathComponent("Tests/ClaudeControlTests/Fixtures/model-pricing-vectors.json")
        let root = try #require(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        let vectors = try #require(root["vectors"] as? [[String: Any]])
        // One for each price in either table, and some for the rest.
        #expect(vectors.count >= ModelPricing.rates.count + ModelPricing.fastRates.count)
        for vector in vectors {
            let name = vector["name"] as? String ?? "?"
            let model = try #require(vector["model"] as? String, "\(name)")
            let usage = try #require(vector["usage"] as? [String: Any], "\(name)")
            let expected = (vector["expectedNanoUsd"] as? NSNumber)?.int64Value
            #expect(ModelPricing.cost(model: model, usage: usage) == expected, "\(name)")
        }
        for model in ModelPricing.rates.keys {
            #expect(vectors.contains { ($0["name"] as? String) == "standard \(model)" }, "\(model)")
        }
        for model in ModelPricing.fastRates.keys {
            #expect(vectors.contains { ($0["name"] as? String) == "fast \(model)" }, "\(model)")
        }
    }
}
