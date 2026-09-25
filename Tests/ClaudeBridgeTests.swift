import ClaudeControl
import XCTest
@testable import Codenotch

/// The fork's notch bridge (Sources/ClaudeBridge) against Codenotch's own
/// types: the store's runtime provider swap and pushed readings (U6), and the
/// field copies from ClaudeControl's summaries into `AgentSession` and
/// `ProviderSnapshot`. The pure rules the bridge relies on (badge layout,
/// attention policy) are tested in the ClaudeControl package; the badge
/// check is repeated here against the real `NotchLayout`.
@MainActor
final class ClaudeBridgeTests: XCTestCase {
    // MARK: - Fixtures

    /// A provider that counts how often it is fetched.
    private actor CountingProvider: UsageProvider {
        nonisolated let id: String
        nonisolated let displayName: String
        nonisolated let glyph: ProviderGlyph = .claude
        private(set) var fetches = 0

        init(id: String) {
            self.id = id
            self.displayName = id
        }

        func fetchSnapshot() async throws -> ProviderSnapshot {
            fetches += 1
            return ClaudeBridgeTests.snapshot(id, used: 0.5)
        }

        nonisolated func account() -> ProviderAccount? { nil }
        nonisolated var signInRoute: SignInRoute { .guidance("") }
        func signOut() async {}
        nonisolated func presentSignIn() {}
        nonisolated func forgetCachedCredential() {}
    }

    /// A provider whose fetch does not answer until released: a pass in flight.
    private actor SlowProvider: UsageProvider {
        nonisolated let id: String
        nonisolated let displayName: String
        nonisolated let glyph: ProviderGlyph = .claude
        private var released = false
        private var waiting: [CheckedContinuation<Void, Never>] = []

        init(id: String) {
            self.id = id
            self.displayName = id
        }

        func fetchSnapshot() async throws -> ProviderSnapshot {
            if !released { await withCheckedContinuation { waiting.append($0) } }
            return ClaudeBridgeTests.snapshot(id, used: 0.1)
        }

        func release() {
            released = true
            waiting.forEach { $0.resume() }
            waiting.removeAll()
        }

        nonisolated func account() -> ProviderAccount? { nil }
        nonisolated var signInRoute: SignInRoute { .guidance("") }
        func signOut() async {}
        nonisolated func presentSignIn() {}
        nonisolated func forgetCachedCredential() {}
    }

    nonisolated static func snapshot(_ id: String, used: Double) -> ProviderSnapshot {
        ProviderSnapshot(id: id, displayName: id, glyph: .claude, fidelity: .official, status: .ok,
                         windows: [LimitWindow(id: "session", label: "Current session", usedFraction: used)],
                         headlineID: "session")
    }

    private func isolatedArchive() -> UsageArchive {
        let name = "ClaudeBridgeTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: name)!
        addTeardownBlock { defaults.removePersistentDomain(forName: name) }
        return UsageArchive(defaults: defaults)
    }

    private func isolatedPreferences() -> Preferences {
        let name = "ClaudeBridgeTests.prefs.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: name)!
        addTeardownBlock { defaults.removePersistentDomain(forName: name) }
        return Preferences(defaults: defaults)
    }

    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    // MARK: - U6: replaceProviders

    func testReplacingProvidersAddsAndRemovesRingsWithoutFetching() async {
        let personal = CountingProvider(id: "claude")
        let other = CountingProvider(id: "openai")
        let store = UsageStore(providers: [personal, other], archive: isolatedArchive())
        XCTAssertEqual(store.snapshots.map(\.id), ["claude", "openai"])
        let revision = store.providerAccountRevision

        let work = CountingProvider(id: "claude-work")
        store.replaceProviders(where: ClaudeBridge.ownsProvider, with: [personal, work])
        // The newcomer shows at once, as a placeholder, where the Claude
        // block sits, and nothing is fetched or pressed.
        XCTAssertEqual(store.snapshots.map(\.id), ["claude", "claude-work", "openai"])
        XCTAssertEqual(store.snapshots.first { $0.id == "claude-work" }?.windows.isEmpty, true)
        XCTAssertTrue(store.refreshing.isEmpty)
        XCTAssertTrue(store.inFlightForTesting.isEmpty)
        XCTAssertGreaterThan(store.providerAccountRevision, revision)
        let fetches = await work.fetches + personal.fetches + other.fetches
        XCTAssertEqual(fetches, 0)

        // A ring that goes away takes its reading with it.
        store.ingest(Self.snapshot("claude", used: 0.3))
        store.replaceProviders(where: ClaudeBridge.ownsProvider, with: [work])
        XCTAssertEqual(store.snapshots.map(\.id), ["claude-work", "openai"])
        XCTAssertFalse(store.knownIDs.contains("claude"))
    }

