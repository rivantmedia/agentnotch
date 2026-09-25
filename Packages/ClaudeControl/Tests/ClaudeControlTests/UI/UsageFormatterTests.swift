import Foundation
import Testing
@testable import ClaudeControl

/// Fixed locale and zone so clock strings don't depend on the machine.
private let gb = Locale(identifier: "en_GB")
private let utc = TimeZone(identifier: "UTC")!

/// 2026-09-22 (a Tuesday) at the given UTC time.
private func tuesday(_ hour: Int, _ minute: Int) -> Date {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = utc
    return calendar.date(from: DateComponents(year: 2026, month: 9, day: 22, hour: hour, minute: minute))!
}

private let minute: TimeInterval = 60
private let hour: TimeInterval = 3_600
private let day: TimeInterval = 86_400

struct UsageFormatterTests {
    @Test func percentRoundsDownAndKeepsOverflow() {
        #expect(UsageFormatter.percent(0) == "0%")
        #expect(UsageFormatter.percent(23.7) == "23%")
        #expect(UsageFormatter.percent(99.99) == "99%")
        #expect(UsageFormatter.percent(110.2) == "110%")
        #expect(UsageFormatter.percent(-3) == "0%")
        #expect(UsageFormatter.percent(.nan) == "–")
    }

    @Test func durationIsCompact() {
        #expect(UsageFormatter.duration(0) == "<1m")
        #expect(UsageFormatter.duration(59) == "<1m")
        #expect(UsageFormatter.duration(-30) == "<1m")
        #expect(UsageFormatter.duration(minute) == "1m")
        #expect(UsageFormatter.duration(45 * minute) == "45m")
        #expect(UsageFormatter.duration(2 * hour + 13 * minute) == "2h 13m")
        #expect(UsageFormatter.duration(2 * hour) == "2h")
        #expect(UsageFormatter.duration(3 * day + 5 * hour) == "3d 5h")
        #expect(UsageFormatter.duration(3 * day + 30 * minute) == "3d")
    }

    @Test func clockTimesFollowLocale() {
        let date = tuesday(14, 5)
        #expect(UsageFormatter.clockTime(date, locale: gb, timeZone: utc) == "14:05")
        #expect(UsageFormatter.weekday(date, locale: gb, timeZone: utc) == "Tue")

        // 12-hour locales keep their AM/PM marker (the space before it varies by ICU version).
        let us = UsageFormatter.clockTime(date, locale: Locale(identifier: "en_US"), timeZone: utc)
        #expect(us.hasPrefix("2:05"))
        #expect(us.hasSuffix("PM"))
    }

    @Test func resetPhraseForSentences() {
        let now = tuesday(11, 52)
        #expect(UsageFormatter.resetPhrase(now.addingTimeInterval(47 * minute), now: now, locale: gb, timeZone: utc)
            == "resets in 47m")
        #expect(UsageFormatter.resetPhrase(now.addingTimeInterval(2 * day), now: now, locale: gb, timeZone: utc)
            == "resets Thu 11:52")
        #expect(UsageFormatter.resetPhrase(now.addingTimeInterval(7 * day - hour), now: now, locale: gb, timeZone: utc)
            == "resets next Tue 10:52")
        #expect(UsageFormatter.resetPhrase(now.addingTimeInterval(-5), now: now) == "has reset")
    }

    /// A 25-hour day (clocks going back) can put a reset more than 24h away
    /// on the same date; that is still "Sun", not "next Sun".
    @Test func sameDayPastTwentyFourHoursIsNotNextWeek() {
        let london = TimeZone(identifier: "Europe/London")!
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = london
        let now = calendar.date(from: DateComponents(year: 2026, month: 10, day: 25, hour: 0, minute: 10))!
        let reset = calendar.date(from: DateComponents(year: 2026, month: 10, day: 25, hour: 23, minute: 50))!
        #expect(reset.timeIntervalSince(now) > 24 * hour)
        #expect(UsageFormatter.resetPhrase(reset, now: now, locale: gb, timeZone: london) == "resets Sun 23:50")
    }

    /// Bad data from a source must never reach an Int conversion (a trap
    /// inside a view body takes the whole app down).
    @Test func survivesNonFiniteAndHugeValues() {
        let now = tuesday(12, 0)
        #expect(UsageFormatter.percent(.infinity) == "–")
        #expect(UsageFormatter.percent(1e300) == "999%")
        #expect(UsageFormatter.duration(.nan) == "<1m")
        #expect(UsageFormatter.duration(.infinity) == "10000d")
        #expect(UsageFormatter.duration(-.infinity) == "<1m")
        #expect(UsageFormatter.duration(1e300) == "10000d")
        #expect(UsageFormatter.age(of: Date(timeIntervalSinceReferenceDate: .nan), now: now) == "just now")
        #expect(UsageFormatter.age(of: Date(timeIntervalSinceReferenceDate: -1e300), now: now) == "10000d ago")
        #expect(UsageFormatter.resetPhrase(Date(timeIntervalSinceReferenceDate: .infinity), now: now) == nil)
        #expect(UsageFormatter.resetPhrase(Date(timeIntervalSinceReferenceDate: .nan), now: now) == nil)
    }

    @Test func freshness() {
        let now = tuesday(12, 0)
        #expect(UsageFormatter.age(of: now.addingTimeInterval(-30), now: now) == "just now")
        #expect(UsageFormatter.age(of: now.addingTimeInterval(30), now: now) == "just now")
        #expect(UsageFormatter.age(of: now.addingTimeInterval(-90), now: now) == "1m ago")
        #expect(UsageFormatter.age(of: now.addingTimeInterval(-3 * hour - 5 * minute), now: now) == "3h ago")
        #expect(UsageFormatter.age(of: now.addingTimeInterval(-2 * day), now: now) == "2d ago")
        #expect(UsageFormatter.updated(now.addingTimeInterval(-60), now: now) == "Updated 1m ago")
    }

    @Test func moneyScalesMinorUnits() {
        let us = Locale(identifier: "en_US")
        #expect(UsageFormatter.money(minorUnits: 1_240, currency: "USD", locale: us) == "$12.40")
        #expect(UsageFormatter.money(minorUnits: 5_000, currency: "usd", locale: us) == "$50")
        #expect(UsageFormatter.money(minorUnits: 5_000, currency: nil, locale: us) == "$50")
    }
}

