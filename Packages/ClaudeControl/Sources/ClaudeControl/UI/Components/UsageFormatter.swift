//
//  UsageFormatter.swift
//  ClaudeControl
//
//  Text for plan usage: percentages, reset countdowns, data freshness and
//  pace. Pure functions of their inputs (the clock, locale and time zone are
//  parameters) so they are unit-testable and safe to call from any actor.
//

import Foundation

nonisolated enum UsageFormatter {
    // MARK: - Percent

    /// Highest percentage shown; anything above reads as "999%".
    static let maxDisplayedPercent: Double = 999

    /// "23%". Rounded down so the UI never shows a limit as hit before it is;
    /// values over 100 are kept ("110%"). Non-finite input reads as "–".
    static func percent(_ utilization: Double) -> String {
        guard utilization.isFinite else { return "–" }
        let clamped = min(max(utilization, 0), maxDisplayedPercent)
        return "\(Int(clamped.rounded(.down)))%"
    }

    // MARK: - Durations and clock times

    /// Longest duration spelled out; longer (or non-finite) ones are capped
    /// here so the Int conversion can never trap on bad data.
    private static let maxDuration: TimeInterval = 10_000 * 86_400

    /// Compact countdown: "<1m", "45m", "2h 13m", "2h", "3d 5h", "3d".
    static func duration(_ interval: TimeInterval) -> String {
        guard !interval.isNaN else { return "<1m" }
        let total = Int(min(max(interval, 0), maxDuration))
        let days = total / 86_400
        let hours = (total % 86_400) / 3_600
        let minutes = (total % 3_600) / 60
        if days > 0 {
            return hours > 0 ? "\(days)d \(hours)h" : "\(days)d"
        }
        if hours > 0 {
            return minutes > 0 ? "\(hours)h \(minutes)m" : "\(hours)h"
        }
        return minutes > 0 ? "\(minutes)m" : "<1m"
    }

    /// Wall-clock time in the user's 12/24-hour style: "14:05" or "2:05 PM".
    static func clockTime(
        _ date: Date,
        locale: Locale = .autoupdatingCurrent,
        timeZone: TimeZone = .autoupdatingCurrent
    ) -> String {
        date.formatted(Date.FormatStyle(locale: locale, timeZone: timeZone).hour().minute())
    }

    /// Abbreviated weekday: "Tue".
    static func weekday(
        _ date: Date,
        locale: Locale = .autoupdatingCurrent,
        timeZone: TimeZone = .autoupdatingCurrent
    ) -> String {
        date.formatted(Date.FormatStyle(locale: locale, timeZone: timeZone).weekday(.abbreviated))
    }

    /// Weekday of a reset at least a day away: "Tue", or "next Tue" when it
    /// falls on today's weekday (a weekly window that just restarted), which
    /// would otherwise read as later today.
    private static func resetWeekday(
        _ date: Date,
        now: Date,
        locale: Locale,
        timeZone: TimeZone
    ) -> String {
        let day = weekday(date, locale: locale, timeZone: timeZone)
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        // Same weekday on a later date (not the same day stretched past 24h
        // by a daylight-saving change).
        let isLaterSameWeekday = calendar.component(.weekday, from: date) == calendar.component(.weekday, from: now)
            && !calendar.isDate(date, inSameDayAs: now)
        return isLaterSameWeekday ? "next \(day)" : day
    }

    // MARK: - Reset phrases

    /// Resets closer than this read as a countdown; later ones as a weekday
    /// ("next Tue" when that is today's weekday, a week out).
    static let countdownHorizon: TimeInterval = 24 * 60 * 60

    /// Reset wording for use inside a sentence: "resets in 47m" within a day,
    /// "resets Tue 09:00" beyond, "has reset" once passed, nil when unknown.
    static func resetPhrase(
        _ resetsAt: Date?,
        now: Date,
        locale: Locale = .autoupdatingCurrent,
        timeZone: TimeZone = .autoupdatingCurrent
    ) -> String? {
        guard let resetsAt else { return nil }
        let remaining = resetsAt.timeIntervalSince(now)
        guard remaining.isFinite else { return nil }
        if remaining <= 0 { return "has reset" }
        if remaining < countdownHorizon {
            return "resets in \(duration(remaining))"
        }
        return "resets \(resetWeekday(resetsAt, now: now, locale: locale, timeZone: timeZone)) \(clockTime(resetsAt, locale: locale, timeZone: timeZone))"
    }

    // MARK: - Freshness

    /// "just now", "1m ago", "3h ago", "2d ago". Future dates read as "just now".
    static func age(of date: Date, now: Date) -> String {
        let seconds = min(now.timeIntervalSince(date), maxDuration)
        if seconds.isNaN || seconds < 60 { return "just now" }
        if seconds < 3_600 { return "\(Int(seconds / 60))m ago" }
        if seconds < 86_400 { return "\(Int(seconds / 3_600))h ago" }
        return "\(Int(seconds / 86_400))d ago"
    }

    /// "Updated 1m ago".
    static func updated(_ date: Date, now: Date) -> String {
        "Updated \(age(of: date, now: now))"
    }

    // MARK: - Money

    /// Formats an extra-usage amount. Claude Code reports `monthly_limit` and
    /// `used_credits` in minor units of the currency (cents for USD), so the
    /// value is scaled by the currency's fraction digits first. Whole amounts
    /// drop their decimals ("$50"); others keep them ("$12.40").
    static func money(
        minorUnits: Double,
        currency: String?,
        locale: Locale = .autoupdatingCurrent
    ) -> String {
        let code = currency.flatMap { $0.isEmpty ? nil : $0.uppercased() } ?? "USD"
        let digits = fractionDigits(forCurrency: code)
        let major = minorUnits / pow(10, Double(digits))
        let isWhole = major.rounded() == major
        return major.formatted(
            .currency(code: code)
                .locale(locale)
                .precision(.fractionLength(isWhole ? 0 : digits))
        )
    }

    private static func fractionDigits(forCurrency code: String) -> Int {
        let formatter = NumberFormatter()
        formatter.numberStyle = .currency
        formatter.currencyCode = code
        return formatter.maximumFractionDigits
    }
}