    func testAReturningRingShowsOnlyAReadingItStillHas() {
        let work = CountingProvider(id: "claude-work")
        let store = UsageStore(providers: [work], archive: isolatedArchive())
        store.ingest(Self.snapshot("claude-work", used: 0.42))
        // Gone, then back: the reading went with it.
        store.replaceProviders(where: ClaudeBridge.ownsProvider, with: [])
        store.replaceProviders(where: ClaudeBridge.ownsProvider, with: [work])
        XCTAssertEqual(store.snapshots.first?.windows.isEmpty, true)

        // A reading an earlier launch archived comes back, dated, when its
        // ring arrives after launch.
        let archive = isolatedArchive()
        UsageStore(providers: [work], archive: archive).ingest(Self.snapshot("claude-work", used: 0.42))
        let relaunched = UsageStore(providers: [], archive: archive)
        relaunched.replaceProviders(where: ClaudeBridge.ownsProvider, with: [work])
        let shown = relaunched.snapshots.first { $0.id == "claude-work" }
        XCTAssertEqual(shown?.headline?.usedFraction, 0.42)
        XCTAssertTrue(shown?.status.isStale ?? false)
    }

    // MARK: - U6: ingest

    func testIngestPublishesWithoutPressingTheRing() async {
        let work = CountingProvider(id: "claude-work")
        let store = UsageStore(providers: [work], archive: isolatedArchive())
        store.ingest(Self.snapshot("claude-work", used: 0.7))
        XCTAssertEqual(store.snapshots.first?.headline?.usedFraction, 0.7)
        XCTAssertTrue(store.refreshing.isEmpty, "a pushed reading must not spin the ring")
        XCTAssertTrue(store.inFlightForTesting.isEmpty)
        XCTAssertFalse(store.isRefreshingForTesting)
        let fetches = await work.fetches
        XCTAssertEqual(fetches, 0)
    }

    func testIngestIgnoresUnknownAndSwitchedOffRings() {
        let work = CountingProvider(id: "claude-work")
        let store = UsageStore(providers: [work], archive: isolatedArchive())
        store.ingest(Self.snapshot("claude-elsewhere", used: 0.9))
        XCTAssertFalse(store.snapshots.contains { $0.id == "claude-elsewhere" })
        store.disconnected = ["claude-work"]
        store.ingest(Self.snapshot("claude-work", used: 0.9))
        XCTAssertTrue(store.snapshots.isEmpty)
    }

    func testIngestKeepsNicknames() {
        let work = CountingProvider(id: "claude-work")
        let store = UsageStore(providers: [work], archive: isolatedArchive())
        store.nicknames = ["claude-work": "Day job"]
        store.ingest(Self.snapshot("claude-work", used: 0.2))
        XCTAssertEqual(store.snapshots.first?.displayName, "Day job")
    }

    // MARK: - Rings coming and going (ClaudeProviderSync)

