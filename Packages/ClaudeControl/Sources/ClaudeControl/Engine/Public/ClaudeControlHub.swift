//
//  ClaudeControlHub.swift
//  ClaudeControl
//
//  The one object the app talks to. It freezes the configuration, starts the
//  engine's services (or seeds fixtures when sealed) and republishes what
//  they know as the plain summaries in ClaudeSummaries.swift: accounts,
//  sessions with where they run and what they want, per-ring and total
//  attention counts, ring readings, the 90-second "just finished" window per
//  ring, and attention transitions.
//
//  The public surface is frozen (design §12). Engine extensions in other
//  files may publish through the `internal(set)` setters.
//

import AppKit
import Combine
import Foundation
import os.log

@MainActor
public final class ClaudeControlHub: ObservableObject {
    // MARK: - Bootstrap

    private static var instance: ClaudeControlHub?

    /// The hub, once `bootstrap` has run.
    public static var shared: ClaudeControlHub? { instance }

    /// Freeze `configuration` for the whole engine and create the hub. Only
    /// the first call counts; later calls return the same hub.
    public static func bootstrap(_ configuration: ClaudeControlConfiguration) -> ClaudeControlHub {
        if let instance { return instance }
        AppIdentity.freeze(configuration)
        let hub = ClaudeControlHub(configuration: AppIdentity.configuration)
        instance = hub
        return hub
    }

    private static var logger: Logger { EngineLog.logger("Hub") }

    let configuration: ClaudeControlConfiguration
    var isSealed: Bool { configuration.mode == .sealed }

    init(configuration: ClaudeControlConfiguration) {
        self.configuration = configuration
    }

    // MARK: - Published state

    @Published public internal(set) var accounts: [ClaudeAccountSummary] = []
    @Published public internal(set) var sessions: [ClaudeSessionSummary] = []
    /// Counts per ring: every tracked account's ring (zero when it has no
    /// sessions), plus the default ring when sessions of unknown accounts
    /// land there.
    @Published public internal(set) var ringCounts: [String: ClaudeAttentionCounts] = [:]
    /// Every session's counts, whichever rings the notch shows: deliberately
    /// unfiltered (the Dock badge counts every session). What the notch
    /// shows is the bridge's `ClaudeNotchState`, routed and totalled once
    /// there (CS-9).
    @Published public internal(set) var totalCounts: ClaudeAttentionCounts = .zero
    @Published public internal(set) var ringReadings: [String: ClaudeRingReading] = [:]
    /// Until when each ring's newest completion is "just finished" (90 s
    /// after it); rings without one are absent. Republished at the boundary.
    @Published public internal(set) var freshSuccessUntil: [String: Date] = [:]
    @Published public internal(set) var setup: ClaudeSetupState = ClaudeSetupState()
    /// Ring ids from when every config folder was an account (`claude`,
    /// `claude-shared`, `claude-paras`, …) that no ring has any more: the
    /// app drops them from Codenotch's order and lists once their names,
    /// switches and readings moved to their account's ring (see
    /// `ClaudeAccountSummary.formerRingIDs`).
    @Published public internal(set) var retiredRingIDs: Set<String> = []
    /// Claude Parallel Profiles (its manifest or a store) was found.
    @Published public internal(set) var parallelProfilesDetected = false
    /// The website: where sync goes, who is signed in, the switches, the
    /// last sync (see ClaudeControlHub+Cloud.swift).
    @Published public internal(set) var cloud = ClaudeCloudState()
    /// Run folders nobody is signed in to (no ring; Settings lists them).
    @Published var unsignedFolders: [String] = []
    /// VS Code windows' folders named after their project (see
    /// `WindowFolderNames`), as learned from sessions this run.
    @Published var windowFolderNames: [String: String] = [:]

    /// Some session is working, on any ring (see `totalCounts`).
    public var isBusy: Bool { totalCounts.working > 0 }

    /// Needs-input, ready-for-review and resolved crossings, on the main
    /// thread. Silent baseline: until every account's registry has been read
    /// (the same baseline the banners use, `AttentionTracker`; in a sealed
    /// run, until the fixtures are first published), changes are recorded without an
    /// announcement, so sessions already waiting at launch are not
    /// announced. After it, a session seen for the first time is news like
    /// any other (one that appears already needing input is announced). A
    /// session that stops needing input or is reviewed, or that goes away
    /// while it needed you or waited for review, is `.resolved`. The rule is
    /// `AttentionNews`, shared with the banners (CS-3).
    public var transitions: AnyPublisher<ClaudeAttentionTransition, Never> {
        transitionSubject.eraseToAnyPublisher()
    }