// MARK: - Pace

/// How a window's usage compares with an even burn across the window.
nonisolated enum UsagePace: Equatable, Sendable {
    /// No reset time (or the window already reset), so elapsed time is unknown.
    case unknown
    /// At or over 100%.
    case limitReached
    /// Within `tolerance` points of an even burn.
    case onPace
    /// Used this many points less than the elapsed share of the window.
    case under(points: Double)
    /// Used this many points more than the elapsed share of the window.
    /// `exhaustsIn` is the projected time until 100% at the current rate,
    /// when that comes before the reset.
    case ahead(points: Double, exhaustsIn: TimeInterval?)

    /// Differences smaller than this many percentage points count as on pace.
    static let tolerance: Double = 5

    init(window: UsageWindow, now: Date = Date()) {
        let used = window.effectiveUtilization(now: now)
        // Garbage from a source (NaN, infinities) has no meaningful pace, and
        // the hint's Int conversion must never see it.
        guard used.isFinite else {
            self = used == .infinity ? .limitReached : .unknown
            return
        }
        if used >= 100 {
            self = .limitReached
            return
        }
        guard !window.hasReset(now: now),
              let resetsAt = window.resetsAt,
              let elapsed = window.elapsedFraction(now: now) else {
            self = .unknown
            return
        }
        let delta = used - elapsed * 100
        if abs(delta) < Self.tolerance {
            self = .onPace
        } else if delta < 0 {
            self = .under(points: -delta)
        } else {
            // Linear projection from the burn so far.
            let elapsedSeconds = elapsed * window.duration
            var exhaustsIn: TimeInterval?
            if elapsedSeconds >= 60, used > 0 {
                let perSecond = used / elapsedSeconds
                let untilFull = (100 - used) / perSecond
                if untilFull < resetsAt.timeIntervalSince(now) {
                    exhaustsIn = untilFull
                }
            }
            self = .ahead(points: delta, exhaustsIn: exhaustsIn)
        }
    }

    /// Ordering used to pick the most pressing window: higher is worse.
    var severity: Int {
        switch self {
        case .unknown: return -1
        case .under: return 0
        case .onPace: return 1
        case .ahead(_, let exhaustsIn): return exhaustsIn == nil ? 2 : 3
        case .limitReached: return 4
        }
    }
}

/// The one-line pace summary at the bottom of the usage detail card.
nonisolated struct UsagePaceHint: Equatable, Sendable {
    enum Tone: Equatable, Sendable {
        /// Plenty of room left.
        case calm
        /// Burning at about the rate the window allows.
        case good
        /// Burning faster than the window allows.
        case warning
        /// Limit reached.
        case critical
    }

    let tone: Tone
    let text: String

    /// Summarises the session and weekly windows by their most pressing pace.
    /// Nil when neither window has a known pace.
    static func make(
        session: UsageWindow?,
        weekly: UsageWindow?,
        now: Date,
        locale: Locale = .autoupdatingCurrent,
        timeZone: TimeZone = .autoupdatingCurrent
    ) -> UsagePaceHint? {
        let candidates: [(name: String, window: UsageWindow, pace: UsagePace)] = [
            ("Session", session),
            ("Weekly", weekly),
        ].compactMap { name, window in
            guard let window else { return nil }
            let pace = UsagePace(window: window, now: now)
            return pace == .unknown ? nil : (name, window, pace)
        }
        guard let worst = candidates.max(by: { $0.pace.severity < $1.pace.severity }) else {
            return nil
        }
        // Only name the window when there is another one to tell it apart from.
        let prefix = candidates.count > 1 ? "\(worst.name) " : ""

        switch worst.pace {
        case .limitReached:
            let reset = UsageFormatter.resetPhrase(worst.window.resetsAt, now: now, locale: locale, timeZone: timeZone)
                .map { " — \($0)" } ?? ""
            return UsagePaceHint(tone: .critical, text: "\(worst.name) limit reached\(reset)")
        case .ahead(let points, let exhaustsIn):
            let consequence = exhaustsIn == nil ? "" : " — may hit limit before reset"
            return UsagePaceHint(tone: .warning, text: "\(prefix)\(Int(points.rounded()))% ahead of pace\(consequence)")
        case .onPace:
            return UsagePaceHint(tone: .good, text: "On pace")
        case .under(let points):
            if candidates.count > 1 {
                return UsagePaceHint(tone: .calm, text: "Under pace in both windows")
            }
            return UsagePaceHint(tone: .calm, text: "\(Int(points.rounded()))% under pace")
        case .unknown:
            return nil
        }
    }
}