    /// A ring the user had switched off comes back (its folder reappears, or
    /// its account is tracked again). The store refetches every provider when
    /// a ring it holds turns on or off; an account arriving must not look
    /// like that.
    func testAReturningSwitchedOffRingRefetchesNothing() async {
        let preferences = isolatedPreferences()
        preferences.reconcile(discoveredIDs: ["claude", "openai", "claude-old"])
        preferences.setConnected(true, for: "openai")
        preferences.setConnected(false, for: "claude-old")
        let personal = CountingProvider(id: "claude")
        let other = CountingProvider(id: "openai")
        let store = UsageStore(providers: [personal, other], archive: isolatedArchive(),
                               disconnected: preferences.disconnectedIDs(among: ["claude", "openai"]))

        let old = CountingProvider(id: "claude-old")
        ClaudeProviderSync.install([personal, old], in: store, preferences: preferences, writesPreferences: true)
        XCTAssertTrue(store.disconnected.contains("claude-old"), "it stays off")
        XCTAssertFalse(store.snapshots.contains { $0.id == "claude-old" })
        XCTAssertFalse(store.isRefreshingForTesting, "an account arriving must not refetch every provider")
        XCTAssertTrue(store.refreshing.isEmpty)
        XCTAssertTrue(store.inFlightForTesting.isEmpty)

        // Going again: nothing refetched, and the off-list lets it go.
        ClaudeProviderSync.install([personal], in: store, preferences: preferences, writesPreferences: true)
        XCTAssertFalse(store.isRefreshingForTesting)
        XCTAssertFalse(store.disconnected.contains("claude-old"))
        XCTAssertFalse(preferences.isConnected("claude-old"), "the user's choice is kept for next time")

        let fetches = await personal.fetches + other.fetches + old.fetches
        XCTAssertEqual(fetches, 0)
    }

    /// A ring nobody has seen comes up switched on, as a placeholder, unfetched.
    func testANewRingComesUpSwitchedOnWithoutAFetch() async {
        let preferences = isolatedPreferences()
        preferences.reconcile(discoveredIDs: ["claude"])
        let personal = CountingProvider(id: "claude")
        let store = UsageStore(providers: [personal], archive: isolatedArchive(),
                               disconnected: preferences.disconnectedIDs(among: ["claude"]))

        let work = CountingProvider(id: "claude-work")
        ClaudeProviderSync.install([personal, work], in: store, preferences: preferences, writesPreferences: true)
        XCTAssertTrue(preferences.isConnected("claude-work"))
        XCTAssertTrue(store.disconnected.isEmpty)
        XCTAssertEqual(store.snapshots.map(\.id), ["claude", "claude-work"])
        XCTAssertFalse(store.isRefreshingForTesting)
        let fetches = await personal.fetches + work.fetches
        XCTAssertEqual(fetches, 0)
    }

    /// A run that may not write Codenotch's connected set (a demo run of the
    /// real app): the newcomer stays off, and still nothing is refetched.
    func testANewRingInARunThatMayNotWritePreferencesStaysOffQuietly() {
        let preferences = isolatedPreferences()
        preferences.reconcile(discoveredIDs: ["claude"])
        let personal = CountingProvider(id: "claude")
        let store = UsageStore(providers: [personal], archive: isolatedArchive(),
                               disconnected: preferences.disconnectedIDs(among: ["claude"]))
        ClaudeProviderSync.install([personal, CountingProvider(id: "claude-work")], in: store,
                                   preferences: preferences, writesPreferences: false)
        XCTAssertFalse(preferences.isConnected("claude-work"))
        XCTAssertEqual(store.disconnected, ["claude-work"])
        XCTAssertEqual(store.snapshots.map(\.id), ["claude"])
        XCTAssertFalse(store.isRefreshingForTesting)
    }

    /// Rings made at launch stay while the hub has not listed their accounts
    /// yet (removing one deletes its archived reading); once listed, a ring
    /// the hub drops goes.
    func testLaunchRingsWaitForTheHubToListThem() {
        XCTAssertEqual(ClaudeProviderSync.keptRings(listed: [], current: ["claude", "claude-work"],
                                                    awaitingHub: ["claude", "claude-work"]),
                       ["claude", "claude-work"], "a hub that has not loaded its accounts takes nothing away")
        XCTAssertEqual(ClaudeProviderSync.keptRings(listed: ["claude"], current: ["claude", "claude-work"],
                                                    awaitingHub: ["claude-work"]),
                       ["claude", "claude-work"])
        XCTAssertEqual(ClaudeProviderSync.keptRings(listed: ["claude-new"], current: ["claude"], awaitingHub: ["claude"]),
                       ["claude-new", "claude"])
        XCTAssertEqual(ClaudeProviderSync.keptRings(listed: ["claude-work"], current: ["claude", "claude-work"],
                                                    awaitingHub: []),
                       ["claude-work"], "listed once, a ring the hub drops is gone")
        XCTAssertEqual(ClaudeProviderSync.keptRings(listed: [], current: [], awaitingHub: []), [])
    }

