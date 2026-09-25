//
//  UsageParser.swift
//  ClaudeControl
//
//  Pure parsing of Claude Code's plan-usage payloads into UsageModels. Three
//  shapes reach the app, all read-only and none involving OAuth tokens:
//
//  - the `/api/oauth/usage` body, found in `.claude.json` →
//    `cachedUsageUtilization.utilization` and in the `get_usage` control
//    response → `rate_limits` (which adds `model_scoped`):
//      five_hour / seven_day            {utilization 0-100, resets_at ISO8601}
//      seven_day_opus / seven_day_sonnet (nullable, older per-model windows)
//      limits[]                         {kind session|weekly_all|weekly_scoped,
//                                        percent, resets_at, scope.model.display_name}
//      model_scoped[]                   {display_name, utilization, resets_at}
//      extra_usage                      {is_enabled, monthly_limit, used_credits,
//                                        utilization, currency}
//  - the `get_usage` response wrapper: subscription_type, rate_limits_available
//  - the status line's `rate_limits`: {five_hour|seven_day: {used_percentage,
//    resets_at (epoch seconds)}}
//
//  Everything is tolerant: numbers may arrive as Int, Double or String, dates
//  as ISO 8601 with or without fractional seconds (or epoch numbers), and any
//  window may be null.
//

import Foundation

/// A usage body parsed into model types, before it is tied to an account.
nonisolated struct ParsedUsage: Equatable, Sendable {
    var fiveHour: UsageWindow?
    var sevenDay: UsageWindow?
    var scoped: [ScopedUsage]
    var extraUsage: ExtraUsage?
    /// "max", "pro", ... when the payload says.
    var subscriptionType: String?
    /// The answer may be Claude Code's persisted fallback rather than a
    /// fresh fetch. When its own usage request fails (a 429, typically),
    /// Claude Code answers `get_usage` with the snapshot it saved last, up to
    /// an hour old, marked `seeded`, and strips `limits` from it on the way
    /// out; the answer carries no fetch time. So a `get_usage` body without a
    /// `limits` array can't be dated by the probe.
    var isPossiblySeeded: Bool

    init(
        fiveHour: UsageWindow? = nil,
        sevenDay: UsageWindow? = nil,
        scoped: [ScopedUsage] = [],
        extraUsage: ExtraUsage? = nil,
        subscriptionType: String? = nil,
        isPossiblySeeded: Bool = false
    ) {
        self.fiveHour = fiveHour
        self.sevenDay = sevenDay
        self.scoped = scoped
        self.extraUsage = extraUsage
        self.subscriptionType = subscriptionType
        self.isPossiblySeeded = isPossiblySeeded
    }

    /// At least one window (session, weekly or scoped) was present.
    var hasWindows: Bool {
        fiveHour != nil || sevenDay != nil || !scoped.isEmpty
    }

    /// The same readings of the same windows as `other` (plan, extra usage
    /// and where it came from aside): how a seeded probe answer is matched to
    /// the dated copy in `.claude.json`. Reset times may differ by rounding.
    func hasSameWindows(as other: ParsedUsage) -> Bool {
        func same(_ lhs: UsageWindow?, _ rhs: UsageWindow?) -> Bool {
            switch (lhs, rhs) {
            case (nil, nil):
                return true
            case let (lhs?, rhs?):
                guard abs(lhs.utilization - rhs.utilization) < 0.5 else { return false }
                switch (lhs.resetsAt, rhs.resetsAt) {
                case (nil, nil): return true
                case let (left?, right?): return abs(left.timeIntervalSince(right)) < 60
                default: return false
                }
            default:
                return false
            }
        }
        guard same(fiveHour, other.fiveHour), same(sevenDay, other.sevenDay) else { return false }
        let mine = Dictionary(scoped.map { ($0.name.lowercased(), $0.window) }, uniquingKeysWith: { first, _ in first })
        let theirs = Dictionary(other.scoped.map { ($0.name.lowercased(), $0.window) }, uniquingKeysWith: { first, _ in first })
        guard Set(mine.keys) == Set(theirs.keys) else { return false }
        return mine.allSatisfy { name, window in same(window, theirs[name]) }
    }

    func accountUsage(accountId: String, source: UsageSource, updatedAt: Date) -> AccountUsage {
        AccountUsage(
            accountId: accountId,
            fiveHour: fiveHour,
            sevenDay: sevenDay,
            scoped: scoped,
            extraUsage: extraUsage,
            subscriptionType: subscriptionType,
            source: source,
            updatedAt: updatedAt
        )
    }
}

/// `cachedUsageUtilization` from an account's `.claude.json`.
nonisolated struct CachedUsageSnapshot: Equatable, Sendable {
    /// When Claude Code fetched it (`fetchedAtMs`).
    var fetchedAt: Date
    /// The account the snapshot belongs to; compared with `oauthAccount.accountUuid`.
    var accountUuid: String?
    var usage: ParsedUsage
}

