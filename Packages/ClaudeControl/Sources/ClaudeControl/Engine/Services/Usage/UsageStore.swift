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
//  Per window the most current reading wins: one known to have been taken
//  after the others, even when lower (an early reset), else the likeliest
//  (see `mostCurrent`); model-scoped limits and extra usage come from the
//  latest full snapshot (a cache, Claude Desktop or the probe). A fresh
//  reading from any source holds off the probe.
//
//  Probes run one at a time, at most every 5 minutes per account on their own
//  schedule (the usage endpoint answers 429 to tighter polling), with
//  exponential backoff on failure, and never for an account whose ring is
//  switched off (`setPausedAccounts`). The last probe time, the backoff and
//  the last full reading persist in `usage-state.json`, so a relaunch neither
//  probes early nor starts empty. Dev runs (`--no-install`) only probe on
//  request unless `AGENTNOTCH_USAGE_PROBE=1`.
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
    /// Every reading taken in, with where it came from, for the usage
    /// history the website keeps (`UsageHistoryRecorder`). Claude Desktop's
    /// cache and `.claude.json` are told apart here; the merged `usage`
    /// only knows them as caches.
    let observations = PassthroughSubject<UsageObservation, Never>()

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
    /// Persisted state is written this long after the last change...
    nonisolated static let stateSaveDelay: TimeInterval = 2
    /// ...or this long after one that only a status line brought.
    nonisolated static let statusLineSaveDelay: TimeInterval = 30
    /// Claude Desktop's readings are dated by the server's `Date:` (whole
    /// seconds, the server's clock) or the cache file: known newer only than
    /// this much before that.
    nonisolated static let externalClockAllowance: TimeInterval = 60
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
    /// config. `AGENTNOTCH_USAGE_PROBE=1` turns them back on; `refresh` always probes.
    nonisolated static var automaticProbesAllowedByDefault: Bool {
        // Sealed runs never launch Claude Code (see ClaudeControlConfiguration).
        if DevFlags.probesDisabled { return false }
        guard DevFlags.installsDisabled else { return true }
        return DevFlags.usageProbeOnDevRun
    }

    /// A reading of one window and what is known of when it was taken: after
    /// `notBefore` (when known), at `at` at the latest. A full snapshot's
    /// time is its `updatedAt` (from its `takenAfter`); a status line's is
    /// dated by `advance`.
    nonisolated struct Reading: Codable, Equatable, Sendable {
        var window: UsageWindow
        /// The latest the data can be from.
        var at: Date
        /// The data is newer than this; nil when that is unknown.
        var notBefore: Date?

        init(_ window: UsageWindow, at: Date, notBefore: Date? = nil) {
            self.window = window
            self.at = at
            self.notBefore = notBefore
        }

        /// A full snapshot's window, taken when the snapshot was.
        init(_ window: UsageWindow, of snapshot: AccountUsage) {
            self.init(window, at: snapshot.updatedAt,
                      notBefore: min(snapshot.takenAfter ?? snapshot.updatedAt, snapshot.updatedAt))
        }
    }

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
    /// When a process started, from the kernel; nil when it isn't running.
    typealias ProcessStart = @MainActor (Int) -> Date?
    /// The Claude Code process a session's hooks name (from the session list).
    typealias SessionProcess = @MainActor (String) -> Int?

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
    private let processStartedAt: ProcessStart
    private let sessionProcessId: SessionProcess

    /// Claude Desktop's cache, when the host provides one.
    private var externalSource: (any ClaudeExternalUsageSource)? {
        externalSourceOverride ?? AppIdentity.configuration.externalUsageSource
    }

    private lazy var stateStore: UsageStateStore = stateStoreOverride
        ?? UsageStateStore(directory: AppIdentity.supportDirectory)

    // MARK: - State

    /// Latest full snapshot (cache, Claude Desktop or probe) per account.
    private var fullSnapshots: [String: AccountUsage] = [:]
    /// What one Claude Code process last said in its status line: its
    /// windows, dated by `advance`, and when it last reported.
    nonisolated struct StatusLineReadings: Codable, Equatable, Sendable {
        var lastReportAt: Date
        var fiveHour: Reading?
        var sevenDay: Reading?
    }
    /// Per account, per folder the sessions ran in, per Claude Code process
    /// (`statusLineKey`).
    private var statusLine: [String: [String: [String: StatusLineReadings]]] = [:]
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
    /// When the pending `saveTask` writes.
    private var saveDueAt: Date?
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
        sessionStartedAt: SessionStart? = nil,
        processStartedAt: ProcessStart? = nil,
        sessionProcessId: SessionProcess? = nil
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
        self.processStartedAt = processStartedAt ?? { pid in ProcessInspector.startDate(pid: pid) }
        self.sessionProcessId = sessionProcessId ?? { id in
            ClaudeSessionMonitor.shared.instances.first { $0.sessionId == id }?.pid
        }
    }

    // MARK: - Lifecycle

    func start() {
        guard !started else { return }
        started = true
        stopped = false
        if !automaticProbes {
            Self.logger.notice("Automatic usage probes are off for this run (set AGENTNOTCH_USAGE_PROBE=1 to enable)")
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
        saveDueAt = nil
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
        Self.mostCurrent(statusCandidates(accountId, window))
    }

    /// The status line readings of an account's window that count (see
    /// `statusCandidates(_:defaultFolder:mirrorsDefault:)`).
    private func statusCandidates(_ accountId: String, _ window: KeyPath<StatusLineReadings, Reading?>) -> [Reading] {
        guard let folders = statusLine[accountId] else { return [] }
        return Self.statusCandidates(folders.mapValues { Self.readings($0, window) },
                                     defaultFolder: AccountRegistry.defaultConfigDir(home: registry.homePath),
                                     mirrorsDefault: registry.mirrorsDefault)
    }

    /// One window of a folder's processes, in a fixed order. Pure.
    nonisolated static func readings(_ processes: [String: StatusLineReadings],
                                     _ window: KeyPath<StatusLineReadings, Reading?>) -> [Reading] {
        processes.sorted { $0.key < $1.key }.compactMap { $0.value[keyPath: window] }
    }

    /// The readings of a window from the folders an account's sessions ran
    /// in (one per Claude Code process) that count, in a fixed order. While
    /// Claude Parallel Profiles mirrors accounts into `~/.claude`, a reading
    /// that came through `~/.claude` counts only until one from the
    /// account's own folders (its VS Code windows, standalone folders)
    /// arrives after it. Pure.
    nonisolated static func statusCandidates(_ byFolder: [String: [Reading]], defaultFolder: String, mirrorsDefault: Bool) -> [Reading] {
        var readings = byFolder
        if mirrorsDefault, let viaDefault = readings[defaultFolder],
           let newestOwn = readings.filter({ $0.key != defaultFolder }).flatMap(\.value).map(\.at).max() {
            readings[defaultFolder] = viaDefault.filter { $0.at > newestOwn }
        }
        return readings.sorted { $0.key < $1.key }.flatMap(\.value)
    }

    /// The most current of those readings (see `mostCurrent`). Pure.
    nonisolated static func combinedStatus(_ byFolder: [String: [Reading]], defaultFolder: String, mirrorsDefault: Bool) -> Reading? {
        mostCurrent(statusCandidates(byFolder, defaultFolder: defaultFolder, mirrorsDefault: mirrorsDefault))
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

        let processId = trustedProcessId(update)
        let startedAt = processId.flatMap(processStartedAt)
        let process = Self.statusLineKey(sessionId: update.sessionId, processId: processId, startedAt: startedAt)
        let previous = statusLine[accountId]?[folderId]?[process]
        let readings = Self.advance(previous, fiveHour: update.fiveHour, sevenDay: update.sevenDay,
                                    receivedAt: update.receivedAt, startedAt: previous == nil ? startedAt : nil)
        statusLine[accountId, default: [:]][folderId, default: [:]][process] = readings
        publish(accountId)
        if readings.fiveHour != previous?.fiveHour || readings.sevenDay != previous?.sevenDay {
            // Kept for the next run, so a relaunch doesn't take a process's
            // old numbers for news; not urgent.
            scheduleSave(after: Self.statusLineSaveDelay)
        }
        // For the history: the windows this line brought (not a repeat) that
        // the account now shows, as the old per-folder rule did for a folder.
        var taken: [CloudSyncRequest.UsageWindow] = []
        if let reading = readings.fiveHour, reading.at == update.receivedAt, reading.window.utilization.isFinite,
           isShown(reading, accountId: accountId, \.fiveHour, \.fiveHour) {
            taken.append(.init(id: UsageRingWindows.sessionID, utilization: reading.window.utilization, resetsAt: reading.window.resetsAt))
        }
        if let reading = readings.sevenDay, reading.at == update.receivedAt, reading.window.utilization.isFinite,
           isShown(reading, accountId: accountId, \.sevenDay, \.sevenDay) {
            taken.append(.init(id: UsageRingWindows.weeklyID, utilization: reading.window.utilization, resetsAt: reading.window.resetsAt))
        }
        if !taken.isEmpty {
            observations.send(UsageObservation(identityId: accountId, source: .statusLine,
                                               observedAt: update.receivedAt, windows: taken))
        }
    }

    /// Tell the usage history about a full snapshot.
    private func observe(_ snapshot: AccountUsage, source: CloudUsageSource) {
        let windows = UsageObservation.windows(from: snapshot)
        guard !windows.isEmpty else { return }
        observations.send(UsageObservation(identityId: snapshot.accountId, source: source,
                                           observedAt: snapshot.updatedAt, windows: windows))
    }

    // MARK: - Merging

    /// Two readings describe the same window when their reset times are close:
    /// consecutive windows reset at least a full window length apart, while
    /// sources disagree by at most rounding (epoch seconds vs. fractional ISO).
    nonisolated static func isSameWindow(_ lhs: UsageWindow, _ rhs: UsageWindow) -> Bool {
        guard let left = lhs.resetsAt, let right = rhs.resetsAt else { return false }
        return abs(left.timeIntervalSince(right)) < min(lhs.duration, rhs.duration) / 4
    }

    /// A reading of the same window known to be later, but lower, counts
    /// only when lower by more than this many points (what the website's
    /// chart takes for a reset too). A smaller drop is rounding (the usage
    /// endpoint's 9 against a status line's 9.4), or a response that started
    /// before the other reading and ended after it: Claude Code takes a
    /// response's rate limits from its headers but applies them when it
    /// ends, so a process's new numbers can be older than its last report.
    nonisolated static let resetDropMinimum: Double = 5

    /// Whether `candidate` is known to have been taken after `other`, and so
    /// wins whatever its numbers say. Usage can come down within a window:
    /// Anthropic resets limits early and keeps the reset time (Claude Code's
    /// own `/limit-reset` keeps "your weekly reset day"), and only the
    /// reading after the reset is right. Not, though, for a reading of a
    /// window that had already reset when it was taken (the usage endpoint
    /// can still answer with the ended window): that says nothing about a
    /// later one. Nor for a small drop (`isSmallDrop`). Pure.
    nonisolated static func supersedes(_ candidate: Reading, _ other: Reading) -> Bool {
        guard let notBefore = candidate.notBefore, notBefore >= other.at, candidate.at > other.at else { return false }
        if let reset = candidate.window.resetsAt, reset <= notBefore,
           let otherReset = other.window.resetsAt, otherReset > reset {
            return false
        }
        return !isSmallDrop(from: other.window, to: candidate.window)
    }

    /// `lower` is the same window as `higher`, lower by no more than
    /// `resetDropMinimum`: rounding or a late response, not a reset. Pure.
    nonisolated static func isSmallDrop(from higher: UsageWindow, to lower: UsageWindow) -> Bool {
        guard isSameWindow(higher, lower) else { return false }
        let drop = higher.utilization - lower.utilization
        return drop > 0 && drop <= resetDropMinimum
    }

    /// Whether `candidate` is a more current reading of a window than `other`
    /// when neither is known to have been taken after the other (see
    /// `supersedes`).
    ///
    /// Arrival time alone can't decide: a status line re-run (a permission-mode
    /// change, a `refreshInterval` tick, an idle session) repeats whatever that
    /// session's last API response said, possibly hours ago. So the data decides
    /// first: a later reset means a newer window, and within one window usage
    /// only grows short of an early reset, so the higher reading is the likelier
    /// the more recent. Arrival time breaks ties and covers readings without a
    /// reset time.
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

    /// The most current of several readings of a window: any known to have
    /// been taken before another is out, and `isMoreCurrent` picks among the
    /// rest, whose order is unknown (of two even that can't tell apart, the
    /// one listed first). The latest `at` is never out, so a non-empty list
    /// always has an answer. Pure.
    nonisolated static func mostCurrent(_ readings: [Reading]) -> Reading? {
        mostCurrentIndex(readings).map { readings[$0] }
    }

    /// Where in `readings` the most current one is (see `mostCurrent`). Pure.
    nonisolated static func mostCurrentIndex(_ readings: [Reading]) -> Int? {
        readings.indices
            .filter { index in !readings.contains { supersedes($0, readings[index]) } }
            .reduce(nil) { best, index in
                guard let best else { return index }
                return isMoreCurrent(readings[index], than: readings[best]) ? index : best
            }
    }

    /// A Claude Code process's status line record after it reported
    /// `fiveHour`/`sevenDay` at `receivedAt`. Its rate limits are its own
    /// latest API response's (one set per process, replaced by each newer
    /// response, even a lower one; none before its first response), and a
    /// status line re-run repeats them however old. So:
    /// - the same window again is no news: the reading keeps its time, so a
    ///   process repeating stale rate limits neither looks fresh nor holds
    ///   off the probe;
    /// - a different one came from a response applied after the previous
    ///   report (its numbers may still be older: see `resetDropMinimum`), so
    ///   a small drop (`isSmallDrop`) is not taken, and the higher reading
    ///   keeps its time;
    /// - a process's first report is newer than the process (`startedAt`,
    ///   from the kernel), when that is known. Pure.
    nonisolated static func advance(
        _ record: StatusLineReadings?,
        fiveHour: UsageWindow?,
        sevenDay: UsageWindow?,
        receivedAt: Date,
        startedAt: Date?
    ) -> StatusLineReadings {
        let notBefore = (record?.lastReportAt ?? startedAt).flatMap { $0 < receivedAt ? $0 : nil }
        func next(_ stored: Reading?, _ window: UsageWindow?) -> Reading? {
            guard let window else { return stored }
            if let stored, isRepeat(stored.window, window) || isSmallDrop(from: stored.window, to: window) { return stored }
            return Reading(window, at: receivedAt, notBefore: notBefore)
        }
        return StatusLineReadings(
            lastReportAt: max(record?.lastReportAt ?? receivedAt, receivedAt),
            fiveHour: next(record?.fiveHour, fiveHour),
            sevenDay: next(record?.sevenDay, sevenDay)
        )
    }

    /// The same numbers again. Reset times within a second are the same
    /// (`usage-state.json` keeps whole seconds; Claude Code sends whole
    /// seconds). Pure.
    nonisolated static func isRepeat(_ lhs: UsageWindow, _ rhs: UsageWindow) -> Bool {
        guard lhs.utilization == rhs.utilization, lhs.duration == rhs.duration else { return false }
        switch (lhs.resetsAt, rhs.resetsAt) {
        case (nil, nil): return true
        case let (left?, right?): return abs(left.timeIntervalSince(right)) < 1
        default: return false
        }
    }

    /// Whose status line an update is: the Claude Code process's when it is
    /// known (rate limits belong to the process, and outlive a `/clear` into
    /// a new session), with its start time when that is known (a pid is
    /// reused), else the session's. Pure.
    nonisolated static func statusLineKey(sessionId: String, processId: Int?, startedAt: Date? = nil) -> String {
        guard let processId else { return "session:\(sessionId)" }
        guard let startedAt else { return "\(processKeyPrefix)\(processId)" }
        return "\(processKeyPrefix)\(processId)@\(Int(startedAt.timeIntervalSince1970))"
    }

    nonisolated static let processKeyPrefix = "pid:"

    /// A Claude Code process not heard from for this long is forgotten: its
    /// weekly window has reset since.
    nonisolated static let statusLineRetention: TimeInterval = UsageWindow.weeklyDuration

    /// Forget the processes not heard from for `statusLineRetention`.
    private func pruneStatusLine(now: Date) {
        for (accountId, folders) in statusLine {
            let kept = folders
                .mapValues { $0.filter { now.timeIntervalSince($0.value.lastReportAt) < Self.statusLineRetention } }
                .filter { !$0.value.isEmpty }
            guard kept != folders else { continue }
            statusLine[accountId] = kept.isEmpty ? nil : kept
            publish(accountId)
        }
    }

    /// Whether `reading` is what the account shows for the window now.
    private func isShown(_ reading: Reading, accountId: String,
                         _ window: KeyPath<StatusLineReadings, Reading?>,
                         _ fullWindow: KeyPath<AccountUsage, UsageWindow?>) -> Bool {
        let snapshot = fullSnapshots[accountId].flatMap { full in full[keyPath: fullWindow].map { Reading($0, of: full) } }
        return Self.winningStatus(statusCandidates(accountId, window), over: snapshot) == reading
    }

    /// The status line's Claude Code process, unless the session's hooks
    /// name another one: a `CLAUDE_PID` inherited from a Claude Code that
    /// started this one would mix two processes' rate limits.
    private func trustedProcessId(_ update: StatusLineUpdate) -> Int? {
        guard let pid = update.processId.flatMap(ProcessID.valid) else { return nil }
        if let hooks = sessionProcessId(update.sessionId), hooks != pid { return nil }
        return pid
    }

    /// Recompute the published usage for one account from its sources.
    private func publish(_ accountId: String) {
        let merged = Self.merge(
            accountId: accountId,
            full: fullSnapshots[accountId],
            statusFiveHour: statusCandidates(accountId, \.fiveHour),
            statusSevenDay: statusCandidates(accountId, \.sevenDay)
        )
        if usage[accountId] != merged {
            usage[accountId] = merged
        }
    }

    /// `merge` with one status line reading per window. Pure.
    nonisolated static func merge(
        accountId: String,
        full: AccountUsage?,
        statusFiveHour: Reading?,
        statusSevenDay: Reading?
    ) -> AccountUsage? {
        merge(accountId: accountId, full: full,
              statusFiveHour: statusFiveHour.map { [$0] } ?? [],
              statusSevenDay: statusSevenDay.map { [$0] } ?? [])
    }

    /// The most current reading wins per window, the full snapshot's and the
    /// status lines' together (`mostCurrent`: one status line known to be
    /// newer than the snapshot is enough, whichever of them shows); scoped
    /// limits, extra usage and plan come from the full snapshot.
    /// `updatedAt`/`source` describe the newest reading used. Pure.
    nonisolated static func merge(
        accountId: String,
        full: AccountUsage?,
        statusFiveHour: [Reading],
        statusSevenDay: [Reading]
    ) -> AccountUsage? {
        guard full != nil || !statusFiveHour.isEmpty || !statusSevenDay.isEmpty else { return nil }

        var result = full ?? AccountUsage(accountId: accountId, source: .statusLine, updatedAt: .distantPast)
        var newest = full?.updatedAt ?? .distantPast
        var newestSource = full?.source ?? .statusLine

        func take(_ readings: [Reading], current: UsageWindow?) -> UsageWindow? {
            let snapshot = current.flatMap { window in full.map { Reading(window, of: $0) } }
            guard let reading = winningStatus(readings, over: snapshot) else { return current }
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

    /// The status line reading that wins a window over the full snapshot's
    /// (see `mostCurrent`), nil when the snapshot's stays (it goes first: of
    /// two readings nothing tells apart, it is kept) or there is none. Pure.
    nonisolated static func winningStatus(_ readings: [Reading], over snapshot: Reading?) -> Reading? {
        let candidates = (snapshot.map { [$0] } ?? []) + readings
        guard let index = mostCurrentIndex(candidates), snapshot == nil || index > 0 else { return nil }
        return candidates[index]
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
        pruneStatusLine(now: clock())
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
                let snapshot = cached.usage.accountUsage(accountId: id, source: .cache, updatedAt: cached.fetchedAt)
                observe(snapshot, source: .claudeJson)
                acceptFullSnapshot(snapshot)
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
            if let reading, var snapshot = UsageRingWindows.accountUsage(from: reading, accountId: id) {
                snapshot.takenAfter = reading.observedAt.addingTimeInterval(-Self.externalClockAllowance)
                externalMisses.remove(id)
                observe(snapshot, source: .desktop)
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
            for (folder, processes) in folders {
                // A process heard from under both keeps its latest record.
                statusLine[identityId, default: [:]][folder, default: [:]].merge(processes) { mine, moved in
                    moved.lastReportAt > mine.lastReportAt ? moved : mine
                }
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
            let launchedAt = self.clock()
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
            self.finishProbe(accountId: accountId, outcome: outcome, cachedCopy: cachedCopy, launchedAt: launchedAt)
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
    /// hour-old fallback; a fresh one was fetched after `launchedAt`), and
    /// whether the probe counts as rate limited for the backoff. Pure.
    nonisolated static func interpretProbeAnswer(
        _ parsed: ParsedUsage,
        accountId: String,
        now: Date,
        cachedCopy: CachedUsageSnapshot?,
        launchedAt: Date? = nil
    ) -> (snapshot: AccountUsage?, rateLimited: Bool) {
        guard parsed.isPossiblySeeded else {
            var snapshot = parsed.accountUsage(accountId: accountId, source: .probe, updatedAt: now)
            snapshot.takenAfter = launchedAt.map { min($0, now) }
            return (snapshot, false)
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

    private func finishProbe(accountId: String, outcome: UsageProbe.Outcome, cachedCopy: CachedUsageSnapshot?,
                             launchedAt: Date? = nil) {
        let now = clock()
        probeTask = nil
        probingAccountId = nil
        lastProbeAt[accountId] = now

        switch outcome {
        case .usage(let parsed):
            let answer = Self.interpretProbeAnswer(parsed, accountId: accountId, now: now, cachedCopy: cachedCopy,
                                                   launchedAt: launchedAt)
            if let snapshot = answer.snapshot {
                // A seeded answer dated from `.claude.json` is that cache's reading.
                observe(snapshot, source: snapshot.source == .probe ? .probe : .claudeJson)
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
        restoreStatusLines(byIdentity)
        if !state.accounts.isEmpty {
            Self.logger.info("Restored usage state for \(state.accounts.count) account(s)")
        }
    }

    /// The last run's status line records, in folders still the account's
    /// (a mirrored `~/.claude`'s by who its sessions started as, like
    /// `ingest`). Each is keyed by its process's pid and start time: a
    /// process still running goes on from its record (its old numbers stay
    /// a repeat), and an ended one's readings still count until retention
    /// ends, as they would have without the relaunch.
    private func restoreStatusLines(_ saved: [String: UsageState.Account]) {
        let now = clock()
        let defaultFolder = AccountRegistry.defaultConfigDir(home: registry.homePath)
        for (id, account) in saved where registry.identity(id: id) != nil {
            var restored = 0
            for line in account.statusLines ?? [] {
                guard Self.isProcessKeyWithStart(line.key),
                      now.timeIntervalSince(line.readings.lastReportAt) < Self.statusLineRetention,
                      registry.identity(forFolderId: line.folder)?.id == id
                        || (line.folder == defaultFolder && registry.mirrorsDefault) else { continue }
                statusLine[id, default: [:]][line.folder, default: [:]][line.key] = line.readings
                restored += 1
            }
            if restored > 0 { publish(id) }
        }
    }

    /// A process's key that names its start time too (see `statusLineKey`). Pure.
    nonisolated static func isProcessKeyWithStart(_ key: String) -> Bool {
        key.hasPrefix(processKeyPrefix) && key.contains("@")
    }

    /// The state worth keeping, as it stands.
    private func persistedState() -> UsageState {
        var state = UsageState()
        let ids = Set(lastProbeAt.keys).union(failureCount.keys).union(nextAttemptAt.keys).union(fullSnapshots.keys)
            .union(statusLine.keys)
        for id in ids {
            let account = UsageState.Account(
                lastProbeAt: lastProbeAt[id],
                failureCount: failureCount[id] ?? 0,
                nextAttemptAt: nextAttemptAt[id],
                lastFullReading: fullSnapshots[id],
                statusLines: savedStatusLines(id)
            )
            if !account.isEmpty { state.accounts[id] = account }
        }
        return state
    }

    /// An identity's process records worth keeping: those whose process is
    /// known by pid and start time (see `restoreStatusLines`).
    private func savedStatusLines(_ accountId: String) -> [UsageState.StatusLine]? {
        guard registry.identity(id: accountId) != nil, let folders = statusLine[accountId] else { return nil }
        let lines = folders.sorted { $0.key < $1.key }.flatMap { folder, processes in
            processes.sorted { $0.key < $1.key }
                .filter { Self.isProcessKeyWithStart($0.key) }
                .map { UsageState.StatusLine(folder: folder, key: $0.key, readings: $0.value) }
        }
        return lines.isEmpty ? nil : lines
    }

    /// Save after `delay`, or sooner if a save is already due sooner (a
    /// probe's backoff must not wait behind a status line's 30 s).
    private func scheduleSave(after delay: TimeInterval = UsageStore.stateSaveDelay) {
        guard started else { return }
        let due = Date().addingTimeInterval(delay)
        if saveTask != nil, let pending = saveDueAt, pending <= due { return }
        saveTask?.cancel()
        saveDueAt = due
        saveTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled, let self else { return }
            self.saveTask = nil
            self.saveDueAt = nil
            self.stateStore.save(self.persistedState())
        }
    }
}