    // MARK: - What the notch counts (ClaudeNotchState)

    /// Badges, the folded pill and the hold count what the rows show: nothing
    /// for a ring switched off, and an unknown account's sessions on the
    /// default ring.
    func testTheNotchCountsOnlyWhatItShows() {
        let state = ClaudeNotchState()
        state.show(ringCounts: ["claude": .init(needsYou: 1), "claude-off": .init(needsYou: 3),
                                "claude-dir-1a2b3c4d": .init(review: 1)],
                   freshSuccessUntil: ["claude-dir-1a2b3c4d": now])
        // Nothing routed yet: the hub's numbers as they are.
        XCTAssertEqual(state.totalCounts, .init(needsYou: 4, review: 1))
        XCTAssertTrue(state.isShown("claude-off"))

        state.route(rings: ["claude", "claude-off"], shown: ["claude", "claude-dir-1a2b3c4d"])
        XCTAssertEqual(state.ringCounts, ["claude": .init(needsYou: 1, review: 1)])
        XCTAssertEqual(state.totalCounts, .init(needsYou: 1, review: 1))
        XCTAssertEqual(state.freshSuccessUntil, ["claude": now])
        XCTAssertFalse(state.settles("claude", now: now.addingTimeInterval(-1)))
        XCTAssertFalse(state.isShown("claude-off"))
        XCTAssertTrue(state.isShown("claude-dir-1a2b3c4d"))
    }

    // MARK: - Field copies

    func testActivityRowsCopyIntoAgentSessionsFieldForField() {
        let since = now.addingTimeInterval(-90)
        let states: [(ClaudeActivityRow.State, AgentSession.State)] = [
            (.busy, .busy), (.waiting, .waiting), (.success, .success), (.idle, .idle),
        ]
        for (state, expected) in states {
            let row = ClaudeActivityRow(id: "s1", name: "Fix the build", detail: "3/7 · Running tests · ctx 42%",
                                        state: state, waitingFor: state == .waiting ? "Allow Bash · npm test" : nil,
                                        since: since, pid: 4242)
            let session = ClaudeSessionFeed.agentSession(row)
            XCTAssertEqual(session.id, "s1")
            XCTAssertEqual(session.name, "Fix the build")
            XCTAssertEqual(session.detail, "3/7 · Running tests · ctx 42%")
            XCTAssertEqual(session.state, expected)
            XCTAssertEqual(session.waitingFor, row.waitingFor)
            XCTAssertEqual(session.since, since)
            XCTAssertEqual(session.processID, 4242)
        }
    }

    func testAReadingCopiesIntoASnapshot() throws {
        let reading = ClaudeRingReading(
            windows: [
                .init(id: "weekly_opus", usedFraction: 0.38),
                .init(id: "weekly_all", usedFraction: 0.64, resetsAt: now.addingTimeInterval(3_600), duration: 604_800),
                .init(id: "session", usedFraction: 0.92, resetsAt: now.addingTimeInterval(600), duration: 18_000),
                .init(id: "extra_usage", label: "Extra usage", usedFraction: 0.1,
                      money: .init(currency: "USD", spent: 5, remaining: 45)),
            ],
            plan: "Max", updatedAt: now.addingTimeInterval(-60), status: .ok)
        let snapshot = try ClaudeUsageProvider.snapshot(ringID: "claude-work", displayName: "Work",
                                                        reading: reading, now: now)
        XCTAssertEqual(snapshot.id, "claude-work")
        XCTAssertEqual(snapshot.displayName, "Work")
        XCTAssertEqual(snapshot.glyph, .claude)
        XCTAssertEqual(snapshot.fidelity, .official)
        XCTAssertEqual(snapshot.status, .ok)
        XCTAssertEqual(snapshot.headlineID, "session")
        XCTAssertEqual(snapshot.weeklyID, "weekly_all")
        XCTAssertEqual(snapshot.plan, "Max")
        // Codenotch's order and its own labels.
        XCTAssertEqual(snapshot.windows.map(\.id), ["session", "weekly_all", "extra_usage", "weekly_opus"])
        XCTAssertEqual(snapshot.windows.map(\.label), ["Current session", "All models", "Extra usage", "Opus"])
        XCTAssertEqual(snapshot.headline?.usedFraction, 0.92)
        XCTAssertEqual(snapshot.weeklyFraction, 0.64)
        XCTAssertEqual(snapshot.windows[0].resetsAt, now.addingTimeInterval(600))
        XCTAssertEqual(snapshot.windows[0].duration, 18_000)
        XCTAssertEqual(snapshot.windows[2].money, UsageMoneyBreakdown(currency: "USD", spent: 5, remaining: 45))
    }

