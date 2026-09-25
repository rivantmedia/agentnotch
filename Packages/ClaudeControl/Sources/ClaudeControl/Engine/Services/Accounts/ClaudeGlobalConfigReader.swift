//
//  ClaudeGlobalConfigReader.swift
//  ClaudeControl
//
//  Reads the parts of an account's global config (`.claude.json`) the app
//  needs: the signed-in identity (`oauthAccount`) and Claude Code's cached
//  usage snapshot (`cachedUsageUtilization`). Nothing else in the file —
//  project history, MCP config, feature flags — is kept.
//
//  `.claude.json` can be several MB and Claude Code rewrites it often, so the
//  parse is cached per path and only redone when the file's modification date
//  or size changes. AccountRegistry and UsageStore share one reader, so a
//  change is parsed once. Blocking; call it off the main actor.
//

import Foundation
import os.log

/// Who is signed in to an account, from `.claude.json` → `oauthAccount`.
nonisolated struct ClaudeAccountIdentity: Equatable, Sendable {
    var accountUuid: String?
    var email: String?
    var displayName: String?
    var organizationName: String?
    /// The claude.ai organization; Claude Desktop keys its usage cache by it.
    var organizationUuid: String?
    /// e.g. "claude_max", "claude_pro", "claude_team".
    var organizationType: String?
    /// e.g. "default_claude_max_20x" (organization tier, else user tier).
    var rateLimitTier: String?
    var billingType: String?
    var hasExtraUsageEnabled: Bool?

    /// "max", "pro", "team", "enterprise" from `organizationType`, when present.
    var subscriptionType: String? {
        guard let type = organizationType?.lowercased(), !type.isEmpty else { return nil }
        if type.hasPrefix("claude_") {
            let plan = String(type.dropFirst("claude_".count))
            return plan.isEmpty ? nil : plan
        }
        return type
    }

    /// Parse an `oauthAccount` object; nil when it has neither a UUID nor an email.
    init?(oauthAccount: [String: Any]) {
        func string(_ key: String) -> String? {
            guard let value = oauthAccount[key] as? String, !value.isEmpty else { return nil }
            return value
        }
        accountUuid = string("accountUuid")
        email = string("emailAddress")
        displayName = string("displayName")
        organizationName = string("organizationName")
        organizationUuid = string("organizationUuid")
        organizationType = string("organizationType")
        rateLimitTier = string("organizationRateLimitTier") ?? string("userRateLimitTier")
        billingType = string("billingType")
        hasExtraUsageEnabled = UsageParser.bool(oauthAccount["hasExtraUsageEnabled"])
        if accountUuid == nil && email == nil { return nil }
    }

    init(
        accountUuid: String? = nil,
        email: String? = nil,
        displayName: String? = nil,
        organizationName: String? = nil,
        organizationUuid: String? = nil,
        organizationType: String? = nil,
        rateLimitTier: String? = nil,
        billingType: String? = nil,
        hasExtraUsageEnabled: Bool? = nil
    ) {
        self.accountUuid = accountUuid
        self.email = email
        self.displayName = displayName
        self.organizationName = organizationName
        self.organizationUuid = organizationUuid
        self.organizationType = organizationType
        self.rateLimitTier = rateLimitTier
        self.billingType = billingType
        self.hasExtraUsageEnabled = hasExtraUsageEnabled
    }
}

/// The subset of `.claude.json` the app uses.
nonisolated struct ClaudeGlobalConfig: Equatable, Sendable {
    /// Nil when nobody is signed in to claude.ai (or with an API-key login).
    var identity: ClaudeAccountIdentity?
    var cachedUsage: CachedUsageSnapshot?
    /// The file's modification date when it was parsed.
    var modifiedAt: Date?

    /// The only top-level keys of `.claude.json` the app reads.
    static let readKeys: Set<String> = ["oauthAccount", "cachedUsageUtilization"]

    /// The fields from a `.claude.json`'s bytes: only `readKeys` are parsed
    /// (`JSONFieldScanner`); nil when the file isn't a JSON object.
    static func parse(_ data: Data, modifiedAt: Date? = nil) -> ClaudeGlobalConfig? {
        guard let fields = JSONFieldScanner.objects(in: data, keys: readKeys) else { return nil }
        return extract(from: fields, modifiedAt: modifiedAt)
    }

    /// Extract the fields from a parsed `.claude.json`.
    static func extract(from json: [String: Any], modifiedAt: Date? = nil) -> ClaudeGlobalConfig {
        ClaudeGlobalConfig(
            identity: (json["oauthAccount"] as? [String: Any]).flatMap(ClaudeAccountIdentity.init(oauthAccount:)),
            cachedUsage: UsageParser.parseCachedUsage(fromGlobalConfig: json),
            modifiedAt: modifiedAt
        )
    }

    /// The cached usage, only if it belongs to the signed-in account (a
    /// `/login` to another account leaves the old snapshot behind).
    var matchingCachedUsage: CachedUsageSnapshot? {
        guard let cachedUsage else { return nil }
        guard let expected = identity?.accountUuid else { return nil }
        guard let owner = cachedUsage.accountUuid else { return nil }
        return owner == expected ? cachedUsage : nil
    }
}

nonisolated final class ClaudeGlobalConfigReader: @unchecked Sendable {
    static let shared = ClaudeGlobalConfigReader()

    private static var logger: Logger { EngineLog.logger("GlobalConfig") }

    private struct Entry {
        let modifiedAt: Date
        let size: Int
        let config: ClaudeGlobalConfig
    }

    private let lock = NSLock()
    private var cache: [String: Entry] = [:]

    init() {}

    /// The parsed config at `path`, or nil when the file doesn't exist or has
    /// never parsed. Reparses only when the modification date or size moved;
    /// if a reparse fails (e.g. caught mid-write) the last good value is
    /// returned and the next call tries again.
    func read(path: String) -> ClaudeGlobalConfig? {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: path),
              let modifiedAt = attributes[.modificationDate] as? Date else {
            lock.lock()
            cache.removeValue(forKey: path)
            lock.unlock()
            return nil
        }
        let size = (attributes[.size] as? NSNumber)?.intValue ?? -1

        lock.lock()
        let cached = cache[path]
        lock.unlock()
        if let cached, cached.modifiedAt == modifiedAt, cached.size == size {
            return cached.config
        }

        // Only `oauthAccount` and `cachedUsageUtilization` are parsed; the
        // rest of the file (project history, MCP servers, …) is skipped over.
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
              let config = ClaudeGlobalConfig.parse(data, modifiedAt: modifiedAt) else {
            Self.logger.debug("Couldn't parse \(path, privacy: .public); keeping the last good read")
            return cached?.config
        }

        lock.lock()
        cache[path] = Entry(modifiedAt: modifiedAt, size: size, config: config)
        lock.unlock()
        return config
    }

    /// Forget every cached parse (tests, or after the user edits paths).
    func invalidate() {
        lock.lock()
        cache.removeAll()
        lock.unlock()
    }
}
