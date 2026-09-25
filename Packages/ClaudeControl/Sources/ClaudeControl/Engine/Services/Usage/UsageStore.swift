//
//  UsageStore.swift
//  ClaudeControl
//
//  Latest plan usage per account — per signed-in identity, whatever folders
//  it lives in (`ClaudeIdentityAccount`; every key here is an identity id) —
//  merged from four read-only sources, none of which involves a token:
//  1. `.claude.json` → `cachedUsageUtilization`, Claude Code's own snapshot,
//     from every folder of the identity, stores included (polled every 20 s,
//     re-parsed only when the file changes; the freshest one whose
//     accountUuid is the identity's wins);
//  2. status line `rate_limits` from the account's live sessions, in any of
//     its run folders (5-hour and 7-day windows only). A session in
//  `~/.claude` counts for the account it started as, not for whoever Claude
//  Parallel Profiles mirrored into `~/.claude` since (a running Claude Code
//  keeps its account); when that can't be told, its rate limits are left
//  out (`FolderAttribution`). A reading from the account's own folders
//  replaces one that came through `~/.claude`;
//  3. Claude Desktop's HTTP cache, through the host's
//     `ClaudeControlConfiguration.externalUsageSource`, for accounts whose
//     claude.ai organization is known: at most once a minute, every 5
//     minutes after a miss, and only with the "Also read Claude Desktop's
//     cached usage" setting on;
//  4. the usage probe (`claude -p` → `get_usage`), run once per identity when
//     the newest data is older than the `usageProbeIntervalMinutes` setting,
//     in one of its run folders (see `UsageProbePlanner`: with Claude
//     Parallel Profiles, a VS Code window's folder, `~/.claude` only when it
//     has none) — never in a Claude Parallel Profiles store, whose config and
//     token must not churn. Who the folder is signed in as is read again
//     right before and after the probe; an answer from a folder that changed
//     hands meanwhile is thrown away. An identity with no run folder is not
//     probed; its passive data shows with its age.
//  Per window the most current reading wins (see `isMoreCurrent`); model-scoped
//  limits and extra usage come from the latest full snapshot (a cache, Claude
//  Desktop or the probe). A fresh reading from any source holds off the probe.
//
//  Probes run one at a time, at most every 5 minutes per account on their own
//  schedule (the usage endpoint answers 429 to tighter polling), with
//  exponential backoff on failure, and never for an account whose ring is
//  switched off (`setPausedAccounts`). The last probe time, the backoff and
//  the last full reading persist in `usage-state.json`, so a relaunch neither
//  probes early nor starts empty. Dev runs (`--no-install`) only probe on
//  request unless `SPCN_USAGE_PROBE=1`.
//

import Combine
import Foundation
import os.log

@MainActor
final class UsageStore: ObservableObject {
    static let shared = UsageStore()

    private static var logger: Logger { EngineLog.logger("Usage") }

    /// Merged usage per account ID.
    @Published private(set) var usage: [String: AccountUsage] = [:]
    /// Probe status per account ID (absent = never probed).
    @Published private(set) var fetchState: [String: UsageFetchState] = [:]
    /// When each account was last probed (success or not).
    @Published private(set) var lastProbeAt: [String: Date] = [:]
    /// The claude.ai organization of each signed-in account, read while
    /// Claude Desktop readings are on (the Desktop cache is keyed by it).
    @Published private(set) var organizationUuids: [String: String] = [:]

    // MARK: - Tuning

    /// How often `.claude.json` is checked for a new cached snapshot.
    nonisolated static let cachePollInterval: TimeInterval = 20
    /// How often the probe schedule is re-evaluated.
    nonisolated static let schedulerInterval: TimeInterval = 30
    /// Scheduled probes never run more often than this per account.
    nonisolated static let minimumProbeInterval: TimeInterval = 5 * 60
    /// `refresh` probes at most this often per account.
    nonisolated static let forcedRefreshInterval: TimeInterval = 60
    /// A ring click probes only when the newest reading is older than this.
    nonisolated static let ringClickFreshness: TimeInterval = 120
    /// How long `refresh(accountId:reason:)` waits for the probe's answer.
    nonisolated static let refreshWaitLimit: TimeInterval = 20
    /// Backoff after the first failure; doubles per failure up to `maxBackoff`.
    nonisolated static let initialBackoff: TimeInterval = 2 * 60
    nonisolated static let maxBackoff: TimeInterval = 30 * 60
    /// Model-scoped limits only come with full snapshots; probe for one when
    /// the last is older than this, even while status lines keep 5h/7d fresh.
    nonisolated static let fullSnapshotMaxAge: TimeInterval = 15 * 60
    /// Claude Desktop's cache is read at most this often per account...
    nonisolated static let externalPollInterval: TimeInterval = 60
    /// ...and this often after a read that found nothing.
    nonisolated static let externalMissInterval: TimeInterval = 5 * 60
    /// A seeded probe answer dated by `.claude.json` within this long of the
    /// probe is Claude Code's own fresh cache, not a fallback.
    nonisolated static let seededFreshWindow: TimeInterval = 90
    /// Persisted state is written this long after the last change.
    nonisolated static let stateSaveDelay: TimeInterval = 2
    /// A check thrown away because its folder changed hands is tried again
    /// after this long at the earliest.
    nonisolated static let discardRetryDelay: TimeInterval = 60

    /// Shown for accounts whose `.claude.json` has no claude.ai login.
    nonisolated static let notSignedIn = UsageFetchState.unavailable("Not signed in to Claude")

    /// An account only its Claude Parallel Profiles store holds (no VS Code
    /// window, and `~/.claude` runs as someone else): never checked there.
    nonisolated static let noRunFolderText = "Not checked: no VS Code workspace or terminal folder runs this account now"
    nonisolated static let noRunFolder = UsageFetchState.unavailable(noRunFolderText)

    /// Whether probes run on their own schedule by default. Dev runs
    /// (`--no-install`) leave them off: a dev build next to the real app would
    /// double the probing of the same accounts (the usage endpoint answers 429
    /// to tight polling), and each probe has Claude Code touch the account's
    /// config. `SPCN_USAGE_PROBE=1` turns them back on; `refresh` always probes.
    nonisolated static var automaticProbesAllowedByDefault: Bool {
        // Sealed runs never launch Claude Code (see ClaudeControlConfiguration).
        if DevFlags.probesDisabled { return false }
        guard DevFlags.installsDisabled else { return true }
        return DevFlags.usageProbeOnDevRun
    }

    /// A status line window and when the app last saw it change.
    typealias Reading = (window: UsageWindow, at: Date)

