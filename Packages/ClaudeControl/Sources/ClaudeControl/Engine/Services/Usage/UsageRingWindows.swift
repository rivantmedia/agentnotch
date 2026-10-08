//
//  UsageRingWindows.swift
//  ClaudeControl
//
//  The engine's usage (`AccountUsage`) in the shape Codenotch draws, and
//  back: window ids that mirror Codenotch's `UsageResponse.limitWindows()`,
//  so the ring, the hover card and saved preferences treat a Claude ring
//  like upstream's own.
//
//  - `session`       the 5-hour window (`five_hour`, `limits[kind=session]`)
//  - `weekly_all`    the 7-day window across all models (`seven_day`)
//  - `weekly_<model>` a model-scoped 7-day window, e.g. `weekly_opus`,
//                    labelled with the model's display name
//  - `extra_usage`   pay-as-you-go credits, metered in money
//
//  Codenotch's order: session, then weekly_all, then the rest by id. Pure.
//

import Foundation

nonisolated enum UsageRingWindows {
    static let sessionID = "session"
    static let weeklyID = "weekly_all"
    static let extraUsageID = "extra_usage"
    static let scopedPrefix = "weekly_"

    // MARK: - Engine → ring

    /// The windows of `usage` as the ring reads them. A window whose reset
    /// time has passed reads 0% (it restarted); money is in major units.
    static func windows(from usage: AccountUsage, now: Date) -> [ClaudeRingReading.Window] {
        var windows: [ClaudeRingReading.Window] = []
        if let five = usage.fiveHour {
            windows.append(window(id: sessionID, label: nil, five, now: now))
        }
        if let seven = usage.sevenDay {
            windows.append(window(id: weeklyID, label: nil, seven, now: now))
        }
        var seen: Set<String> = [sessionID, weeklyID]
        for scoped in usage.scoped {
            let id = scopedID(forModel: scoped.name)
            guard seen.insert(id).inserted else { continue }
            windows.append(window(id: id, label: scoped.name, scoped.window, now: now))
        }
        if let extra = extraUsageWindow(usage.extraUsage) {
            windows.append(extra)
        }
        return windows.sorted(by: displayOrder)
    }

    private static func window(id: String, label: String?, _ window: UsageWindow, now: Date) -> ClaudeRingReading.Window {
        ClaudeRingReading.Window(
            id: id,
            label: label,
            usedFraction: window.effectiveUtilization(now: now) / 100,
            resetsAt: window.resetsAt,
            duration: window.duration
        )
    }

    /// `next` with each window's reset time as `previous` had it, when the
    /// two are the same window. Usage sources disagree on a reset time by
    /// rounding (the status line's whole epoch seconds against the usage
    /// endpoint's fractional ISO dates), and whichever reading wins decides
    /// the time; a reset time that moved later by a fraction of a second
    /// reads to Codenotch's limit watcher as a new window, and it announced
    /// "limit reached" again each time. A real new window resets at least a
    /// quarter of its length later (`UsageStore.isSameWindow`). Pure.
    static func keepingResetTimes(_ next: ClaudeRingReading, previous: ClaudeRingReading?) -> ClaudeRingReading {
        guard let previous else { return next }
        var reading = next
        for index in reading.windows.indices {
            let window = reading.windows[index]
            guard let resetsAt = window.resetsAt,
                  let before = previous.windows.first(where: { $0.id == window.id }),
                  let kept = before.resetsAt, kept != resetsAt else { continue }
            let lengths = [window.duration, before.duration].compactMap { $0 }.filter { $0 > 0 }
            let tolerance = lengths.min().map { $0 / 4 } ?? sameWindowFallbackTolerance
            if abs(resetsAt.timeIntervalSince(kept)) < tolerance {
                reading.windows[index].resetsAt = kept
            }
        }
        return reading
    }

    /// For a window of unknown length: far more than rounding, far less than
    /// any window.
    static let sameWindowFallbackTolerance: TimeInterval = 60

    /// Extra usage when it is switched on and has a limit or spend to show.
    /// Claude Code reports credits in minor units (cents for USD).
    static func extraUsageWindow(_ extra: ExtraUsage?) -> ClaudeRingReading.Window? {
        guard let extra, extra.isEnabled else { return nil }
        let limit = extra.monthlyLimit
        let used = extra.usedCredits
        guard limit != nil || used != nil else { return nil }
        let currency = extra.currency.flatMap { $0.isEmpty ? nil : $0.uppercased() } ?? "USD"
        let scale = pow(10, Double(fractionDigits(forCurrency: currency)))
        let fraction: Double
        if let utilization = extra.utilization {
            fraction = utilization / 100
        } else if let limit, let used, limit > 0 {
            fraction = used / limit
        } else {
            fraction = 0
        }
        let spent = (used ?? 0) / scale
        let remaining = max(((limit ?? used ?? 0) - (used ?? 0)) / scale, 0)
        return ClaudeRingReading.Window(
            id: extraUsageID,
            label: "Extra usage",
            usedFraction: fraction,
            money: .init(currency: currency, spent: spent, remaining: remaining)
        )
    }

    /// `weekly_<slug>` for a model family: lowercased, anything but letters
    /// and digits folded to `_` ("Sonnet 4.5" → `weekly_sonnet_4_5`).
    static func scopedID(forModel name: String) -> String {
        var slug = ""
        var lastWasSeparator = false
        for scalar in name.lowercased().unicodeScalars {
            if CharacterSet.alphanumerics.contains(scalar), scalar.isASCII {
                slug.unicodeScalars.append(scalar)
                lastWasSeparator = false
            } else if !lastWasSeparator, !slug.isEmpty {
                slug.append("_")
                lastWasSeparator = true
            }
        }
        while slug.hasSuffix("_") { slug.removeLast() }
        return scopedPrefix + (slug.isEmpty ? "scoped" : slug)
    }

    /// Codenotch's display order: session, then weekly_all, then by id.
    static func displayOrder(_ lhs: ClaudeRingReading.Window, _ rhs: ClaudeRingReading.Window) -> Bool {
        func rank(_ id: String) -> Int {
            if id == sessionID { return 0 }
            if id == weeklyID { return 1 }
            return 2
        }
        let (left, right) = (rank(lhs.id), rank(rhs.id))
        return left == right ? lhs.id < rhs.id : left < right
    }

    /// The name Codenotch gives a window id when the reading carries none
    /// (`UsageResponse.label(forKind:)`), for the engine's own text.
    static func label(forID id: String) -> String {
        switch id {
        case sessionID: return "Current session"
        case weeklyID: return "All models"
        case "weekly_opus": return "Opus"
        case "weekly_sonnet": return "Sonnet"
        case "weekly_scoped", "scoped": return "Scoped"
        case extraUsageID: return "Extra usage"
        default:
            return id
                .replacingOccurrences(of: scopedPrefix, with: "")
                .replacingOccurrences(of: "_", with: " ")
                .capitalized
        }
    }

    // MARK: - Ring → engine

    /// A reading from another client (Claude Desktop's cache) as a full
    /// usage snapshot dated when that client saw it. Nil when it has no
    /// session or weekly window to show.
    static func accountUsage(from reading: ClaudeExternalUsageReading, accountId: String) -> AccountUsage? {
        var usage = AccountUsage(accountId: accountId, source: .cache, updatedAt: reading.observedAt)
        for window in reading.windows where window.usedFraction.isFinite {
            let utilization = window.usedFraction * 100
            switch window.id {
            case sessionID:
                usage.fiveHour = UsageWindow(utilization: utilization, resetsAt: window.resetsAt,
                                             duration: window.duration ?? UsageWindow.sessionDuration)
            case weeklyID:
                usage.sevenDay = UsageWindow(utilization: utilization, resetsAt: window.resetsAt,
                                             duration: window.duration ?? UsageWindow.weeklyDuration)
            case extraUsageID:
                guard let money = window.money else { continue }
                let scale = pow(10, Double(fractionDigits(forCurrency: money.currency)))
                usage.extraUsage = ExtraUsage(
                    isEnabled: true,
                    monthlyLimit: (money.spent + money.remaining) * scale,
                    usedCredits: money.spent * scale,
                    utilization: utilization,
                    currency: money.currency
                )
            case let id where id.hasPrefix(scopedPrefix):
                let name = window.label?.trimmingCharacters(in: .whitespaces).nilIfEmpty ?? label(forID: id)
                guard !usage.scoped.contains(where: { $0.name.caseInsensitiveCompare(name) == .orderedSame }) else { continue }
                usage.scoped.append(ScopedUsage(
                    name: name,
                    window: UsageWindow(utilization: utilization, resetsAt: window.resetsAt,
                                        duration: window.duration ?? UsageWindow.weeklyDuration)
                ))
            default:
                continue
            }
        }
        guard usage.fiveHour != nil || usage.sevenDay != nil else { return nil }
        return usage
    }

    // MARK: - Money

    /// Minor-unit digits of an ISO 4217 currency (2 for USD, 0 for JPY).
    static func fractionDigits(forCurrency code: String) -> Int {
        let formatter = NumberFormatter()
        formatter.numberStyle = .currency
        formatter.currencyCode = code
        return formatter.maximumFractionDigits
    }
}

private extension String {
    nonisolated var nilIfEmpty: String? { isEmpty ? nil : self }
}