    /// Requests to show the panel (notification clicks, `requestPanel`).
    public var panelRequests: AnyPublisher<ClaudePanelRoute, Never> {
        panelSubject.eraseToAnyPublisher()
    }

    // MARK: - Private state

    private let transitionSubject = PassthroughSubject<ClaudeAttentionTransition, Never>()
    private let panelSubject = PassthroughSubject<ClaudePanelRoute, Never>()
    private var cancellables = Set<AnyCancellable>()
    private var started = false
    /// When `start` last ran: completions from before it are not announced.
    private var startedAt: Date?
    /// Watches app activations for the "viewed in its own tab" check.
    private var activationObserver: NSObjectProtocol?
    private var viewedCheckTask: Task<Void, Never>?
    /// Rings the user has switched on in Codenotch; nil until the bridge says.
    private var shownRings: Set<String>?
    private var nicknames: [String: String] = [:]
    /// Last attention per session, for transitions. Nil until the baseline.
    private(set) var previousAttention: [String: AttentionSnapshot]?
    private var boundaryTask: Task<Void, Never>?
    /// Sealed: fixture usage per account id (the third account's included,
    /// for when the sealed demo adds it).
    private var fixtureUsage: [String: AccountUsage] = [:]
    /// Claude Desktop's records of the sessions it hosts, looked up (not sealed).
    private lazy var desktopSessions = DesktopSessionAttributor(
        root: DesktopHostedSessions.root(home: configuration.homeDirectory))

    /// How long a completion counts as "just finished" on its ring.
    public nonisolated static let freshSuccessWindow: TimeInterval = 90

    // MARK: - Lifecycle

    /// The accounts to make rings for at launch, synchronously, on the main
    /// actor: accounts.json, a listing of the home folder, and for each
    /// `~/.claude-*` / `~/.claude_*` candidate the discovery markers
    /// (`AccountRegistry.discover`): a byte scan of its `.claude.json`
    /// (memory-mapped) for a signed-in `oauthAccount`, and, when it has none,
    /// a look at its `sessions/` folder for a live process. Milliseconds per
    /// account folder (CS-10).
    public func launchAccounts() -> [ClaudeAccountSummary] {
        let home = configuration.homeDirectory
        if isSealed {
            let grouping = AccountIdentityGrouping.group(Self.fixtureAccounts, prefs: [:], home: home)
            return grouping.identities.map { ClaudeHostProjections.account(identity: $0, hookStatuses: [:], home: home) }
        }
        // The registry discovers, classifies and reads identities
        // synchronously when it is made, so these are the rings it keeps.
        return AccountRegistry.shared.visibleIdentities.map {
            ClaudeHostProjections.account(identity: $0, hookStatuses: [:], home: home)
        }
    }

    /// Start the services (live) or seed the fixtures (sealed). Idempotent.
    public func start() {
        guard !started else { return }
        started = true
        startedAt = Date()
        DevFlags.logActiveFlags()
        subscribe()
        if isSealed {
            startSealed()
        } else {
            startLive()
        }
        recompute()
    }

    /// Stop the services: unlink the socket, flush the review queue and the
    /// usage state. Banners stay until what they are about is resolved.
    public func stop() {
        guard started else { return }
        started = false
        boundaryTask?.cancel()
        boundaryTask = nil
        viewedCheckTask?.cancel()
        viewedCheckTask = nil
        if let activationObserver {
            NSWorkspace.shared.notificationCenter.removeObserver(activationObserver)
            self.activationObserver = nil
        }
        cancellables.removeAll()
        // The next start takes a new silent baseline: what changed while
        // stopped is not announced.
        previousAttention = nil
        guard !isSealed else { return }
        NotificationService.shared.stop()
        RegistryDirsBridge.shared.stop()
        CloudSync.shared.stop()
        UsageStore.shared.stop()
        AccountHookManager.shared.stop()
        AttentionTracker.shared.stop()
        AccountRegistry.shared.stop()
        ClaudeSessionMonitor.shared.stop()
        SessionHostCache.shared.stop()
        RunningApps.shared.stop()
    }

    private func startLive() {
        // What host-app names and the focus checks read, kept current off
        // the main actor from here on.
        RunningApps.shared.start()
        SessionHostCache.shared.start()
        // Accounts first: the hook manager reads every account the registry
        // knows (and installs only after consent), and the usage store polls
        // each of them.
        AccountRegistry.shared.start()
        AccountHookManager.shared.start()
        UsageStore.shared.start()
        // After the usage store, whose readings it records (only while the
        // user has turned sync on).
        CloudSync.shared.start(usage: UsageStore.shared.observations.eraseToAnyPublisher())
        ClaudeSessionMonitor.shared.start()
        AttentionTracker.shared.start()
        RegistryDirsBridge.shared.start()
        NotificationService.shared.start()
        observeActivations()
    }

