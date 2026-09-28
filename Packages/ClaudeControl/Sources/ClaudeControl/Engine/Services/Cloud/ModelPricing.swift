//
//  ModelPricing.swift
//  ClaudeControl
//
//  What a response cost at Anthropic's list prices, worked out the way
//  Claude Code works out its own `total_cost_usd` (the status line's cost,
//  `/cost`), for the sessions whose figure from Claude Code is missing or
//  can't be used: the VS Code extension's chat panel, Claude Desktop and
//  the SDK never run the status line, a session found only on disk was
//  never seen running, a session more than one account ran can't divide
//  the one it reported, and one continued where no status line runs
//  outgrows it.
//
//  Per response: input, output, cache reads and cache writes (the 1-hour
//  ones at their own price) per million tokens, 10% more when inference was
//  kept in the US (`inference_geo` "us"), a cent per web search, and the
//  advisor tool's calls at the advisor's model. Fast mode has its own
//  prices. Claude Code 2.1.282's, read from its bundle: the standard ones
//  are its model catalog (`pricing_tiers` and each model's tier), the fast
//  ones are constants in its cost function's `speed === "fast"` branch,
//  outside the catalog. Update both from a newer Claude Code, and then bump
//  `SessionTokenScanner.State.currentVersion` and
//  `CloudSyncPass.payloadVersion`: a response is priced when it is read,
//  so only a rescan and a rebuild give sessions already sent the new
//  prices.
//
//  A model the table doesn't know is priced at nothing rather than guessed
//  (Claude Code would use its default model's prices): its session's cost
//  is then unknown. Prices an organisation sets for itself (managed
//  settings' `modelPricing`) can't be known here. On a subscription the
//  figure is what the usage would have cost through the API, not what was
//  paid.
//

import Foundation

/// A cost in billionths of a US dollar: a session's responses add up
/// exactly, and a response written again with new usage is taken back
/// exactly.
typealias NanoUSD = Int64

