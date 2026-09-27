import Foundation
import Testing
@testable import ClaudeControl

struct UsageProbeParsingTests {
    private func line(_ text: String) -> Data { Data(text.utf8) }

    @Test func initializeResponse() {
        #expect(UsageProbe.parseLine(line(
            #"{"type":"control_response","response":{"subtype":"success","request_id":"agentnotch-init","response":{"account":{"email":"x"}}}}"#
        )) == .initialized(error: nil))
        #expect(UsageProbe.parseLine(line(
            #"{"type":"control_response","response":{"subtype":"error","request_id":"agentnotch-init","error":"boom"}}"#
        )) == .initialized(error: "boom"))
    }

    @Test func usageResponse() {
        let event = UsageProbe.parseLine(line("""
        {"type":"control_response","response":{"subtype":"success","request_id":"agentnotch-usage","response":{"subscription_type":"max","rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":23,"resets_at":"2026-09-24T12:50:00.257626+00:00"},"seven_day":{"utilization":41,"resets_at":"2026-09-29T06:00:00+00:00"},"model_scoped":[]},"behaviors":null}}}
        """))
        guard case .usage(.usage(let usage)) = event else {
            Issue.record("expected usage, got \(event)")
            return
        }
        #expect(usage.fiveHour?.utilization == 23)
        #expect(usage.subscriptionType == "max")
    }

    @Test func usageErrors() {
        #expect(UsageProbe.parseLine(line(
            #"{"type":"control_response","response":{"subtype":"error","request_id":"agentnotch-usage","error":"Not logged in · Please run /login"}}"#
        )) == .usage(.unavailable("Not signed in to Claude")))
        #expect(UsageProbe.parseLine(line(
            #"{"type":"control_response","response":{"subtype":"error","request_id":"agentnotch-usage","error":"Request failed with status 429"}}"#
        )) == .usage(.rateLimited))
        #expect(UsageProbe.parseLine(line(
            #"{"type":"control_response","response":{"subtype":"success","request_id":"agentnotch-usage","response":{"rate_limits_available":false}}}"#
        )) == .usage(.unavailable("Usage limits aren't available for this login")))
    }

    @Test func otherLinesAreIgnored() {
        #expect(UsageProbe.parseLine(line(#"{"type":"system","subtype":"init","session_id":"x"}"#)) == .other)
        #expect(UsageProbe.parseLine(line(#"{"type":"control_response","response":{"subtype":"success","request_id":"other"}}"#)) == .other)
        #expect(UsageProbe.parseLine(line("not json")) == .other)
    }

    @Test func environmentIsScrubbed() {
        let base = [
            "PATH": "/usr/bin",
            "HOME": "/Users/me",
            "CLAUDECODE": "1",
            "CLAUDE_PID": "123",
            "CLAUDE_EFFORT": "high",
            "CLAUDE_CODE_SESSION_ID": "s",
            "CLAUDE_CODE_MESSAGING_TOKEN": "secret",
            "CLAUDE_CODE_ENTRYPOINT": "cli",
            "CLAUDE_AGENT_SDK_VERSION": "1",
            "AI_AGENT": "claude",
            "CLAUDE_CONFIG_DIR": "/Users/me/.claude-other",
        ]
        #expect(UsageProbe.environment(base: base, configDirEnv: nil) == ["PATH": "/usr/bin", "HOME": "/Users/me"])
        #expect(UsageProbe.environment(base: base, configDirEnv: "/Users/me/.claude-work/")
            == ["PATH": "/usr/bin", "HOME": "/Users/me", "CLAUDE_CONFIG_DIR": "/Users/me/.claude-work/"])
    }

    @Test func requestsAreSingleLines() throws {
        for data in [UsageProbe.initializeRequest, UsageProbe.usageRequest] {
            #expect(data.last == 0x0A)
            #expect(data.dropLast().contains(0x0A) == false)
        }
        let usage = try JSONSerialization.jsonObject(with: UsageProbe.usageRequest) as? [String: Any]
        let request = usage?["request"] as? [String: Any]
        #expect(request?["subtype"] as? String == "get_usage")
        #expect(request?["skip_behaviors"] as? Bool == true)
    }
}