    /// Fixture accounts, sessions and usage. What happens to them after
    /// launch (a third account signing in, sessions finishing) is the sealed
    /// demo's timeline, played by the app through the same registry and
    /// store; the hub projects it like any live change.
    private func startSealed() {
        Self.logger.notice("Sealed: showing fixture accounts, sessions and usage")
        AccountRegistry.shared.replaceAllWithFixtures(Self.fixtureAccounts, layout: SampleLayout.layout)
        let now = Date()
        fixtureUsage = SampleData.usage(now: now)
        let sessions = SampleSessions.all().map { Self.freshened($0, to: now) }
        Task { await SessionStore.shared.replaceAllWithFixtures(sessions) }
        cloud = ClaudeCloudState.sealedFixture(now: now)
    }

    private func subscribe() {
        // `@Published` fires before the value changes; `receive(on:)` hops to
        // the next main-queue turn, when it has.
        func follow<P: Publisher>(_ publisher: P) where P.Failure == Never {
            publisher
                .dropFirst()
                .receive(on: DispatchQueue.main)
                .sink { [weak self] _ in self?.recompute() }
                .store(in: &cancellables)
        }
        follow(AccountRegistry.shared.$accounts)
        follow(AccountRegistry.shared.$identities)
        follow(ClaudeSessionMonitor.shared.$instances)
        if !isSealed {
            follow(AccountHookManager.shared.$status)
            follow(UsageStore.shared.$usage)
            follow(UsageStore.shared.$fetchState)
            follow(UsageStore.shared.$organizationUuids)
            follow(RunningApps.shared.$revision)
            // A2's hook manager and the socket server are the only sources
            // of `setup` (consent, the Superpowered Vibe Notch guard, other
            // apps' hooks, the socket error).
            followSetupChanges()?.store(in: &cancellables)
            SessionHostCache.shared.changes
                .receive(on: DispatchQueue.main)
                .sink { [weak self] in self?.recompute() }
                .store(in: &cancellables)
            CloudSync.shared.$state
                .receive(on: DispatchQueue.main)
                .sink { [weak self] state in
                    if self?.cloud != state { self?.cloud = state }
                }
                .store(in: &cancellables)
        }
        AppEventBus.shared.panelRequests
            .sink { [weak self] route in
                switch route {
                case .session(let id): self?.panelSubject.send(.session(id: id))
                case .sessions, .usage: self?.panelSubject.send(.sessions(ringID: nil))
                case .settings: break
                }
            }
            .store(in: &cancellables)
    }

    // MARK: - Projection

