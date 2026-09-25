//
//  CloudModels.swift
//  ClaudeControl
//
//  The app ⇄ website API, version 1, as Swift values. The contract is
//  `web/contract/README.md`; its fixtures are what the tests encode and
//  decode against, so a field renamed here without the website breaks a test.
//
//  - Dates are ISO 8601 in UTC with a `Z`. The app writes milliseconds and
//    reads either form (`CloudJSON`).
//  - Nullable fields are sent as an explicit `null` (the website's schemas
//    require the key); `sessions[].summary` is left out when there is none,
//    which tells the website to keep the summary it has.
//  - Keys that name a Claude account or a project are digests made here
//    (`CloudKeys`): the website never sees an account UUID or a path. The
//    account key is a plain SHA-256 (pooling joins on it across people);
//    the project key is an HMAC keyed with this install's own secret, so a
//    guessed path can't be checked against it.
//  - Every date in a request lies between 2023-01-01 and a day after now;
//    a session or reading with a date outside is left out (`clamped`). A
//    window's reset time may lie up to 32 days ahead; one outside that is
//    sent as null.
//

import CryptoKit
import Foundation

/// Constants of the contract.
nonisolated enum CloudContract {
    static let schemaVersion = 1
    /// Where Supabase sends the browser back after Google sign-in.
    static let redirectURL = "agentnotch://auth-callback"
    static let callbackScheme = "agentnotch"
    static let callbackHost = "auth-callback"

    /// Upper bounds the website enforces (lengths in UTF-16 units, as its
    /// schemas count them).
    enum Limit {
        static let deviceName = 120
        static let appVersion = 40
        static let accounts = 50
        static let email = 320
        static let organizationName = 200
        static let plan = 60
        static let label = 80
        static let sessions = 200
        static let projectName = 120
        static let title = 200
        static let models = 10
        static let usage = 500
        static let windows = 20
        static let summaryText = 2000
        static let summaryModel = 80
        /// Not in the contract's table: what the website's schemas and
        /// columns hold (a model id, Postgres `integer`, JavaScript's
        /// largest exact integer, `numeric(14, 6)`).
        static let modelId = 200
        static let messageCount = 2_147_483_647
        static let tokenCount = 9_007_199_254_740_991
        static let costUsd: Double = 100_000_000
    }

    /// Paths under the website's address.
    enum Path {
        static let config = "api/app/v1/config"
        static let me = "api/app/v1/me"
        static let sync = "api/app/v1/sync"
    }

    /// The earliest date a request may carry: 2023-01-01T00:00:00Z.
    static let earliestDate = Date(timeIntervalSince1970: 1_672_531_200)
    /// How far past the clock a date may lie.
    static let futureAllowance: TimeInterval = 24 * 60 * 60

    /// How far past the clock a usage window's `resetsAt` may lie (a weekly
    /// window resets within a week).
    static let resetsAtAllowance: TimeInterval = 32 * 24 * 60 * 60

    /// A date the website takes: from 2023-01-01 to a day after `now`. Pure.
    static func accepts(_ date: Date, now: Date) -> Bool {
        date >= earliestDate && date <= now.addingTimeInterval(futureAllowance)
    }

    /// A window's reset time the website takes: from 2023-01-01 to 32 days
    /// after `now`. Pure.
    static func acceptsResetsAt(_ date: Date, now: Date) -> Bool {
        date >= earliestDate && date <= now.addingTimeInterval(resetsAtAllowance)
    }
}

// MARK: - Dates and JSON

/// One encoder and decoder for everything the website sends and receives.
nonisolated enum CloudJSON {
    private static let withFraction = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
    private static let wholeSeconds = Date.ISO8601FormatStyle()

    /// `2026-09-25T09:47:03.120Z`: rounded to the millisecond (the
    /// formatter truncates, which turns a parsed `.482` into `.481`).
    static func string(from date: Date) -> String {
        let milliseconds = (date.timeIntervalSince1970 * 1000).rounded()
        let seconds = (milliseconds / 1000).rounded(.down)
        let whole = Date(timeIntervalSince1970: seconds).formatted(wholeSeconds)
        let fraction = Int(milliseconds - seconds * 1000)
        return String(whole.dropLast()) + "." + String(format: "%03d", fraction) + "Z"
    }

    /// ISO 8601 with or without fractional seconds.
    static func date(from string: String) -> Date? {
        (try? withFraction.parse(string)) ?? (try? wholeSeconds.parse(string))
    }

    static func makeEncoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        encoder.dateEncodingStrategy = .custom { date, encoder in
            var container = encoder.singleValueContainer()
            try container.encode(string(from: date))
        }
        return encoder
    }

    static func makeDecoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let text = try container.decode(String.self)
            guard let date = date(from: text) else {
                throw DecodingError.dataCorruptedError(in: container, debugDescription: "Not an ISO 8601 date: \(text)")
            }
            return date
        }
        return decoder
    }
}