/// Runs the probe's process plumbing against a stand-in `claude` that speaks
/// the stream-json control protocol. Not main-actor isolated: the probe's
/// timers and pipes must not queue behind other suites' main-actor work.
nonisolated struct UsageProbeProcessTests {
    private func stubClaude(_ behaviour: String) throws -> (binary: String, dir: URL) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("agentnotch-probe-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let script = dir.appendingPathComponent("claude")
        let source = """
        #!/usr/bin/env python3
        import json, os, sys, time
        dir = os.path.dirname(os.path.abspath(__file__))
        with open(os.path.join(dir, "pid"), "w") as f:
            f.write(str(os.getpid()))
        with open(os.path.join(dir, "env.json"), "w") as f:
            json.dump({k: v for k, v in os.environ.items() if k.startswith("CLAUDE") or k == "AI_AGENT"}, f)
        with open(os.path.join(dir, "argv.json"), "w") as f:
            json.dump(sys.argv[1:], f)
        behaviour = \(behaviour.debugDescription)
        def reply(rid, payload=None, error=None):
            if error is None:
                msg = {"type": "control_response", "response": {"subtype": "success", "request_id": rid, "response": payload or {}}}
            else:
                msg = {"type": "control_response", "response": {"subtype": "error", "request_id": rid, "error": error}}
            sys.stdout.write(json.dumps(msg) + "\\n"); sys.stdout.flush()
        if behaviour == "exit":
            sys.stderr.write("Error: Invalid API key\\n"); sys.exit(1)
        if behaviour == "hang":
            time.sleep(3600); sys.exit(0)
        sys.stdout.write(json.dumps({"type": "system", "subtype": "init"}) + "\\n"); sys.stdout.flush()
        for line in sys.stdin:
            msg = json.loads(line)
            rid, sub = msg["request_id"], msg["request"]["subtype"]
            if sub == "initialize":
                reply(rid, {"commands": ["x" * 70000]})  # large, like the real one
            elif sub == "get_usage":
                if behaviour == "error":
                    reply(rid, error="Not logged in · Please run /login")
                else:
                    reply(rid, {"subscription_type": "pro", "rate_limits_available": True, "rate_limits": {
                        "five_hour": {"utilization": 55, "resets_at": "2026-09-24T12:50:00.257626+00:00"},
                        "seven_day": {"utilization": 12, "resets_at": "2026-09-29T06:00:00+00:00"},
                        "model_scoped": [{"display_name": "Opus", "utilization": 4, "resets_at": "2026-09-29T06:00:00+00:00"}]}})
        """
        try source.write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        return (script.path, dir)
    }

    private func isAlive(_ pid: pid_t) -> Bool {
        kill(pid, 0) == 0 || errno != ESRCH
    }

    @Test func probeReturnsUsageAndScrubsEnv() async throws {
        let stub = try stubClaude("ok")
        defer { try? FileManager.default.removeItem(at: stub.dir) }

        let outcome = await UsageProbe.run(
            claudePath: stub.binary,
            configDirEnv: "/tmp/agentnotch-probe-tests/.claude-work/",
            workingDirectory: stub.dir.appendingPathComponent("cwd"),
            timeout: 15
        )
        guard case .usage(let usage) = outcome else {
            Issue.record("expected usage, got \(outcome)")
            return
        }
        #expect(usage.fiveHour?.utilization == 55)
        #expect(usage.sevenDay?.utilization == 12)
        #expect(usage.scoped.map(\.name) == ["Opus"])
        #expect(usage.subscriptionType == "pro")

        let env = try JSONSerialization.jsonObject(with: Data(contentsOf: stub.dir.appendingPathComponent("env.json"))) as? [String: String]
        #expect(env == ["CLAUDE_CONFIG_DIR": "/tmp/agentnotch-probe-tests/.claude-work/"])
        let argv = try JSONSerialization.jsonObject(with: Data(contentsOf: stub.dir.appendingPathComponent("argv.json"))) as? [String]
        #expect(argv == UsageProbe.arguments)
    }

    @Test func errorResponseBecomesUnavailable() async throws {
        let stub = try stubClaude("error")
        defer { try? FileManager.default.removeItem(at: stub.dir) }
        let outcome = await UsageProbe.run(
            claudePath: stub.binary, configDirEnv: nil,
            workingDirectory: stub.dir.appendingPathComponent("cwd"), timeout: 15
        )
        #expect(outcome == .unavailable("Not signed in to Claude"))
    }

    @Test func earlyExitIsReportedWithStderr() async throws {
        let stub = try stubClaude("exit")
        defer { try? FileManager.default.removeItem(at: stub.dir) }
        let outcome = await UsageProbe.run(
            claudePath: stub.binary, configDirEnv: nil,
            workingDirectory: stub.dir.appendingPathComponent("cwd"), timeout: 15
        )
        #expect(outcome == .failed("Claude Code exited (1): Error: Invalid API key"))
    }

    @Test(.timeLimit(.minutes(1)))
    func hangingChildIsKilledAndReaped() async throws {
        let stub = try stubClaude("hang")
        defer { try? FileManager.default.removeItem(at: stub.dir) }
        let outcome = await UsageProbe.run(
            claudePath: stub.binary, configDirEnv: nil,
            workingDirectory: stub.dir.appendingPathComponent("cwd"), timeout: 2
        )
        // Answered at the timeout: the child would sleep for an hour.
        #expect(outcome == .failed("Claude Code didn't answer within 2s"))

        // On a very loaded Mac the stub can be stopped before Python got as
        // far as writing its pid: then there is nothing left to reap.
        guard let pidText = try? String(contentsOf: stub.dir.appendingPathComponent("pid"), encoding: .utf8),
              let pid = pid_t(pidText) else { return }
        // Terminated after a short grace period, killed if it lingers, then
        // reaped (a zombie would still answer kill(pid, 0)).
        let deadline = Date().addingTimeInterval(30)
        while isAlive(pid), Date() < deadline {
            try await Task.sleep(for: .milliseconds(250))
        }
        #expect(!isAlive(pid))
    }

    @Test func missingBinaryFails() async {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("agentnotch-probe-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let outcome = await UsageProbe.run(
            claudePath: dir.appendingPathComponent("definitely-not-claude").path,
            configDirEnv: nil,
            workingDirectory: dir.appendingPathComponent("cwd"),
            timeout: 5
        )
        guard case .failed = outcome else {
            Issue.record("expected failure, got \(outcome)")
            return
        }
    }
}