    private func recompute() {
        let now = Date()
        let home = configuration.homeDirectory
        let registry = AccountRegistry.shared
        let usageStore = UsageStore.shared
        let hookStatus = isSealed ? [:] : AccountHookManager.shared.status
        let organizations = isSealed ? [:] : usageStore.organizationUuids

        // Accounts (one per identity), with Codenotch's nicknames.
        let defaultRingOwner = Self.defaultRingOwner(registry)
        let adopted = registry.layout.adoptedByExtension
        var accounts = registry.identities.map {
            ClaudeHostProjections.account(identity: $0, hookStatuses: hookStatus,
                                          organizationUuid: organizations[$0.id], defaultRing: defaultRingOwner,
                                          adopted: adopted, home: home)
        }
        for index in accounts.indices {
            if let nickname = nicknames[accounts[index].ringID], !nickname.isEmpty {
                accounts[index].label = nickname
            }
        }

        func usage(_ accountId: String) -> AccountUsage? {
            guard isSealed else { return usageStore.usage[accountId] }
            // Fixture usage is written per folder.
            return fixtureUsage[accountId]
                ?? registry.folders(for: accountId).lazy.compactMap { self.fixtureUsage[$0.id] }.first
        }

        // Sessions of tracked accounts. A session's folder names its account
        // (the identity that folder runs as; for `~/.claude` while Claude
        // Parallel Profiles mirrors into it, the one it ran as when the
        // session started); a folder not known yet goes to the default ring
        // (the account `~/.claude` runs as).
        let hiddenAccountIDs = Set(registry.accounts.filter(\.isHidden).map(\.id))
        let knownRingIDs = Set(accounts.map(\.ringID))
        let defaultRing = Self.defaultRingID(accounts: accounts)
        var snapshots: [AttentionSnapshot] = []
        // Sessions whose account is known for certain, for the website's
        // session ledger (a guess is never recorded), those whose account
        // can't be told now (their responses count for no one), and those
        // not placed yet (they wait; see `cloudPlacement`).
        var attributed: [(state: SessionState, identity: ClaudeIdentityAccount)] = []
        var unsure: Set<String> = []
        var waiting: Set<String> = []
        // Claude Desktop runs the sessions it hosts as its own account: its
        // record of each tells whose (never when sealed).
        var desktopCandidates: [DesktopHostedSessions.Candidate]?
        for state in ClaudeSessionMonitor.shared.instances {
            let folderId = state.accountId ?? AccountRegistry.defaultConfigDir(home: registry.homePath)
            var attribution = registry.attribution(forFolderId: folderId, startedAt: state.pidStartedAt)
            let isDesktopHosted = !isSealed && DesktopHostedSessions.isDesktopHosted(entrypoint: state.entrypoint)
            if isDesktopHosted {
                if desktopCandidates == nil { desktopCandidates = Self.desktopCandidates(registry) }
                let desktopIdentity = desktopSessions.identity(hostSessionId: state.hostSessionId,
                                                               candidates: desktopCandidates ?? [], now: now)
                attribution = DesktopHostedSessions.attribution(folder: attribution, isDesktopHosted: true,
                                                                desktopIdentity: desktopIdentity)
            }
            let identity: ClaudeIdentityAccount?
            if case .known(let id?) = attribution {
                // Switched off, or forgotten (BHV-3): not on any ring, never announced.
                if registry.isForgotten(id) { continue }
                identity = registry.identity(id: id)
                if let identity, !identity.isHidden { attributed.append((state, identity)) }
            } else {
                if Self.cloudPlacement(attribution, isDesktopHosted: isDesktopHosted, state: state, now: now) == .waiting {
                    waiting.insert(state.sessionId)
                } else {
                    unsure.insert(state.sessionId)
                }
                if hiddenAccountIDs.contains(folderId) || registry.isForgotten(folderId) { continue }
                identity = attribution.bestGuess.flatMap(registry.identity(id:)) ?? registry.identity(forFolderId: folderId)
            }
            if identity?.isHidden == true { continue }
            let limitHit = identity.flatMap { usage($0.id)?.limitHit(now: now) }
            var summary = ClaudeHostProjections.session(
                state,
                home: home,
                hostApp: hostAppName(for: state),
                limitHit: limitHit,
                now: now
            )
            summary.ringID = Self.ringID(identity?.ringID ?? summary.ringID, knownRingIDs: knownRingIDs,
                                         defaultRingID: defaultRing)
            snapshots.append(AttentionSnapshot(attention: state.attention, summary: summary,
                                               completedAt: state.completedAt,
                                               isQuietCompletion: state.completionIsQuiet))
        }
        let sessions = snapshots.map(\.summary)
        if !isSealed {
            // A pid whose session ended may be reused by another app later.
            SessionHostCache.shared.retain(pids: Set(ClaudeSessionMonitor.shared.instances.compactMap(\.pid)))
            desktopSessions.retain(Set(ClaudeSessionMonitor.shared.instances.compactMap(\.hostSessionId)))
            feedCloud(attributed, unsure: unsure, waiting: waiting,
                      liveIDs: Set(ClaudeSessionMonitor.shared.instances.map(\.sessionId)))
        }

        // Readings per tracked account.
        var readings: [String: ClaudeRingReading] = [:]
        let staleThreshold = isSealed ? ClaudeRingReading.staleAfter : usageStore.staleThreshold
        for identity in registry.identities where !identity.isHidden {
            readings[identity.ringID] = ClaudeHostProjections.ringReading(
                usage: usage(identity.id),
                fetchState: isSealed ? nil : usageStore.fetchState[identity.id],
                plan: identity.planName,
                organizationUuid: organizations[identity.id] ?? identity.organizationUuid,
                staleThreshold: staleThreshold,
                now: now
            )
        }
        let retired = Self.retiredRingIDs(current: Set(accounts.map(\.ringID)), registry: registry, home: home)
        let unsigned = registry.unsignedFolders.map(\.configDir)
        let learned = WindowFolderNames.names(windowDirs: registry.layout.windowDirs,
                                              paths: ClaudeSessionMonitor.shared.instances.map(\.cwd), home: home)
        let windowNames = windowFolderNames.merging(learned) { known, _ in known }

        let ringCounts = Self.ringCounts(sessions: sessions, ringIDs: accounts.filter(\.isTracked).map(\.ringID))
        let total = ClaudeAttentionCounts.of(sessions)
        let fresh = Self.freshSuccessUntil(sessions: sessions, now: now)

        let setup = currentSetupState()

        if self.accounts != accounts { self.accounts = accounts }
        if self.sessions != sessions { self.sessions = sessions }
        if self.ringCounts != ringCounts { self.ringCounts = ringCounts }
        if totalCounts != total { totalCounts = total }
        if ringReadings != readings { ringReadings = readings }
        if freshSuccessUntil != fresh { freshSuccessUntil = fresh }
        if self.setup != setup { self.setup = setup }
        if retiredRingIDs != retired { retiredRingIDs = retired }
        if parallelProfilesDetected != registry.layout.extensionDetected { parallelProfilesDetected = registry.layout.extensionDetected }
        if unsignedFolders != unsigned { unsignedFolders = unsigned }
        if windowFolderNames != windowNames { windowFolderNames = windowNames }
        applyPausedRings()

        emitTransitions(snapshots)
        scheduleBoundary(Self.nextBoundary(freshSuccessUntil: fresh, readings: readings, now: now))
    }