// MARK: - Responses

/// `GET /api/app/v1/config`: what a sign-in needs.
nonisolated struct CloudConfig: Codable, Equatable, Sendable {
    var supabaseUrl: String
    var supabasePublishableKey: String
    var redirectUrl: String
    var dashboardUrl: String
}

/// `GET /api/app/v1/me`: the signed-in user.
nonisolated struct CloudMe: Codable, Equatable, Sendable {
    nonisolated struct User: Codable, Equatable, Sendable {
        var id: String
        var email: String?
        var name: String?
    }

    var user: User
    var dashboardUrl: String
}

/// `POST /api/app/v1/sync` answered 200.
nonisolated struct CloudSyncResponse: Codable, Equatable, Sendable {
    nonisolated struct Accepted: Codable, Equatable, Sendable {
        var sessions: Int
        var usage: Int
    }

    var accepted: Accepted
    var serverTime: Date
}

/// The website's error body: `{"error": {"code", "message"}}`.
nonisolated struct CloudErrorBody: Codable, Equatable, Sendable {
    nonisolated struct Detail: Codable, Equatable, Sendable {
        var code: String
        var message: String
    }

    var error: Detail
}

/// The error codes the contract names.
nonisolated enum CloudErrorCode: String, Sendable, CaseIterable {
    case unauthorized = "UNAUTHORIZED"
    case forbidden = "FORBIDDEN"
    case badRequest = "BAD_REQUEST"
    case payloadTooLarge = "PAYLOAD_TOO_LARGE"
    case rateLimited = "RATE_LIMITED"
    case internalError = "INTERNAL"
}

// MARK: - Sync request

/// Where a session ran: `sessions[].source`.
nonisolated enum CloudSessionSource: String, Codable, Sendable, CaseIterable {
    case cli
    case vscode
    case desktop
    case sdk
    case other

    /// From Claude Code's entrypoint (`CLAUDE_CODE_ENTRYPOINT`, the
    /// registry's or a transcript line's `entrypoint`): `cli`,
    /// `claude-vscode`, `claude-desktop` (Claude Desktop hosts Claude Code
    /// sessions too), `sdk-ts`/`sdk-py`/`sdk-cli`. Unknown or missing is `other`.
    static func from(entrypoint: String?) -> CloudSessionSource {
        guard let value = entrypoint?.trimmingCharacters(in: .whitespaces).lowercased(), !value.isEmpty else {
            return .other
        }
        if value == "cli" { return .cli }
        if value.contains("vscode") { return .vscode }
        if value.contains("desktop") { return .desktop }
        if value.hasPrefix("sdk") { return .sdk }
        return .other
    }
}

/// Where a usage reading came from: `usage[].source`. Claude Desktop's
/// cache and Claude Code's `.claude.json` cache are told apart here.
nonisolated enum CloudUsageSource: String, Codable, Sendable, CaseIterable {
    case probe
    case statusLine
    case claudeJson
    case desktop
}

