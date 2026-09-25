import ClaudeControl
import Combine
import Foundation

/// Keeps Codenotch's store in step with ClaudeControl's accounts and readings
/// (design §4.4), without a restart and without a refetch.
///
/// - **Rings.** One `ClaudeUsageProvider` per account. When the set of rings
///   changes (an account signs in, is added or forgotten) the store's Claude
///   providers are replaced in place (`UsageStore.replaceProviders`, U6); a
///   new ring then shows its archived reading or a placeholder, and a newly
///   discovered one is switched on the way Codenotch switches on any new
///   Claude profile — before the store holds it, so no ring changes state
///   inside the store (which would refetch every provider).
/// - **Launch.** The rings made at launch (U3) come from ClaudeControl's
///   files and a folder listing; the hub lists its accounts when it has
///   loaded them, which need not be by the time this starts. A launch ring
///   the hub has not listed yet is kept until it does, or until
///   `launchGrace` has passed: taking it away would also delete its archived
///   reading (U6 forgets a removed ring), so every relaunch would start the
///   ring empty.
/// - **Readings.** Each ring's newest reading is pushed with
///   `UsageStore.ingest` when what the ring shows (its windows, status or
///   plan) changes, at most once every 5 s per ring. Never through
///   `refresh(providerID:)`: that presses and spins the ring, which is for a
///   click, not for a reading that simply arrived.
/// - **Names.** `ClaudeRingNames` follows the accounts, so a ring is named
///   from its signed-in address as soon as it has one.
///
/// Fork-only file. Owned by WP-C.
@MainActor
final class ClaudeProviderSync {
    /// The most often one ring is pushed a reading.
    static let ingestInterval: TimeInterval = 5
    /// How long a ring made at launch waits for the hub to list its account
    /// before it is treated as gone. Well past the registry's first discovery
    /// pass (a directory listing), short enough that a folder removed while
    /// the app was closed does not linger.
    static let launchGrace: TimeInterval = 20

    private weak var hub: ClaudeControlHub?
    private weak var store: UsageStore?
    private weak var preferences: Preferences?
    /// Whether Codenotch's connected set may be written: never by a sealed
    /// run in a domain that is not its own (see `ClaudeSealedDemo`).
    private let writesPreferences: Bool
    private let log: (String) -> Void
    private var cancellables = Set<AnyCancellable>()

    /// The providers in the store, by ring id, reused while their ring stays.
    private var providers: [String: ClaudeUsageProvider] = [:]
    private var order: [String] = []
    /// What each ring was last pushed, and when.
    private var pushed: [String: ProviderSnapshot] = [:]
    private var pushedAt: [String: Date] = [:]
    private var trailing: [String: DispatchWorkItem] = [:]
    /// Rings made at launch that the hub has not listed yet.
    private var awaitingHub: Set<String> = []
    private var graceEnd: DispatchWorkItem?

    init(hub: ClaudeControlHub, store: UsageStore, preferences: Preferences,
         writesPreferences: Bool, log: @escaping (String) -> Void = { _ in }) {
        self.hub = hub
        self.store = store
        self.preferences = preferences
        self.writesPreferences = writesPreferences
        self.log = log
    }

    /// The providers made at launch (U3), so the sync starts from what the
    /// store already holds rather than replacing it on its first pass.
    func adopt(_ launched: [ClaudeUsageProvider]) {
        for provider in launched where providers[provider.id] == nil {
            providers[provider.id] = provider
            order.append(provider.id)
            awaitingHub.insert(provider.id)
        }
    }

