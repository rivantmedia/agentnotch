import AppKit
@_spi(Sealed) import ClaudeControl
import Combine
import Foundation

/// Sealed runs (`AGENTNOTCH_SAFE_MODE=1`, `CODENOTCH_DEMO=1`): ClaudeControl's
/// fixture accounts, sessions and usage on the rings, beside upstream's
/// non-Claude fixtures, going through the same store, sync, feed and
/// reactions a live run uses. Nothing real is read and nothing is written
/// outside the process — except, in the sealed bundle's own preferences
/// domain (which `spm-run-sealed.sh` deletes), what Codenotch writes there
/// itself.
///
/// The timeline (`ClaudeControlHub.SealedDemoStep`): at 1.5 s the work
/// ring's prompts are answered and it starts working; at 3 s a third account
/// appears as a new ring, with a finished session whose green arc settles at
/// about 8 s; at 4.5 s two sessions on two rings finish together (one chime,
/// one peek); at 6 s one stops for a permission prompt.
///
/// Development switches, sealed only:
/// - `AGENTNOTCH_SEALED_SWITCH_OFF=<ring id>` switches that ring off at 7.5 s
///   (the sealed bundle's own domain only), to watch it and its sessions go.
/// - `AGENTNOTCH_SEALED_CAPTURE=<dir>` renders every notch, folded and open, at
///   points along the timeline (see `ClaudeNotchSnapshots.capture`).
///
/// The run's log (stderr, `build/sealed/run.log`) gets one `[agentnotch-sealed]`
/// line per step and per change in what the rings show.
///
/// Fork-only file. Owned by WP-C.
@MainActor
enum ClaudeSealedDemo {
    // MARK: - Tracing

    private static let launchedAt = NSRunningApplication.current.launchDate ?? Date()

    /// One line on stderr, stamped with the time since launch.
    static let trace: (String) -> Void = { line in
        let elapsed = max(0, Date().timeIntervalSince(launchedAt))
        let stamped = String(format: "[agentnotch-sealed] +%.1fs ", elapsed) + line
        FileHandle.standardError.write(Data((stamped + "\n").utf8))
        Log.usage.notice("\(stamped, privacy: .public)")
    }

    /// Whether this run owns its preferences domain: the sealed bundle
    /// (`…agentnotch.sealed`), not a demo run of the real app,
    /// whose preferences are the user's.
    static var ownsPreferencesDomain: Bool {
        Bundle.main.bundleIdentifier?.hasSuffix(".sealed") ?? false
    }

    // MARK: - The store

    /// A usage store over the Claude rings and upstream's non-Claude
    /// fixtures, wired to the fleet the way AppDelegate wires the real one.
    /// Its archive lives in memory: a demo run of the real app must not
    /// overwrite the real one's remembered readings.
    static func makeStore(claude: [ClaudeUsageProvider], fleet: NotchFleet, preferences: Preferences) -> UsageStore {
        let others: [UsageProvider] = Fixtures.snapshots()
            .filter { !ClaudeBridge.ownsProvider($0.id) }
            .map { FixtureUsageProvider(snapshot: $0) }
        let providers: [UsageProvider] = claude + others
        let ids = providers.map(\.id)
        if ownsPreferencesDomain {
            // Upstream's sealed run draws every fixture ring; a fresh domain
            // would switch the non-Claude ones off (only Claude and Codex
            // start on). The on-list is written from the rings first, so the
            // Claude ones stay on with them.
            preferences.reconcile(discoveredIDs: ids)
            for provider in others { preferences.setConnected(true, for: provider.id) }
        }
        let store = UsageStore(
            providers: providers,
            archive: UsageArchive(defaults: SealedMemoryDefaults()),
            disconnected: preferences.disconnectedIDs(among: ids),
            order: preferences.providerOrder
        )
        let draw = { [weak fleet, weak preferences] (snapshots: [ProviderSnapshot]) in
            guard let fleet, let preferences else { return }
            fleet.setSnapshots(AppDelegate.drawn(snapshots, weekly: preferences.weeklyHeadline,
                                                 paced: preferences.claudeDailyPaceRing))
        }
        // Drawn now as well as on every change, so the first controller is
        // built with these rings rather than upstream's fixture ones.
        draw(store.notchSnapshots)
        store.$notchSnapshots
            .dropFirst()
            .receive(on: DispatchQueue.main)
            .sink { draw($0) }
            .store(in: &subscriptions)
        store.$refreshing
            .removeDuplicates()
            .receive(on: DispatchQueue.main)
            .sink { [weak fleet] ids in
                fleet?.setRefreshing(ids)
                // Only the store's own schedule may put a Claude ring in
                // here: its first pass at launch. A reading arriving never
                // should.
                let claude = ids.filter { ClaudeBridge.ownsProvider($0) }.sorted()
                if !claude.isEmpty { trace("store refreshing: \(claude.joined(separator: ", "))") }
            }
            .store(in: &subscriptions)
        Publishers.CombineLatest(preferences.$connectedProviders, preferences.$disabledModels)
            .receive(on: DispatchQueue.main)
            .sink { [weak store, weak preferences] _, _ in
                guard let store, let preferences else { return }
                store.disconnected = preferences.disconnectedIDs(among: store.knownIDs)
            }
            .store(in: &subscriptions)
        preferences.$accountNicknames
            .receive(on: DispatchQueue.main)
            .sink { [weak store] in store?.nicknames = $0 }
            .store(in: &subscriptions)
        fleet.onRefreshProvider = { [weak store] id in
            trace("store.refresh(providerID: \(id))")
            await store?.refresh(providerID: id)?.value
        }
        store.start()
        // The app keeps no store in a sealed run; this one lives as long as
        // the process.
        sealedStore = store
        return store
    }