    func testReadingStatusesMapToSnapshotStatuses() throws {
        let window = ClaudeRingReading.Window(id: "session", usedFraction: 0.5)
        func snapshot(_ reading: ClaudeRingReading) throws -> ProviderSnapshot {
            try ClaudeUsageProvider.snapshot(ringID: "claude", displayName: "Claude", reading: reading, now: now)
        }
        // Fifteen minutes old or less: ok. Older: stale since then.
        XCTAssertEqual(try snapshot(.init(windows: [window], updatedAt: now.addingTimeInterval(-900), status: .ok)).status, .ok)
        let old = now.addingTimeInterval(-901)
        XCTAssertEqual(try snapshot(.init(windows: [window], updatedAt: old, status: .ok)).status, .stale(since: old))
        // The reading's own threshold (scaled with the probe interval) wins,
        // and a used-up window is never stale before its reset.
        var slow = ClaudeRingReading(windows: [window], updatedAt: old, status: .ok)
        slow.staleThreshold = 3600
        XCTAssertEqual(try snapshot(slow).status, .ok)
        let spent = ClaudeRingReading.Window(id: "session", usedFraction: 1, resetsAt: now.addingTimeInterval(600))
        XCTAssertEqual(try snapshot(.init(windows: [spent], updatedAt: old, status: .ok)).status, .ok)
        // Waiting: nothing to draw, and no reading date.
        let waiting = try snapshot(.init(windows: [window], status: .waitingForFirstReading))
        XCTAssertTrue(waiting.windows.isEmpty)
        XCTAssertEqual(waiting.status, .stale(since: .distantPast))
        // Signed out or unavailable: said in the card.
        XCTAssertEqual(try snapshot(.init(status: .signInNeeded("Run claude, then /login."))).status,
                       .unsupported("Run claude, then /login."))
        XCTAssertEqual(try snapshot(.init(status: .unavailable("No usage for this plan"))).status,
                       .unsupported("No usage for this plan"))
        // Failed with nothing to show: the store's failure path.
        XCTAssertThrowsError(try snapshot(.init(status: .failed("rate limited")))) { error in
            guard case UsageProviderError.apiError(let text) = error else { return XCTFail("\(error)") }
            XCTAssertEqual(text, "rate limited")
        }
        // Failed with a reading: the reading, aged like any other.
        XCTAssertEqual(try snapshot(.init(windows: [window], updatedAt: now, status: .failed("x"))).status, .ok)
        // Nothing is pushed for a ring still waiting.
        XCTAssertNil(ClaudeUsageProvider.pushed(ringID: "claude", displayName: "Claude",
                                                reading: .init(status: .waitingForFirstReading), now: now))
    }

    // MARK: - Rules the bridge keeps

    func testOnlyAChangeTheRingShowsIsPushed() {
        let base = Self.snapshot("claude", used: 0.5)
        XCTAssertTrue(ClaudeProviderSync.changesRing(from: nil, to: base))
        XCTAssertFalse(ClaudeProviderSync.changesRing(from: base, to: base))
        XCTAssertTrue(ClaudeProviderSync.changesRing(from: base, to: Self.snapshot("claude", used: 0.6)))
        var stale = base
        stale.status = .stale(since: now)
        XCTAssertTrue(ClaudeProviderSync.changesRing(from: base, to: stale))
        var planned = base
        planned.plan = "Pro"
        XCTAssertTrue(ClaudeProviderSync.changesRing(from: base, to: planned))
    }