    /// The known identities as Claude Desktop's records name them: account
    /// UUID and organization (the one the account's key uses).
    static func desktopCandidates(_ registry: AccountRegistry) -> [DesktopHostedSessions.Candidate] {
        registry.identities.compactMap { identity in
            guard let uuid = AccountIdentityGrouping.accountUuid(ofKey: identity.id), !uuid.isEmpty else { return nil }
            return DesktopHostedSessions.Candidate(
                identityId: identity.id, accountUuid: uuid,
                organizationUuid: CloudKeys.organization(of: identity, correctedFolders: registry.correctedFolders))
        }
    }

    /// A session's ring: its account's when that account has one, else the
    /// default ring (a folder seen in a hook before the registry knows it).
    /// Pure.
    nonisolated static func ringID(_ ringID: String, knownRingIDs: Set<String>,
                                   defaultRingID: String = ClaudeRingIdentity.defaultRingID) -> String {
        knownRingIDs.contains(ringID) ? ringID : defaultRingID
    }

    /// The ring sessions of unknown folders go to: the account `~/.claude`
    /// runs as, else the first tracked one. Pure.
    nonisolated static func defaultRingID(accounts: [ClaudeAccountSummary]) -> String {
        accounts.first { $0.isDefault && $0.isTracked }?.ringID
            ?? accounts.first(where: \.isTracked)?.ringID
            ?? ClaudeRingIdentity.defaultRingID
    }

    /// Per-folder ring ids of every folder known (infrastructure included)
    /// that no ring has now.
    static func retiredRingIDs(current: Set<String>, registry: AccountRegistry, home: String) -> Set<String> {
        let folders = registry.accounts.map(\.configDir) + registry.infrastructureDirs
        return Set(folders.map { ClaudeRingIdentity.ringID(configDir: $0, home: home) }).subtracting(current)
    }

    /// Every account the registry knows, tracked or not, with its former
    /// ring ids, and the ring ids no ring has any more: what the app needs
    /// to move Codenotch's per-folder choices to the accounts' rings. Usable
    /// at launch, before `start` (the registry reads its folders when made).
    public func ringMigrationSources() -> (accounts: [ClaudeAccountSummary], retired: Set<String>) {
        guard !isSealed else { return ([], []) }
        let home = configuration.homeDirectory
        let registry = AccountRegistry.shared
        let defaultRing = Self.defaultRingOwner(registry)
        let accounts = registry.identities.map {
            ClaudeHostProjections.account(identity: $0, hookStatuses: [:], defaultRing: defaultRing, home: home)
        }
        return (accounts, Self.retiredRingIDs(current: Set(accounts.map(\.ringID)), registry: registry, home: home))
    }

    /// Whose `~/.claude`'s old ring is (see `ClaudeHostProjections.DefaultRingOwner`).
    static func defaultRingOwner(_ registry: AccountRegistry) -> ClaudeHostProjections.DefaultRingOwner {
        let defaultFolder = AccountRegistry.defaultConfigDir(home: registry.homePath)
        return ClaudeHostProjections.DefaultRingOwner(mirrored: registry.correctedFolders.contains(defaultFolder),
                                                      owner: registry.defaultOwner)
    }

    /// Where a session runs, from the host-app cache (resolved off the main
    /// actor; a session whose app isn't known yet shows none until the
    /// cache's next pass republishes).
    private func hostAppName(for state: SessionState) -> String? {
        if isSealed { return SampleSessions.hostApp(for: state.sessionId) }
        var host: HostApp?
        if !state.isInTmux, let pid = state.pid, case .some(let cached) = SessionHostCache.shared.host(forPid: pid) {
            host = cached
        }
        return ClaudeHostProjections.hostAppName(isInTmux: state.isInTmux, host: host, entrypoint: state.entrypoint)
    }