struct UsageStoreLogicTests {
    let now = Date(timeIntervalSince1970: 1_790_000_000)

    private func window(_ utilization: Double, _ duration: TimeInterval = UsageWindow.sessionDuration) -> UsageWindow {
        UsageWindow(utilization: utilization, resetsAt: now.addingTimeInterval(3600), duration: duration)
    }

    /// A weekly window that resets `resetIn` from `now`.
    private func week(_ utilization: Double, resetIn: TimeInterval = 3 * 86400) -> UsageWindow {
        UsageWindow(utilization: utilization, resetsAt: now.addingTimeInterval(resetIn), duration: UsageWindow.weeklyDuration)
    }

    /// A status line reading that arrived `at` seconds from `now`, taken
    /// after `after` seconds from `now` when that is known.
    private func reading(_ window: UsageWindow, at: TimeInterval, after: TimeInterval? = nil) -> UsageStore.Reading {
        UsageStore.Reading(window, at: now.addingTimeInterval(at), notBefore: after.map { now.addingTimeInterval($0) })
    }

    @Test func statusLineAloneMakesUsage() {
        let merged = UsageStore.merge(
            accountId: "a",
            full: nil,
            statusFiveHour: reading(window(20), at: 0),
            statusSevenDay: nil
        )
        #expect(merged?.fiveHour == window(20))
        #expect(merged?.sevenDay == nil)
        #expect(merged?.source == .statusLine)
        #expect(merged?.updatedAt == now)
        #expect(UsageStore.merge(accountId: "a", full: nil, statusFiveHour: nil, statusSevenDay: nil) == nil)
    }

    @Test func newestReadingWinsPerWindow() {
        let full = AccountUsage(
            accountId: "a",
            fiveHour: window(10),
            sevenDay: window(30, UsageWindow.weeklyDuration),
            scoped: [ScopedUsage(name: "Opus", window: window(5, UsageWindow.weeklyDuration))],
            extraUsage: ExtraUsage(isEnabled: true, monthlyLimit: 10, usedCredits: 1, utilization: 10, currency: "USD"),
            subscriptionType: "max",
            source: .probe,
            updatedAt: now
        )
        // Newer status line 5h, older status line 7d.
        let merged = UsageStore.merge(
            accountId: "a",
            full: full,
            statusFiveHour: reading(window(25), at: 60),
            statusSevenDay: reading(window(20, UsageWindow.weeklyDuration), at: -60)
        )
        #expect(merged?.fiveHour == window(25))
        #expect(merged?.sevenDay == window(30, UsageWindow.weeklyDuration))
        #expect(merged?.scoped == full.scoped)
        #expect(merged?.extraUsage == full.extraUsage)
        #expect(merged?.subscriptionType == "max")
        #expect(merged?.source == .statusLine)
        #expect(merged?.updatedAt == now.addingTimeInterval(60))
    }

