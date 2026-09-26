//
//  SessionRegistryScanner.swift
//  ClaudeIsland
//
//  Polls Claude Code's session registry, `<configDir>/sessions/<pid>.json`,
//  for every known account. Each running Claude Code process keeps its file
//  current with its status (busy / idle / shell / waiting + waitingFor), so
//  the registry covers sessions started before the app, VS Code sessions and
//  interrupts that no hook reports.
//
//  Only `*.json` files are read. The `*.key` files next to them are secrets
//  and are never opened.
//
//  Claude Parallel Profiles links every config folder's `sessions/` to one
//  shared folder (`~/.claude-shared/sessions`). Each physical folder is read
//  once however many config folders lead to it (deduplicated by resolved
//  path), and when it is shared (or reached through a link) each entry is
//  attributed to the config folder its process actually runs with: the
//  process's `CLAUDE_CONFIG_DIR` from the kernel (`ProcessConfigDir`), unset
//  meaning `~/.claude`. Snapshots are then handed on per attributed folder.
//

import Foundation
import os.log

nonisolated private var logger: Logger { EngineLog.logger("Registry") }

/// One entry of `<configDir>/sessions/<pid>.json`.
nonisolated struct SessionRegistryEntry: Equatable, Sendable {
    let pid: Int
    let sessionId: String
    let cwd: String?
    /// interactive | bg | daemon | daemon-worker (missing in older versions)
    let kind: String?
    /// cli | claude-vscode | sdk-...
    let entrypoint: String?
    let name: String?
    /// "derived" when Claude Code made the name up from the project folder.
    let nameSource: String?
    let version: String?
    /// busy | idle | shell | waiting
    let status: String?
    /// "permission prompt" | "input needed" | "dialog open" | "sandbox request" | "worker request"
    let waitingFor: String?
    let startedAt: Date?
    let updatedAt: Date?
    let statusUpdatedAt: Date?
    /// `ps -o lstart` of the process, in UTC.
    let procStart: String?
    /// Claude Desktop's id for a session it hosts (`local_…`), written only
    /// with a Desktop entrypoint (see `DesktopHostedSessions`); nil when
    /// missing or not in Claude Code's own form.
    let hostSessionId: String?

    init(
        pid: Int,
        sessionId: String,
        cwd: String? = nil,
        kind: String? = "interactive",
        entrypoint: String? = "cli",
        name: String? = nil,
        nameSource: String? = nil,
        version: String? = nil,
        status: String? = nil,
        waitingFor: String? = nil,
        startedAt: Date? = nil,
        updatedAt: Date? = nil,
        statusUpdatedAt: Date? = nil,
        procStart: String? = nil,
        hostSessionId: String? = nil
    ) {
        self.pid = pid
        self.sessionId = sessionId
        self.cwd = cwd
        self.kind = kind
        self.entrypoint = entrypoint
        self.name = name
        self.nameSource = nameSource
        self.version = version
        self.status = status
        self.waitingFor = waitingFor
        self.startedAt = startedAt
        self.updatedAt = updatedAt
        self.statusUpdatedAt = statusUpdatedAt
        self.procStart = procStart
        self.hostSessionId = hostSessionId.flatMap { DesktopHostedSessions.isHostSessionId($0) ? $0 : nil }
    }

    /// Parses a registry file's JSON object. Timestamps are epoch milliseconds.
    init?(json: [String: Any]) {
        guard let pid = JSONValue.int(json["pid"]).flatMap(ProcessID.valid),
              let sessionId = JSONValue.string(json["sessionId"]) else { return nil }
        self.init(
            pid: pid,
            sessionId: sessionId,
            cwd: JSONValue.string(json["cwd"]),
            kind: JSONValue.string(json["kind"]),
            entrypoint: JSONValue.string(json["entrypoint"]),
            name: JSONValue.string(json["name"]),
            nameSource: JSONValue.string(json["nameSource"]),
            version: JSONValue.string(json["version"]),
            status: JSONValue.string(json["status"]),
            waitingFor: JSONValue.string(json["waitingFor"]),
            startedAt: Self.date(fromMilliseconds: json["startedAt"]),
            updatedAt: Self.date(fromMilliseconds: json["updatedAt"]),
            statusUpdatedAt: Self.date(fromMilliseconds: json["statusUpdatedAt"]),
            procStart: JSONValue.string(json["procStart"]),
            hostSessionId: JSONValue.string(json["hostSessionId"])
        )
    }

    /// Interactive sessions the app tracks (bg/daemon kinds and SDK entrypoints are ignored).
    var isTracked: Bool {
        !SessionFilter.isIgnored(registryKind: kind, entrypoint: entrypoint)
    }

    /// Claude Desktop hosts the session: it runs as Desktop's account, not
    /// as its config folder's (`DesktopHostedSessions`).
    var isDesktopHosted: Bool {
        DesktopHostedSessions.isDesktopHosted(entrypoint: entrypoint)
    }

    /// The name was derived by Claude Code rather than chosen by the user.
    var isNameDerived: Bool {
        nameSource == "derived"
    }

    /// When the status last changed (falls back to the last update).
    var statusChangedAt: Date? {
        statusUpdatedAt ?? updatedAt
    }

    private static func date(fromMilliseconds raw: Any?) -> Date? {
        guard let ms = JSONValue.double(raw), ms > 0 else { return nil }
        return Date(timeIntervalSince1970: ms / 1000)
    }
}