    /// Counts per ring, with every ring in `ringIDs` present. Pure.
    nonisolated static func ringCounts(sessions: [ClaudeSessionSummary], ringIDs: [String]) -> [String: ClaudeAttentionCounts] {
        var counts = Dictionary(grouping: sessions, by: \.ringID).mapValues { ClaudeAttentionCounts.of($0) }
        for ringID in ringIDs where counts[ringID] == nil {
            counts[ringID] = .zero
        }
        return counts
    }

    /// Per ring, until when its newest completion is "just finished":
    /// `freshSuccessWindow` after it, for completions still waiting for
    /// review; rings whose window has passed are absent. Pure.
    nonisolated static func freshSuccessUntil(sessions: [ClaudeSessionSummary], now: Date) -> [String: Date] {
        var fresh: [String: Date] = [:]
        for session in sessions where session.attention == .readyForReview {
            let until = session.attentionSince.addingTimeInterval(freshSuccessWindow)
            guard until > now else { continue }
            fresh[session.ringID] = max(fresh[session.ringID] ?? .distantPast, until)
        }
        return fresh
    }

    /// The next moment the projection changes with no new input: a ring's
    /// "just finished" window ends (the arc settles), or a usage window
    /// resets (it reads 0% again, a used-up limit lifts, and rate-limited
    /// rows lose their "resets 14:05"). Nil when neither is ahead. Pure.
    nonisolated static func nextBoundary(
        freshSuccessUntil: [String: Date],
        readings: [String: ClaudeRingReading],
        now: Date
    ) -> Date? {
        let resets = readings.values.flatMap(\.windows).compactMap(\.resetsAt)
        return (Array(freshSuccessUntil.values) + resets).filter { $0 > now }.min()
    }