    private static var sealedStore: UsageStore?

    // MARK: - The timeline

    private static var subscriptions = Set<AnyCancellable>()
    private static var summaryTimer: Timer?
    private static var lastSummary = ""

    static func start(hub: ClaudeControlHub, fleet: NotchFleet, preferences: Preferences, store: UsageStore?) {
        let environment = ProcessInfo.processInfo.environment
        for step in ClaudeControlHub.SealedDemoStep.allCases {
            DispatchQueue.main.asyncAfter(deadline: .now() + step.secondsAfterLaunch) { [weak hub] in
                MainActor.assumeIsolated {
                    guard let hub else { return }
                    trace("step \(step.rawValue)")
                    Task { await hub.runSealedDemoStep(step) }
                }
            }
        }

        if let ring = environment["AGENTNOTCH_SEALED_SWITCH_OFF"], !ring.isEmpty {
            DispatchQueue.main.asyncAfter(deadline: .now() + 7.5) { [weak preferences, weak store] in
                MainActor.assumeIsolated {
                    guard let preferences else { return }
                    guard ownsPreferencesDomain else {
                        trace("switch-off of \(ring) skipped: not the sealed bundle's own preferences")
                        return
                    }
                    trace("switching \(ring) off")
                    // The on-list is written for the first time here, from
                    // what is on screen, so nothing else goes off with it.
                    if let store { preferences.reconcile(discoveredIDs: store.knownIDs) }
                    preferences.setConnected(false, for: ring)
                }
            }
        }

        if let directory = environment["AGENTNOTCH_SEALED_CAPTURE"], !directory.isEmpty {
            ClaudeNotchSnapshots.scheduleCaptures(into: URL(fileURLWithPath: directory, isDirectory: true),
                                                  fleet: fleet, trace: trace)
        }

        // What the rings show, as one line whenever it changes.
        let timer = Timer(timeInterval: 0.25, repeats: true) { [weak hub, weak fleet] _ in
            MainActor.assumeIsolated {
                guard let hub, let fleet else { return }
                let line = summary(hub: hub, fleet: fleet)
                guard line != lastSummary else { return }
                lastSummary = line
                trace("rings: " + line)
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        summaryTimer = timer
    }

    /// Each ring in the notch: its session reading, the arc its sessions
    /// give it (and whether a green one has settled), and its badge counts.
    private static func summary(hub: ClaudeControlHub, fleet: NotchFleet) -> String {
        let model = fleet.menuModel
        let state = ClaudeNotchState.shared
        let now = Date()
        // Claude rings that are not in the notch (switched off, or gone),
        // and how many hover-card rows they still hold: none, once the feed
        // has caught up.
        let onScreen = Set(model.snapshots.map(\.id))
        let offScreen = fleet.sessions.keys.filter { ClaudeBridge.ownsProvider($0) && !onScreen.contains($0) }.sorted()
            .map { "\($0) (not shown) rows=\(fleet.sessions[$0]?.count ?? 0)" }
        let rings = model.snapshots.map { snapshot -> String in
            guard ClaudeBridge.ownsProvider(snapshot.id) else { return snapshot.id }
            let reading = snapshot.hasReading ? "\(Int(((snapshot.headline?.usedFraction ?? 0) * 100).rounded()))%" : "—"
            let arc: String
            switch model.activity(for: snapshot)?.state {
            case .waiting?: arc = "amber-pulse"
            case .working?: arc = "white-spinner"
            case .success?: arc = state.settles(snapshot.id, now: now) ? "green-steady" : "green-pulse"
            case .idle?, nil: arc = "none"
            }
            let counts = state.ringCounts[snapshot.id] ?? .zero
            let rows = model.sessions[snapshot.id]?.count ?? 0
            return "\(snapshot.id) \(reading) arc=\(arc) needs=\(counts.needsYou) review=\(counts.review) working=\(counts.working) rows=\(rows)"
        }
        return (rings + offScreen).joined(separator: " | ")
    }
}

/// One of upstream's fixture rings as a provider: the same snapshot, every
/// time, with no I/O.
private actor FixtureUsageProvider: UsageProvider {
    nonisolated let id: String
    nonisolated let displayName: String
    nonisolated let glyph: ProviderGlyph
    private let snapshot: ProviderSnapshot

    init(snapshot: ProviderSnapshot) {
        self.id = snapshot.id
        self.displayName = snapshot.displayName
        self.glyph = snapshot.glyph
        self.snapshot = snapshot
    }

    func fetchSnapshot() async throws -> ProviderSnapshot { snapshot }
    nonisolated func account() -> ProviderAccount? { nil }
    nonisolated var signInRoute: SignInRoute { .guidance("Sealed run: fixture data.") }
    func signOut() async {}
    nonisolated func presentSignIn() {}
    nonisolated func forgetCachedCredential() {}
}

/// Defaults that live and die with the process: the sealed store's archive.
private final class SealedMemoryDefaults: UserDefaults, @unchecked Sendable {
    private var values: [String: Any] = [:]

    override func object(forKey defaultName: String) -> Any? { values[defaultName] }
    override func data(forKey defaultName: String) -> Data? { values[defaultName] as? Data }
    override func set(_ value: Any?, forKey defaultName: String) { values[defaultName] = value }
    override func removeObject(forKey defaultName: String) { values[defaultName] = nil }
}