nonisolated struct CloudSyncRequest: Codable, Equatable, Sendable {
    var schemaVersion: Int = CloudContract.schemaVersion
    var device: Device
    var accounts: [Account]
    var sessions: [Session]
    var usage: [UsageReading]

    nonisolated struct Device: Codable, Equatable, Sendable {
        var id: String
        var name: String
        var appVersion: String
    }

    nonisolated struct Account: Codable, Equatable, Sendable {
        var key: String
        var email: String?
        var organizationName: String?
        var plan: String?
        var label: String?

        private enum CodingKeys: String, CodingKey { case key, email, organizationName, plan, label }

        init(key: String, email: String?, organizationName: String?, plan: String?, label: String?) {
            self.key = key
            self.email = email
            self.organizationName = organizationName
            self.plan = plan
            self.label = label
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            try container.encode(key, forKey: .key)
            try container.encode(email, forKey: .email)
            try container.encode(organizationName, forKey: .organizationName)
            try container.encode(plan, forKey: .plan)
            try container.encode(label, forKey: .label)
        }
    }

    nonisolated struct Project: Codable, Equatable, Sendable {
        var key: String
        var name: String
    }

    nonisolated struct Tokens: Codable, Equatable, Sendable {
        var input: Int
        var output: Int
        var cacheCreation: Int
        var cacheRead: Int

        static let zero = Tokens(input: 0, output: 0, cacheCreation: 0, cacheRead: 0)
    }

    nonisolated struct Summary: Codable, Equatable, Sendable {
        var text: String
        var model: String
        var generatedAt: Date
    }

    nonisolated struct Session: Codable, Equatable, Sendable {
        var accountKey: String
        var sessionId: String
        var project: Project
        var title: String?
        var source: CloudSessionSource
        var models: [String]
        var startedAt: Date
        var lastActivityAt: Date
        var endedAt: Date?
        var messageCount: Int
        var tokens: Tokens
        var costUsd: Double?
        /// Absent unless session summaries are on and one was made.
        var summary: Summary?

        private enum CodingKeys: String, CodingKey {
            case accountKey, sessionId, project, title, source, models, startedAt, lastActivityAt, endedAt
            case messageCount, tokens, costUsd, summary
        }

        init(accountKey: String, sessionId: String, project: Project, title: String?, source: CloudSessionSource,
             models: [String], startedAt: Date, lastActivityAt: Date, endedAt: Date?, messageCount: Int,
             tokens: Tokens, costUsd: Double?, summary: Summary? = nil) {
            self.accountKey = accountKey
            self.sessionId = sessionId
            self.project = project
            self.title = title
            self.source = source
            self.models = models
            self.startedAt = startedAt
            self.lastActivityAt = lastActivityAt
            self.endedAt = endedAt
            self.messageCount = messageCount
            self.tokens = tokens
            self.costUsd = costUsd
            self.summary = summary
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            try container.encode(accountKey, forKey: .accountKey)
            try container.encode(sessionId, forKey: .sessionId)
            try container.encode(project, forKey: .project)
            try container.encode(title, forKey: .title)
            try container.encode(source, forKey: .source)
            try container.encode(models, forKey: .models)
            try container.encode(startedAt, forKey: .startedAt)
            try container.encode(lastActivityAt, forKey: .lastActivityAt)
            try container.encode(endedAt, forKey: .endedAt)
            try container.encode(messageCount, forKey: .messageCount)
            try container.encode(tokens, forKey: .tokens)
            try container.encode(costUsd, forKey: .costUsd)
            try container.encodeIfPresent(summary, forKey: .summary)
        }
    }

    nonisolated struct UsageWindow: Codable, Equatable, Hashable, Sendable {
        /// `session`, `weekly_all`, `weekly_<model>` or `extra_usage`.
        var id: String
        /// 0–100, may exceed 100.
        var utilization: Double
        var resetsAt: Date?

        private enum CodingKeys: String, CodingKey { case id, utilization, resetsAt }

        init(id: String, utilization: Double, resetsAt: Date?) {
            self.id = id
            self.utilization = utilization
            self.resetsAt = resetsAt
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            try container.encode(id, forKey: .id)
            try container.encode(utilization, forKey: .utilization)
            try container.encode(resetsAt, forKey: .resetsAt)
        }
    }

    nonisolated struct UsageReading: Codable, Equatable, Sendable {
        var accountKey: String
        var source: CloudUsageSource
        /// When Claude Code or Claude Desktop took the reading.
        var observedAt: Date
        var windows: [UsageWindow]
    }
}

// MARK: - Limits

