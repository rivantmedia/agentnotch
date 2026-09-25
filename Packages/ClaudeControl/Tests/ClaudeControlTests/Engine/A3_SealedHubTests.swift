import Foundation
import Testing
@testable import ClaudeControl

/// A sealed hub shows the fixtures (every attention state on two accounts)
/// and touches nothing: no socket, no probe, no terminal query, nothing
/// written. (The sealed app run checks the whole process: open files,
/// sockets and child processes; see Scripts/spm-run-sealed.sh.)
@MainActor
@Suite(.serialized)
struct A3_SealedHubTests {
    /// Start from an empty store: the store and the monitor are shared, and
    /// a hub's baseline must not be another test's leftovers.
    private func clearSessions() async throws {
        await SessionStore.shared.replaceAllWithFixtures([])
        for _ in 0..<300 where !ClaudeSessionMonitor.shared.instances.isEmpty {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(ClaudeSessionMonitor.shared.instances.isEmpty)
    }

    @Test func sealedHubShowsEveryStateAndTouchesNothing() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("spcn-sealed-test-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        var configuration = ClaudeControlConfiguration.sealed(appDisplayName: "Test", bundleIdentifier: "com.example.sealed-test")
        configuration.supportDirectory = root.appendingPathComponent("Claude", isDirectory: true)
        configuration.socketPath = root.appendingPathComponent("Claude/hook.sock").path
        // The `~` the fixture accounts are written against; nothing under it is read.
        configuration.homeDirectory = AppIdentity.homeDirectory
        #expect(configuration.mode == .sealed && !configuration.probesAllowed && !configuration.installsAllowed)

        let hub = ClaudeControlHub(configuration: configuration)
        var transitions: [ClaudeAttentionTransition] = []
        let subscription = hub.transitions.sink { transitions.append($0) }
        defer { subscription.cancel() }

        try await clearSessions()
        hub.start()
        defer { hub.stop() }
        let expected = SampleSessions.all().count
        for _ in 0..<300 where hub.sessions.count < expected {
            try await Task.sleep(for: .milliseconds(10))
        }

        // Two accounts, as rings.
        #expect(hub.accounts.map(\.ringID) == [Rings.personal, Rings.work])
        #expect(hub.accounts.map(\.label) == ["Personal", "Work"])

        // Every attention state.
        #expect(hub.sessions.count == expected)
        var kinds: Set<String> = []
        for session in hub.sessions {
            switch session.attention {
            case .needsInput(let input): kinds.insert("\(input.kind)")
            case .working: kinds.insert("working")
            case .readyForReview: kinds.insert("review")
            case .idle: kinds.insert("idle")
            }
            #expect(session.hostApp != nil, "\(session.id)")
        }
        #expect(kinds == ["permission", "question", "plan", "elicitation", "dialog", "error", "working", "review", "idle"])

        // Counts per ring and in total.
        let total = hub.totalCounts
        // The rate-limited one is failed, not needs-you (GUX-2).
        #expect(total.needsYou == 5 && total.failed == 1 && total.review == 3 && total.working == 3 && total.idle == 4)
        #expect((hub.ringCounts[Rings.personal]?.needsYou ?? 0) + (hub.ringCounts[Rings.work]?.needsYou ?? 0) == 5)
        #expect(hub.isBusy)

        // Readings: both rings drawn; the work ring's session window is used up.
        for ring in [Rings.personal, Rings.work] {
            let reading = try #require(hub.ringReadings[ring])
            #expect(reading.status == .ok)
            #expect(reading.windows.first?.id == "session")
        }
        #expect(hub.ringReadings[Rings.work]?.exhaustedWindow(now: Date())?.id == "session")

        // The rate-limited session says when it can go on.
        let rows = hub.activityRows(ringID: Rings.work, now: Date())
        #expect(rows.contains { $0.detail.hasPrefix("Stopped · Rate limited (5-hour) · resets ") && $0.state == .idle })
        #expect(rows.allSatisfy { $0.pid != nil })

        // The just-finished review keeps its ring's arc pulsing for 90 s.
        let fresh = try #require(hub.freshSuccessUntil[Rings.personal])
        #expect(fresh > Date() && fresh <= Date().addingTimeInterval(ClaudeControlHub.freshSuccessWindow))

        // Hidden rings get no rows.
        hub.setShownRings([Rings.personal])
        #expect(hub.activityRows(ringID: Rings.work, now: Date()).isEmpty)
        #expect(!hub.activityRows(ringID: Rings.personal, now: Date()).isEmpty)

        // Reviewing works on fixtures, and is announced as resolved.
        let review = try #require(hub.sessions.first { $0.id == "review-darkmode" })
        #expect(await hub.focus(sessionId: review.id))
        for _ in 0..<300 where hub.sessions.first(where: { $0.id == review.id })?.attention == .readyForReview {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(hub.sessions.first { $0.id == review.id }?.attention == .idle)
        #expect(transitions.map(\.kind) == [.resolved])
        #expect(transitions.first?.session.id == review.id)

        // Nothing real was touched.
        #expect(!FileManager.default.fileExists(atPath: configuration.socketPath))
        let written = (try? FileManager.default.contentsOfDirectory(atPath: configuration.supportDirectory.path)) ?? []
        #expect(written.isEmpty, "\(written)")
        #expect(await !hub.isTerminalFocused(sessionId: review.id))
        #expect(await !hub.isAnyTerminalVisible())
        await hub.refreshUsage(ringID: Rings.personal, reason: .forced)
        #expect(UsageStore.shared.lastProbeAt.isEmpty && !UsageStore.shared.isProbing)
    }

    /// A sealed configuration in a temporary support folder, against the
    /// `~` the fixture accounts are written with (nothing under it is read).
    private func sealedConfiguration(root: URL) -> ClaudeControlConfiguration {
        var configuration = ClaudeControlConfiguration.sealed(appDisplayName: "Test", bundleIdentifier: "com.example.sealed-test")
        configuration.supportDirectory = root.appendingPathComponent("Claude", isDirectory: true)
        configuration.socketPath = root.appendingPathComponent("Claude/hook.sock").path
        configuration.homeDirectory = AppIdentity.homeDirectory
        return configuration
    }

    private func waitForSessions(_ hub: ClaudeControlHub, count: Int) async throws {
        for _ in 0..<500 where hub.sessions.count != count {
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    /// A third account signs in while running (the sealed demo does this a
    /// few seconds in, through the same registry and store): its ring, its
    /// sessions and its reading arrive with no restart, and nothing else
    /// changes.
    @Test func aThirdAccountArrivesLater() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("spcn-sealed-test-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let configuration = sealedConfiguration(root: root)

        let hub = ClaudeControlHub(configuration: configuration)
        var transitions: [ClaudeAttentionTransition] = []
        let subscription = hub.transitions.sink { transitions.append($0) }
        defer { subscription.cancel() }
        #expect(hub.launchAccounts().map(\.ringID) == [Rings.personal, Rings.work])

        try await clearSessions()
        hub.start()
        defer { hub.stop() }
        let before = SampleSessions.all().count
        try await waitForSessions(hub, count: before)
        #expect(Set(hub.accounts.map(\.ringID)) == [Rings.personal, Rings.work])
        #expect(hub.ringReadings[Rings.side] == nil)
        let countsBefore = hub.ringCounts

        AccountRegistry.shared.replaceAllWithFixtures(AccountRegistry.shared.accounts + [SampleSessions.side])
        let arriving = SampleSessions.sideProject().map { ClaudeControlHub.freshened($0, to: Date()) }
        await SessionStore.shared.replaceAllWithFixtures(ClaudeSessionMonitor.shared.instances + arriving)
        let after = before + arriving.count
        try await waitForSessions(hub, count: after)

        #expect(Set(hub.accounts.map(\.ringID)) == [Rings.personal, Rings.work, Rings.side])
        #expect(hub.accounts.first { $0.ringID == Rings.side }?.label == "Side project")
        #expect(hub.sessions.count == after)
        #expect(hub.ringCounts[Rings.side] == ClaudeAttentionCounts(needsYou: 0, review: 1, working: 1, idle: 0))
        #expect(hub.ringCounts[Rings.personal] == countsBefore[Rings.personal])
        #expect(hub.ringCounts[Rings.work] == countsBefore[Rings.work])
        #expect(hub.ringReadings[Rings.side]?.status == .ok)
        // Its review finished two minutes ago: that ring's arc has settled.
        #expect(hub.freshSuccessUntil[Rings.side] == nil)
        #expect(hub.activityRows(ringID: Rings.side, now: Date()).count == 2)
        // New sessions are recorded, not announced.
        #expect(transitions.isEmpty)
        let written = (try? FileManager.default.contentsOfDirectory(atPath: configuration.supportDirectory.path)) ?? []
        #expect(written.isEmpty, "\(written)")
    }

    /// What changes while the hub is stopped is not announced when it starts
    /// again: stopping drops the baseline, so the next start takes a new,
    /// silent one.
    @Test func stoppingDropsTheTransitionBaseline() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("spcn-sealed-test-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let hub = ClaudeControlHub(configuration: sealedConfiguration(root: root))
        #expect(hub.previousAttention == nil)
        try await clearSessions()
        hub.start()
        try await waitForSessions(hub, count: SampleSessions.all().count)
        #expect(hub.previousAttention?.count == SampleSessions.all().count)
        hub.stop()
        #expect(hub.previousAttention == nil)
    }
}

/// The fixture accounts' ring ids (one per identity, whatever folders).
private enum Rings {
    static let personal = ClaudeRingIdentity.ringID(accountKey: SampleLayout.personalUUID)
    static let work = ClaudeRingIdentity.ringID(accountKey: SampleLayout.workUUID)
    static let side = ClaudeRingIdentity.ringID(accountKey: SampleLayout.sideUUID)
}