    func start() {
        guard let hub else { return }
        // A turn after `@Published` fires, when the value has landed.
        hub.$accounts
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in self?.accountsChanged() }
            .store(in: &cancellables)
        hub.$ringReadings
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in self?.readingsChanged() }
            .store(in: &cancellables)
        store?.$disconnected
            .removeDuplicates()
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in self?.connectionsChanged() }
            .store(in: &cancellables)
        if !awaitingHub.isEmpty {
            let work = DispatchWorkItem { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, !self.awaitingHub.isEmpty else { return }
                    self.log("claude rings the hub never listed: \(self.awaitingHub.sorted().joined(separator: ", "))")
                    self.awaitingHub.removeAll()
                    self.accountsChanged()
                }
            }
            graceEnd = work
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.launchGrace, execute: work)
        }
        accountsChanged()
    }

    func stop() {
        cancellables.removeAll()
        trailing.values.forEach { $0.cancel() }
        trailing.removeAll()
        graceEnd?.cancel()
        graceEnd = nil
    }

    // MARK: - Rings

    private func accountsChanged() {
        guard let hub, let store else { return }
        let accounts = hub.accounts
        // A folder that became part of an account (a folder added by hand
        // signing in, say): its ring's name, switch and place move over.
        if writesPreferences, let preferences {
            ClaudeRingMigrator.migratePreferences(accounts: accounts, retired: hub.retiredRingIDs,
                                                  preferences: preferences, store: nil, log: log)
        }
        let renamed = ClaudeRingNames.shared.update(accounts)
        let listed = Self.ringIDs(accounts)
        awaitingHub.subtract(listed)
        if awaitingHub.isEmpty {
            graceEnd?.cancel()
            graceEnd = nil
        }
        let rings = Self.keptRings(listed: listed, current: order, awaitingHub: awaitingHub)
        if Set(rings) != Set(order) {
            let added = rings.filter { providers[$0] == nil }
            let removed = order.filter { !rings.contains($0) }
            var next: [String: ClaudeUsageProvider] = [:]
            for ring in rings { next[ring] = providers[ring] ?? ClaudeUsageProvider(ringID: ring) }
            providers = next
            order = rings
            Self.install(rings.compactMap { next[$0] }, in: store, preferences: preferences,
                         writesPreferences: writesPreferences)
            for ring in removed { forget(ring) }
            log("claude rings: \(rings.joined(separator: ", ")) (added \(added.joined(separator: ", ")), removed \(removed.joined(separator: ", ")))")
            // Newcomers get their reading at once, not at the next poll.
            for ring in added { push(ring, force: true) }
        } else if !renamed.isEmpty {
            // Same rings, new names or addresses: bump the store's account
            // revision so Settings re-reads them, and redraw the names.
            // (Where the rings sit is the user's order, not the accounts'.)
            order = rings
            store.replaceProviders(where: ClaudeBridge.ownsProvider, with: rings.compactMap { providers[$0] })
        }
        for ring in renamed where providers[ring] != nil { push(ring, force: true) }
    }

    private func forget(_ ring: String) {
        trailing.removeValue(forKey: ring)?.cancel()
        pushed[ring] = nil
        pushedAt[ring] = nil
    }

    // MARK: - Readings

    private func readingsChanged() {
        for ring in order { push(ring, force: false) }
    }

    /// A ring switched off in the notch loses its reading in the store, so
    /// what it was pushed is forgotten; switched back on, the store shows a
    /// placeholder and refetches — unless a pass is already in flight, when
    /// it skips the refetch and the ring would wait a whole polling interval
    /// for its reading. So it is pushed again at once.
    private func connectionsChanged() {
        guard let store else { return }
        let plan = Self.connectionPlan(rings: order, disconnected: store.disconnected, pushed: Set(pushed.keys))
        for ring in plan.forget { forget(ring) }
        for ring in plan.push { push(ring, force: true) }
    }

    /// Push `ring`'s reading if it changes what the ring shows. At most once
    /// per `ingestInterval`; a change inside the interval is pushed when it
    /// ends, so the newest reading is never lost. `force` skips the interval
    /// (a ring that just arrived, was renamed or switched back on), not the
    /// comparison.
    private func push(_ ring: String, force: Bool) {
        guard let hub, let store, providers[ring] != nil else { return }
        // A ring switched off is not pushed, nor remembered as pushed: the
        // store drops its reading, and once the ring is back on the same
        // reading must not look already delivered.
        guard !store.disconnected.contains(ring) else {
            forget(ring)
            return
        }
        let now = Date()
        guard let snapshot = ClaudeUsageProvider.pushed(
            ringID: ring,
            displayName: ClaudeRingNames.shared.name(for: ring),
            reading: hub.ringReadings[ring] ?? ClaudeRingReading(status: .waitingForFirstReading),
            resetCredits: DesktopUsageSource.shared.resetCredits(
                organizationUuid: ClaudeRingNames.shared.entry(ring)?.organizationUuid, now: now),
            now: now
        ) else { return }
        guard Self.changesRing(from: pushed[ring], to: snapshot) else { return }
        if !force, let last = pushedAt[ring], now.timeIntervalSince(last) < Self.ingestInterval {
            guard trailing[ring] == nil else { return }
            let work = DispatchWorkItem { [weak self] in
                MainActor.assumeIsolated {
                    self?.trailing[ring] = nil
                    self?.push(ring, force: false)
                }
            }
            trailing[ring] = work
            DispatchQueue.main.asyncAfter(deadline: .now() + last.addingTimeInterval(Self.ingestInterval).timeIntervalSince(now),
                                          execute: work)
            return
        }
        trailing.removeValue(forKey: ring)?.cancel()
        pushed[ring] = snapshot
        pushedAt[ring] = now
        store.ingest(snapshot)
    }

    // MARK: - The store

    /// Make `fresh` the store's Claude rings, in place of the ones it holds,
    /// without a refetch.
    ///
    /// The on/off state of every ring is settled around the swap rather than
    /// after it: a ring arriving is switched on (when this run may write
    /// Codenotch's connected set) or kept off before the store holds it, and
    /// rings leaving drop out of the off-list only once they are gone. The
    /// store refetches every provider when a ring it holds turns on or off
    /// (`UsageStore.disconnected`), which is right for a switch in Settings
    /// and wrong for an account that merely came or went.
    static func install(_ fresh: [UsageProvider], in store: UsageStore, preferences: Preferences?,
                        writesPreferences: Bool) {
        let held = Set(store.knownIDs)
        let arriving = store.knownIDs + fresh.map(\.id).filter { !held.contains($0) }
        if let preferences {
            // A ring nobody has seen before comes up switched on, as a new
            // Claude profile does upstream.
            if writesPreferences { preferences.reconcile(discoveredIDs: arriving) }
            store.disconnected = preferences.disconnectedIDs(among: arriving)
        }
        store.replaceProviders(where: ClaudeBridge.ownsProvider, with: fresh)
        if let preferences {
            store.disconnected = preferences.disconnectedIDs(among: store.knownIDs)
        }
    }

    // MARK: - Rules (pure)

    /// The rings to hold: every ring the hub lists, then any ring made at
    /// launch that it has not listed yet, where it already was.
    static func keptRings(listed: [String], current: [String], awaitingHub: Set<String>) -> [String] {
        let listedSet = Set(listed)
        return listed + current.filter { awaitingHub.contains($0) && !listedSet.contains($0) }
    }

    /// One ring per tracked account (a signed-in identity, however many
    /// folders it lives in), in the accounts' order; two accounts that share
    /// a ring id share one.
    ///
    /// Tracked only, because that is what the hub reads usage for and what
    /// `launchAccounts` makes rings for at launch: a ring for an untracked
    /// account would sit waiting for a reading that never comes. Whether a
    /// tracked account's ring is drawn is Codenotch's own switch (its
    /// connected set), not this list.
    static func ringIDs(_ accounts: [ClaudeAccountSummary]) -> [String] {
        var seen = Set<String>()
        return accounts.filter(\.isTracked).map(\.ringID).filter { seen.insert($0).inserted }
    }

    /// What to do when the store's off-list changes: rings now off forget
    /// what they were pushed, and rings on with nothing pushed are pushed.
    static func connectionPlan(rings: [String], disconnected: Set<String>,
                               pushed: Set<String>) -> (forget: [String], push: [String]) {
        (rings.filter { disconnected.contains($0) && pushed.contains($0) },
         rings.filter { !disconnected.contains($0) && !pushed.contains($0) })
    }

    /// Whether pushing `next` would change what the ring shows: its windows,
    /// its status or its plan. A reading that only moved its timestamp does
    /// not; the store's own poll picks that up.
    static func changesRing(from previous: ProviderSnapshot?, to next: ProviderSnapshot) -> Bool {
        guard let previous else { return true }
        return previous.windows != next.windows
            || previous.status != next.status
            || previous.plan != next.plan
            || previous.displayName != next.displayName
            || previous.resetCredits != next.resetCredits
    }
}