extension CloudSyncRequest {
    /// The request with every string and list cut to the contract's limits,
    /// counts and token totals within what the website stores, numbers JSON
    /// can't carry (NaN, infinity) dropped, and any session or reading the
    /// website's schemas would refuse left out (one bad item fails the whole
    /// request there): an id that isn't a UUID, a key that isn't 64 hex
    /// digits, an account missing from `accounts`, a date before 2023 or
    /// more than a day after `now` (a summary with such a date goes; its
    /// session stays). What is sent is always this. A window's `resetsAt`
    /// may be up to 32 days ahead (a weekly window resets up to a week
    /// ahead); one before 2023 or further ahead (a zero or a garbled value)
    /// is sent as null, and the reading goes with its utilization.
    nonisolated func clamped(now: Date = Date()) -> CloudSyncRequest {
        typealias L = CloudContract.Limit
        func accepts(_ date: Date) -> Bool { CloudContract.accepts(date, now: now) }
        var copy = self
        copy.device.name = device.name.clampedUTF16(L.deviceName)
        copy.device.appVersion = device.appVersion.clampedUTF16(L.appVersion)
        copy.accounts = accounts.filter { CloudKeys.isKey($0.key) }.prefix(L.accounts).map { account in
            var account = account
            account.email = account.email?.clampedUTF16(L.email)
            account.organizationName = account.organizationName?.clampedUTF16(L.organizationName)
            account.plan = account.plan?.clampedUTF16(L.plan)
            account.label = account.label?.clampedUTF16(L.label)
            return account
        }
        let known = Set(copy.accounts.map(\.key))
        copy.sessions = sessions.filter { session in
            known.contains(session.accountKey) && CloudKeys.isKey(session.project.key)
                && CloudKeys.isUUID(session.sessionId)
                && accepts(session.startedAt) && accepts(session.lastActivityAt)
                && (session.endedAt.map(accepts) ?? true)
        }.prefix(L.sessions).map { session in
            var session = session
            session.project.name = session.project.name.clampedUTF16(L.projectName)
            session.title = session.title?.clampedUTF16(L.title)
            var models: [String] = []
            for model in session.models.map({ $0.clampedUTF16(L.modelId) }) where !model.isEmpty && !models.contains(model) {
                models.append(model)
            }
            session.models = Array(models.prefix(L.models))
            session.messageCount = min(max(0, session.messageCount), L.messageCount)
            func count(_ value: Int) -> Int { min(max(0, value), L.tokenCount) }
            session.tokens = Tokens(input: count(session.tokens.input), output: count(session.tokens.output),
                                    cacheCreation: count(session.tokens.cacheCreation),
                                    cacheRead: count(session.tokens.cacheRead))
            if let cost = session.costUsd, !cost.isFinite || cost < 0 || cost >= L.costUsd { session.costUsd = nil }
            if var summary = session.summary {
                summary.text = summary.text.clampedUTF16(L.summaryText)
                summary.model = summary.model.clampedUTF16(L.summaryModel)
                session.summary = accepts(summary.generatedAt) ? summary : nil
            }
            if session.lastActivityAt < session.startedAt { session.lastActivityAt = session.startedAt }
            if let ended = session.endedAt, ended < session.lastActivityAt { session.endedAt = session.lastActivityAt }
            return session
        }
        copy.usage = usage.filter { known.contains($0.accountKey) && accepts($0.observedAt) }.prefix(L.usage).compactMap { reading in
            var reading = reading
            reading.windows = Array(reading.windows
                .filter { $0.utilization.isFinite && CloudKeys.isWindowID($0.id) }
                .map { window in
                    CloudSyncRequest.UsageWindow(id: window.id, utilization: max(0, window.utilization),
                                                 resetsAt: window.resetsAt.flatMap { CloudContract.acceptsResetsAt($0, now: now) ? $0 : nil })
                }
                .prefix(L.windows))
            return reading.windows.isEmpty ? nil : reading
        }
        return copy
    }
}

extension String {
    /// At most `limit` UTF-16 units (what the website's schemas count),
    /// never splitting a character.
    nonisolated func clampedUTF16(_ limit: Int) -> String {
        guard utf16.count > limit else { return self }
        var result = ""
        var used = 0
        for character in self {
            let size = character.utf16.count
            guard used + size <= limit else { break }
            result.append(character)
            used += size
        }
        return result
    }
}

// MARK: - Keys