    /// One probe to run: the account and the folders Claude Code's binary is
    /// looked for next to.
    struct ProbeRequest: Sendable {
        let accountId: String
        /// Raw CLAUDE_CONFIG_DIR, nil for the default account.
        let configDirEnv: String?
        let configDirs: [String]
        let workingDirectory: URL
    }

    typealias ProbeRunner = @Sendable (ProbeRequest) async -> UsageProbe.Outcome

    /// When a session's Claude Code process started (from the session list).
    typealias SessionStart = @MainActor (String) -> Date?

    // MARK: - Dependencies

    private let registry: AccountRegistry
    private let configReader: ClaudeGlobalConfigReader
    private let stateStoreOverride: UsageStateStore?
    private let externalSourceOverride: (any ClaudeExternalUsageSource)?
    private let readsExternalUsage: () -> Bool
    private let probeRunner: ProbeRunner
    private let automaticProbes: Bool
    private let clock: () -> Date
    private let refreshWaitLimit: TimeInterval
    private let sessionStartedAt: SessionStart

    /// Claude Desktop's cache, when the host provides one.
    private var externalSource: (any ClaudeExternalUsageSource)? {
        externalSourceOverride ?? AppIdentity.configuration.externalUsageSource
    }

    private lazy var stateStore: UsageStateStore = stateStoreOverride
        ?? UsageStateStore(directory: AppIdentity.supportDirectory)

    // MARK: - State

    /// Latest full snapshot (cache, Claude Desktop or probe) per account.
    private var fullSnapshots: [String: AccountUsage] = [:]
    /// A folder's most current status line windows, with when they last changed.
    struct StatusLineReadings {
        var fiveHour: Reading?
        var sevenDay: Reading?
    }
    /// Per account, per folder the sessions ran in.
    private var statusLine: [String: [String: StatusLineReadings]] = [:]
    /// Who each folder ran as when last seen (a reading from a folder that
    /// changed hands since is dropped).
    private var identityOfFolder: [String: String] = [:]
    /// Whether the account's `.claude.json` shows a signed-in claude.ai login.
    /// Absent until the first cache poll has looked.
    private var signedIn: [String: Bool] = [:]

    private var failureCount: [String: Int] = [:]
    private var nextAttemptAt: [String: Date] = [:]
    /// Accounts waiting for a forced probe, in request order.
    private var forcedQueue: [String] = []
    private var probingAccountId: String?
    /// Accounts whose ring is switched off: never probed or read from Desktop.
    private var pausedAccountIds: Set<String> = []

    private var lastExternalPollAt: [String: Date] = [:]
    private var externalMisses: Set<String> = []
    private var externalPollInFlight = false

    /// Callers of `refresh(accountId:reason:)` waiting for an account's probe.
    private var probeWaiters: [String: [ProbeWaiter]] = [:]

    private var started = false
    /// Set by `stop()` (quitting): no new probe starts after it, not even
    /// the next one in the queue when a running probe finishes.
    private var stopped = false
    private var cancellables = Set<AnyCancellable>()
    private var cachePollTask: Task<Void, Never>?
    private var schedulerTask: Task<Void, Never>?
    private var probeTask: Task<Void, Never>?
    private var saveTask: Task<Void, Never>?
    private var cachePollInFlight = false

    /// - Parameters:
    ///   - stateStore: where `usage-state.json` lives (default: the engine's folder).
    ///   - externalSource: Claude Desktop's cache (default: the configuration's).
    ///   - readsExternalUsage: the "Also read Claude Desktop's cached usage" setting.
    ///   - probeRunner: runs one probe (default: Claude Code's `get_usage`).
    ///   - automaticProbes: probes on a schedule (default: see `automaticProbesAllowedByDefault`).
    init(
        registry: AccountRegistry? = nil,
        configReader: ClaudeGlobalConfigReader = .shared,
        stateStore: UsageStateStore? = nil,
        externalSource: (any ClaudeExternalUsageSource)? = nil,
        readsExternalUsage: (() -> Bool)? = nil,
        probeRunner: ProbeRunner? = nil,
        automaticProbes: Bool? = nil,
        refreshWaitLimit: TimeInterval = UsageStore.refreshWaitLimit,
        clock: @escaping () -> Date = { Date() },
        sessionStartedAt: SessionStart? = nil
    ) {
        self.registry = registry ?? .shared
        self.configReader = configReader
        self.stateStoreOverride = stateStore
        self.externalSourceOverride = externalSource
        self.readsExternalUsage = readsExternalUsage ?? { ClaudeControlSettings.readsDesktopUsageCache }
        self.probeRunner = probeRunner ?? UsageStore.runClaudeCodeProbe
        self.automaticProbes = automaticProbes ?? Self.automaticProbesAllowedByDefault
        self.refreshWaitLimit = refreshWaitLimit
        self.clock = clock
        self.sessionStartedAt = sessionStartedAt ?? { id in
            ClaudeSessionMonitor.shared.instances.first { $0.sessionId == id }?.pidStartedAt
        }
    }

    // MARK: - Lifecycle