    /// Republish at the next boundary (see `nextBoundary`).
    private func scheduleBoundary(_ next: Date?) {
        boundaryTask?.cancel()
        boundaryTask = nil
        guard let next else { return }
        let delay = max(0.05, next.timeIntervalSinceNow + 0.05)
        boundaryTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled else { return }
            self?.recompute()
        }
    }

    // MARK: - Transitions

    /// A session's attention as the engine sees it (the reason included),
    /// with its summary.
    nonisolated struct AttentionSnapshot: Equatable, Sendable {
        var attention: SessionAttention
        var summary: ClaudeSessionSummary
        /// The turn's completion, for "viewed in its own tab" and for
        /// telling completions from before launch.
        var completedAt: Date? = nil
        /// A /loop or cron tick, or a turn that scheduled a wake-up: stays in
        /// the review queue, never announced. (A turn waiting on background
        /// agents is still working.)
        var isQuietCompletion = false
    }

    private func emitTransitions(_ snapshots: [AttentionSnapshot]) {
        let current = Dictionary(snapshots.map { ($0.summary.id, $0) }, uniquingKeysWith: { first, _ in first })
        // The banners' baseline too: silent until every registry was read.
        // Sealed (no registries): until the fixtures are first in.
        let inBaseline = isSealed ? (previousAttention?.isEmpty ?? true) : AttentionTracker.shared.isInBaseline()
        let previous = inBaseline ? nil : previousAttention
        defer { previousAttention = current }
        let launchedAt = isSealed ? startedAt : AttentionTracker.shared.launchedAt
        for transition in Self.transitions(previous: previous, current: snapshots, launchedAt: launchedAt) {
            transitionSubject.send(transition)
        }
        // A turn that finished while its own tab was in front was watched:
        // that completion is reviewed (quiet ones too; never a later one).
        guard !isSealed, let previous else { return }
        for snapshot in snapshots where snapshot.attention == .readyForReview {
            guard let before = previous[snapshot.summary.id], before.attention != .readyForReview,
                  let completedAt = snapshot.completedAt else { continue }
            markViewedIfFocused(sessionId: snapshot.summary.id, completedAt: completedAt)
        }
    }

    // MARK: - Viewed in its own tab

    /// Mark the completion at `completedAt` viewed when the session's own
    /// tab or pane is the one in front (a tab-precise check: nothing is
    /// asked of a terminal unless it is the frontmost app).
    private func markViewedIfFocused(sessionId: String, completedAt: Date) {
        Task { [weak self] in
            guard let self, await self.isTerminalFocused(sessionId: sessionId) else { return }
            Self.logger.info("Viewed in its own tab: \(sessionId, privacy: .public)")
            ClaudeSessionMonitor.shared.markViewed(sessionId: sessionId, completedAt: completedAt)
        }
    }

    /// How long an app switch has to stay put before the sessions waiting
    /// for review in its front tab count as seen.
    nonisolated static let viewedDwell: TimeInterval = 1.5

    /// After an app activation that stays for `viewedDwell`, sessions ready
    /// for review whose own tab is now in front are viewed.
    private func observeActivations() {
        guard activationObserver == nil else { return }
        activationObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.scheduleViewedCheck() }
        }
    }

    private func scheduleViewedCheck() {
        viewedCheckTask?.cancel()
        viewedCheckTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(Self.viewedDwell))
            guard !Task.isCancelled, let self else { return }
            let candidates = ClaudeSessionMonitor.shared.instances.compactMap { state -> (String, Date)? in
                guard state.attention == .readyForReview, let completedAt = state.completedAt else { return nil }
                return (state.sessionId, completedAt)
            }
            for (sessionId, completedAt) in candidates where !Task.isCancelled {
                if await self.isTerminalFocused(sessionId: sessionId) {
                    Self.logger.info("Viewed after switching to its tab: \(sessionId, privacy: .public)")
                    ClaudeSessionMonitor.shared.markViewed(sessionId: sessionId, completedAt: completedAt)
                }
            }
        }
    }

    /// The crossings between two publishes, by `AttentionNews` (the rule
    /// the banners use too). Nil `previous` is the silent baseline; after
    /// it, a session seen for the first time is news like any other change
    /// (one that appears already needing input, or finished after launch).
    /// A session that goes away while it needed input or waited for review
    /// is resolved, with the last summary it had. Pure.
    nonisolated static func transitions(
        previous: [String: AttentionSnapshot]?,
        current: [AttentionSnapshot],
        launchedAt: Date? = nil
    ) -> [ClaudeAttentionTransition] {
        guard let previous else { return [] }
        var result: [ClaudeAttentionTransition] = []
        var seen: Set<String> = []
        func append(_ kinds: [AttentionNews.Kind], _ summary: ClaudeSessionSummary) {
            for kind in kinds {
                switch kind {
                case .needsInput: result.append(.init(kind: .needsInput, session: summary))
                case .readyForReview: result.append(.init(kind: .readyForReview, session: summary))
                case .resolved: result.append(.init(kind: .resolved, session: summary))
                }
            }
        }
        for snapshot in current {
            let id = snapshot.summary.id
            seen.insert(id)
            append(AttentionNews.kinds(from: previous[id]?.attention, to: snapshot.attention,
                                       isQuietCompletion: snapshot.isQuietCompletion,
                                       completedAt: snapshot.completedAt, launchedAt: launchedAt),
                   snapshot.summary)
        }
        for (id, gone) in previous.sorted(by: { $0.key < $1.key }) where !seen.contains(id) {
            append(AttentionNews.kinds(from: gone.attention, to: nil, isQuietCompletion: false,
                                       completedAt: nil, launchedAt: launchedAt),
                   gone.summary)
        }
        return result
    }

    // MARK: - Queries

    /// Hover-card rows for one ring; none for a ring that is switched off.
    public func activityRows(ringID: String, now: Date) -> [ClaudeActivityRow] {
        if let shownRings, !shownRings.contains(ringID) { return [] }
        return sessions
            .filter { $0.ringID == ringID }
            .map { ClaudeHostProjections.activityRow($0) }
    }

    public func session(pid: Int32) -> ClaudeSessionSummary? {
        sessions.first { $0.pid == pid }
    }

    /// The name to show for an account in banners: Codenotch's nickname for
    /// its ring, else the account's own label. Takes an account id or the
    /// id of one of its folders.
    func displayLabel(forAccountId accountId: String) -> String? {
        let id = AccountRegistry.shared.identityId(for: accountId) ?? accountId
        return accounts.first { $0.id == id || $0.configDirs.contains(accountId) }?.label
    }

    /// The account (identity) summary a folder belongs to.
    func account(forFolderId folderId: String) -> ClaudeAccountSummary? {
        let id = AccountRegistry.shared.identityId(for: folderId) ?? folderId
        return accounts.first { $0.id == id } ?? accounts.first { $0.configDirs.contains(folderId) }
    }

    // MARK: - Actions

    /// Bring the session's terminal to the front; marks it reviewed on success.
    @discardableResult
    public func focus(sessionId: String) async -> Bool {
        if isSealed {
            markReviewed(sessionId: sessionId)
            return true
        }
        guard let state = ClaudeSessionMonitor.shared.instances.first(where: { $0.sessionId == sessionId }) else {
            return false
        }
        return await SessionFocusService.shared.focus(state)
    }

    public func markReviewed(sessionId: String) {
        ClaudeSessionMonitor.shared.markReviewed(sessionId: sessionId)
    }

    /// Reviewed as of `date`, the moment the user asked (BHV-9: a deferred
    /// commit keeps turns that completed after the click unreviewed).
    public func markReviewed(sessionId: String, at date: Date) {
        ClaudeSessionMonitor.shared.markReviewed(sessionId: sessionId, at: date)
    }

    /// Dismiss a failed turn (rate limit, overload, sign-in): it stops
    /// counting as failed, and is not restored after a relaunch, until the
    /// next failure (GUX-2).
    public func dismissFailure(sessionId: String, at date: Date = Date()) {
        ClaudeSessionMonitor.shared.dismissFailure(sessionId: sessionId, at: date)
    }

    /// Whether the user is looking at this session's own tab or pane
    /// (session-precise: see `TerminalVisibilityDetector.isSessionFocused`).
    public func isTerminalFocused(sessionId: String) async -> Bool {
        guard !isSealed,
              let state = ClaudeSessionMonitor.shared.instances.first(where: { $0.sessionId == sessionId }),
              let probe = TerminalVisibilityDetector.probe(for: state)
        else { return false }
        return await TerminalVisibilityDetector.isSessionFocused(probe)
    }

    /// Whether a terminal or editor window is visible, and not covered, on
    /// the current space.
    public func isAnyTerminalVisible() async -> Bool {
        guard !isSealed else { return false }
        return TerminalVisibilityDetector.isTerminalVisibleOnCurrentSpace()
    }

    public func requestPanel(_ route: ClaudePanelRoute) {
        panelSubject.send(route)
    }

    /// Ask for fresh usage for one ring. `.ringClick` asks Claude Code only
    /// when the newest reading is more than 120 s old; `.forced` unless it
    /// was asked in the last 60 s. Caches, Claude Desktop's included, are
    /// re-read first. Waits at most 20 s for the answer.
    public func refreshUsage(ringID: String, reason: ClaudeRefreshReason) async {
        guard !isSealed, let accountId = accountId(forRingID: ringID) else { return }
        await UsageStore.shared.refresh(accountId: accountId, reason: reason)
    }

    /// The rings switched on in Codenotch. Activity rows go only to those,
    /// and accounts whose ring is off aren't probed or read from Claude
    /// Desktop until it is back.
    public func setShownRings(_ ringIDs: Set<String>) {
        guard shownRings != ringIDs else { return }
        shownRings = ringIDs
        applyPausedRings()
    }

    /// Codenotch's account nicknames, by ring id, for labels in the panel and banners.
    public func setNicknames(_ byRingID: [String: String]) {
        guard nicknames != byRingID else { return }
        nicknames = byRingID
        if started { recompute() }
    }

    private func accountId(forRingID ringID: String) -> String? {
        AccountRegistry.shared.identities.first { $0.ringID == ringID }?.id
    }

    /// Accounts whose ring isn't shown (none until the bridge says). Pure.
    nonisolated static func pausedAccountIds(accounts: [ClaudeAccountSummary], shownRings: Set<String>?) -> Set<String> {
        guard let shownRings else { return [] }
        return Set(accounts.filter { !shownRings.contains($0.ringID) }.map(\.id))
    }

    private func applyPausedRings() {
        guard !isSealed, started else { return }
        UsageStore.shared.setPausedAccounts(Self.pausedAccountIds(accounts: accounts, shownRings: shownRings))
    }

    // MARK: - Fixtures and helpers

    /// Sealed accounts: two identities spread over `~/.claude`, three VS
    /// Code windows and three Claude Parallel Profiles stores
    /// (`SampleLayout`), with custom labels; the sealed demo adds
    /// `SampleSessions.side` a few seconds in.
    static var fixtureAccounts: [ClaudeAccount] { SampleSessions.accounts }

    /// Fixture sessions shifted from `SampleData.now` to `now`, so their
    /// elapsed times and the 90 s window read as they do in the snapshots.
    static func freshened(_ session: SessionState, to now: Date) -> SessionState {
        let shift = now.timeIntervalSince(SampleData.now)
        var copy = session
        copy.turnStartedAt = copy.turnStartedAt?.addingTimeInterval(shift)
        copy.completedAt = copy.completedAt?.addingTimeInterval(shift)
        copy.reviewedAt = copy.reviewedAt?.addingTimeInterval(shift)
        copy.lastActivity = copy.lastActivity.addingTimeInterval(shift)
        copy.lastEventAt = copy.lastEventAt.addingTimeInterval(shift)
        if case .waitingForApproval(let context) = copy.phase {
            copy.phase = .waitingForApproval(PermissionContext(
                toolUseId: context.toolUseId,
                toolName: context.toolName,
                toolInput: context.toolInput,
                receivedAt: context.receivedAt.addingTimeInterval(shift),
                permissionSuggestions: context.permissionSuggestions,
                hasSyntheticToolUseId: context.hasSyntheticToolUseId
            ))
        }
        return copy
    }

}