/// Outcome of a `get_usage` control response.
nonisolated enum GetUsageResult: Equatable, Sendable {
    case usage(ParsedUsage)
    /// Usage can't be had for this login (API key, not claude.ai, ...).
    case unavailable(String)
    /// Claude Code's usage request was rate limited (HTTP 429 or a
    /// `rate_limit_error` body); back off.
    case rateLimited
    /// The payload didn't have the expected shape.
    case malformed(String)
}

nonisolated enum UsageParser {

    // MARK: - Usage Body

    /// Parse an `/api/oauth/usage`-shaped body (cache or probe).
    static func parseUsageBody(_ body: [String: Any]) -> ParsedUsage {
        var result = ParsedUsage()
        let limits = (body["limits"] as? [Any] ?? []).compactMap { $0 as? [String: Any] }

        result.fiveHour = window(body["five_hour"], duration: UsageWindow.sessionDuration)
            ?? limitWindow(limits, kind: "session", duration: UsageWindow.sessionDuration)
        result.sevenDay = window(body["seven_day"], duration: UsageWindow.weeklyDuration)
            ?? limitWindow(limits, kind: "weekly_all", duration: UsageWindow.weeklyDuration)

        // Scoped weekly limits, most specific source first; later sources only
        // add model families not seen yet (compared case-insensitively).
        var scoped: [ScopedUsage] = []
        func add(_ name: String?, _ window: UsageWindow?) {
            guard let name = name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty,
                  let window else { return }
            guard !scoped.contains(where: { $0.name.caseInsensitiveCompare(name) == .orderedSame }) else { return }
            scoped.append(ScopedUsage(name: name, window: window))
        }

        for limit in limits where (limit["kind"] as? String) == "weekly_scoped" {
            let scope = limit["scope"] as? [String: Any]
            let model = scope?["model"] as? [String: Any]
            add(
                model?["display_name"] as? String,
                windowFrom(value: limit["percent"] ?? limit["utilization"], resetsAt: limit["resets_at"], duration: UsageWindow.weeklyDuration)
            )
        }
        for entry in (body["model_scoped"] as? [Any] ?? []).compactMap({ $0 as? [String: Any] }) {
            add(
                entry["display_name"] as? String,
                windowFrom(value: entry["utilization"] ?? entry["percent"], resetsAt: entry["resets_at"], duration: UsageWindow.weeklyDuration)
            )
        }
        add("Opus", window(body["seven_day_opus"], duration: UsageWindow.weeklyDuration))
        add("Sonnet", window(body["seven_day_sonnet"], duration: UsageWindow.weeklyDuration))
        result.scoped = scoped

        result.extraUsage = extraUsage(body["extra_usage"])
        if let subscription = body["subscription_type"] as? String, !subscription.isEmpty {
            result.subscriptionType = subscription
        }
        return result
    }

    /// `{utilization, resets_at}`; nil for null, a non-object, or no utilization.
    static func window(_ value: Any?, duration: TimeInterval) -> UsageWindow? {
        guard let object = value as? [String: Any] else { return nil }
        return windowFrom(value: object["utilization"], resetsAt: object["resets_at"], duration: duration)
    }

    private static func windowFrom(value: Any?, resetsAt: Any?, duration: TimeInterval) -> UsageWindow? {
        guard let utilization = number(value) else { return nil }
        return UsageWindow(utilization: utilization, resetsAt: parseDate(resetsAt), duration: duration)
    }

    private static func limitWindow(_ limits: [[String: Any]], kind: String, duration: TimeInterval) -> UsageWindow? {
        guard let limit = limits.first(where: { ($0["kind"] as? String) == kind }) else { return nil }
        return windowFrom(value: limit["percent"] ?? limit["utilization"], resetsAt: limit["resets_at"], duration: duration)
    }

    private static func extraUsage(_ value: Any?) -> ExtraUsage? {
        guard let object = value as? [String: Any] else { return nil }
        return ExtraUsage(
            isEnabled: bool(object["is_enabled"]) ?? false,
            monthlyLimit: number(object["monthly_limit"]),
            usedCredits: number(object["used_credits"]),
            utilization: number(object["utilization"]),
            currency: object["currency"] as? String
        )
    }

    // MARK: - get_usage

    /// Parse the inner `response` of a successful `get_usage` control response.
    static func parseGetUsageResponse(_ response: [String: Any]) -> GetUsageResult {
        let subscription = response["subscription_type"] as? String

        if bool(response["rate_limits_available"]) == false {
            return .unavailable("Usage limits aren't available for this login")
        }
        // Claude Code answers `rate_limits: null` (with `rate_limits_available:
        // true`) when its own usage fetch failed: a 429, a network error, an
        // expired login it couldn't refresh. It doesn't say which.
        guard let rateLimits = response["rate_limits"] as? [String: Any] else {
            return .malformed("Claude Code couldn't load usage right now")
        }
        if let error = rateLimits["error"] as? [String: Any] {
            if (error["type"] as? String) == "rate_limit_error" {
                return .rateLimited
            }
            return .malformed((error["message"] as? String) ?? "Usage request failed")
        }

        var usage = parseUsageBody(rateLimits)
        if let subscription, !subscription.isEmpty {
            usage.subscriptionType = subscription
        }
        usage.isPossiblySeeded = !(rateLimits["limits"] is [Any])
        guard usage.hasWindows else {
            return .malformed("No usage windows in the response")
        }
        return .usage(usage)
    }

    // MARK: - .claude.json Cache

    /// `cachedUsageUtilization` from a parsed `.claude.json`, if present.
    static func parseCachedUsage(fromGlobalConfig json: [String: Any]) -> CachedUsageSnapshot? {
        guard let cached = json["cachedUsageUtilization"] as? [String: Any],
              let utilization = cached["utilization"] as? [String: Any],
              let fetchedAtMs = number(cached["fetchedAtMs"]) else {
            return nil
        }
        let usage = parseUsageBody(utilization)
        guard usage.hasWindows else { return nil }
        return CachedUsageSnapshot(
            fetchedAt: Date(timeIntervalSince1970: fetchedAtMs / 1000),
            accountUuid: cached["accountUuid"] as? String,
            usage: usage
        )
    }

    // MARK: - Status Line

    /// The status line's `rate_limits` object → 5-hour and 7-day windows.
    static func parseStatusLineRateLimits(_ value: Any?) -> (fiveHour: UsageWindow?, sevenDay: UsageWindow?) {
        guard let rateLimits = value as? [String: Any] else { return (nil, nil) }
        return (
            statusLineWindow(rateLimits["five_hour"], duration: UsageWindow.sessionDuration),
            statusLineWindow(rateLimits["seven_day"], duration: UsageWindow.weeklyDuration)
        )
    }

    /// `{used_percentage 0-100, resets_at epoch seconds}`.
    static func statusLineWindow(_ value: Any?, duration: TimeInterval) -> UsageWindow? {
        guard let object = value as? [String: Any],
              let used = number(object["used_percentage"] ?? object["utilization"]) else { return nil }
        return UsageWindow(utilization: used, resetsAt: parseDate(object["resets_at"]), duration: duration)
    }

    // MARK: - Scalars

    /// A number from Int, Double, NSNumber or a numeric String. Booleans are
    /// not numbers here, and neither are NaN or infinities.
    static func number(_ value: Any?) -> Double? {
        switch value {
        case let number as NSNumber:
            // JSONSerialization hands booleans over as NSNumber too.
            if CFGetTypeID(number) == CFBooleanGetTypeID() { return nil }
            let double = number.doubleValue
            return double.isFinite ? double : nil
        case let string as String:
            guard let double = Double(string.trimmingCharacters(in: .whitespaces)), double.isFinite else { return nil }
            return double
        default:
            return nil
        }
    }

    static func bool(_ value: Any?) -> Bool? {
        switch value {
        case let number as NSNumber:
            return number.boolValue
        case let string as String:
            switch string.lowercased() {
            case "true", "1", "yes": return true
            case "false", "0", "no": return false
            default: return nil
            }
        default:
            return nil
        }
    }

    /// ISO 8601 (with or without fractional seconds, `Z` or an offset), or an
    /// epoch number in seconds or milliseconds.
    static func parseDate(_ value: Any?) -> Date? {
        if let seconds = number(value) {
            // Anything this large is milliseconds (1e12 s is the year 33658).
            return Date(timeIntervalSince1970: seconds > 1e12 ? seconds / 1000 : seconds)
        }
        guard let raw = (value as? String)?.trimmingCharacters(in: .whitespaces), !raw.isEmpty else { return nil }
        if let date = parseISO8601(raw) { return date }
        // Tolerate a timestamp that lost its zone designator: treat it as UTC.
        if !hasZoneDesignator(raw) {
            return parseISO8601(raw + "Z")
        }
        return nil
    }

    private static func parseISO8601(_ string: String) -> Date? {
        if let date = try? Date(string, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) {
            return date
        }
        return try? Date(string, strategy: Date.ISO8601FormatStyle())
    }

    private static func hasZoneDesignator(_ string: String) -> Bool {
        guard let timeStart = string.firstIndex(of: "T") else { return false }
        let time = string[timeStart...]
        return time.hasSuffix("Z") || time.contains("+") || time.dropFirst().contains("-")
    }
}