    /// Switched off, a ring forgets what it was pushed (the store dropped the
    /// reading); back on, it is pushed at once rather than left on a
    /// placeholder until the next poll whenever the store skips its refetch.
    func testARingSwitchedBackOnIsPushedAgain() {
        let rings = ["claude", "claude-work", "claude-side"]
        let off = ClaudeProviderSync.connectionPlan(rings: rings, disconnected: ["claude-work"],
                                                    pushed: ["claude", "claude-work"])
        XCTAssertEqual(off.forget, ["claude-work"])
        XCTAssertEqual(off.push, ["claude-side"], "only a ring that is on and was never pushed")
        let backOn = ClaudeProviderSync.connectionPlan(rings: rings, disconnected: [], pushed: ["claude", "claude-side"])
        XCTAssertEqual(backOn.forget, [])
        XCTAssertEqual(backOn.push, ["claude-work"])
        let settled = ClaudeProviderSync.connectionPlan(rings: rings, disconnected: [], pushed: Set(rings))
        XCTAssertTrue(settled.forget.isEmpty && settled.push.isEmpty)
    }

    /// The store skips a refetch while a pass is in flight, which is why the
    /// sync has to push a ring that comes back on itself: the store alone
    /// leaves it on its placeholder.
    func testAStoreBusyWithAPassLeavesARingSwitchedBackOnEmpty() async {
        let work = CountingProvider(id: "claude-work")
        let slow = SlowProvider(id: "openai")
        let store = UsageStore(providers: [work, slow], archive: isolatedArchive())
        store.ingest(Self.snapshot("claude-work", used: 0.4))
        store.refreshNow()
        XCTAssertTrue(store.isRefreshingForTesting)
        store.disconnected = ["claude-work"]
        store.disconnected = []
        XCTAssertEqual(store.snapshots.first { $0.id == "claude-work" }?.windows.isEmpty, true,
                       "a placeholder until something delivers the reading")
        store.ingest(Self.snapshot("claude-work", used: 0.4))
        XCTAssertEqual(store.snapshots.first { $0.id == "claude-work" }?.headline?.usedFraction, 0.4)
        await slow.release()
    }

    func testOneRingPerTrackedAccount() {
        func account(_ ring: String, tracked: Bool = true) -> ClaudeAccountSummary {
            ClaudeAccountSummary(id: "/h/.\(ring)", ringID: ring, configDir: "/h/.\(ring)", label: ring,
                                 isDefault: ring == "claude", isTracked: tracked, launchCommand: "claude")
        }
        XCTAssertEqual(ClaudeProviderSync.ringIDs([account("claude"), account("claude-work"), account("claude"),
                                                   account("claude-old", tracked: false)]),
                       ["claude", "claude-work"])
    }

    func testTheFeedFoldsUnknownAccountsIntoTheDefaultRingAndEmptiesTheRest() {
        let row = { (ring: String) in
            [ClaudeActivityRow(id: "s-\(ring)", name: ring, detail: "", state: .busy, since: Date(timeIntervalSince1970: 0))]
        }
        let rows = ClaudeSessionFeed.rows(
            rings: ["claude", "claude-work", "claude-off"],
            sessionRings: ["claude", "claude-work", "claude-dir-1234abcd", "claude-work"],
            previous: ["claude-gone"],
            isShown: { $0 != "claude-off" },
            rows: row)
        XCTAssertEqual(rows["claude"]?.map(\.id), ["s-claude", "s-claude-dir-1234abcd"])
        XCTAssertEqual(rows["claude-work"]?.map(\.id), ["s-claude-work"])
        XCTAssertEqual(rows["claude-off"]?.isEmpty, true, "a ring switched off shows no sessions")
        XCTAssertEqual(rows["claude-gone"]?.isEmpty, true, "a ring that went away is emptied")
        XCTAssertNil(rows["claude-dir-1234abcd"])

        XCTAssertEqual(ClaudeSessionFeed.shownRings(rings: ["claude", "claude-off"],
                                                    sessionRings: ["claude-dir-1234abcd"],
                                                    isShown: { $0 != "claude-off" }),
                       ["claude", "claude-dir-1234abcd"])
        XCTAssertEqual(ClaudeSessionFeed.shownRings(rings: ["claude", "claude-work"],
                                                    sessionRings: ["claude-dir-1234abcd"],
                                                    isShown: { $0 != "claude" }),
                       ["claude-work"], "with the default ring off, sessions of unknown accounts show nowhere")
    }