    @Test func statusLineFillsAWindowTheSnapshotLacks() {
        let full = AccountUsage(accountId: "a", fiveHour: nil, sevenDay: window(30, UsageWindow.weeklyDuration), source: .cache, updatedAt: now)
        let merged = UsageStore.merge(accountId: "a", full: full, statusFiveHour: reading(window(7), at: -600), statusSevenDay: nil)
        #expect(merged?.fiveHour == window(7))
        #expect(merged?.source == .cache)
        #expect(merged?.updatedAt == now)
    }

    // A status line re-run repeats the session's last API response, however
    // old: it must not beat a fresher probe just by arriving later.
    @Test func staleStatusLineDoesNotBeatAFresherSnapshot() {
        let full = AccountUsage(accountId: "a", fiveHour: window(55), sevenDay: window(30, UsageWindow.weeklyDuration),
                                source: .probe, updatedAt: now)
        let merged = UsageStore.merge(
            accountId: "a",
            full: full,
            statusFiveHour: reading(window(40), at: 60),
            statusSevenDay: reading(window(28, UsageWindow.weeklyDuration), at: 60)
        )
        #expect(merged?.fiveHour == window(55))
        #expect(merged?.sevenDay == window(30, UsageWindow.weeklyDuration))
        #expect(merged?.source == .probe)
        #expect(merged?.updatedAt == now)
        // Even one that changed, when the change may predate the probe.
        let straddling = UsageStore.merge(accountId: "a", full: full, statusFiveHour: reading(window(40), at: 60, after: -30),
                                          statusSevenDay: nil)
        #expect(straddling?.fiveHour == window(55))
    }

    @Test func aNewerWindowWinsEvenWhenLower() {
        let expired = UsageWindow(utilization: 80, resetsAt: now.addingTimeInterval(-600), duration: UsageWindow.sessionDuration)
        let fresh = UsageWindow(utilization: 5, resetsAt: now.addingTimeInterval(4 * 3600), duration: UsageWindow.sessionDuration)
        let full = AccountUsage(accountId: "a", fiveHour: expired, source: .cache, updatedAt: now)
        let merged = UsageStore.merge(accountId: "a", full: full, statusFiveHour: reading(fresh, at: -60), statusSevenDay: nil)
        #expect(merged?.fiveHour == fresh)
        // ... and an older window never beats a newer one, whenever it arrived.
        let reversed = AccountUsage(accountId: "a", fiveHour: fresh, source: .probe, updatedAt: now)
        #expect(UsageStore.merge(accountId: "a", full: reversed, statusFiveHour: reading(expired, at: 60), statusSevenDay: nil)?.fiveHour == fresh)
    }

    @Test func sameWindowToleratesRounding() {
        // The endpoint's ISO reset has fractional seconds; the status line's is whole epoch seconds.
        let endpoint = UsageWindow(utilization: 9, resetsAt: Date(timeIntervalSince1970: 1_790_254_200.257626), duration: UsageWindow.sessionDuration)
        let statusLine = UsageWindow(utilization: 9.4, resetsAt: Date(timeIntervalSince1970: 1_790_254_200), duration: UsageWindow.sessionDuration)
        let nextWindow = UsageWindow(utilization: 1, resetsAt: Date(timeIntervalSince1970: 1_790_254_200 + 5 * 3600), duration: UsageWindow.sessionDuration)
        #expect(UsageStore.isSameWindow(endpoint, statusLine))
        #expect(!UsageStore.isSameWindow(endpoint, nextWindow))
        #expect(!UsageStore.isSameWindow(endpoint, UsageWindow(utilization: 1, resetsAt: nil, duration: UsageWindow.sessionDuration)))
        // A later reading a rounding step lower is no reset: the higher stays.
        let full = AccountUsage(accountId: "a", fiveHour: endpoint, source: .probe, updatedAt: now)
        #expect(UsageStore.merge(accountId: "a", full: full, statusFiveHour: reading(statusLine, at: -60), statusSevenDay: nil)?.fiveHour == statusLine)
    }

