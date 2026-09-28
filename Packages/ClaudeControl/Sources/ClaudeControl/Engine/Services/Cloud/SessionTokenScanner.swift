//
//  SessionTokenScanner.swift
//  ClaudeControl
//
//  Per-session token totals from Claude Code's transcripts, for sync:
//  input, output, cache writes and cache reads, subagents included; what
//  they cost at list prices (`ModelPricing`), for sessions whose cost from
//  Claude Code is missing or can't be used; how many responses; the models
//  used, most used first; the first and last timestamp; and, for sessions
//  found only on disk, the working directory, entrypoint and title the
//  transcript records.
//
//  What counts, and once:
//  - A session is its transcript `<project>/<sessionId>.jsonl` plus its
//    subagents: `<project>/<sessionId>/subagents/**/agent-*.jsonl`, and the
//    older flat `<project>/agent-*.jsonl`, whose session is the one its
//    lines name.
//  - One API response is written as a line per content block, each
//    repeating the same `message.usage`: responses are keyed by
//    `message.id|requestId`, and a repeat replaces the earlier value.
//  - A response is counted by the first file that claims it. Older
//    transcripts repeat subagent lines inside the parent, and a resumed or
//    forked session may copy earlier lines into its new file: neither is
//    counted twice. Lines in a session's own transcript that name another
//    session (copied from it), or that `/branch` copied from another one
//    (`forkedFrom`), are left to that session altogether, their timestamps
//    and titles too.
//  - `<synthetic>` messages (local errors) aren't responses.
//  - Two config folders that share `projects/` (Claude Parallel Profiles)
//    name one file two ways: files are kept by their real path.
//  - A session more than one account ran (resumed under another account)
//    is split by its owners (`SessionOwner`, from the ledger): each line
//    counts for the account that ran the session at its timestamp. Every
//    session's totals are kept per account (`parts`).
//
//  Incremental: per file a watermark {size, mtime, inode, offset}, its share
//  of the totals per account and short hashes of the responses it counted
//  (the claims; the index from response to file is rebuilt from them in
//  memory). An unchanged file is not opened; a grown one is read from the
//  offset; one that shrank or was replaced (a new inode), or whose owners
//  changed for lines already counted, is recounted from the start, its
//  claims released first. State is kept in `cloud-scan-state.json`.
//
//  Only `.jsonl` files under a `projects` folder are ever opened; nothing
//  in a config folder's `sessions/` is. Synchronous and thread-safe: call it
//  off the main actor.
//

import CryptoKit
import Foundation
import os.log

/// Token totals in the contract's terms.
nonisolated struct CloudTokenTotals: Codable, Equatable, Sendable {
    var input = 0
    var output = 0
    var cacheCreation = 0
    var cacheRead = 0

    mutating func add(_ other: CloudTokenTotals) {
        input += other.input
        output += other.output
        cacheCreation += other.cacheCreation
        cacheRead += other.cacheRead
    }

    mutating func subtract(_ other: CloudTokenTotals) {
        input -= other.input
        output -= other.output
        cacheCreation -= other.cacheCreation
        cacheRead -= other.cacheRead
    }

    var total: Int { input + output + cacheCreation + cacheRead }

    var contract: CloudSyncRequest.Tokens {
        CloudSyncRequest.Tokens(input: max(0, input), output: max(0, output),
                                cacheCreation: max(0, cacheCreation), cacheRead: max(0, cacheRead))
    }
}

/// One account's share of a session (or the whole of it).
nonisolated struct SessionTokenPart: Equatable, Sendable {
    var tokens = CloudTokenTotals()
    /// Responses counted once.
    var messageCount = 0
    /// Most used first.
    var models: [String] = []
    var firstTimestamp: Date?
    var lastTimestamp: Date?
    /// What its responses cost at list prices; nil when a model that made
    /// one has no known price.
    var cost: NanoUSD?

    /// The cost in dollars, as the website gets it.
    var estimatedCostUsd: Double? { cost.map(ModelPricing.dollars) }
}