/// Polls the session registries of all known config dirs on a background queue.
nonisolated final class SessionRegistryScanner: @unchecked Sendable {
    static let shared = SessionRegistryScanner()

    /// Called with the live, tracked entries of one config dir whenever they change.
    typealias SnapshotHandler = @Sendable (_ configDir: String, _ entries: [SessionRegistryEntry]) -> Void

    static let scanInterval: TimeInterval = 3
    /// After a Stop, the registry is read again this soon (Claude Code marks
    /// the session idle once every Stop hook ran), then once more.
    static let quickRescanDelays: [TimeInterval] = [0.3, 1.2]
    /// The accounts' config dirs normally arrive within a moment of start;
    /// the first pass isn't held back longer than this waiting for them.
    static let initialDirsGrace: TimeInterval = 2

    private let queue = DispatchQueue(label: EngineLog.queueLabel("registry"), qos: .utility)

    /// `~/.claude`: always scanned, and where a process with no
    /// `CLAUDE_CONFIG_DIR` runs.
    private let defaultDir: String

    // Queue-confined state
    private var dirs: Set<String>
    /// Last snapshot handed on, per (attributed) config folder.
    private var lastSnapshots: [String: [SessionRegistryEntry]] = [:]
    /// The config folders each physical sessions folder's entries were last
    /// attributed to (so a folder whose last session ended gets its `[]`).
    private var attributedByFolder: [String: Set<String>] = [:]
    /// How many times each physical sessions folder was read (for tests: one
    /// read per pass, however many config folders lead there).
    private var readsByFolder: [String: Int] = [:]
    /// Which config folder a process runs with (injectable for tests).
    private let configDirOfProcess: @Sendable (Int32) -> ProcessConfigDir.Value
    /// `configDirOfProcess` is the shared kernel cache (not a test's stand-in).
    private let usesSharedProcessCache: Bool
    private var timer: DispatchSourceTimer?
    private var handler: SnapshotHandler?
    private var quickRescans: Set<String> = []

    /// First pass: every config dir known once the accounts arrived has been
    /// read once. Sessions found before it are the ones already running at
    /// launch, which is not news.
    private var initialScanHandler: (@Sendable () -> Void)?
    private var accountDirsArrived = false
    private var initialScanReported = false
    private var scannedDirs: Set<String> = []

    private init() {
        configDirOfProcess = { ProcessConfigDirCache.shared.value(pid: $0) }
        usesSharedProcessCache = true
        defaultDir = AccountPaths.defaultConfigDir
        dirs = Self.initialConfigDirs()
    }

    /// A scanner for tests: its own `~/.claude`, and a stand-in for the
    /// kernel's process environments.
    init(defaultDir: String, configDirOfProcess: @escaping @Sendable (Int32) -> ProcessConfigDir.Value) {
        self.configDirOfProcess = configDirOfProcess
        usesSharedProcessCache = false
        self.defaultDir = AccountPaths.normalize(defaultDir)
        dirs = [self.defaultDir]
    }

    /// ~/.claude plus the configuration's extra dirs (`AGENTNOTCH_EXTRA_CONFIG_DIRS`).
    private static func initialConfigDirs() -> Set<String> {
        Set([AccountPaths.defaultConfigDir] + extraConfigDirs())
    }

    /// Dev-run extra config dirs, normalized.
    static func extraConfigDirs() -> [String] {
        DevFlags.extraConfigDirs
    }

    /// Config dirs being scanned (normalized). Always includes ~/.claude.
    var configDirs: Set<String> {
        queue.sync { dirs }
    }

    /// `body` run on the scanner's queue with each sessions folder's read
    /// count, so what a test compares it with was recorded by the same passes.
    func withFolderReads<T>(_ body: ([String: Int]) -> T) -> T {
        queue.sync { body(readsByFolder) }
    }

    /// Adds an account's config dir (e.g. from an AccountSighting); scans it right away.
    func addConfigDir(_ configDir: String) {
        let dir = AccountPaths.normalize(configDir)
        queue.async { [self] in
            guard dirs.insert(dir).inserted else { return }
            logger.info("Watching session registry of \(AccountPaths.shortName(forConfigDir: dir), privacy: .public)")
            scan(dir)
        }
    }

    /// Replaces the scanned dirs (the default dir and the extra dirs are always kept).
    func setConfigDirs(_ configDirs: Set<String>) {
        let normalized = Set(configDirs.map(AccountPaths.normalize))
            .union([defaultDir])
            .union(Self.extraConfigDirs())
        queue.async { [self] in
            let added = normalized.subtracting(dirs)
            dirs = normalized
            // Folders reached from the remaining config folders keep their
            // attributed snapshots; the others are forgotten.
            let folders = Set(normalized.map(Self.sessionsFolder(of:)))
            for (folder, attributed) in attributedByFolder where !folders.contains(folder) {
                for dir in attributed { lastSnapshots.removeValue(forKey: dir) }
                attributedByFolder.removeValue(forKey: folder)
            }
            // Each physical folder once, however many new folders lead there.
            for (folder, aliases) in Self.groupedBySessionsFolder(dirs) where !aliases.isDisjoint(with: added) {
                scanFolder(folder, aliases: aliases)
            }
            accountDirsArrived = true
            reportInitialScanIfDone()
        }
    }

    /// Reads a config dir's registry again in a moment (a turn just stopped),
    /// instead of waiting for the next periodic scan.
    func scanSoon(configDir: String) {
        let dir = AccountPaths.normalize(configDir)
        queue.async { [self] in
            // A session attributed to a window reaches its folder through
            // any config folder that leads to the same sessions folder.
            let target = dirs.contains(dir) ? dir
                : dirs.first { Self.sessionsFolder(of: $0) == Self.sessionsFolder(of: dir) } ?? dir
            guard handler != nil, dirs.contains(target), quickRescans.insert(target).inserted else { return }
            for (index, delay) in Self.quickRescanDelays.enumerated() {
                queue.asyncAfter(deadline: .now() + delay) { [weak self] in
                    guard let self, self.handler != nil else { return }
                    if index == Self.quickRescanDelays.count - 1 {
                        self.quickRescans.remove(target)
                    }
                    self.scan(target)
                }
            }
        }
    }

    /// - Parameter onInitialScan: called once, on the scanner's queue, when
    ///   every account's registry has been read once.
    func start(onSnapshot: @escaping SnapshotHandler, onInitialScan: (@Sendable () -> Void)? = nil) {
        queue.async { [self] in
            guard timer == nil else { return }
            handler = onSnapshot
            initialScanHandler = onInitialScan
            let timer = DispatchSource.makeTimerSource(queue: queue)
            timer.schedule(deadline: .now(), repeating: Self.scanInterval, leeway: .milliseconds(500))
            timer.setEventHandler { [weak self] in
                self?.scanAll()
            }
            self.timer = timer
            timer.resume()
            queue.asyncAfter(deadline: .now() + Self.initialDirsGrace) { [weak self] in
                guard let self else { return }
                self.accountDirsArrived = true
                self.reportInitialScanIfDone()
            }
        }
    }

    func stop() {
        queue.async { [self] in
            timer?.cancel()
            timer = nil
            handler = nil
            initialScanHandler = nil
        }
    }

    // MARK: - Scanning

    private func scanAll() {
        var pids = Set<Int32>()
        for (folder, aliases) in Self.groupedBySessionsFolder(dirs) {
            pids.formUnion(scanFolder(folder, aliases: aliases))
        }
        // The cached CLAUDE_CONFIG_DIR of processes that are gone.
        if usesSharedProcessCache { ProcessConfigDirCache.shared.retain(pids: pids) }
        reportInitialScanIfDone()
    }

    /// Read the physical sessions folder `configDir` leads to (with every
    /// other config folder that leads there too).
    private func scan(_ configDir: String) {
        let folder = Self.sessionsFolder(of: configDir)
        let aliases = dirs.filter { Self.sessionsFolder(of: $0) == folder }
        scanFolder(folder, aliases: aliases.isEmpty ? [configDir] : aliases)
        reportInitialScanIfDone()
    }

    @discardableResult
    private func scanFolder(_ folder: String, aliases: Set<String>) -> Set<Int32> {
        let sorted = aliases.sorted()
        guard let first = sorted.first else { return [] }
        readsByFolder[folder, default: 0] += 1
        let entries = Self.liveEntries(configDir: first)
        scannedDirs.formUnion(aliases)
        let attributed = Self.attribute(entries, aliases: aliases,
                                        isShared: aliases.count > 1 || Self.isLinkedSessionsFolder(of: first),
                                        defaultDir: defaultDir,
                                        configDirOfProcess: configDirOfProcess)
        let previous = attributedByFolder[folder] ?? []
        attributedByFolder[folder] = Set(attributed.keys)
        for dir in previous.union(attributed.keys).sorted() {
            let snapshot = attributed[dir] ?? []
            if lastSnapshots[dir] != snapshot {
                lastSnapshots[dir] = snapshot
                handler?(dir, snapshot)
            }
        }
        return Set(entries.compactMap { Int32(exactly: $0.pid) })
    }

    // MARK: - Shared folders

    /// The physical sessions folder of a config folder, links resolved.
    static func sessionsFolder(of configDir: String) -> String {
        let path = (AccountPaths.normalize(configDir) as NSString).appendingPathComponent("sessions")
        return URL(fileURLWithPath: path).resolvingSymlinksInPath().path
    }

    /// Whether the config folder's `sessions` (or the folder itself) is a
    /// link: its entries may belong to other config folders then.
    static func isLinkedSessionsFolder(of configDir: String) -> Bool {
        let path = (AccountPaths.normalize(configDir) as NSString).appendingPathComponent("sessions")
        return URL(fileURLWithPath: path).resolvingSymlinksInPath().path != path
    }

    /// Config folders grouped by the physical sessions folder they lead to.
    static func groupedBySessionsFolder(_ dirs: Set<String>) -> [String: Set<String>] {
        var groups: [String: Set<String>] = [:]
        for dir in dirs {
            groups[sessionsFolder(of: dir), default: []].insert(dir)
        }
        return groups
    }

    /// Which config folder each entry belongs to. A folder only one config
    /// folder reaches directly keeps them all; a shared one asks each
    /// process: its `CLAUDE_CONFIG_DIR`, unset meaning `~/.claude`. A process
    /// that can't be asked goes to `~/.claude` if that leads here, else to the
    /// first folder that does. Pure apart from `configDirOfProcess`.
    ///
    /// A process whose `CLAUDE_CONFIG_DIR` names a folder by another path
    /// (the resolved target of a linked config folder, say `~/dotfiles/claude-work`
    /// for `~/.claude-work`) goes to the alias that resolves to the same place.
    static func attribute(
        _ entries: [SessionRegistryEntry],
        aliases: Set<String>,
        isShared: Bool,
        defaultDir: String,
        configDirOfProcess: (Int32) -> ProcessConfigDir.Value,
        resolve: (String) -> String = { URL(fileURLWithPath: $0).resolvingSymlinksInPath().path }
    ) -> [String: [SessionRegistryEntry]] {
        let sorted = aliases.sorted()
        guard let first = sorted.first else { return [:] }
        guard isShared else { return entries.isEmpty ? [:] : [first: entries] }
        let fallback = aliases.contains(defaultDir) ? defaultDir : first
        var resolvedAliases: [String: String]?
        func alias(for value: String) -> String {
            let normalized = AccountPaths.normalize(value)
            guard !aliases.contains(normalized) else { return normalized }
            if resolvedAliases == nil {
                resolvedAliases = Dictionary(sorted.map { (AccountPaths.normalize(resolve($0)), $0) }, uniquingKeysWith: { first, _ in first })
            }
            return resolvedAliases?[AccountPaths.normalize(resolve(normalized))] ?? normalized
        }
        var result: [String: [SessionRegistryEntry]] = [:]
        for entry in entries {
            let dir: String
            switch Int32(exactly: entry.pid).map(configDirOfProcess) ?? .unreadable {
            case .set(let value): dir = alias(for: value)
            case .unset: dir = defaultDir
            case .unreadable: dir = fallback
            }
            result[dir, default: []].append(entry)
        }
        return result
    }

    private func reportInitialScanIfDone() {
        guard !initialScanReported, accountDirsArrived, handler != nil, dirs.isSubset(of: scannedDirs) else { return }
        initialScanReported = true
        initialScanHandler?()
        initialScanHandler = nil
    }

    /// Tracked entries of `<configDir>/sessions/*.json` whose process is alive, sorted by pid.
    static func liveEntries(configDir: String, fileManager: FileManager = .default) -> [SessionRegistryEntry] {
        readEntries(configDir: configDir, fileManager: fileManager)
            .filter { $0.isTracked && isLive($0) }
            .sorted { $0.pid < $1.pid }
    }

    /// All parseable entries of the registry, without liveness filtering.
    static func readEntries(configDir: String, fileManager: FileManager = .default) -> [SessionRegistryEntry] {
        let sessionsDir = (AccountPaths.normalize(configDir) as NSString).appendingPathComponent("sessions")
        guard let names = try? fileManager.contentsOfDirectory(atPath: sessionsDir) else { return [] }
        var entries: [SessionRegistryEntry] = []
        // `<pid>.json` only; never `*.key` (secret material) or anything else.
        for name in names where name.hasSuffix(".json") && Int(name.dropLast(5)) != nil {
            let path = (sessionsDir as NSString).appendingPathComponent(name)
            guard let data = fileManager.contents(atPath: path),
                  data.count < 1_000_000,
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let entry = SessionRegistryEntry(json: json) else { continue }
            entries.append(entry)
        }
        return entries
    }

    // MARK: - Liveness

    /// The entry's process is running and is the same process that wrote it
    /// (guards against pid reuse by comparing process start times).
    static func isLive(_ entry: SessionRegistryEntry) -> Bool {
        guard ProcessID.isRunning(entry.pid) else { return false }
        guard let started = processStartDate(pid: entry.pid) else { return true }
        if let recorded = entry.procStart.flatMap(parseProcStart) {
            // lstart has one-second resolution.
            return abs(started.timeIntervalSince(recorded)) <= 2
        }
        if let startedAt = entry.startedAt {
            // A reused pid belongs to a process started after the entry was written.
            return started <= startedAt.addingTimeInterval(5)
        }
        return true
    }

    /// Start time of a running process, from the kernel.
    static func processStartDate(pid: Int) -> Date? {
        guard let pid = Int32(exactly: pid) else { return nil }
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.stride)
        let result = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size)
        guard result == size else { return nil }
        return Date(timeIntervalSince1970: TimeInterval(info.pbi_start_tvsec) + TimeInterval(info.pbi_start_tvusec) / 1_000_000)
    }

    private static let procStartFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "UTC")
        formatter.dateFormat = "EEE MMM d HH:mm:ss yyyy"
        return formatter
    }()

    /// Parses `ps -o lstart` text such as "Wed Sep  3 04:43:16 2026" (UTC).
    static func parseProcStart(_ text: String) -> Date? {
        let collapsed = text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
        return procStartFormatter.date(from: collapsed)
    }
}