    @Test func repeatedReadingsKeepTheirTime() {
        let first = UsageStore.advance(nil, fiveHour: window(20), sevenDay: nil, receivedAt: now, startedAt: nil)
        #expect(first.fiveHour == reading(window(20), at: 0))
        // The same numbers again (an idle session re-rendering): not news.
        let repeated = UsageStore.advance(first, fiveHour: window(20), sevenDay: nil, receivedAt: now.addingTimeInterval(300), startedAt: nil)
        #expect(repeated.fiveHour == first.fiveHour)
        #expect(repeated.lastReportAt == now.addingTimeInterval(300))
        // A small step back (a response that started before the last one
        // ended after it): the higher reading stays, with its time.
        let stepBack = UsageStore.advance(repeated, fiveHour: window(20 - UsageStore.resetDropMinimum), sevenDay: nil,
                                          receivedAt: now.addingTimeInterval(350), startedAt: nil)
        #expect(stepBack.fiveHour == first.fiveHour)
        #expect(stepBack.lastReportAt == now.addingTimeInterval(350))
        // Different numbers, even lower by a reset: the process's own newer
        // response, taken after the report before.
        let lower = UsageStore.advance(stepBack, fiveHour: window(12), sevenDay: week(40), receivedAt: now.addingTimeInterval(400), startedAt: nil)
        #expect(lower.fiveHour == reading(window(12), at: 400, after: 350))
        #expect(lower.sevenDay == reading(week(40), at: 400, after: 350))
        // A reset time that lost its fraction of a second (a relaunch) is the same.
        var rounded = window(12)
        rounded.resetsAt = Date(timeIntervalSince1970: rounded.resetsAt!.timeIntervalSince1970.rounded(.down) + 0.4)
        #expect(UsageStore.advance(lower, fiveHour: rounded, sevenDay: week(40), receivedAt: now.addingTimeInterval(450), startedAt: nil).fiveHour
            == lower.fiveHour)
        #expect(!UsageStore.isRepeat(window(15), window(16)))
        // A window the line leaves out keeps what it had.
        #expect(UsageStore.advance(lower, fiveHour: nil, sevenDay: week(40), receivedAt: now.addingTimeInterval(500), startedAt: nil).fiveHour == lower.fiveHour)
    }

    /// Claude Code has no rate limits before a process's first API
    /// response, so a process's first report is newer than the process.
    @Test func aFirstReportIsNewerThanItsProcess() {
        let first = UsageStore.advance(nil, fiveHour: nil, sevenDay: week(3), receivedAt: now, startedAt: now.addingTimeInterval(-60))
        #expect(first.sevenDay == reading(week(3), at: 0, after: -60))
        // A start time that isn't before the report is no bound.
        let odd = UsageStore.advance(nil, fiveHour: nil, sevenDay: week(3), receivedAt: now, startedAt: now)
        #expect(odd.sevenDay?.notBefore == nil)
    }

    @Test func readingsInUnknownOrderKeepTheOldRules() {
        // Lower within the same window from a process whose data may be older: the higher stays.
        #expect(UsageStore.mostCurrent([reading(window(20), at: 0), reading(window(15), at: 300)])?.window == window(20))
        // Higher: fresh.
        #expect(UsageStore.mostCurrent([reading(window(20), at: 0), reading(window(25), at: 300)])?.window == window(25))
        // No reset times to compare: arrival order decides.
        let bare = { (value: Double) in UsageWindow(utilization: value, resetsAt: nil, duration: UsageWindow.sessionDuration) }
        #expect(UsageStore.mostCurrent([reading(bare(20), at: 0), reading(bare(10), at: 1)])?.window == bare(10))
        #expect(UsageStore.mostCurrent([]) == nil)
    }

    // MARK: Early resets

    /// Anthropic reset the week early and kept the reset time: the probe
    /// after it reads low, a status line from before it still says high.
    @Test func aLaterSnapshotWinsAfterAnEarlyReset() {
        // The reset time kept, moved a day later (inside the same-window
        // tolerance), or moved two days earlier.
        let shifts: [TimeInterval] = [0, 86400, -2 * 86400]
        for shift in shifts {
            let resetIn = 3 * 86400 + shift
            let after = AccountUsage(accountId: "a", sevenDay: week(3, resetIn: resetIn), source: .probe, updatedAt: now)
            let merged = UsageStore.merge(accountId: "a", full: after, statusFiveHour: nil,
                                          statusSevenDay: reading(week(62), at: -600, after: -900))
            #expect(merged?.sevenDay == week(3, resetIn: resetIn), "reset moved by \(Int(shift)) s")
            #expect(merged?.source == .probe)
        }
    }

    /// ...and the other way round: the reading kept from before the reset
    /// (usage-state.json, an old cache) loses to a status line taken after it.
    @Test func aLaterStatusLineWinsOverAnOlderSnapshot() {
        let before = AccountUsage(accountId: "a", sevenDay: week(62), source: .cache, updatedAt: now)
        // A process that changed after the snapshot, or started after it.
        let merged = UsageStore.merge(accountId: "a", full: before, statusFiveHour: nil,
                                      statusSevenDay: reading(week(3), at: 120, after: 60))
        #expect(merged?.sevenDay == week(3))
        #expect(merged?.updatedAt == now.addingTimeInterval(120))
        // One that may have been taken before it (a re-run, a straddling change) doesn't.
        #expect(UsageStore.merge(accountId: "a", full: before, statusFiveHour: nil,
                                 statusSevenDay: reading(week(3), at: 120))?.sevenDay == week(62))
        #expect(UsageStore.merge(accountId: "a", full: before, statusFiveHour: nil,
                                 statusSevenDay: reading(week(3), at: 120, after: -60))?.sevenDay == week(62))
    }

    /// Two sessions: A keeps working through the reset, B went idle before
    /// it and keeps re-rendering its old numbers.
    @Test func anIdleSessionsOldNumbersDontOutliveTheReset() {
        var a = UsageStore.advance(nil, fiveHour: nil, sevenDay: week(61), receivedAt: now, startedAt: nil)
        let b = UsageStore.advance(nil, fiveHour: nil, sevenDay: week(62), receivedAt: now.addingTimeInterval(10), startedAt: nil)
        a = UsageStore.advance(a, fiveHour: nil, sevenDay: week(61), receivedAt: now.addingTimeInterval(20), startedAt: nil)
        func shown() -> UsageWindow? {
            UsageStore.combinedStatus(["/w": UsageStore.readings(["pid:1": a, "pid:2": b], \.sevenDay)],
                                      defaultFolder: "/h/.claude", mirrorsDefault: false)?.window
        }
        #expect(shown() == week(62))
        // The reset; A's next response reads 3%.
        a = UsageStore.advance(a, fiveHour: nil, sevenDay: week(3), receivedAt: now.addingTimeInterval(600), startedAt: nil)
        #expect(shown() == week(3))
        // B re-renders the same old numbers: still 3%.
        let bAgain = UsageStore.advance(b, fiveHour: nil, sevenDay: week(62), receivedAt: now.addingTimeInterval(700), startedAt: nil)
        #expect(UsageStore.combinedStatus(["/w": [a.sevenDay!], "/v": [bAgain.sevenDay!]],
                                          defaultFolder: "/h/.claude", mirrorsDefault: false)?.window == week(3))
        // Usage grows again after the reset.
        a = UsageStore.advance(a, fiveHour: nil, sevenDay: week(4), receivedAt: now.addingTimeInterval(800), startedAt: nil)
        #expect(UsageStore.combinedStatus(["/w": [a.sevenDay!], "/v": [bAgain.sevenDay!]],
                                          defaultFolder: "/h/.claude", mirrorsDefault: false)?.window == week(4))
    }

    /// A reset that moves the reset time earlier: the reading known to be
    /// later still wins, though its window resets first.
    @Test func aResetThatMovesTheResetTimeEarlierIsFollowed() {
        let before = reading(week(62, resetIn: 5 * 86400), at: 0, after: -60)
        let after = reading(week(1, resetIn: 2 * 86400), at: 600, after: 300)
        #expect(UsageStore.mostCurrent([before, after]) == after)
        #expect(UsageStore.mostCurrent([after, before]) == after)
        // Order unknown: the later reset still wins, as before.
        #expect(UsageStore.mostCurrent([before, reading(week(1, resetIn: 2 * 86400), at: 600)]) == before)
    }

    @Test func supersedesIsStrictAndNeverMutual() {
        let snapshot = UsageStore.Reading(week(3), at: now, notBefore: now)
        // A point in time doesn't supersede itself, nor a reading from the same moment.
        #expect(!UsageStore.supersedes(snapshot, snapshot))
        #expect(!UsageStore.supersedes(snapshot, reading(week(62), at: 0)))
        #expect(UsageStore.supersedes(snapshot, reading(week(62), at: -1)))
        // The reading with the latest arrival is never out; a later one a
        // point lower is no reset, so the higher stays.
        let readings = [snapshot, reading(week(62), at: -1), reading(week(5), at: 30, after: -10), reading(week(4), at: 60, after: 30)]
        #expect(UsageStore.mostCurrent(readings) == reading(week(5), at: 30, after: -10))
        for x in readings {
            for y in readings where UsageStore.supersedes(x, y) {
                #expect(!UsageStore.supersedes(y, x))
            }
        }
    }

    @Test func statusLineKeyIsTheProcessWhenKnown() {
        #expect(UsageStore.statusLineKey(sessionId: "s1", processId: 4242) == "pid:4242")
        #expect(UsageStore.statusLineKey(sessionId: "s1", processId: nil) == "session:s1")
    }

    /// A response that started before a probe and ended after it: its
    /// process reports numbers a little older than the probe's, as news.
    /// Only a reset-sized drop overrides the higher reading.
    @Test func aSmallDropIsNoResetEvenWhenLater() {
        let probe = AccountUsage(accountId: "a", sevenDay: week(42), source: .probe, updatedAt: now)
        #expect(UsageStore.merge(accountId: "a", full: probe, statusFiveHour: nil,
                                 statusSevenDay: reading(week(40), at: 60, after: 30))?.sevenDay == week(42))
        #expect(UsageStore.merge(accountId: "a", full: probe, statusFiveHour: nil,
                                 statusSevenDay: reading(week(42 - UsageStore.resetDropMinimum), at: 60, after: 30))?.sevenDay == week(42))
        #expect(UsageStore.merge(accountId: "a", full: probe, statusFiveHour: nil,
                                 statusSevenDay: reading(week(36), at: 60, after: 30))?.sevenDay == week(36))
    }

    /// The snapshot and every process's reading are weighed together: one
    /// process known to be newer than the snapshot puts it out, even when
    /// another process's reading is what shows.
    @Test func theSnapshotAndEveryStatusLineAreWeighedTogether() {
        let full = AccountUsage(accountId: "a", sevenDay: week(10), source: .probe, updatedAt: now)
        let lines = [reading(week(5), at: 100, after: -50), reading(week(3), at: 60, after: 30)]
        #expect(UsageStore.merge(accountId: "a", full: full, statusFiveHour: [], statusSevenDay: lines)?.sevenDay == week(5))
        #expect(UsageStore.winningStatus(lines, over: UsageStore.Reading(week(10), of: full)) == lines[0])
        // Neither line alone is known newer than the snapshot... than 5 is.
        #expect(UsageStore.winningStatus([lines[0]], over: UsageStore.Reading(week(10), of: full)) == nil)
    }

    /// A snapshot is known newer only than its `takenAfter` (a probe's
    /// launch, a margin for Claude Desktop's date), never after its `updatedAt`.
    @Test func aSnapshotIsDatedFromItsTakenAfter() {
        var snapshot = AccountUsage(accountId: "a", sevenDay: week(3), source: .probe, updatedAt: now)
        #expect(UsageStore.Reading(week(3), of: snapshot) == UsageStore.Reading(week(3), at: now, notBefore: now))
        snapshot.takenAfter = now.addingTimeInterval(-5)
        #expect(UsageStore.Reading(week(3), of: snapshot).notBefore == now.addingTimeInterval(-5))
        snapshot.takenAfter = now.addingTimeInterval(5)
        #expect(UsageStore.Reading(week(3), of: snapshot).notBefore == now)
        // A probe's answer came after its launch.
        let answer = UsageStore.interpretProbeAnswer(ParsedUsage(sevenDay: week(3)), accountId: "a", now: now, cachedCopy: nil,
                                                     launchedAt: now.addingTimeInterval(-4))
        #expect(answer.snapshot?.takenAfter == now.addingTimeInterval(-4))
        // A status line that arrived while the probe ran is not known older than it.
        let during = reading(week(62), at: -2, after: -30)
        let probe = UsageStore.Reading(week(3), of: answer.snapshot!)
        #expect(!UsageStore.supersedes(probe, during))
        #expect(UsageStore.supersedes(probe, reading(week(62), at: -5)))
    }

    @Test func cachePollKeepsTheProbesVerdict() {
        let probeSaysSignedOut = UsageStore.notSignedIn
        // Signed out per .claude.json: always shown.
        #expect(UsageStore.fetchStateAfterCachePoll(current: .idle, wasSignedIn: true, isSignedIn: false) == UsageStore.notSignedIn)
        // Signed back in: cleared.
        #expect(UsageStore.fetchStateAfterCachePoll(current: UsageStore.notSignedIn, wasSignedIn: false, isSignedIn: true) == .idle)
        #expect(UsageStore.fetchStateAfterCachePoll(current: UsageStore.notSignedIn, wasSignedIn: nil, isSignedIn: true) == .idle)
        // Still signed in per the file, but the probe found the login dead: keep that.
        #expect(UsageStore.fetchStateAfterCachePoll(current: probeSaysSignedOut, wasSignedIn: true, isSignedIn: true) == probeSaysSignedOut)
        #expect(UsageStore.fetchStateAfterCachePoll(current: .failed("x"), wasSignedIn: true, isSignedIn: true) == .failed("x"))
        #expect(UsageStore.fetchStateAfterCachePoll(current: nil, wasSignedIn: nil, isSignedIn: true) == nil)
    }

    @Test func backoffDoublesAndCaps() {
        #expect(UsageStore.backoff(afterFailures: 0) == 0)
        #expect(UsageStore.backoff(afterFailures: 1) == 120)
        #expect(UsageStore.backoff(afterFailures: 2) == 240)
        #expect(UsageStore.backoff(afterFailures: 4) == 960)
        #expect(UsageStore.backoff(afterFailures: 5) == 1800)
        #expect(UsageStore.backoff(afterFailures: 50) == 1800)
    }

    @Test func probeIntervalHasAFloor() {
        #expect(UsageStore.effectiveProbeInterval(minutes: 0) == nil)
        #expect(UsageStore.effectiveProbeInterval(minutes: 1) == 300)
        #expect(UsageStore.effectiveProbeInterval(minutes: 5) == 300)
        #expect(UsageStore.effectiveProbeInterval(minutes: 30) == 1800)
    }
}