    func start() {
        guard !started else { return }
        started = true
        stopped = false
        if !automaticProbes {
            Self.logger.notice("Automatic usage probes are off for this run (set SPCN_USAGE_PROBE=1 to enable)")
        }
        restoreState()

        AppEventBus.shared.statusLineUpdates
            .receive(on: DispatchQueue.main)
            .sink { [weak self] update in
                self?.ingest(update)
            }
            .store(in: &cancellables)

        registry.$identities
            .map { identities in identities.map { "\($0.id)|\($0.isHidden)|\($0.folderIds.joined(separator: ","))" } }
            .removeDuplicates()
            .dropFirst()
            .debounce(for: .seconds(1), scheduler: DispatchQueue.main)
            .sink { [weak self] _ in
                self?.accountsChanged()
            }
            .store(in: &cancellables)

        cachePollTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.pollCycle()
                try? await Task.sleep(for: .seconds(Self.cachePollInterval))
            }
        }
        schedulerTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(Self.schedulerInterval))
                self?.scheduleProbes()
            }
        }
    }

    func stop() {
        cachePollTask?.cancel()
        schedulerTask?.cancel()
        cachePollTask = nil
        schedulerTask = nil
        cancellables.removeAll()
        if saveTask != nil {
            saveStateNow()
        }
        for id in forcedQueue where fetchState[id] == .fetching {
            fetchState[id] = .idle
        }
        forcedQueue.removeAll()
        for id in Array(probeWaiters.keys) { resumeWaiters(for: id) }
        started = false
        stopped = true
    }

    /// Write `usage-state.json` now instead of after the usual delay.
    func saveStateNow() {
        saveTask?.cancel()
        saveTask = nil
        stateStore.saveNow(persistedState())
    }

    // MARK: - Queries

    func usage(for accountId: String) -> AccountUsage? {
        usage[accountId]
    }

    func fetchState(for accountId: String) -> UsageFetchState {
        fetchState[accountId] ?? .idle
    }

    /// Any account is being probed, or waiting for a requested probe.
    var isFetching: Bool {
        fetchState.values.contains(.fetching)
    }

    /// A probe is running now.
    var isProbing: Bool { probeTask != nil }

    /// When the account's newest reading of any kind was taken.
    func newestDataAt(_ accountId: String) -> Date? {
        [fullSnapshots[accountId]?.updatedAt, statusReading(accountId, \.fiveHour)?.at, statusReading(accountId, \.sevenDay)?.at]
            .compactMap { $0 }
            .max()
    }

    /// An account's status line window over every folder its sessions ran
    /// in (see `combinedStatus`).
    private func statusReading(_ accountId: String, _ window: KeyPath<StatusLineReadings, Reading?>) -> Reading? {
        guard let folders = statusLine[accountId] else { return nil }
        return Self.combinedStatus(folders.compactMapValues { $0[keyPath: window] },
                                   defaultFolder: AccountRegistry.defaultConfigDir(home: registry.homePath),
                                   mirrorsDefault: registry.mirrorsDefault)
    }

    /// The most current reading of a window over the folders an account's
    /// sessions ran in. While Claude Parallel Profiles mirrors accounts into
    /// `~/.claude`, a reading that came through `~/.claude` counts only
    /// until one from the account's own folders (its VS Code windows,
    /// standalone folders) arrives after it. Pure.
    nonisolated static func combinedStatus(_ byFolder: [String: Reading], defaultFolder: String, mirrorsDefault: Bool) -> Reading? {
        var readings = byFolder
        if mirrorsDefault, let viaDefault = readings[defaultFolder],
           let newestOwn = readings.filter({ $0.key != defaultFolder }).map(\.value.at).max(), newestOwn >= viaDefault.at {
            readings.removeValue(forKey: defaultFolder)
        }
        return readings.sorted { $0.key < $1.key }.reduce(nil) { stored, entry in combine(stored: stored, new: entry.value) }
    }

    /// How old a reading may get before it shows as stale, for the current
    /// probe setting (see `AccountUsage.staleThreshold`).
    var staleThreshold: TimeInterval {
        let minutes = automaticProbes ? ClaudeControlSettings.usageProbeIntervalMinutes : 0
        return AccountUsage.staleThreshold(probeIntervalMinutes: minutes)
    }

    // MARK: - Requests

    /// Ask Claude Code for fresh usage now: one account, or every visible
    /// account whose ring is on when nil. Clears any failure backoff. An
    /// account probed less than a minute ago isn't probed again (its caches
    /// are still re-read).
    func refresh(accountId: String? = nil) {
        let targets = (accountId.map { [$0] } ?? registry.visibleIdentities.map(\.id).filter { !pausedAccountIds.contains($0) })
            .filter { registry.identity(id: $0) != nil }
        for id in targets {
            enqueueForcedProbe(id, clearingBackoff: true)
        }
        Task {
            await pollCaches()
            await pollExternal(only: Set(targets), force: true)
            runNextProbe()
        }
    }

    /// Fresh usage for one account, on request:
    /// - `.ringClick` asks Claude Code only when the newest reading (of any
    ///   source) is more than 2 minutes old, and respects a running backoff;
    /// - `.forced` asks unless Claude Code was asked in the last minute, and
    ///   clears the backoff.
    /// Neither asks for an account whose ring is switched off. Caches (and
    /// Claude Desktop's) are re-read first, since they cost nothing. Returns
    /// once the probe answered, or after 20 s at most.
    func refresh(accountId: String, reason: ClaudeRefreshReason) async {
        guard registry.identity(id: accountId) != nil else { return }
        await pollCaches()
        await pollExternal(only: [accountId], force: true)
        // Claude Code is never asked about an account whose ring is off.
        guard !pausedAccountIds.contains(accountId) else { return }

        let now = clock()
        switch reason {
        case .ringClick:
            if let newest = newestDataAt(accountId), now.timeIntervalSince(newest) <= Self.ringClickFreshness { return }
            if let next = nextAttemptAt[accountId], next > now { return }
            enqueueForcedProbe(accountId, clearingBackoff: false)
        case .forced:
            enqueueForcedProbe(accountId, clearingBackoff: true)
        }
        runNextProbe()
        await waitForProbe(accountId: accountId, timeout: refreshWaitLimit)
    }

    /// The accounts whose ring is switched off: no probe and no Desktop read
    /// runs for them until they are back.
    func setPausedAccounts(_ ids: Set<String>) {
        guard ids != pausedAccountIds else { return }
        let resumed = pausedAccountIds.subtracting(ids)
        pausedAccountIds = ids
        // Requests still queued for rings just switched off are dropped, and
        // with them their "checking…" state and anyone waiting on them.
        let dropped = forcedQueue.filter { ids.contains($0) && probingAccountId != $0 }
        forcedQueue.removeAll { dropped.contains($0) }
        for id in dropped {
            if fetchState[id] == .fetching { fetchState[id] = .idle }
            resumeWaiters(for: id)
        }
        if !resumed.isEmpty, started {
            Task {
                await pollExternal(only: resumed, force: false)
                scheduleProbes()
            }
        }
    }

    /// Change the probe interval (minutes, 0 = off) and apply it now.
    func setProbeInterval(minutes: Int) {
        ClaudeControlSettings.usageProbeIntervalMinutes = minutes
        scheduleProbes()
    }

    /// The effective automatic probe interval, or nil when probing is off.
    nonisolated static func effectiveProbeInterval(minutes: Int) -> TimeInterval? {
        guard minutes > 0 else { return nil }
        return max(TimeInterval(minutes) * 60, minimumProbeInterval)
    }

    /// Delay before retrying after `failures` consecutive failures.
    nonisolated static func backoff(afterFailures failures: Int) -> TimeInterval {
        guard failures > 0 else { return 0 }
        let exponent = min(failures - 1, 10)
        return min(initialBackoff * pow(2, Double(exponent)), maxBackoff)
    }

    // MARK: - Status Line

    /// A live terminal session reported its account's rate limits.
    func ingest(_ update: StatusLineUpdate) {
        guard update.fiveHour != nil || update.sevenDay != nil else { return }
        let folderId = AccountPaths.normalize(update.accountId
            ?? update.configDirEnv.flatMap { $0.isEmpty ? nil : AccountPaths.accountId(forConfigDir: $0) }
            ?? AccountPaths.accountId(forConfigDir: registry.homePath + "/.claude"))
        // The account the session runs as: a window's folder is whoever it
        // names now (a window that switches account reloads); ~/.claude is
        // whoever it named when the session started.
        let accountId: String
        switch registry.attribution(forFolderId: folderId, startedAt: sessionStartedAt(update.sessionId)) {
        case .known(let identity?):
            accountId = identity
        case .known(nil):
            // A folder not grouped yet: kept under the folder, moved to its
            // identity once it has one.
            accountId = registry.identityId(for: folderId) ?? folderId
        case .unsure:
            Self.logger.debug("Rate limits from ~/.claude session \(update.sessionId.prefix(8), privacy: .public) left out: ~/.claude changed hands around when it started")
            return
        }

        var readings = statusLine[accountId]?[folderId] ?? StatusLineReadings()
        if let window = update.fiveHour {
            readings.fiveHour = Self.combine(stored: readings.fiveHour, new: (window, update.receivedAt))
        }
        if let window = update.sevenDay {
            readings.sevenDay = Self.combine(stored: readings.sevenDay, new: (window, update.receivedAt))
        }
        statusLine[accountId, default: [:]][folderId] = readings
        publish(accountId)
    }

    // MARK: - Merging

    /// Two readings describe the same window when their reset times are close:
    /// consecutive windows reset at least a full window length apart, while
    /// sources disagree by at most rounding (epoch seconds vs. fractional ISO).
    nonisolated static func isSameWindow(_ lhs: UsageWindow, _ rhs: UsageWindow) -> Bool {
        guard let left = lhs.resetsAt, let right = rhs.resetsAt else { return false }
        return abs(left.timeIntervalSince(right)) < min(lhs.duration, rhs.duration) / 4
    }

    /// Whether `candidate` is a more current reading of a window than `other`.
    ///
    /// Arrival time alone can't decide: a status line re-run (a permission-mode
    /// change, a `refreshInterval` tick, an idle session) repeats whatever that
    /// session's last API response said, possibly hours ago. So the data decides
    /// first: a later reset means a newer window, and within one window usage
    /// only grows, so the higher reading is the more recent. Arrival time breaks
    /// ties and covers readings without a reset time.
    nonisolated static func isMoreCurrent(_ candidate: Reading, than other: Reading) -> Bool {
        if let candidateReset = candidate.window.resetsAt, let otherReset = other.window.resetsAt {
            if !isSameWindow(candidate.window, other.window) {
                return candidateReset > otherReset
            }
            if candidate.window.utilization != other.window.utilization {
                return candidate.window.utilization > other.window.utilization
            }
        }
        return candidate.at > other.at
    }

    /// Fold a new status line reading into the stored one. A reading equal to
    /// the stored one keeps the stored time, so a session repeating stale rate
    /// limits neither looks fresh nor holds off the probe. Pure.
    nonisolated static func combine(stored: Reading?, new: Reading) -> Reading {
        guard let stored else { return new }
        if stored.window == new.window { return stored }
        return isMoreCurrent(new, than: stored) ? new : stored
    }

    /// Recompute the published usage for one account from its sources.
    private func publish(_ accountId: String) {
        let merged = Self.merge(
            accountId: accountId,
            full: fullSnapshots[accountId],
            statusFiveHour: statusReading(accountId, \.fiveHour),
            statusSevenDay: statusReading(accountId, \.sevenDay)
        )
        if usage[accountId] != merged {
            usage[accountId] = merged
        }
    }

    /// The most current reading wins per window (`isMoreCurrent`); scoped
    /// limits, extra usage and plan come from the full snapshot.
    /// `updatedAt`/`source` describe the newest reading used. Pure.
    nonisolated static func merge(
        accountId: String,
        full: AccountUsage?,
        statusFiveHour: Reading?,
        statusSevenDay: Reading?
    ) -> AccountUsage? {
        guard full != nil || statusFiveHour != nil || statusSevenDay != nil else { return nil }

        var result = full ?? AccountUsage(accountId: accountId, source: .statusLine, updatedAt: .distantPast)
        var newest = full?.updatedAt ?? .distantPast
        var newestSource = full?.source ?? .statusLine

        func take(_ reading: Reading?, current: UsageWindow?) -> UsageWindow? {
            guard let reading else { return current }
            if let current, let full, !isMoreCurrent(reading, than: (current, full.updatedAt)) {
                return current
            }
            if reading.at > newest {
                newest = reading.at
                newestSource = .statusLine
            }
            return reading.window
        }
        result.fiveHour = take(statusFiveHour, current: result.fiveHour)
        result.sevenDay = take(statusSevenDay, current: result.sevenDay)
        result.updatedAt = newest
        result.source = newestSource
        return result
    }

    /// Keep `snapshot` as the account's full snapshot if it is newer than
    /// the one held. Returns whether it was taken.
    @discardableResult
    private func acceptFullSnapshot(_ snapshot: AccountUsage) -> Bool {
        guard Self.isNewerFullSnapshot(snapshot, than: fullSnapshots[snapshot.accountId]) else { return false }
        fullSnapshots[snapshot.accountId] = snapshot
        publish(snapshot.accountId)
        scheduleSave()
        let age = Int(clock().timeIntervalSince(snapshot.updatedAt))
        Self.logger.info("Usage for \(snapshot.accountId, privacy: .public) from \(snapshot.source.rawValue, privacy: .public), \(age)s old")
        return true
    }

    /// Full snapshots replace each other by when they were taken, whichever
    /// source they came from (Claude Desktop, `.claude.json`, the probe). Pure.
    nonisolated static func isNewerFullSnapshot(_ snapshot: AccountUsage, than current: AccountUsage?) -> Bool {
        guard let current else { return true }
        return snapshot.updatedAt > current.updatedAt
    }

    // MARK: - Caches

    /// One pass of the regular cycle: caches, Claude Desktop, then the schedule.
    func pollCycle() async {
        await pollCaches()
        await pollExternal(only: nil, force: false)
        scheduleProbes()
    }

    /// Read every account's `.claude.json` (off the main actor; re-parsed only
    /// when changed): refresh the registry's identity, take a newer
    /// `cachedUsageUtilization` snapshot and, while Claude Desktop readings
    /// are on, the account's organization.
    private func pollCaches() async {
        guard !cachePollInFlight else { return }
        cachePollInFlight = true
        defer { cachePollInFlight = false }

        let wantsOrganizations = externalSource != nil && readsExternalUsage()
        // Every folder, stores included: an account with no open window
        // still has Claude Code's cached snapshot in its store.
        let targets = registry.accounts.map { ($0.id, registry.globalConfigPath(for: $0)) }
        let reader = configReader
        let results: [(String, ClaudeGlobalConfig?)] = await Task.detached(priority: .utility) {
            targets.map { id, path in (id, reader.read(path: path)) }
        }.value

        // Who is signed in where first: the identities below follow from it.
        var configs: [String: ClaudeGlobalConfig] = [:]
        for (id, config) in results {
            registry.applyIdentity(config?.identity, forAccountId: id, modifiedAt: config?.modifiedAt)
            if let config { configs[id] = config }
        }

        var organizations: [String: String] = [:]
        for identity in registry.identities {
            let id = identity.id
            let isSignedIn = identity.isSignedIn
            let wasSignedIn = signedIn[id]
            signedIn[id] = isSignedIn
            let state = Self.fetchStateAfterCachePoll(
                current: fetchState[id],
                wasSignedIn: wasSignedIn,
                isSignedIn: isSignedIn
            )
            if fetchState[id] != state {
                fetchState[id] = state
            }
            if wantsOrganizations { organizations[id] = identity.organizationUuid }
            if let cached = Self.freshestCachedUsage(identity: identity, configs: configs) {
                acceptFullSnapshot(cached.usage.accountUsage(accountId: id, source: .cache, updatedAt: cached.fetchedAt))
            }
        }
        if organizations != organizationUuids {
            organizationUuids = organizations
        }
    }

    /// Claude Code's cached snapshot for an identity: of its folders'
    /// `cachedUsageUtilization`s (stores included), the freshest whose
    /// accountUuid is the identity's. Pure.
    nonisolated static func freshestCachedUsage(
        identity: ClaudeIdentityAccount,
        configs: [String: ClaudeGlobalConfig]
    ) -> CachedUsageSnapshot? {
        guard let uuid = identity.accountUuid?.lowercased(), !uuid.isEmpty else { return nil }
        // One login in two organizations: only the folders of this one.
        let folders = identity.folders.filter { folder in
            identity.organizationScope.map { folder.organizationUuid?.lowercased() == $0 } ?? true
        }
        return folders
            .compactMap { configs[$0.id]?.cachedUsage }
            .filter { $0.accountUuid?.lowercased() == uuid }
            .max { $0.fetchedAt < $1.fetchedAt }
    }

    /// The fetch state after a `.claude.json` read. Signed out shows as
    /// unavailable; signing (back) in clears that. A signed-in account keeps
    /// whatever the probe last reported (including a probe's own "not signed
    /// in", e.g. a revoked login the file doesn't know about yet). Pure.
    nonisolated static func fetchStateAfterCachePoll(
        current: UsageFetchState?,
        wasSignedIn: Bool?,
        isSignedIn: Bool
    ) -> UsageFetchState? {
        if !isSignedIn { return notSignedIn }
        if wasSignedIn != true, current == notSignedIn { return .idle }
        return current
    }

    /// Whether Claude Desktop's cache is due a read for an account. Pure.
    nonisolated static func isExternalPollDue(lastPollAt: Date?, lastWasMiss: Bool, now: Date) -> Bool {
        guard let lastPollAt else { return true }
        let interval = lastWasMiss ? externalMissInterval : externalPollInterval
        return now.timeIntervalSince(lastPollAt) >= interval
    }

    /// Read Claude Desktop's cache for the accounts that are due (or for
    /// `only`, right away when `force`). A reading is a full snapshot dated
    /// when Desktop saw it, so an older one never replaces a newer probe.
    private func pollExternal(only: Set<String>?, force: Bool) async {
        guard let source = externalSource, readsExternalUsage(), !externalPollInFlight else { return }
        externalPollInFlight = true
        defer { externalPollInFlight = false }

        for account in registry.visibleIdentities {
            let id = account.id
            guard only.map({ $0.contains(id) }) ?? true,
                  !pausedAccountIds.contains(id),
                  signedIn[id] == true,
                  let organization = organizationUuids[id] else { continue }
            let now = clock()
            // A forced read still leaves Desktop alone for a few seconds.
            let due = force
                ? now.timeIntervalSince(lastExternalPollAt[id] ?? .distantPast) >= 5
                : Self.isExternalPollDue(lastPollAt: lastExternalPollAt[id], lastWasMiss: externalMisses.contains(id), now: now)
            guard due else { continue }
            lastExternalPollAt[id] = now
            let reading = await source.reading(organizationUuid: organization, now: now)
            if let reading, let snapshot = UsageRingWindows.accountUsage(from: reading, accountId: id) {
                externalMisses.remove(id)
                acceptFullSnapshot(snapshot)
            } else {
                externalMisses.insert(id)
            }
        }
    }

    private func accountsChanged() {
        adoptFolderKeyedState()
        let known = Set(registry.identities.map(\.id))
        for id in Array(usage.keys) where !known.contains(id) {
            usage.removeValue(forKey: id)
        }
        for id in Array(fetchState.keys) where !known.contains(id) {
            fetchState.removeValue(forKey: id)
        }
        for id in Array(lastProbeAt.keys) where !known.contains(id) {
            lastProbeAt.removeValue(forKey: id)
        }
        fullSnapshots = fullSnapshots.filter { known.contains($0.key) }
        statusLine = statusLine.filter { known.contains($0.key) }
        dropReadingsOfFoldersThatChangedHands()
        signedIn = signedIn.filter { known.contains($0.key) }
        failureCount = failureCount.filter { known.contains($0.key) }
        nextAttemptAt = nextAttemptAt.filter { known.contains($0.key) }
        lastExternalPollAt = lastExternalPollAt.filter { known.contains($0.key) }
        externalMisses = externalMisses.filter { known.contains($0) }
        let organizations = organizationUuids.filter { known.contains($0.key) }
        if organizations != organizationUuids { organizationUuids = organizations }
        forcedQueue.removeAll { !known.contains($0) }
        scheduleSave()
        Task { await pollCycle() }
    }

    /// A folder that changed hands (a VS Code window switched account; a
    /// login in a standalone folder) takes the readings it gave its old
    /// account with it: they were read under a mapping that no longer
    /// holds. `~/.claude`, while mirrored into, is attributed per session
    /// instead (see `ingest`), so its readings stay.
    private func dropReadingsOfFoldersThatChangedHands() {
        var now: [String: String] = [:]
        for identity in registry.identities {
            for folder in identity.folders { now[folder.id] = identity.id }
        }
        let defaultFolder = AccountRegistry.defaultConfigDir(home: registry.homePath)
        for (folder, identity) in now where identityOfFolder[folder] != nil && identityOfFolder[folder] != identity {
            guard !(folder == defaultFolder && registry.mirrorsDefault) else { continue }
            for account in Array(statusLine.keys) where account != identity && statusLine[account]?[folder] != nil {
                statusLine[account]?.removeValue(forKey: folder)
                publish(account)
            }
        }
        identityOfFolder = now
    }

    /// State recorded under a folder id (a status line from a folder the
    /// registry had not grouped yet) moves to the folder's identity.
    private func adoptFolderKeyedState() {
        for (id, folders) in statusLine where registry.identity(id: id) == nil {
            guard let identityId = registry.identityId(for: id) else { continue }
            for (folder, readings) in folders {
                var merged = statusLine[identityId]?[folder] ?? StatusLineReadings()
                if let reading = readings.fiveHour { merged.fiveHour = Self.combine(stored: merged.fiveHour, new: reading) }
                if let reading = readings.sevenDay { merged.sevenDay = Self.combine(stored: merged.sevenDay, new: reading) }
                statusLine[identityId, default: [:]][folder] = merged
            }
            statusLine.removeValue(forKey: id)
            publish(identityId)
        }
        for (id, snapshot) in fullSnapshots where registry.identity(id: id) == nil {
            guard let identityId = registry.identityId(for: id) else { continue }
            fullSnapshots.removeValue(forKey: id)
            var moved = snapshot
            moved.accountId = identityId
            if Self.isNewerFullSnapshot(moved, than: fullSnapshots[identityId]) {
                fullSnapshots[identityId] = moved
                publish(identityId)
            }
        }
        for (id, date) in lastProbeAt where registry.identity(id: id) == nil {
            guard let identityId = registry.identityId(for: id) else { continue }
            lastProbeAt[identityId] = max(lastProbeAt[identityId] ?? .distantPast, date)
        }
    }

    // MARK: - Probe Scheduling

    /// Which account to probe next on the schedule, if any: the signed-in,
    /// visible, unpaused account with the oldest data, among those whose
    /// newest reading is older than the interval (or whose last full
    /// snapshot is older than 15 minutes), outside their backoff and not
    /// probed in the last 5 minutes. Pure.
    nonisolated static func nextScheduledProbe(
        candidates: [String],
        interval: TimeInterval,
        now: Date,
        signedIn: [String: Bool],
        paused: Set<String>,
        nextAttemptAt: [String: Date],
        lastProbeAt: [String: Date],
        newestDataAt: [String: Date],
        newestFullAt: [String: Date]
    ) -> String? {
        let due = candidates.filter { id in
            guard signedIn[id] == true, !paused.contains(id) else { return false }
            if let next = nextAttemptAt[id], next > now { return false }
            if let last = lastProbeAt[id], now.timeIntervalSince(last) < minimumProbeInterval { return false }
            let newest = newestDataAt[id] ?? .distantPast
            let newestFull = newestFullAt[id] ?? .distantPast
            return now.timeIntervalSince(newest) >= interval
                || now.timeIntervalSince(newestFull) >= max(interval, fullSnapshotMaxAge)
        }
        return due.min { (newestDataAt[$0] ?? .distantPast) < (newestDataAt[$1] ?? .distantPast) }
    }

    /// Queue the most out-of-date account that is due, if nothing is probing.
    private func scheduleProbes() {
        guard probeTask == nil else { return }
        if !forcedQueue.isEmpty {
            runNextProbe()
            return
        }
        guard automaticProbes,
              let interval = Self.effectiveProbeInterval(minutes: ClaudeControlSettings.usageProbeIntervalMinutes) else { return }
        // Only identities Claude Code runs somewhere: a store is never probed.
        let ids = registry.visibleIdentities.filter { !$0.runDirs.isEmpty }.map(\.id)
        var newest: [String: Date] = [:]
        var newestFull: [String: Date] = [:]
        for id in ids {
            newest[id] = newestDataAt(id)
            newestFull[id] = fullSnapshots[id]?.updatedAt
        }
        guard let next = Self.nextScheduledProbe(
            candidates: ids,
            interval: interval,
            now: clock(),
            signedIn: signedIn,
            paused: pausedAccountIds,
            nextAttemptAt: nextAttemptAt,
            lastProbeAt: lastProbeAt,
            newestDataAt: newest,
            newestFullAt: newestFull
        ) else { return }
        startProbe(accountId: next)
    }

    private func enqueueForcedProbe(_ id: String, clearingBackoff: Bool) {
        if clearingBackoff {
            failureCount[id] = 0
            nextAttemptAt[id] = nil
        }
        if let last = lastProbeAt[id], clock().timeIntervalSince(last) < Self.forcedRefreshInterval {
            return
        }
        guard !forcedQueue.contains(id), probingAccountId != id else { return }
        forcedQueue.append(id)
        if fetchState[id] != .fetching {
            fetchState[id] = .fetching
        }
    }

    private func runNextProbe() {
        guard probeTask == nil else { return }
        while let id = forcedQueue.first {
            forcedQueue.removeFirst()
            guard registry.identity(id: id) != nil else {
                // Forgotten while it waited; don't leave a spinner behind.
                fetchState.removeValue(forKey: id)
                resumeWaiters(for: id)
                continue
            }
            if signedIn[id] == false {
                fetchState[id] = Self.notSignedIn
                resumeWaiters(for: id)
                continue
            }
            startProbe(accountId: id)
            if probeTask != nil { return }
        }
        scheduleProbes()
    }

    private func startProbe(accountId: String) {
        // Sealed runs never launch Claude Code, not even on request.
        guard !DevFlags.probesDisabled else {
            fetchState[accountId] = .unavailable("Usage probes are off")
            resumeWaiters(for: accountId)
            return
        }
        guard !stopped else {
            if fetchState[accountId] == .fetching { fetchState[accountId] = .idle }
            resumeWaiters(for: accountId)
            return
        }
        guard probeTask == nil, let identity = registry.identity(id: accountId) else { return }
        // Never in a store: only a folder Claude Code runs in as this identity.
        guard !identity.runDirs.isEmpty else {
            fetchState[accountId] = Self.noRunFolder
            resumeWaiters(for: accountId)
            return
        }
        probingAccountId = accountId
        fetchState[accountId] = .fetching

        let runner = probeRunner
        let reader = configReader
        let home = registry.homePath
        let mirrorsDefault = registry.mirrorsDefault

        probeTask = Task { [weak self] in
            // Who is signed in where, as of now: Claude Parallel Profiles may
            // have mirrored another account into ~/.claude since the last poll.
            await self?.pollCaches()
            guard let self else { return }
            guard let identity = self.registry.identity(id: accountId), !identity.runDirs.isEmpty else {
                self.finishProbe(accountId: accountId, outcome: .unavailable(Self.noRunFolderText), cachedCopy: nil)
                return
            }
            let runDirs = identity.runDirs
            let paths = runDirs.map { self.registry.globalConfigPath(for: $0) }
            let allRunDirs = self.registry.accounts.filter { $0.kind == .run }.map(\.configDir)
            // Where: the most recently active run folder (read off the main actor).
            let activity: [Date?] = await Task.detached(priority: .utility) {
                zip(runDirs, paths).map { folder, path in
                    // ~/.claude.json is rewritten by every mirror and every
                    // Claude Code in ~/.claude: not a sign of this account.
                    let isMirroredDefault = mirrorsDefault && AccountRegistry.isDefault(folder, home: home)
                    let modified = isMirroredDefault ? nil
                        : (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date
                    return [folder.lastSeenAt, modified].compactMap { $0 }.max()
                }
            }.value
            guard let folder = UsageProbePlanner.probeFolder(runDirs: runDirs, activity: activity, home: home,
                                                             prefersOwnFolders: mirrorsDefault),
                  folder.kind == .run, let index = runDirs.firstIndex(where: { $0.id == folder.id }) else {
                self.finishProbe(accountId: accountId, outcome: .unavailable(Self.noRunFolderText), cachedCopy: nil)
                return
            }
            let configPath = paths[index]
            // Right before it runs: the folder still signed in as this account,
            // and still not a store (whatever the last classification said).
            let before: (runsAs: Bool, store: Bool) = await Task.detached(priority: .utility) {
                (Self.folderRuns(reader.read(path: configPath)?.identity, as: identity),
                 HookInstaller.isNeverInstallTarget(configDir: folder.configDir, home: home))
            }.value
            guard !before.store else {
                self.finishProbe(accountId: accountId, outcome: .unavailable(Self.noRunFolderText), cachedCopy: nil)
                return
            }
            guard before.runsAs else {
                self.discardProbe(accountId: accountId, reason: "\(folder.configDir) is signed in as another account now")
                return
            }
            let request = ProbeRequest(
                accountId: accountId,
                configDirEnv: folder.configDirEnv.flatMap { $0.isEmpty ? nil : $0 },
                configDirs: allRunDirs,
                workingDirectory: AppIdentity.supportDirectory.appendingPathComponent("usage-probe", isDirectory: true)
            )
            let outcome = await runner(request)
            // A possibly seeded answer is dated from the copy Claude Code
            // keeps in `.claude.json`, read after the probe; and the folder
            // must still be this account's (a mirror mid-probe swaps the token).
            let after: (config: ClaudeGlobalConfig?, runsAs: Bool) = await Task.detached(priority: .utility) {
                let config = reader.read(path: configPath)
                return (config, Self.folderRuns(config?.identity, as: identity))
            }.value
            guard after.runsAs else {
                self.discardProbe(accountId: accountId, reason: "\(folder.configDir) changed hands during the check")
                return
            }
            var cachedCopy: CachedUsageSnapshot?
            if case .usage(let parsed) = outcome, parsed.isPossiblySeeded {
                cachedCopy = after.config?.matchingCachedUsage
            }
            self.finishProbe(accountId: accountId, outcome: outcome, cachedCopy: cachedCopy)
        }
    }

    /// Whether a folder's login (as its `.claude.json` names it now) is
    /// `identity`: by email first, since a mirrored `.claude.json` keeps
    /// another account's UUID; by UUID when there is no email. Pure.
    nonisolated static func folderRuns(_ login: ClaudeAccountIdentity?, as identity: ClaudeIdentityAccount) -> Bool {
        guard let login else { return false }
        if let scope = identity.organizationScope, let organization = login.organizationUuid?.lowercased(),
           organization != scope, login.email?.lowercased() == identity.email?.lowercased(),
           login.accountUuid?.lowercased() == identity.accountUuid?.lowercased() {
            return false
        }
        if let email = login.email?.lowercased(), let wanted = identity.email?.lowercased() {
            return email == wanted
        }
        if let uuid = login.accountUuid?.lowercased(), let wanted = identity.accountUuid?.lowercased() {
            return uuid == wanted
        }
        return false
    }

    /// A probe whose folder changed hands: its answer (if any) would be
    /// another account's. Nothing is recorded against the account, no
    /// backoff; the schedule tries again.
    private func discardProbe(accountId: String, reason: String) {
        probeTask = nil
        probingAccountId = nil
        Self.logger.notice("Usage check for \(accountId, privacy: .public) discarded: \(reason, privacy: .public)")
        // Not a failure (no backoff), but not straight away either: the
        // folder may still be changing hands.
        nextAttemptAt[accountId] = max(nextAttemptAt[accountId] ?? .distantPast, clock().addingTimeInterval(Self.discardRetryDelay))
        if fetchState[accountId] == .fetching { fetchState[accountId] = .idle }
        resumeWaiters(for: accountId)
        runNextProbe()
    }

    /// What a probe's usage answer means: the snapshot to keep (dated by
    /// when Claude Code fetched it, never "now" for an answer that may be its
    /// hour-old fallback), and whether the probe counts as rate limited for
    /// the backoff. Pure.
    nonisolated static func interpretProbeAnswer(
        _ parsed: ParsedUsage,
        accountId: String,
        now: Date,
        cachedCopy: CachedUsageSnapshot?
    ) -> (snapshot: AccountUsage?, rateLimited: Bool) {
        guard parsed.isPossiblySeeded else {
            return (parsed.accountUsage(accountId: accountId, source: .probe, updatedAt: now), false)
        }
        guard let cachedCopy, cachedCopy.usage.hasSameWindows(as: parsed) else {
            // Can't be dated: the cache poll takes the file's own copy.
            return (nil, true)
        }
        let dated = parsed.accountUsage(accountId: accountId, source: .cache, updatedAt: cachedCopy.fetchedAt)
        let fresh = now.timeIntervalSince(cachedCopy.fetchedAt) <= seededFreshWindow
        return (dated, !fresh)
    }

    /// "Usage check paused (too many requests), retrying in 5 min". A 429 on
    /// the usage check says nothing about the account's own rate limits. Pure.
    nonisolated static func pausedText(retryAt: Date, now: Date) -> String {
        let minutes = max(1, Int((retryAt.timeIntervalSince(now) / 60).rounded(.up)))
        return "Usage check paused (too many requests), retrying in \(minutes) min"
    }

    private func finishProbe(accountId: String, outcome: UsageProbe.Outcome, cachedCopy: CachedUsageSnapshot?) {
        let now = clock()
        probeTask = nil
        probingAccountId = nil
        lastProbeAt[accountId] = now

        switch outcome {
        case .usage(let parsed):
            let answer = Self.interpretProbeAnswer(parsed, accountId: accountId, now: now, cachedCopy: cachedCopy)
            if let snapshot = answer.snapshot {
                acceptFullSnapshot(snapshot)
            }
            if let plan = parsed.subscriptionType {
                for folder in registry.folders(for: accountId) {
                    registry.noteSubscriptionType(plan, forAccountId: folder.id)
                }
            }
            if answer.rateLimited {
                noteRateLimited(accountId, now: now)
            } else {
                failureCount[accountId] = 0
                nextAttemptAt[accountId] = nil
                fetchState[accountId] = .idle
            }

        case .unavailable(let reason):
            failureCount[accountId] = 0
            nextAttemptAt[accountId] = now.addingTimeInterval(Self.maxBackoff)
            fetchState[accountId] = .unavailable(reason)

        case .rateLimited:
            noteRateLimited(accountId, now: now)

        case .failed(let reason):
            let failures = (failureCount[accountId] ?? 0) + 1
            failureCount[accountId] = failures
            nextAttemptAt[accountId] = now.addingTimeInterval(Self.backoff(afterFailures: failures))
            fetchState[accountId] = .failed(reason)
            Self.logger.error("Usage probe for \(accountId, privacy: .public) failed (\(failures)): \(reason, privacy: .public)")
        }

        scheduleSave()
        resumeWaiters(for: accountId)
        runNextProbe()
    }

    private func noteRateLimited(_ accountId: String, now: Date) {
        let failures = (failureCount[accountId] ?? 0) + 1
        failureCount[accountId] = failures
        let retryAt = now.addingTimeInterval(max(Self.backoff(afterFailures: failures), Self.minimumProbeInterval))
        nextAttemptAt[accountId] = retryAt
        fetchState[accountId] = .failed(Self.pausedText(retryAt: retryAt, now: now))
        Self.logger.notice("Usage check for \(accountId, privacy: .public) rate limited (\(failures)); next at \(retryAt, privacy: .public)")
    }

    /// Claude Code's `get_usage`, for the account's config folder.
    /// Before bootstrap (tests, the snapshots tool: the real home) this never
    /// runs `claude`; injected runners are unaffected (S6).
    nonisolated static let runClaudeCodeProbe: ProbeRunner = { request in
        guard AppIdentity.isFrozen else { return .failed("Usage checks need a bootstrapped engine") }
        let claudePath = await Task.detached(priority: .utility) {
            ClaudeBinaryLocator.resolve(configDirs: request.configDirs)
        }.value
        guard let claudePath else { return .failed("Claude Code not found") }
        return await UsageProbe.run(
            claudePath: claudePath,
            configDirEnv: request.configDirEnv,
            workingDirectory: request.workingDirectory
        )
    }

    // MARK: - Waiting for a probe

    /// Resumed once, by the probe finishing or by the timeout.
    private final class ProbeWaiter {
        private var continuation: CheckedContinuation<Void, Never>?

        init(_ continuation: CheckedContinuation<Void, Never>) {
            self.continuation = continuation
        }

        func resume() {
            continuation?.resume()
            continuation = nil
        }
    }

    /// Returns when the account's queued or running probe has finished, or
    /// after `timeout`; at once when none is queued or running.
    private func waitForProbe(accountId: String, timeout: TimeInterval) async {
        guard probingAccountId == accountId || forcedQueue.contains(accountId) else { return }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            let waiter = ProbeWaiter(continuation)
            probeWaiters[accountId, default: []].append(waiter)
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(timeout))
                waiter.resume()
            }
        }
    }

    private func resumeWaiters(for accountId: String) {
        guard let waiters = probeWaiters.removeValue(forKey: accountId) else { return }
        waiters.forEach { $0.resume() }
    }

    // MARK: - Persistence

    /// Take back the last run's probe times, backoff and readings.
    func restoreState() {
        let state = stateStore.load().restored(knownAccountIds: nil, now: clock())
        // Saved per folder before accounts were identities: each folder's
        // state goes to the identity it belongs to (the first one wins); a
        // mirrored ~/.claude's to its owner, not to whoever the extension
        // mirrored in (nobody's when its owner is unknown).
        var byIdentity: [String: UsageState.Account] = [:]
        for (id, saved) in state.accounts.sorted(by: { $0.key < $1.key }) {
            let isIdentity = registry.identity(id: id) != nil
            guard let key = isIdentity ? id : (registry.owner(ofSavedFolder: id) ?? (registry.account(id: id) == nil ? id : nil)) else {
                continue
            }
            if byIdentity[key] == nil || id == key { byIdentity[key] = saved }
        }
        for (id, saved) in byIdentity {
            if let at = saved.lastProbeAt { lastProbeAt[id] = at }
            if saved.failureCount > 0 { failureCount[id] = saved.failureCount }
            if let next = saved.nextAttemptAt { nextAttemptAt[id] = next }
            if var reading = saved.lastFullReading {
                reading.accountId = id
                if Self.isNewerFullSnapshot(reading, than: fullSnapshots[id]) {
                    fullSnapshots[id] = reading
                    publish(id)
                }
            }
        }
        if !state.accounts.isEmpty {
            Self.logger.info("Restored usage state for \(state.accounts.count) account(s)")
        }
    }

    /// The state worth keeping, as it stands.
    private func persistedState() -> UsageState {
        var state = UsageState()
        let ids = Set(lastProbeAt.keys).union(failureCount.keys).union(nextAttemptAt.keys).union(fullSnapshots.keys)
        for id in ids {
            let account = UsageState.Account(
                lastProbeAt: lastProbeAt[id],
                failureCount: failureCount[id] ?? 0,
                nextAttemptAt: nextAttemptAt[id],
                lastFullReading: fullSnapshots[id]
            )
            if !account.isEmpty { state.accounts[id] = account }
        }
        return state
    }

    private func scheduleSave() {
        guard started, saveTask == nil else { return }
        saveTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(Self.stateSaveDelay))
            guard !Task.isCancelled, let self else { return }
            self.saveTask = nil
            self.stateStore.save(self.persistedState())
        }
    }
}