struct UsagePaceTests {
    /// A 5-hour window halfway through.
    private func halfway(_ used: Double, now: Date) -> UsageWindow {
        UsageWindow(utilization: used, resetsAt: now.addingTimeInterval(2.5 * hour), duration: UsageWindow.sessionDuration)
    }

    @Test func classifiesAgainstAnEvenBurn() {
        let now = tuesday(12, 0)
        #expect(UsagePace(window: halfway(52, now: now), now: now) == .onPace)
        #expect(UsagePace(window: halfway(30, now: now), now: now) == .under(points: 20))
        #expect(UsagePace(window: halfway(100, now: now), now: now) == .limitReached)
        #expect(UsagePace(window: halfway(112, now: now), now: now) == .limitReached)

        guard case .ahead(let points, let exhaustsIn) = UsagePace(window: halfway(75, now: now), now: now) else {
            Issue.record("expected ahead of pace")
            return
        }
        #expect(abs(points - 25) < 0.001)
        // 75% in 2.5h → the remaining 25% takes 50 minutes at that rate.
        #expect(abs((exhaustsIn ?? 0) - 50 * minute) < 1)
    }

    @Test func nonFiniteUtilizationHasNoPace() {
        let now = tuesday(12, 0)
        #expect(UsagePace(window: halfway(.nan, now: now), now: now) == .unknown)
        #expect(UsagePace(window: halfway(-.infinity, now: now), now: now) == .unknown)
        #expect(UsagePace(window: halfway(.infinity, now: now), now: now) == .limitReached)
        // The hint falls back to the window that makes sense instead of trapping.
        let weekly = UsageWindow(utilization: 10, resetsAt: now.addingTimeInterval(3.5 * day), duration: UsageWindow.weeklyDuration)
        #expect(UsagePaceHint.make(session: halfway(.nan, now: now), weekly: weekly, now: now)
            == UsagePaceHint(tone: .calm, text: "40% under pace"))
    }

    @Test func unknownWithoutAResetTime() {
        let now = tuesday(12, 0)
        let noReset = UsageWindow(utilization: 40, resetsAt: nil, duration: UsageWindow.sessionDuration)
        #expect(UsagePace(window: noReset, now: now) == .unknown)
        let expired = UsageWindow(utilization: 80, resetsAt: now.addingTimeInterval(-60), duration: UsageWindow.sessionDuration)
        #expect(UsagePace(window: expired, now: now) == .unknown)
    }

    @Test func hintPicksTheMostPressingWindow() {
        let now = tuesday(12, 0)
        let weeklyUnder = UsageWindow(utilization: 10, resetsAt: now.addingTimeInterval(3.5 * day), duration: UsageWindow.weeklyDuration)

        let ahead = UsagePaceHint.make(session: halfway(62, now: now), weekly: weeklyUnder, now: now)
        #expect(ahead == UsagePaceHint(tone: .warning, text: "Session 12% ahead of pace — may hit limit before reset"))

        let calm = UsagePaceHint.make(session: halfway(20, now: now), weekly: weeklyUnder, now: now)
        #expect(calm == UsagePaceHint(tone: .calm, text: "Under pace in both windows"))

        let single = UsagePaceHint.make(session: halfway(20, now: now), weekly: nil, now: now)
        #expect(single == UsagePaceHint(tone: .calm, text: "30% under pace"))

        let onPace = UsagePaceHint.make(session: halfway(50, now: now), weekly: nil, now: now)
        #expect(onPace == UsagePaceHint(tone: .good, text: "On pace"))

        let limited = UsageWindow(utilization: 104, resetsAt: now.addingTimeInterval(47 * minute), duration: UsageWindow.sessionDuration)
        let critical = UsagePaceHint.make(session: limited, weekly: weeklyUnder, now: now)
        #expect(critical == UsagePaceHint(tone: .critical, text: "Session limit reached — resets in 47m"))

        #expect(UsagePaceHint.make(session: nil, weekly: nil, now: now) == nil)
        let unknown = UsageWindow(utilization: 40, resetsAt: nil, duration: UsageWindow.weeklyDuration)
        #expect(UsagePaceHint.make(session: nil, weekly: unknown, now: now) == nil)
    }
}