/// Asks the real Claude Code for the default account's usage. Off unless
/// `AGENTNOTCH_LIVE_PROBE=1`: it runs `claude -p` (read-only: no session is saved and
/// hooks are disabled). Prints only percentages and reset times.
@Suite(.enabled(if: Foundation.ProcessInfo.processInfo.environment["AGENTNOTCH_LIVE_PROBE"] == "1"))
struct LiveUsageProbeTests {
    @Test func probeTheDefaultAccount() async throws {
        let claude = try #require(
            ClaudeBinaryLocator.fixedCandidates().first { FileManager.default.isExecutableFile(atPath: $0) },
            "claude not found in the usual places"
        )
        let start = Date()
        let outcome = await UsageProbe.run(
            claudePath: claude,
            configDirEnv: nil,
            workingDirectory: FileManager.default.temporaryDirectory.appendingPathComponent("agentnotch-live-probe-cwd")
        )
        let seconds = String(format: "%.1f", Date().timeIntervalSince(start))
        guard case .usage(let usage) = outcome else {
            Issue.record("live probe did not return usage: \(outcome) after \(seconds)s")
            return
        }
        func describe(_ name: String, _ window: UsageWindow?) -> String {
            guard let window else { return "\(name): none" }
            return "\(name): \(window.utilization)% resets \(window.resetsAt.map { $0.formatted(.iso8601) } ?? "?")"
        }
        print("[live probe] \(seconds)s plan=\(usage.subscriptionType ?? "?")")
        print("[live probe] " + describe("5h", usage.fiveHour))
        print("[live probe] " + describe("7d", usage.sevenDay))
        for scoped in usage.scoped {
            print("[live probe] " + describe("scoped \(scoped.name)", scoped.window))
        }
        if let extra = usage.extraUsage {
            print("[live probe] extra usage enabled=\(extra.isEnabled) utilization=\(extra.utilization.map { "\($0)%" } ?? "n/a")")
        }
        #expect(usage.fiveHour != nil || usage.sevenDay != nil)
    }
}