    func testRingNamesFollowCodenotchsRule() {
        func account(_ ring: String, email: String?, dir: String) -> ClaudeAccountSummary {
            ClaudeAccountSummary(id: dir, ringID: ring, configDir: dir, label: ring, email: email,
                                 isDefault: ring == "claude", launchCommand: "claude")
        }
        let names = ClaudeRingNames.displayNames(for: [
            account("claude", email: "me@gmail.com", dir: "/h/.claude"),
            account("claude-work", email: "me@company.com", dir: "/h/.claude-work"),
            account("claude-side", email: nil, dir: "/h/.claude-side"),
            account("claude-dir-1a2b3c4d", email: nil, dir: "/Volumes/x/claude-home"),
        ])
        XCTAssertEqual(names["claude"], "Claude \(ClaudeProfile.accountLabel(forAddress: "me@gmail.com") ?? "")")
        XCTAssertEqual(names["claude-side"], "Claude (side)")
        XCTAssertEqual(names["claude-dir-1a2b3c4d"], "Claude (claude-home)")
        // Two addresses that would share a name get the whole address.
        let twins = ClaudeRingNames.displayNames(for: [
            account("claude", email: "a@gmail.com", dir: "/h/.claude"),
            account("claude-b", email: "b@gmail.com", dir: "/h/.claude-b"),
        ])
        XCTAssertEqual(twins["claude"], "Claude a@gmail.com")
        XCTAssertEqual(twins["claude-b"], "Claude b@gmail.com")
    }

    func testTheExternalTabStepOnlyTakesTheTerminalsClaudeControlCannot() {
        func request(_ bundleID: String?, pid: Int32 = 42) -> ClaudeExternalTabRequest {
            ClaudeExternalTabRequest(bundleID: bundleID, pid: pid, tty: nil, cwd: "/tmp")
        }
        XCTAssertTrue(CodenotchTabFocus.handles(request("com.mitchellh.ghostty"), sealed: false))
        XCTAssertTrue(CodenotchTabFocus.handles(request("com.cmuxterm.app"), sealed: false))
        XCTAssertFalse(CodenotchTabFocus.handles(request("com.googlecode.iterm2"), sealed: false))
        XCTAssertFalse(CodenotchTabFocus.handles(request("com.apple.Terminal"), sealed: false))
        XCTAssertFalse(CodenotchTabFocus.handles(request(nil), sealed: false))
        XCTAssertFalse(CodenotchTabFocus.handles(request("com.mitchellh.ghostty", pid: 0), sealed: false))
        XCTAssertFalse(CodenotchTabFocus.handles(request("com.mitchellh.ghostty"), sealed: true))
    }

    func testDesktopCacheReadingsTranslateAndExpire() {
        let windows = [
            LimitWindow(id: "session", label: "Current session", usedFraction: 0.3, resetsAt: now.addingTimeInterval(600)),
            LimitWindow(id: "weekly_all", label: "Weekly", usedFraction: 0.5),
            LimitWindow(id: "weekly_opus", label: "Opus", usedFraction: nil),
        ]
        let reading = ClaudeDesktopUsageCache.Reading(windows: windows, capturedAt: now.addingTimeInterval(-30),
                                                      entry: URL(fileURLWithPath: "/dev/null"))
        let translated = DesktopUsageSource.translate(reading, now: now)
        XCTAssertEqual(translated?.observedAt, now.addingTimeInterval(-30))
        // A window with nothing used is not a reading; the standard label is
        // left to the app, another one is kept.
        XCTAssertEqual(translated?.windows.map(\.id), ["session", "weekly_all"])
        XCTAssertEqual(translated?.windows.map(\.label), [nil, "Weekly"])
        // A window whose reset has passed makes the whole response old.
        XCTAssertNil(DesktopUsageSource.translate(reading, now: now.addingTimeInterval(601)))
    }