nonisolated enum ModelPricing {
    /// US dollars per million tokens, and per web search.
    nonisolated struct Rates: Equatable, Sendable {
        var input: Double
        var output: Double
        var cacheWrite5m: Double
        var cacheWrite1h: Double
        var cacheRead: Double
        var webSearch = 0.01
    }

    // Claude Code's pricing tiers.
    static let tier2x10 = Rates(input: 2, output: 10, cacheWrite5m: 2.5, cacheWrite1h: 4, cacheRead: 0.2)
    static let tier3x15 = Rates(input: 3, output: 15, cacheWrite5m: 3.75, cacheWrite1h: 6, cacheRead: 0.3)
    static let tier4x20 = Rates(input: 4, output: 20, cacheWrite5m: 5, cacheWrite1h: 8, cacheRead: 0.2)
    static let tier5x25 = Rates(input: 5, output: 25, cacheWrite5m: 6.25, cacheWrite1h: 10, cacheRead: 0.5)
    static let tier10x50 = Rates(input: 10, output: 50, cacheWrite5m: 12.5, cacheWrite1h: 20, cacheRead: 1)
    static let tier10x50CheapReads = Rates(input: 10, output: 50, cacheWrite5m: 12.5, cacheWrite1h: 20, cacheRead: 0.25)
    static let tier15x75 = Rates(input: 15, output: 75, cacheWrite5m: 18.75, cacheWrite1h: 30, cacheRead: 1.5)
    static let haiku35 = Rates(input: 0.8, output: 4, cacheWrite5m: 1, cacheWrite1h: 1.6, cacheRead: 0.08)
    static let haiku45 = Rates(input: 1, output: 5, cacheWrite5m: 1.25, cacheWrite1h: 2, cacheRead: 0.1)
    /// Fast mode on Opus 4.6 and 4.7.
    static let tier30x150 = Rates(input: 30, output: 150, cacheWrite5m: 37.5, cacheWrite1h: 60, cacheRead: 3)
    /// Fast mode on Opus 5.5.
    static let tier8x40 = Rates(input: 8, output: 40, cacheWrite5m: 10, cacheWrite1h: 16, cacheRead: 0.4)

    /// By the model's short id (Claude Code's canonical name).
    static let rates: [String: Rates] = [
        "claude-3-5-haiku": haiku35,
        "claude-haiku-4-5": haiku45,
        "claude-3-5-sonnet": tier3x15,
        "claude-3-7-sonnet": tier3x15,
        "claude-sonnet-4-0": tier3x15,
        "claude-sonnet-4-5": tier3x15,
        "claude-sonnet-4-6": tier3x15,
        "claude-sonnet-5": tier2x10,
        "claude-opus-4-0": tier15x75,
        "claude-opus-4-1": tier15x75,
        "claude-opus-4-5": tier5x25,
        "claude-opus-4-6": tier5x25,
        "claude-opus-4-7": tier5x25,
        "claude-opus-4-8": tier5x25,
        "claude-opus-5": tier5x25,
        "claude-opus-5-5": tier4x20,
        "claude-fable-5": tier10x50,
        "claude-fable-5-1": tier10x50CheapReads,
        "claude-mythos-5": tier10x50,
        "claude-mythos-5-1": tier10x50CheapReads,
    ]

    /// Fast mode's prices (`usage.speed` "fast"); other models keep theirs.
    static let fastRates: [String: Rates] = [
        "claude-opus-4-6": tier30x150,
        "claude-opus-4-7": tier30x150,
        "claude-opus-4-8": tier10x50,
        "claude-opus-5": tier10x50,
        "claude-opus-5-5": tier8x40,
    ]

    /// The names a model id may carry, and the short id each stands for:
    /// "claude-opus-4" and "claude-sonnet-4" followed by a date are the
    /// first Claude 4 models.
    static let names: [(name: String, shortId: String)] =
        rates.keys.sorted { $0.count != $1.count ? $0.count > $1.count : $0 < $1 }.map { ($0, $0) }
        + [("claude-opus-4", "claude-opus-4-0"), ("claude-sonnet-4", "claude-sonnet-4-0")]

    /// The short id of a model as the API, Bedrock or Vertex name it
    /// ("claude-sonnet-4-5-20250929", "us.anthropic.claude-opus-4-1-20250805-v1:0",
    /// "claude-opus-4-5@20251101"); nil for a model the table doesn't know.
    /// A name whose version number goes on ("claude-opus-5" in
    /// "claude-opus-5-6", "claude-opus-4-1" in "claude-opus-4-10") is
    /// another model, not this one: a new model is never priced as its
    /// predecessor. Pure.
    static func shortId(of model: String) -> String? {
        let id = model.lowercased()
        for (name, shortId) in names {
            var from = id.startIndex
            while let found = id.range(of: name, range: from..<id.endIndex) {
                if !continuesVersion(id[found.upperBound...]) { return shortId }
                from = found.upperBound
            }
        }
        return nil
    }

    /// What follows a name goes on with its version number: a digit, or
    /// "-6" / "-10-…" (fewer than 8 digits: not a date like "-20250929").
    /// Pure.
    static func continuesVersion(_ rest: Substring) -> Bool {
        func isDigit(_ character: Character) -> Bool { character.isASCII && character.isNumber }
        if let first = rest.first, isDigit(first) { return true }
        guard rest.first == "-" else { return false }
        return (1...7).contains(rest.dropFirst().prefix(while: isDigit).count)
    }

    /// What one response cost, from its `message.model` and
    /// `message.usage`; nil when a model's prices aren't known (the
    /// response's or its advisor's), or for a figure no response reaches
    /// (the website's limit, from a damaged line). Pure.
    static func cost(model: String, usage: [String: Any]) -> NanoUSD? {
        guard let shortId = shortId(of: model) else { return nil }
        let isFast = (usage["speed"] as? String) == "fast"
        guard let rates = (isFast ? fastRates[shortId] : nil) ?? rates[shortId] else { return nil }
        let geo = usage["inference_geo"]
        var dollars = tokenDollars(usage, at: rates, geo: geo)
            + count((usage["server_tool_use"] as? [String: Any])?["web_search_requests"]) * rates.webSearch
        // The advisor tool's calls, at the advisor's standard prices and
        // the response's `inference_geo`, as Claude Code adds them.
        for call in (usage["iterations"] as? [[String: Any]]) ?? [] where (call["type"] as? String) == "advisor_message" {
            guard let advisor = (call["model"] as? String).flatMap(shortId(of:)),
                  let advisorRates = Self.rates[advisor] else { return nil }
            dollars += tokenDollars(call, at: advisorRates, geo: geo)
        }
        guard dollars.isFinite, dollars >= 0, dollars < CloudContract.Limit.costUsd else { return nil }
        return NanoUSD((dollars * 1_000_000_000).rounded())
    }

    /// The tokens of a usage object in dollars. Pure.
    static func tokenDollars(_ usage: [String: Any], at rates: Rates, geo: Any?) -> Double {
        let written = count(usage["cache_creation_input_tokens"])
        let writtenFor1h = min(count((usage["cache_creation"] as? [String: Any])?["ephemeral_1h_input_tokens"]), written)
        let perMillion = count(usage["input_tokens"]) * rates.input
            + count(usage["output_tokens"]) * rates.output
            + count(usage["cache_read_input_tokens"]) * rates.cacheRead
            + (written - writtenFor1h) * rates.cacheWrite5m
            + writtenFor1h * rates.cacheWrite1h
        let dollars = perMillion / 1_000_000
        return (geo as? String) == "us" ? dollars * 1.1 : dollars
    }

    /// A token or request count; anything else counts as none. Pure.
    static func count(_ value: Any?) -> Double {
        Double(max(0, JSONValue.int(value) ?? 0))
    }

    /// A cost in dollars, to the millionth the website keeps (so what is
    /// sent is what it stores). Pure.
    static func dollars(_ cost: NanoUSD) -> Double {
        (Double(max(0, cost)) / 1000).rounded() / 1_000_000
    }
}