/// The digests that name accounts and projects on the website.
nonisolated enum CloudKeys {
    /// Lowercase hex SHA-256 of a UTF-8 string.
    static func sha256Hex(_ text: String) -> String {
        sha256Hex(Data(text.utf8))
    }

    static func sha256Hex(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    /// 64 lowercase hex digits, as the website checks keys.
    static func isKey(_ text: String) -> Bool {
        text.utf8.count == 64 && text.utf8.allSatisfy { (0x30...0x39).contains($0) || (0x61...0x66).contains($0) }
    }

    /// A UUID in any case (Claude Code's session ids; the device id).
    static func isUUID(_ text: String) -> Bool {
        text.utf8.count == 36 && UUID(uuidString: text) != nil
    }

    /// `session`, `weekly_all`, `extra_usage` or `weekly_<model>` as the
    /// website accepts it: a lowercase letter or digit, then up to 56 of
    /// lowercase letters, digits, `_`, `.` and `-` (the website refuses a
    /// whole batch over one bad window id).
    static func isWindowID(_ id: String) -> Bool {
        if id == UsageRingWindows.sessionID || id == UsageRingWindows.weeklyID || id == UsageRingWindows.extraUsageID {
            return true
        }
        guard id.hasPrefix(UsageRingWindows.scopedPrefix) else { return false }
        let rest = Array(id.utf8.dropFirst(UsageRingWindows.scopedPrefix.utf8.count))
        guard let first = rest.first, rest.count <= 57, isAlphanumeric(first) else { return false }
        return rest.dropFirst().allSatisfy { isAlphanumeric($0) || $0 == 0x5F || $0 == 0x2E || $0 == 0x2D }
    }

    /// A window id as the website takes it: as is, or a `weekly_<model>`
    /// shortened to fit. Nil for anything else.
    static func contractWindowID(_ id: String) -> String? {
        let id = id.lowercased()
        if isWindowID(id) { return id }
        guard id.hasPrefix(UsageRingWindows.scopedPrefix) else { return nil }
        let shortened = UsageRingWindows.scopedPrefix + id.dropFirst(UsageRingWindows.scopedPrefix.count).prefix(57)
        return isWindowID(shortened) ? shortened : nil
    }

    private static func isAlphanumeric(_ byte: UInt8) -> Bool {
        (0x30...0x39).contains(byte) || (0x61...0x7A).contains(byte)
    }

    /// SHA-256 of the lowercased `<accountUuid>/<organizationUuid>` (both
    /// from `oauthAccount`), or of the lowercased `accountUuid` alone when
    /// the organization is unknown. It never depends on what else is signed
    /// in on this Mac, so the same Claude account in the same organization
    /// has the same key for every user and Mac, which is what pooling joins on.
    static func accountKey(accountUuid: String, organizationUuid: String?) -> String {
        let account = accountUuid.trimmingCharacters(in: .whitespaces)
        let base: String
        if let organization = organizationUuid?.trimmingCharacters(in: .whitespaces), !organization.isEmpty {
            base = account + AccountIdentityGrouping.organizationSeparator + organization
        } else {
            base = account
        }
        return sha256Hex(base.lowercased())
    }

    /// The key of an engine identity: its account UUID and its own
    /// organization. Nil for `email:` and `dir:` identities, which have no
    /// account UUID to share across Macs.
    ///
    /// - Parameter correctedFolders: folders whose `.claude.json` names
    ///   another account's details (a Claude Parallel Profiles mirror): their
    ///   organization is stale and never used.
    static func accountKey(identity: ClaudeIdentityAccount, correctedFolders: Set<String> = []) -> String? {
        guard let account = AccountIdentityGrouping.accountUuid(ofKey: identity.id), !account.isEmpty else { return nil }
        return accountKey(accountUuid: account, organizationUuid: organization(of: identity, correctedFolders: correctedFolders))
    }

    /// The organization an identity is signed in to: the one it was split
    /// by, else the one its own folders' `oauthAccount` names (they name at
    /// most one, or the identity would have been split). A mirrored copy's
    /// is never used. An identity with no folders (tests, fixtures) has its
    /// own field. Nil when nothing says. Pure.
    static func organization(of identity: ClaudeIdentityAccount, correctedFolders: Set<String> = []) -> String? {
        func clean(_ value: String?) -> String? {
            guard let value = value?.trimmingCharacters(in: .whitespaces).lowercased(), !value.isEmpty else { return nil }
            return value
        }
        if let scope = clean(identity.organizationScope) { return scope }
        guard !identity.folders.isEmpty else { return clean(identity.organizationUuid) }
        return identity.folders
            .filter { !correctedFolders.contains($0.id) }
            .lazy.compactMap { clean($0.organizationUuid) }
            .first
    }

    /// HMAC-SHA256, keyed with this install's secret
    /// (`CloudInstallSecret`), of `<accountKey>:<project path>`: stable per
    /// install, and the path can't be guessed back from it.
    static func projectKey(accountKey: String, path: String, secret: Data) -> String {
        let code = HMAC<SHA256>.authenticationCode(for: Data((accountKey + ":" + path).utf8), using: SymmetricKey(data: secret))
        return Data(code).map { String(format: "%02x", $0) }.joined()
    }

    /// A session's working directory as the project key uses it: `~`
    /// expanded against `home`, symbolic links resolved, no trailing slash.
    static func projectPath(forCwd cwd: String, home: String) -> String {
        var path = cwd
        if path == "~" {
            path = home
        } else if path.hasPrefix("~/") {
            path = (home as NSString).appendingPathComponent(String(path.dropFirst(2)))
        }
        path = URL(fileURLWithPath: path).resolvingSymlinksInPath().path
        path = (path as NSString).standardizingPath
        while path.count > 1 && path.hasSuffix("/") { path.removeLast() }
        return path
    }

    /// The working directory's last path component (what the website shows).
    static func projectName(forCwd cwd: String) -> String {
        var path = cwd
        while path.count > 1 && path.hasSuffix("/") { path.removeLast() }
        let name = (path as NSString).lastPathComponent
        return name.isEmpty ? path : name
    }
}