    /// `--snapshot-claude <dir>` is answered however it is spelled; the
    /// environment variable only in a sealed run, so a live app started from
    /// a shell that still exports it starts normally instead of quitting.
    func testSnapshotRequestsAreReadFromTheFlagAndOnlySealedFromTheEnvironment() {
        let dir = "/tmp/agentnotch-snapshots"
        XCTAssertEqual(ClaudeNotchSnapshots.requestedDirectory(arguments: ["app", "--snapshot-claude", dir],
                                                               environment: [:], sealed: false)?.path, dir)
        XCTAssertEqual(ClaudeNotchSnapshots.requestedDirectory(arguments: ["app", "--snapshot-claude=\(dir)"],
                                                               environment: [:], sealed: true)?.path, dir)
        XCTAssertNil(ClaudeNotchSnapshots.requestedDirectory(arguments: ["app", "--snapshot-claude"],
                                                             environment: [:], sealed: true))
        let exported = ["AGENTNOTCH_SNAPSHOT_CLAUDE": dir]
        XCTAssertEqual(ClaudeNotchSnapshots.requestedDirectory(arguments: ["app"], environment: exported,
                                                               sealed: true)?.path, dir)
        XCTAssertNil(ClaudeNotchSnapshots.requestedDirectory(arguments: ["app"], environment: exported, sealed: false))
        XCTAssertNil(ClaudeNotchSnapshots.requestedDirectory(arguments: ["app"], environment: ["AGENTNOTCH_SNAPSHOT_CLAUDE": ""],
                                                             sealed: true))
    }

    func testHoldPolicyMapsOneToOne() {
        XCTAssertEqual(ClaudeAttentionReactions.holdPolicy(.auto), .auto)
        XCTAssertEqual(ClaudeAttentionReactions.holdPolicy(.always), .always)
        XCTAssertEqual(ClaudeAttentionReactions.holdPolicy(.never), .never)
    }

    // MARK: - Badges in the real cell

    /// `C_RingBadgeLayoutTests` checks the badges against the design frame;
    /// this checks them against `NotchLayout` itself: inside the body on the
    /// side edges, clear of the percent label everywhere.
    func testBadgesStayInsideTheCellOnEveryEdge() {
        let ring = NotchLayout.ringDiameter
        let labelTop = ring + NotchLayout.ringLabelGap
        let knockout = ClaudeRingBadgeLayout.knockout
        for edge in NotchEdge.allCases {
            for slot in [ClaudeRingBadgeSlot.needsYou, .review] {
                let badge = ClaudeRingBadgeLayout.frame(slot, edge: edge.claudeEdge, compact: false, ringDiameter: ring)
                    .insetBy(dx: -knockout, dy: -knockout)
                XCTAssertLessThan(badge.maxY, labelTop, "\(edge) \(slot)")
                if edge.isVertical {
                    let margin = (NotchLayout.bodyDepth(for: edge) - ring) / 2
                    XCTAssertGreaterThanOrEqual(badge.minX, -margin, "\(edge) \(slot)")
                    XCTAssertLessThanOrEqual(badge.maxX, ring + margin, "\(edge) \(slot)")
                }
            }
        }
    }

    /// The folded pill holds three dots at every size Settings offers.
    func testRestingMarksFitThePill() {
        for scale in [CGFloat(Preferences.customScaleRange.lowerBound), 1, CGFloat(Preferences.customScaleRange.upperBound)] {
            let visible = NotchLayout.pillWidth * scale - NotchRootView.bezelBleed
            XCTAssertTrue(ClaudeRestingMarkLayout.fits(count: 3, pillLength: NotchLayout.pillHeight * scale,
                                                        visibleDepth: visible), "\(scale)")
        }
    }

    // MARK: - Keychain remap (seam KC)

    /// The app's own keychain items are filed under the fork's service
    /// names; anything else (Claude Code's own item included) is left as it
    /// is, and never read by this app anyway.
    func testOwnKeychainItemsAreTheForksAndOthersAreUntouched() {
        XCTAssertEqual(Fork.keychainService("ollama-api-key"), Fork.bundleID + ".ollama-api-key")
        XCTAssertEqual(Fork.keychainService("com.vinzdg.codenotch.custom-endpoint"), Fork.bundleID + ".custom-endpoint")
        XCTAssertEqual(Fork.keychainService("Claude Code-credentials"), "Claude Code-credentials")
    }
}
