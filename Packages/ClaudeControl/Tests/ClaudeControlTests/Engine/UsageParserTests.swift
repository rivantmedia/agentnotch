import Foundation
import Testing
@testable import ClaudeControl

/// Fixtures follow the shapes Claude Code 2.1.280 produces: the
/// `/api/oauth/usage` body (as cached in `.claude.json` and returned by
/// `get_usage`), the `get_usage` control response, and the status line.
struct UsageParserTests {
    private func json(_ text: String) throws -> [String: Any] {
        let object = try JSONSerialization.jsonObject(with: Data(text.utf8))
        return try #require(object as? [String: Any])
    }

    private func date(_ iso: String) -> Date {
        try! Date(iso, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: iso.contains(".")))
    }

    /// A full usage body as the endpoint returns it today.
    static let liveBody = """
    {"five_hour":{"utilization":9,"resets_at":"2026-09-24T12:50:00.257626+00:00","limit_dollars":null,"used_dollars":null,"remaining_dollars":null,"locked_reason":null},
     "seven_day":{"utilization":3.0,"resets_at":"2026-09-29T06:00:00+00:00","limit_dollars":null,"used_dollars":null,"remaining_dollars":null,"locked_reason":null},
     "seven_day_oauth_apps":null,"seven_day_opus":null,"seven_day_sonnet":null,
     "seven_day_cowork":null,"seven_day_omelette":null,"cinder_cove":null,
     "iguana_necktie":{"utilization":0,"resets_at":null,"limit_dollars":250},
     "extra_usage":{"is_enabled":false,"monthly_limit":null,"used_credits":null,"utilization":null,"currency":null,"disabled_reason":null},
     "limits":[
       {"kind":"session","group":"session","percent":9,"severity":"normal","resets_at":"2026-09-24T12:50:00.257626+00:00","scope":null,"is_active":true},
       {"kind":"weekly_all","group":"weekly","percent":3,"severity":"normal","resets_at":"2026-09-29T06:00:00+00:00","scope":null,"is_active":true},
       {"kind":"weekly_scoped","group":"weekly","percent":0,"severity":"normal","resets_at":"2026-09-29T06:00:00+00:00","scope":{"model":{"id":null,"display_name":"Fable"},"surface":null},"is_active":false}
     ],
     "spend":{"limit_dollars":null},
     "seven_day_breakdown":{"rows":[{"key":"claude_code","percent":92}]}}
    """

    @Test func parsesLiveBody() throws {
        let usage = UsageParser.parseUsageBody(try json(Self.liveBody))

        let fiveHour = try #require(usage.fiveHour)
        #expect(fiveHour.utilization == 9)
        #expect(fiveHour.duration == UsageWindow.sessionDuration)
        #expect(fiveHour.resetsAt == date("2026-09-24T12:50:00.257626+00:00"))

        let sevenDay = try #require(usage.sevenDay)
        #expect(sevenDay.utilization == 3)
        #expect(sevenDay.duration == UsageWindow.weeklyDuration)
        #expect(sevenDay.resetsAt == date("2026-09-29T06:00:00Z"))

        #expect(usage.scoped.map(\.name) == ["Fable"])
        #expect(usage.scoped.first?.window.utilization == 0)
        #expect(usage.scoped.first?.window.duration == UsageWindow.weeklyDuration)

        let extra = try #require(usage.extraUsage)
        #expect(extra.isEnabled == false)
        #expect(extra.monthlyLimit == nil)
        #expect(usage.subscriptionType == nil)
    }

    @Test func fractionalSecondsAreKept() {
        let withFraction = UsageParser.parseDate("2026-09-24T12:50:00.257626+00:00")
        let without = UsageParser.parseDate("2026-09-24T12:50:00+00:00")
        let zulu = UsageParser.parseDate("2026-09-24T12:50:00Z")
        let millis = UsageParser.parseDate("2026-09-24T12:50:00.257Z")
        #expect(without == zulu)
        #expect(without?.timeIntervalSince1970 == 1_790_254_200)
        #expect(abs((withFraction?.timeIntervalSince1970 ?? 0) - 1_790_254_200.257626) < 0.001)
        #expect(abs((millis?.timeIntervalSince1970 ?? 0) - 1_790_254_200.257) < 0.001)
    }

    @Test func parsesOffsetsAndMissingZones() {
        #expect(UsageParser.parseDate("2026-09-24T18:20:00+05:30")?.timeIntervalSince1970 == 1_790_254_200)
        // No zone designator: read as UTC rather than dropped.
        #expect(UsageParser.parseDate("2026-09-24T12:50:00")?.timeIntervalSince1970 == 1_790_254_200)
    }

    @Test func parsesEpochDates() {
        #expect(UsageParser.parseDate(1_790_254_200)?.timeIntervalSince1970 == 1_790_254_200)
        #expect(UsageParser.parseDate(1_790_254_200_000)?.timeIntervalSince1970 == 1_790_254_200)
        #expect(UsageParser.parseDate(1_790_254_200.5)?.timeIntervalSince1970 == 1_790_254_200.5)
    }

    @Test(arguments: ["", "soon", "2026-13-45T99:00:00Z"])
    func rejectsBadDates(_ text: String) {
        #expect(UsageParser.parseDate(text) == nil)
    }

    @Test func rejectsNonDates() {
        #expect(UsageParser.parseDate(nil) == nil)
        #expect(UsageParser.parseDate(NSNull()) == nil)
        #expect(UsageParser.parseDate(true) == nil)
    }

    @Test func utilizationAcceptsIntDoubleAndString() throws {
        let body = try json("""
        {"five_hour":{"utilization":"42.5","resets_at":null},
         "seven_day":{"utilization":7,"resets_at":"2026-09-29T06:00:00Z"},
         "seven_day_opus":{"utilization":12.25,"resets_at":"2026-09-29T06:00:00.000Z"}}
        """)
        let usage = UsageParser.parseUsageBody(body)
        #expect(usage.fiveHour?.utilization == 42.5)
        #expect(usage.fiveHour?.resetsAt == nil)
        #expect(usage.sevenDay?.utilization == 7)
        #expect(usage.scoped == [
            ScopedUsage(
                name: "Opus",
                window: UsageWindow(utilization: 12.25, resetsAt: date("2026-09-29T06:00:00Z"), duration: UsageWindow.weeklyDuration)
            ),
        ])
    }

    @Test func booleansAreNotNumbers() {
        #expect(UsageParser.number(true) == nil)
        #expect(UsageParser.number(NSNumber(value: false)) == nil)
        #expect(UsageParser.number(NSNumber(value: 3)) == 3)
        #expect(UsageParser.number(" 4.5 ") == 4.5)
        #expect(UsageParser.number("nan") == nil)
    }

    @Test func nullWindowsAreNil() throws {
        let usage = UsageParser.parseUsageBody(try json("""
        {"five_hour":null,"seven_day":{"utilization":null,"resets_at":null},"seven_day_opus":null,"seven_day_sonnet":null,"extra_usage":null}
        """))
        #expect(usage.fiveHour == nil)
        #expect(usage.sevenDay == nil)
        #expect(usage.scoped.isEmpty)
        #expect(usage.extraUsage == nil)
        #expect(!usage.hasWindows)
    }

    @Test func limitsFillMissingTopLevelWindows() throws {
        let usage = UsageParser.parseUsageBody(try json("""
        {"five_hour":null,"seven_day":null,
         "limits":[{"kind":"session","percent":61,"resets_at":"2026-09-24T12:50:00Z"},
                   {"kind":"weekly_all","percent":"18","resets_at":"2026-09-29T06:00:00Z"}]}
        """))
        #expect(usage.fiveHour?.utilization == 61)
        #expect(usage.fiveHour?.duration == UsageWindow.sessionDuration)
        #expect(usage.sevenDay?.utilization == 18)
        #expect(usage.sevenDay?.duration == UsageWindow.weeklyDuration)
    }

    @Test func scopedLimitsMergeAndDedupeByName() throws {
        let usage = UsageParser.parseUsageBody(try json("""
        {"five_hour":{"utilization":20,"resets_at":"2026-09-24T12:50:00Z"},
         "seven_day":{"utilization":30,"resets_at":"2026-09-29T06:00:00Z"},
         "seven_day_opus":{"utilization":99,"resets_at":"2026-09-29T06:00:00Z"},
         "seven_day_sonnet":{"utilization":14,"resets_at":"2026-09-29T06:00:00Z"},
         "limits":[{"kind":"weekly_scoped","percent":41,"resets_at":"2026-09-29T06:00:00Z","scope":{"model":{"id":null,"display_name":"Opus"}}},
                   {"kind":"weekly_scoped","percent":5,"resets_at":"2026-09-29T06:00:00Z","scope":{"model":{"display_name":"Fable"}}},
                   {"kind":"weekly_scoped","percent":8,"scope":null}],
         "model_scoped":[{"display_name":"fable","utilization":6,"resets_at":"2026-09-29T06:00:00Z"},
                         {"display_name":"Haiku","utilization":2.5,"resets_at":"2026-09-29T06:00:00.5Z"}]}
        """))
        // limits[] first, then model_scoped, then the legacy per-model keys;
        // names compare case-insensitively and the first reading wins.
        #expect(usage.scoped.map(\.name) == ["Opus", "Fable", "Haiku", "Sonnet"])
        #expect(usage.scoped.map(\.window.utilization) == [41, 5, 2.5, 14])
    }

    @Test func extraUsageWhenEnabled() throws {
        let usage = UsageParser.parseUsageBody(try json("""
        {"five_hour":{"utilization":100,"resets_at":"2026-09-24T12:50:00Z"},
         "extra_usage":{"is_enabled":true,"monthly_limit":5000,"used_credits":1234.5,"utilization":24.69,"currency":"USD"}}
        """))
        #expect(usage.extraUsage == ExtraUsage(isEnabled: true, monthlyLimit: 5000, usedCredits: 1234.5, utilization: 24.69, currency: "USD"))
    }

    // MARK: - get_usage

    @Test func getUsageResponse() throws {
        let response = try json("""
        {"session":{"id":"x"},"subscription_type":"max","rate_limits_available":true,
         "rate_limits":{"five_hour":{"utilization":23,"resets_at":"2026-09-24T12:50:00.257626+00:00"},
                        "seven_day":{"utilization":41,"resets_at":"2026-09-29T06:00:00+00:00"},
                        "seven_day_sonnet":null,
                        "limits":[],
                        "model_scoped":[{"display_name":"Opus","utilization":12,"resets_at":"2026-09-29T06:00:00+00:00"}]},
         "behaviors":null}
        """)
        guard case .usage(let usage) = UsageParser.parseGetUsageResponse(response) else {
            Issue.record("expected usage")
            return
        }
        #expect(usage.fiveHour?.utilization == 23)
        #expect(usage.sevenDay?.utilization == 41)
        #expect(usage.scoped.map(\.name) == ["Opus"])
        #expect(usage.subscriptionType == "max")
    }

    @Test func getUsageUnavailable() throws {
        let response = try json(#"{"subscription_type":null,"rate_limits_available":false,"rate_limits":null}"#)
        #expect(UsageParser.parseGetUsageResponse(response) == .unavailable("Usage limits aren't available for this login"))
    }

    @Test func getUsageRateLimited() throws {
        let response = try json(#"{"rate_limits_available":true,"rate_limits":{"error":{"type":"rate_limit_error","message":"Rate limited"}}}"#)
        #expect(UsageParser.parseGetUsageResponse(response) == .rateLimited)
    }

    /// Claude Code 2.1.280 answers `rate_limits: null` with availability true
    /// when its own fetch failed (429, network): a retryable failure.
    @Test func getUsageWhenClaudeCodesFetchFailed() throws {
        let response = try json(#"{"session":{"total_cost_usd":0},"subscription_type":"max","rate_limits_available":true,"rate_limits":null,"behaviors":null}"#)
        #expect(UsageParser.parseGetUsageResponse(response) == .malformed("Claude Code couldn't load usage right now"))
    }

    @Test func getUsageWithoutWindowsIsMalformed() throws {
        let response = try json(#"{"rate_limits_available":true,"rate_limits":{"five_hour":null,"seven_day":null}}"#)
        guard case .malformed = UsageParser.parseGetUsageResponse(response) else {
            Issue.record("expected malformed")
            return
        }
    }

    // MARK: - .claude.json

    @Test func cachedUsageFromGlobalConfig() throws {
        let config = try json("""
        {"numStartups":12,"projects":{"/Users/me/p":{"allowedTools":[]}},
         "oauthAccount":{"accountUuid":"acc-1","emailAddress":"me@example.com","organizationType":"claude_max","organizationRateLimitTier":"default_claude_max_20x","displayName":"Me","organizationName":"Me's Org","hasExtraUsageEnabled":false},
         "cachedUsageUtilization":{"fetchedAtMs":1790250000123,"accountUuid":"acc-1","utilization":\(Self.liveBody)}}
        """)
        let snapshot = try #require(UsageParser.parseCachedUsage(fromGlobalConfig: config))
        #expect(snapshot.accountUuid == "acc-1")
        #expect(abs(snapshot.fetchedAt.timeIntervalSince1970 - 1_790_250_000.123) < 0.0001)
        #expect(snapshot.usage.fiveHour?.utilization == 9)

        let extracted = ClaudeGlobalConfig.extract(from: config)
        #expect(extracted.identity?.email == "me@example.com")
        #expect(extracted.identity?.subscriptionType == "max")
        #expect(extracted.identity?.rateLimitTier == "default_claude_max_20x")
        #expect(extracted.matchingCachedUsage == snapshot)
    }

    @Test func cachedUsageFromAnotherLoginIsIgnored() throws {
        let config = try json("""
        {"oauthAccount":{"accountUuid":"acc-2","emailAddress":"other@example.com"},
         "cachedUsageUtilization":{"fetchedAtMs":1790250000123,"accountUuid":"acc-1","utilization":\(Self.liveBody)}}
        """)
        let extracted = ClaudeGlobalConfig.extract(from: config)
        #expect(extracted.cachedUsage != nil)
        #expect(extracted.matchingCachedUsage == nil)
    }

    @Test func signedOutConfigHasNoIdentity() throws {
        let extracted = ClaudeGlobalConfig.extract(from: try json(#"{"numStartups":1,"oauthAccount":{}}"#))
        #expect(extracted.identity == nil)
        #expect(extracted.cachedUsage == nil)
    }

    @Test func userTierIsTheFallback() throws {
        let identity = try #require(ClaudeAccountIdentity(oauthAccount: try json(
            #"{"accountUuid":"a","userRateLimitTier":"default_claude_max_5x","organizationType":"claude_pro"}"#
        )))
        #expect(identity.rateLimitTier == "default_claude_max_5x")
        #expect(identity.subscriptionType == "pro")
    }

    // MARK: - Status Line

    @Test func statusLineRateLimits() throws {
        let rateLimits = try json(#"{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},"seven_day":{"used_percentage":41.2,"resets_at":1738857600}}"#)
        let parsed = UsageParser.parseStatusLineRateLimits(rateLimits)
        #expect(parsed.fiveHour == UsageWindow(utilization: 23.5, resetsAt: Date(timeIntervalSince1970: 1_738_425_600), duration: UsageWindow.sessionDuration))
        #expect(parsed.sevenDay == UsageWindow(utilization: 41.2, resetsAt: Date(timeIntervalSince1970: 1_738_857_600), duration: UsageWindow.weeklyDuration))
    }

    @Test func statusLineWithOnlyOneWindow() throws {
        let parsed = UsageParser.parseStatusLineRateLimits(try json(#"{"seven_day":{"used_percentage":0,"resets_at":1738857600}}"#))
        #expect(parsed.fiveHour == nil)
        #expect(parsed.sevenDay?.utilization == 0)
        #expect(UsageParser.parseStatusLineRateLimits(nil).fiveHour == nil)
    }
}