/// One session's totals over its files.
nonisolated struct SessionTokenSummary: Equatable, Sendable {
    var sessionId: String
    var tokens = CloudTokenTotals()
    /// Responses counted once.
    var messageCount = 0
    /// Most used first.
    var models: [String] = []
    var firstTimestamp: Date?
    var lastTimestamp: Date?
    /// What its responses cost at list prices; nil when a model that made
    /// one has no known price.
    var cost: NanoUSD?
    /// The working directory the transcript's first line records.
    var cwd: String?
    var entrypoint: String?
    /// The transcript's own title (custom, else AI-generated, else summary). Never a prompt.
    var title: String?
    /// Bytes of the main transcript, and when it was last written.
    var transcriptBytes: UInt64 = 0
    var transcriptModified: Double = 0
    /// By the account key of the owner each line was counted for ("" when
    /// scanned with no owner).
    var parts: [String: SessionTokenPart] = [:]

    /// What `accountKey` ran of it (nothing when it ran none).
    func part(for accountKey: String) -> SessionTokenPart {
        parts[accountKey] ?? SessionTokenPart()
    }
}

nonisolated final class SessionTokenScanner: @unchecked Sendable {
    static let fileName = "cloud-scan-state.json"

    private static var logger: Logger { EngineLog.logger("TokenScanner") }

    /// Responses a file remembers for replacing repeats (a response's lines
    /// are written together, so a few are enough).
    static let rememberedResponses = 32

    nonisolated struct Response: Codable, Equatable, Sendable {
        var key: String
        var model: String
        var usage: CloudTokenTotals
        /// The account it was counted for.
        var owner: String
        /// At list prices; nil when its model has no known price.
        var cost: NanoUSD?
    }

    /// A file's share of one account's part.
    nonisolated struct Part: Codable, Equatable, Sendable {
        var totals = CloudTokenTotals()
        var responses = 0
        var modelCounts: [String: Int] = [:]
        var first: Date?
        var last: Date?
        /// What the priced responses cost, and how many had no known price.
        var cost: NanoUSD = 0
        var unpriced = 0

        mutating func note(_ stamp: Date) {
            first = min(first ?? stamp, stamp)
            last = max(last ?? stamp, stamp)
        }

        // Wrapping, so a damaged transcript's sums can't trap and a
        // removal always undoes its add; `knownCost` refuses what wrapped.
        mutating func add(cost response: NanoUSD?) {
            if let response { cost &+= response } else { unpriced += 1 }
        }

        mutating func remove(cost response: NanoUSD?) {
            if let response { cost &-= response } else { unpriced -= 1 }
        }

        /// Nil while any response has no known price, or when the sum is
        /// beyond what the website takes.
        var knownCost: NanoUSD? {
            guard unpriced <= 0, cost >= 0, Double(cost) < CloudContract.Limit.costUsd * 1_000_000_000 else { return nil }
            return cost
        }
    }

    nonisolated struct FileState: Codable, Equatable, Sendable {
        /// The session the file belongs to ("" until a flat agent file names it).
        var sessionId: String
        var size: UInt64 = 0
        var mtime: Double = 0
        var inode: UInt64 = 0
        var offset: UInt64 = 0
        /// The owners the file was counted with.
        var owners: [SessionOwner] = []
        /// By owner account key.
        var parts: [String: Part] = [:]
        var first: Date?
        var last: Date?
        var cwd: String?
        var entrypoint: String?
        var customTitle: String?
        var aiTitle: String?
        var summaryTitle: String?
        var recent: [Response] = []
        /// Hashes of the responses this file counted.
        var claimed: [String] = []
        /// Whether this is the session's own transcript (not a subagent's).
        var isMain = false
    }

    nonisolated struct State: Codable, Equatable, Sendable {
        /// 3: totals per owner account. 4: and what they cost (every
        /// transcript is read again once).
        static let currentVersion = 4
        var version = State.currentVersion
        /// By real path.
        var files: [String: FileState] = [:]
    }

    private let lock = NSLock()
    private var state: State
    /// Response hash → real path of the file that counted it (from `claimed`).
    private var claims: [String: String] = [:]
    private var filesOfSession: [String: Set<String>] = [:]
    private var dirty = false
    private let file: CloudStateFile<State>
    /// A transcript's first own timestamp, by real path and inode (a
    /// transcript is only ever appended to), for ordering the backfill.
    private var heads: [String: (inode: UInt64, first: Date?)] = [:]

    /// - Parameters:
    ///   - fileURL: where the watermarks live (nil: memory only).
    ///   - persists: false for sealed runs.
    init(fileURL: URL?, persists: Bool) {
        file = CloudStateFile(url: fileURL, persists: persists, label: "cloud-scan")
        if let saved = file.load(), saved.version == State.currentVersion {
            state = saved
        } else {
            state = State()
        }
        rebuildIndex()
    }

    private func rebuildIndex() {
        filesOfSession = [:]
        claims = [:]
        for (path, file) in state.files {
            if !file.sessionId.isEmpty { filesOfSession[file.sessionId, default: []].insert(path) }
            for key in file.claimed where claims[key] == nil { claims[key] = path }
        }
    }

    // MARK: - Paths

    /// A transcript the scanner may open: a `.jsonl` file under a `projects`
    /// folder, with no `sessions` folder below that. Pure.
    static func isSafeTranscriptPath(_ path: String) -> Bool {
        guard path.hasSuffix(".jsonl") else { return false }
        let components = (path as NSString).pathComponents
        guard let projects = components.lastIndex(of: "projects") else { return false }
        let below = components[(projects + 1)...]
        return below.count >= 2 && !below.contains("sessions") && !below.contains("..") && !below.contains(".")
    }

    /// Main transcripts under a physical `projects` folder:
    /// `<root>/<slug>/<sessionId>.jsonl`, where the id looks like a UUID.
    static func sessionFiles(inProjectsRoot root: String, fileManager: FileManager = .default) -> [(sessionId: String, path: String)] {
        guard (root as NSString).lastPathComponent == "projects",
              let slugs = try? fileManager.contentsOfDirectory(atPath: root) else { return [] }
        var found: [(String, String)] = []
        for slug in slugs.sorted() where !slug.hasPrefix(".") && slug != "sessions" {
            let folder = (root as NSString).appendingPathComponent(slug)
            guard let names = try? fileManager.contentsOfDirectory(atPath: folder) else { continue }
            for name in names.sorted() where name.hasSuffix(".jsonl") {
                let id = String(name.dropLast(".jsonl".count))
                guard isSessionId(id) else { continue }
                found.append((id, (folder as NSString).appendingPathComponent(name)))
            }
        }
        return found
    }

    /// Claude Code's session ids are UUIDs.
    static func isSessionId(_ id: String) -> Bool {
        CloudKeys.isUUID(id)
    }

    // MARK: - Scanning

    /// Bring the session's files up to date and return its totals, split by
    /// `owners` (who ran it from when; empty: one unnamed owner, ""). Nil
    /// when its transcript can't be read.
    func scan(sessionId: String, transcriptPath: String, owners: [SessionOwner] = []) -> SessionTokenSummary? {
        guard Self.isSessionId(sessionId), Self.isSafeTranscriptPath(transcriptPath) else { return nil }
        let main = TranscriptLocator.realPath(transcriptPath)
        guard Self.isSafeTranscriptPath(main), FileManager.default.fileExists(atPath: main) else { return nil }
        let projectDir = (main as NSString).deletingLastPathComponent
        return lock.withLock {
            // Files of this session that went away take their counts with them.
            for path in filesOfSession[sessionId] ?? [] where path != main && !FileManager.default.fileExists(atPath: path) {
                dropFile(path)
            }
            update(file: main, sessionId: sessionId, isMain: true, owners: owners)
            for path in Self.nestedSubagentFiles(projectDir: projectDir, sessionId: sessionId) {
                update(file: path, sessionId: sessionId, isMain: false, owners: owners)
            }
            // Flat agent files name their session in their lines: this
            // session's, and ones not read yet (another session's are read
            // when that one is scanned).
            for path in Self.flatAgentFiles(projectDir: projectDir) {
                let owner = state.files[path]?.sessionId
                guard owner == nil || owner == "" || owner == sessionId else { continue }
                update(file: path, sessionId: nil, isMain: false, owners: owners)
            }
            return summary(of: sessionId, main: main)
        }
    }

    /// The first timestamp of a transcript's own lines (not those copied
    /// from another session), reading lines until one is found (giving up
    /// at a line longer than a megabyte); the counted value when the file
    /// was scanned. For ordering and dating the backfill without scanning.
    /// Nil when none is found. Like `scan`, the path a link resolves to is
    /// checked too: a `<id>.jsonl` link to a file outside `projects` (one
    /// in a config folder's `sessions/`, a credential file) is never opened.
    func firstTimestamp(ofTranscript path: String, sessionId: String) -> Date? {
        guard Self.isSafeTranscriptPath(path) else { return nil }
        let real = TranscriptLocator.realPath(path)
        guard Self.isSafeTranscriptPath(real) else { return nil }
        var info = stat()
        guard stat(real, &info) == 0 else { return nil }
        let inode = UInt64(info.st_ino)
        if let known = lock.withLock({ () -> Date?? in
            if let file = state.files[real], file.inode == inode, file.isMain { return .some(file.first) }
            if let head = heads[real], head.inode == inode { return .some(head.first) }
            return nil
        }) {
            return known
        }
        let first = Self.readFirstTimestamp(path: real, sessionId: sessionId)
        lock.withLock { heads[real] = (inode, first) }
        return first
    }

    /// Reads lines from the start until one of the session's own carries a
    /// timestamp (giving up at a line longer than `limit` bytes). Pure
    /// apart from the read.
    static func readFirstTimestamp(path: String, sessionId: String, limit: Int = 1024 * 1024) -> Date? {
        guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
        defer { try? handle.close() }
        var buffer = Data()
        while buffer.count < limit {
            guard let chunk = try? handle.read(upToCount: 64 * 1024), !chunk.isEmpty else { break }
            buffer.append(chunk)
            guard let lastNewline = buffer.lastIndex(of: 0x0A) else { continue }
            var found: Date?
            TranscriptLineReader.forEachLine(in: buffer[buffer.startIndex...lastNewline]) { line in
                guard found == nil, let json = ConversationParser.decode(line), !isCopied(json, into: sessionId) else { return }
                found = (json["timestamp"] as? String).flatMap(ConversationParser.parseDate)
            }
            if let found { return found }
            buffer = Data(buffer[buffer.index(after: lastNewline)...])
        }
        return nil
    }

    /// When the file was created (for ordering ties); nil when unknown.
    static func birthTime(of path: String) -> Date? {
        var info = stat()
        guard stat(path, &info) == 0 else { return nil }
        return Date(timeIntervalSince1970: Double(info.st_birthtimespec.tv_sec) + Double(info.st_birthtimespec.tv_nsec) / 1e9)
    }

    /// A line of a session's own transcript that belongs to another
    /// session: one naming it, or one `/branch` copied from it. Pure.
    static func isCopied(_ json: [String: Any], into sessionId: String) -> Bool {
        if let owner = json["sessionId"] as? String, !owner.isEmpty, owner != sessionId { return true }
        if let origin = (json["forkedFrom"] as? [String: Any])?["sessionId"] as? String, !origin.isEmpty,
           origin != sessionId {
            return true
        }
        return false
    }

    /// The session's totals as last scanned (no file is read).
    func cachedSummary(of sessionId: String) -> SessionTokenSummary? {
        lock.withLock {
            guard let main = filesOfSession[sessionId]?.first(where: { state.files[$0]?.isMain == true }) else { return nil }
            return summary(of: sessionId, main: main)
        }
    }

    /// Forget files that are gone (Claude Code deletes old transcripts):
    /// their totals were sent, and their claims no longer need keeping.
    /// Returns how many were forgotten.
    @discardableResult
    func pruneMissingFiles(fileManager: FileManager = .default) -> Int {
        lock.withLock {
            let gone = state.files.keys.filter { !fileManager.fileExists(atPath: $0) }
            for path in gone { dropFile(path) }
            return gone.count
        }
    }

    /// Write the watermarks if anything changed (in the background).
    func save() {
        let snapshot: State? = lock.withLock {
            guard dirty else { return nil }
            dirty = false
            return state
        }
        if let snapshot { file.save(snapshot) }
    }

    /// Write the watermarks now.
    func saveNow() {
        let snapshot = lock.withLock { () -> State in
            dirty = false
            return state
        }
        file.saveNow(snapshot)
    }

    static func nestedSubagentFiles(projectDir: String, sessionId: String) -> [String] {
        guard isSessionId(sessionId) else { return [] }
        let folder = (projectDir as NSString).appendingPathComponent("\(sessionId)/subagents")
        guard let enumerator = FileManager.default.enumerator(atPath: folder) else { return [] }
        var files: [String] = []
        while let relative = enumerator.nextObject() as? String {
            let name = (relative as NSString).lastPathComponent
            guard name.hasPrefix("agent-"), name.hasSuffix(".jsonl") else { continue }
            let path = (folder as NSString).appendingPathComponent(relative)
            guard isSafeTranscriptPath(path) else { continue }
            files.append(path)
        }
        return files.sorted()
    }

    static func flatAgentFiles(projectDir: String) -> [String] {
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: projectDir) else { return [] }
        return names.filter { $0.hasPrefix("agent-") && $0.hasSuffix(".jsonl") }
            .sorted()
            .map { (projectDir as NSString).appendingPathComponent($0) }
            .filter(isSafeTranscriptPath)
    }

    /// A response's key as the claims keep it: 16 hex digits of its SHA-256
    /// (short, and a collision among a Mac's responses is out of reach). Pure.
    static func claimKey(_ key: String) -> String {
        SHA256.hash(data: Data(key.utf8)).prefix(8).map { String(format: "%02x", $0) }.joined()
    }

    // MARK: - One file (lock held)

    private func update(file path: String, sessionId: String?, isMain: Bool, owners: [SessionOwner]) {
        var info = stat()
        guard stat(path, &info) == 0 else {
            if state.files[path] != nil { dropFile(path) }
            return
        }
        let size = UInt64(max(0, info.st_size))
        let mtime = Double(info.st_mtimespec.tv_sec) + Double(info.st_mtimespec.tv_nsec) / 1_000_000_000
        let inode = UInt64(info.st_ino)

        var current = state.files[path] ?? FileState(sessionId: sessionId ?? "", owners: owners)
        if let existing = state.files[path] {
            if existing.size == size, existing.mtime == mtime, existing.inode == inode, existing.owners == owners { return }
            let ownersHold = SessionOwners.agree(existing.owners, owners, through: existing.last)
            if size < existing.offset || (existing.inode != 0 && existing.inode != inode) || !ownersHold {
                // Rewritten or replaced, or lines already counted belong to
                // another account now: counted again from the start.
                releaseClaims(of: path)
                current = FileState(sessionId: sessionId ?? existing.sessionId, owners: owners)
            }
            current.owners = owners
        }
        if let sessionId, current.sessionId != sessionId {
            current.sessionId = sessionId
        }
        current.isMain = current.isMain || isMain

        var offset = current.offset
        var outcome = TranscriptLineReader.forEachLine(path: path, from: &offset) { line in
            consume(line, into: &current, path: path)
        }
        if outcome?.didReset == true {
            // It shrank between the stat and the read: count it again from
            // the start, releasing what the saved state and this read claimed.
            releaseClaims(of: path)
            release(current.claimed, of: path)
            let keptSession = current.sessionId
            current = FileState(sessionId: keptSession, owners: owners)
            current.isMain = isMain
            offset = 0
            outcome = TranscriptLineReader.forEachLine(path: path, from: &offset) { line in
                consume(line, into: &current, path: path)
            }
        }
        guard outcome != nil else { return }
        current.offset = offset
        current.size = size
        current.mtime = mtime
        current.inode = inode

        let previousSession = state.files[path]?.sessionId
        if let previousSession, previousSession != current.sessionId {
            filesOfSession[previousSession]?.remove(path)
        }
        state.files[path] = current
        if !current.sessionId.isEmpty {
            filesOfSession[current.sessionId, default: []].insert(path)
        }
        dirty = true
    }

    private func consume(_ line: Data, into file: inout FileState, path: String) {
        guard let json = ConversationParser.decode(line) else { return }
        let lineSession = (json["sessionId"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        if file.sessionId.isEmpty, let lineSession {
            file.sessionId = lineSession
        }
        // A line a resumed or forked session copied from another one: that
        // session's, counted (and dated) from its own transcript.
        if file.isMain, Self.isCopied(json, into: file.sessionId) { return }
        let stamp = (json["timestamp"] as? String).flatMap(ConversationParser.parseDate)
        // Whose it is: the account that ran the session then (a line with no
        // time goes with the latest one seen).
        let owner = SessionOwners.owner(at: stamp ?? file.last, in: file.owners)
        if let stamp {
            file.first = min(file.first ?? stamp, stamp)
            file.last = max(file.last ?? stamp, stamp)
            file.parts[owner, default: Part()].note(stamp)
        }
        if file.cwd == nil, let cwd = json["cwd"] as? String, !cwd.isEmpty {
            file.cwd = cwd
        }
        if file.entrypoint == nil, let entrypoint = json["entrypoint"] as? String, !entrypoint.isEmpty {
            file.entrypoint = entrypoint
        }
        switch json["type"] as? String {
        case "summary":
            if let text = json["summary"] as? String, !text.isEmpty { file.summaryTitle = text }
        case "ai-title":
            if let text = json["aiTitle"] as? String, !text.isEmpty { file.aiTitle = text }
        case "custom-title":
            if let text = json["customTitle"] as? String, !text.isEmpty { file.customTitle = text }
        case "assistant":
            consumeResponse(json, into: &file, path: path, owner: owner)
        default:
            break
        }
    }

    private func consumeResponse(_ json: [String: Any], into file: inout FileState, path: String, owner: String) {
        guard let message = json["message"] as? [String: Any],
              let usage = message["usage"] as? [String: Any] else { return }
        let model = (message["model"] as? String) ?? ""
        if model == "<synthetic>" { return }
        let raw: String
        if let id = message["id"] as? String, !id.isEmpty {
            raw = "\(id)|\((json["requestId"] as? String) ?? "")"
        } else if let uuid = json["uuid"] as? String, !uuid.isEmpty {
            raw = "uuid:\(uuid)"
        } else {
            return
        }
        let key = Self.claimKey(raw)
        let entry = CloudTokenTotals(
            input: max(0, JSONValue.int(usage["input_tokens"]) ?? 0),
            output: max(0, JSONValue.int(usage["output_tokens"]) ?? 0),
            cacheCreation: max(0, JSONValue.int(usage["cache_creation_input_tokens"]) ?? 0),
            cacheRead: max(0, JSONValue.int(usage["cache_read_input_tokens"]) ?? 0)
        )
        switch claims[key] {
        case nil:
            let cost = ModelPricing.cost(model: model, usage: usage)
            claims[key] = path
            file.claimed.append(key)
            file.parts[owner, default: Part()].totals.add(entry)
            file.parts[owner, default: Part()].responses += 1
            file.parts[owner, default: Part()].add(cost: cost)
            if !model.isEmpty { file.parts[owner, default: Part()].modelCounts[model, default: 0] += 1 }
            file.recent.append(Response(key: key, model: model, usage: entry, owner: owner, cost: cost))
            if file.recent.count > Self.rememberedResponses {
                file.recent.removeFirst(file.recent.count - Self.rememberedResponses)
            }
        case path:
            // Another line of a response this file counted: its latest usage
            // wins, in the part it was first counted in.
            guard let index = file.recent.lastIndex(where: { $0.key == key }) else { return }
            let counted = file.recent[index].owner
            let cost = ModelPricing.cost(model: model.isEmpty ? file.recent[index].model : model, usage: usage)
            file.parts[counted, default: Part()].totals.subtract(file.recent[index].usage)
            file.parts[counted, default: Part()].totals.add(entry)
            file.parts[counted, default: Part()].remove(cost: file.recent[index].cost)
            file.parts[counted, default: Part()].add(cost: cost)
            file.recent[index].cost = cost
            let previousModel = file.recent[index].model
            if previousModel != model, !model.isEmpty {
                if !previousModel.isEmpty {
                    file.parts[counted, default: Part()].modelCounts[previousModel, default: 1] -= 1
                    if file.parts[counted]?.modelCounts[previousModel] ?? 0 <= 0 {
                        file.parts[counted]?.modelCounts.removeValue(forKey: previousModel)
                    }
                }
                file.parts[counted, default: Part()].modelCounts[model, default: 0] += 1
                file.recent[index].model = model
            }
            file.recent[index].usage = entry
        default:
            // Counted by another file (a subagent's own file, the session a
            // fork copied it from).
            return
        }
    }

    private func releaseClaims(of path: String) {
        guard let claimed = state.files[path]?.claimed else { return }
        release(claimed, of: path)
    }

    private func release(_ keys: [String], of path: String) {
        for key in keys where claims[key] == path {
            claims.removeValue(forKey: key)
        }
    }

    private func dropFile(_ path: String) {
        releaseClaims(of: path)
        if let sessionId = state.files[path]?.sessionId {
            filesOfSession[sessionId]?.remove(path)
        }
        state.files.removeValue(forKey: path)
        dirty = true
    }

    // MARK: - Totals (lock held)

    private func summary(of sessionId: String, main: String) -> SessionTokenSummary? {
        guard let mainFile = state.files[main] else { return nil }
        var result = SessionTokenSummary(sessionId: sessionId)
        var whole = Part()
        var parts: [String: Part] = [:]
        func add(_ part: Part, to total: inout Part) {
            total.totals.add(part.totals)
            total.responses += part.responses
            let (cost, overflowed) = total.cost.addingReportingOverflow(part.cost)
            total.cost = cost
            total.unpriced += part.unpriced + (overflowed ? 1 : 0)
            for (model, count) in part.modelCounts { total.modelCounts[model, default: 0] += count }
            if let first = part.first { total.first = min(total.first ?? first, first) }
            if let last = part.last { total.last = max(total.last ?? last, last) }
        }
        for path in (filesOfSession[sessionId] ?? []).sorted() {
            guard let file = state.files[path] else { continue }
            for (owner, part) in file.parts {
                add(part, to: &whole)
                add(part, to: &parts[owner, default: Part()])
            }
            // Timestamps of lines outside any part (none today) still date the session.
            if let first = file.first { whole.first = min(whole.first ?? first, first) }
            if let last = file.last { whole.last = max(whole.last ?? last, last) }
        }
        func models(_ counts: [String: Int]) -> [String] {
            counts.filter { $0.value > 0 }.sorted { lhs, rhs in
                lhs.value != rhs.value ? lhs.value > rhs.value : lhs.key < rhs.key
            }.map(\.key)
        }
        result.tokens = whole.totals
        result.messageCount = whole.responses
        result.models = models(whole.modelCounts)
        result.firstTimestamp = whole.first
        result.lastTimestamp = whole.last
        result.cost = whole.knownCost
        result.parts = parts.mapValues { part in
            SessionTokenPart(tokens: part.totals, messageCount: part.responses, models: models(part.modelCounts),
                             firstTimestamp: part.first, lastTimestamp: part.last, cost: part.knownCost)
        }
        result.cwd = mainFile.cwd
        result.entrypoint = mainFile.entrypoint
        result.title = mainFile.customTitle ?? mainFile.aiTitle ?? mainFile.summaryTitle
        result.transcriptBytes = mainFile.size
        result.transcriptModified = mainFile.mtime
        return result
    }
}
