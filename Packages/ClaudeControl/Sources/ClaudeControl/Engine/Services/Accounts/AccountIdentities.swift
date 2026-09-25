//
//  AccountIdentities.swift
//  ClaudeControl
//
//  An account is a signed-in identity, not a folder. Claude Parallel
//  Profiles keeps one account in many folders (its stores, a working copy
//  per VS Code window, and `~/.claude`, which it mirrors the last-used
//  account into), so the folders the registry knows are grouped by who is
//  signed in to them: `oauthAccount.accountUuid`, else the lowercased email.
//  One group, one ring, one row, one usage reading. Pure.
//
//  `~/.claude` needs one correction. The extension mirrors an account into
//  it by rewriting `~/.claude.json`'s `oauthAccount` email, display name and
//  organization name, but keeps the rest of the object, `accountUuid`
//  included. After a switch the file can name one person's email with
//  another person's UUID. When a folder's UUID is known elsewhere under a
//  different email, and its email is known elsewhere under a different UUID,
//  the email wins: the folder joins the identity that email belongs to.
//  `~/.claude` itself (when the extension mirrors into it) joins the email's
//  owner as soon as no other folder pairs its UUID with its email: after the
//  extension forgets an account, the stale UUID may exist nowhere else.
//  That stale UUID is still the one signed in there before the mirroring
//  began, so choices saved for `~/.claude` (its old ring, its name) go to
//  its owner (`defaultOwner`), not to whoever the extension mirrored in.
//
//  One login in two organizations (a personal plan and a Team seat) shares
//  one `accountUuid` but has two quotas: when folders the correction didn't
//  touch name one UUID with two `organizationUuid`s, each organization is
//  its own identity (`uuid:<account>/<organization>`).
//

import CryptoKit
import Foundation

/// What the user chose for an identity (its name, colour, tracking), kept
/// in accounts.json by identity so every folder of it, a VS Code window
/// opened tomorrow included, follows.
nonisolated struct IdentityPrefs: Codable, Equatable, Sendable {
    var customLabel: String?
    var colorIndex: Int
    var isHidden: Bool
}

/// One Claude account: everything signed in as one identity.
nonisolated struct ClaudeIdentityAccount: Identifiable, Hashable, Sendable {
    /// `uuid:<accountUuid>`, `email:<address>`, or `dir:<folder>` for a folder
    /// added by hand that nobody has signed in to yet.
    let id: String
    /// Codenotch's ring id, stable whatever folders come and go.
    let ringID: String

    var email: String?
    var displayName: String?
    var organizationName: String?
    var organizationUuid: String?
    var accountUuid: String?
    var subscriptionType: String?
    var rateLimitTier: String?

    /// Folders Claude Code runs in as this identity: `~/.claude` first, then
    /// VS Code windows, then standalone folders.
    var runDirs: [ClaudeAccount]
    /// Claude Parallel Profiles stores holding this identity (never written).
    var storeDirs: [ClaudeAccount]

    var customLabel: String?
    var colorIndex: Int
    var isHidden: Bool
    /// `~/.claude` (used without CLAUDE_CONFIG_DIR) runs as this identity now.
    var includesDefault: Bool = false
    /// Set when one login has several organizations: the one this identity
    /// is (its folders, cached usage and quota are that organization's).
    var organizationScope: String?
    /// Its run folders that are VS Code windows' working copies.
    var windowDirIds: [String] = []
    /// Told apart from every other identity's (see `AccountNaming`).
    var defaultLabel: String?
    var defaultMonogram: String?

    var folders: [ClaudeAccount] { runDirs + storeDirs }
    var folderIds: [String] { folders.map(\.id) }
    var isSignedIn: Bool { email != nil || accountUuid != nil }
    /// A folder added by hand that nobody has signed in to: its ring says
    /// how to sign in.
    var isStandaloneUnsigned: Bool { id.hasPrefix(AccountIdentityGrouping.dirPrefix) }
    var windowDirs: [ClaudeAccount] { runDirs.filter { windowDirIds.contains($0.id) } }
    /// The folder the account is shown and launched from: a run folder when
    /// it has one (the default first), else its first store.
    var primaryDir: ClaudeAccount? { runDirs.first ?? storeDirs.first }
    var lastSeenAt: Date? { folders.compactMap(\.lastSeenAt).max() }

    var label: String {
        if let customLabel, !customLabel.isEmpty { return customLabel }
        return defaultLabel ?? representative.label
    }

    var monogram: String { defaultMonogram ?? representative.monogram }

    var planName: String? { representative.planName }

    /// How to start Claude Code as this identity from a terminal, when there
    /// is a way: `claude` while `~/.claude` runs as it, else a standalone run
    /// folder's `CLAUDE_CONFIG_DIR=… claude`. Nil when it runs only in VS Code
    /// windows or only a store holds it: a window's working copy belongs to
    /// that window (a `/login` there switches the window), and Claude Code is
    /// never run in a store.
    var terminalLaunchCommand: String? {
        if includesDefault { return "claude" }
        let standalone = runDirs.first { folder in
            folder.kind == .run && folder.configDirEnv?.isEmpty == false && !windowDirIds.contains(folder.id)
        }
        return standalone?.launchCommand
    }

    /// Whether it can be forgotten: any but the one `~/.claude` runs as on
    /// its own (forgetting that would forget every terminal session). One
    /// Claude Parallel Profiles keeps in a store or a VS Code window can,
    /// whoever `~/.claude` holds now (that only says which window was
    /// focused last).
    var canBeForgotten: Bool {
        !includesDefault || !storeDirs.isEmpty || !windowDirIds.isEmpty
    }

    /// `terminalLaunchCommand`, else plain `claude` (a caller that must have
    /// a line; the settings row shows guidance instead).
    var launchCommand: String { terminalLaunchCommand ?? "claude" }

    /// A folder-shaped stand-in (the primary folder with this identity's
    /// fields), for rules written for folders: names, plans.
    var representative: ClaudeAccount {
        var account = primaryDir ?? ClaudeAccount(configDir: AccountPaths.defaultConfigDir)
        account.email = email
        account.displayName = displayName
        account.organizationName = organizationName
        account.organizationUuid = organizationUuid
        account.accountUuid = accountUuid
        account.subscriptionType = subscriptionType
        account.rateLimitTier = rateLimitTier
        account.customLabel = customLabel
        account.colorIndex = colorIndex
        account.isHidden = isHidden
        account.defaultLabel = nil
        account.defaultMonogram = nil
        return account
    }
}

nonisolated enum AccountIdentityGrouping {
    static let uuidPrefix = "uuid:"
    static let emailPrefix = "email:"
    static let dirPrefix = "dir:"

    /// A folder's own key, before any correction: its UUID, else its email.
    static func baseKey(accountUuid: String?, email: String?) -> String? {
        if let uuid = accountUuid?.trimmingCharacters(in: .whitespaces).lowercased(), !uuid.isEmpty {
            return uuidPrefix + uuid
        }
        if let email = email?.trimmingCharacters(in: .whitespaces).lowercased(), !email.isEmpty {
            return emailPrefix + email
        }
        return nil
    }

    /// How a folder's identity key was settled.
    struct Resolution: Equatable, Sendable {
        var key: String
        /// The folder's own UUID belongs to someone else (a mirrored
        /// `~/.claude.json`); its other fields may be stale too.
        var corrected: Bool
    }

    /// The identity key of every signed-in folder (see the file comment for
    /// the mirror correction and organizations). Folders nobody is signed
    /// in to have none.
    ///
    /// - Parameter mirroredDefault: `~/.claude`'s folder id when Claude
    ///   Parallel Profiles mirrors accounts into it.
    static func identityKeys(_ folders: [ClaudeAccount], mirroredDefault: String? = nil) -> [String: Resolution] {
        struct Login { let folder: String; let uuid: String?; let email: String?; let organization: String? }
        func clean(_ value: String?) -> String? {
            guard let value = value?.trimmingCharacters(in: .whitespaces).lowercased(), !value.isEmpty else { return nil }
            return value
        }
        let logins = folders.compactMap { folder -> Login? in
            let uuid = clean(folder.accountUuid)
            let email = clean(folder.email)
            guard uuid != nil || email != nil else { return nil }
            return Login(folder: folder.id, uuid: uuid, email: email, organization: clean(folder.organizationUuid))
        }
        var result: [String: Resolution] = [:]
        for login in logins {
            let others = logins.filter { $0.folder != login.folder }
            // UUIDs the same email has in other folders, most common first.
            func uuids(forEmail email: String) -> [String] {
                let found = others.filter { $0.email == email }.compactMap(\.uuid)
                let counts = Dictionary(found.map { ($0, 1) }, uniquingKeysWith: +)
                return counts.keys.sorted { (counts[$0] ?? 0, $1) > (counts[$1] ?? 0, $0) }
            }
            switch (login.uuid, login.email) {
            case let (uuid?, email?):
                let uuidElsewhereAsOthers = others.contains { $0.uuid == uuid && $0.email != nil && $0.email != email }
                let emailElsewhere = uuids(forEmail: email).filter { $0 != uuid }
                // The folder the extension mirrors into: its UUID is stale as
                // soon as nothing else pairs it with this email (the account
                // it belonged to may have been forgotten everywhere else).
                let staleMirror = login.folder == mirroredDefault
                    && !others.contains { $0.uuid == uuid && $0.email == email }
                if uuidElsewhereAsOthers || staleMirror, let owner = emailElsewhere.first {
                    result[login.folder] = Resolution(key: uuidPrefix + owner, corrected: true)
                } else {
                    result[login.folder] = Resolution(key: uuidPrefix + uuid, corrected: false)
                }
            case let (uuid?, nil):
                result[login.folder] = Resolution(key: uuidPrefix + uuid, corrected: false)
            case let (nil, email?):
                // An email-only login joins the UUID that email has elsewhere.
                if let owner = uuids(forEmail: email).first {
                    result[login.folder] = Resolution(key: uuidPrefix + owner, corrected: false)
                } else {
                    result[login.folder] = Resolution(key: emailPrefix + email, corrected: false)
                }
            case (nil, nil):
                break
            }
        }

        // One UUID in several organizations: one identity per organization.
        // Only folders the correction left alone count (a mirrored file keeps
        // a stale organization too); the rest join the most common one.
        let organizations = Dictionary(logins.map { ($0.folder, $0.organization) }, uniquingKeysWith: { first, _ in first })
        let byKey = Dictionary(grouping: result.keys.sorted(), by: { result[$0]?.key ?? "" })
        for (key, members) in byKey where key.hasPrefix(uuidPrefix) {
            let own = members.filter { result[$0]?.corrected == false }.compactMap { organizations[$0] ?? nil }
            let counts = Dictionary(own.map { ($0, 1) }, uniquingKeysWith: +)
            guard counts.count > 1 else { continue }
            let common = counts.keys.sorted { (counts[$0] ?? 0, $1) > (counts[$1] ?? 0, $0) }[0]
            for member in members {
                guard let resolution = result[member] else { continue }
                let organization = resolution.corrected ? common : ((organizations[member] ?? nil) ?? common)
                result[member] = Resolution(key: key + organizationSeparator + organization, corrected: resolution.corrected)
            }
        }
        return result
    }

    /// Between an account's UUID and its organization in a split key.
    static let organizationSeparator = "/"

    /// The account UUID of a `uuid:` key (without any organization).
    static func accountUuid(ofKey key: String) -> String? {
        guard key.hasPrefix(uuidPrefix) else { return nil }
        let value = key.dropFirst(uuidPrefix.count)
        return String(value.split(separator: Character(organizationSeparator), maxSplits: 1).first ?? value)
    }

    /// The organization of a split `uuid:<account>/<organization>` key.
    static func organization(ofKey key: String) -> String? {
        guard key.hasPrefix(uuidPrefix) else { return nil }
        let parts = key.dropFirst(uuidPrefix.count).split(separator: Character(organizationSeparator), maxSplits: 1)
        return parts.count == 2 ? String(parts[1]) : nil
    }

    /// Who `~/.claude` belongs to, whoever the extension mirrored into it:
    /// the identity its own `accountUuid` names (the extension never
    /// rewrites that), when that identity exists. Nil when it names nobody
    /// known (then its choices go nowhere by themselves).
    static func defaultOwner(defaultFolder: ClaudeAccount?, identityKeys keys: [String]) -> String? {
        guard let folder = defaultFolder,
              let own = baseKey(accountUuid: folder.accountUuid, email: folder.accountUuid == nil ? folder.email : nil) else { return nil }
        if keys.contains(own) { return own }
        let split = keys.filter { $0.hasPrefix(own + organizationSeparator) }
        if split.count == 1 { return split[0] }
        if let organization = folder.organizationUuid?.lowercased(),
           let match = split.first(where: { $0 == own + organizationSeparator + organization }) {
            return match
        }
        return nil
    }

    /// Codenotch's ring id for an identity: `claude-acct-` and the first 12
    /// hex digits of the SHA-256 of its UUID (or email), so it never depends
    /// on which folders exist. `dir:` identities keep their folder's ring id.
    static func ringID(identityKey key: String, home: String) -> String {
        if key.hasPrefix(dirPrefix) {
            return ClaudeRingIdentity.ringID(configDir: String(key.dropFirst(dirPrefix.count)), home: home)
        }
        let value: String
        if key.hasPrefix(uuidPrefix) {
            value = String(key.dropFirst(uuidPrefix.count))
        } else if key.hasPrefix(emailPrefix) {
            value = String(key.dropFirst(emailPrefix.count))
        } else {
            value = key
        }
        return ClaudeRingIdentity.ringID(accountKey: value)
    }

    /// What grouping makes of the registry's folders.
    struct Result: Equatable, Sendable {
        /// One per identity, by label (stable while ~/.claude changes hands).
        var identities: [ClaudeIdentityAccount]
        /// Folder id → identity id, for every grouped folder.
        var identityOfFolder: [String: String]
        /// Run folders nobody is signed in to that have no ring (listed as
        /// "not signed in" in Settings).
        var unsignedFolders: [ClaudeAccount]
        /// Folders of identities the user forgot.
        var forgottenFolders: [ClaudeAccount]
        /// Prefs for every identity, including those derived just now.
        var prefs: [String: IdentityPrefs]
        /// Folders whose key was corrected (see the file comment).
        var correctedFolders: Set<String>
        /// Folder id → identity key for every signed-in folder, forgotten
        /// identities' included.
        var folderKeys: [String: String] = [:]
        /// The identity `~/.claude` belongs to by its own `accountUuid`
        /// (`defaultOwner(defaultFolder:identityKeys:)`), which may not be
        /// the one the extension mirrored into it now.
        var defaultOwner: String? = nil
    }

    /// Group `folders` (the registry's, infrastructure excluded) into
    /// identities. `prefs` holds what the user chose per identity; an
    /// identity without an entry takes its first folder's (the default
    /// folder first, then by path): its name, colour and tracking, the way
    /// accounts.json from before identities recorded them per folder.
    ///
    /// - Parameter savedFolders: folders whose choices were saved (per
    ///   folder, before identities): they are the ones asked first when an
    ///   identity's prefs are derived, ahead of folders found since.
    ///   - mirrorsDefault: Claude Parallel Profiles mirrors accounts into
    ///     `~/.claude` (see `identityKeys`).
    static func group(
        _ folders: [ClaudeAccount],
        prefs: [String: IdentityPrefs],
        forgotten: Set<String> = [],
        savedFolders: Set<String> = [],
        mirrorsDefault: Bool = false,
        home: String
    ) -> Result {
        let home = AccountPaths.normalize(home)
        let usable = folders.filter { $0.kind != .infrastructure }
        let defaultFolder = usable.first { AccountRegistry.isDefault($0, home: home) }
        let keys = identityKeys(usable, mirroredDefault: mirrorsDefault ? defaultFolder?.id : nil)
        let signedInExists = keys.values.contains { !forgotten.contains($0.key) }

        var buckets: [String: [ClaudeAccount]] = [:]
        var unsigned: [ClaudeAccount] = []
        var forgottenFolders: [ClaudeAccount] = []
        for folder in usable {
            if let resolution = keys[folder.id] {
                if forgotten.contains(resolution.key) {
                    forgottenFolders.append(folder)
                } else {
                    buckets[resolution.key, default: []].append(folder)
                }
                continue
            }
            // Nobody signed in. Only a store never has a ring then; a run
            // folder added by hand does ("Run … then /login"), and so does
            // ~/.claude while no identity exists anywhere.
            guard folder.kind == .run else { continue }
            let isWindow = ParallelProfiles.isWindowDir(folder.configDir, home: home)
            let isDefault = AccountRegistry.isDefault(folder, home: home)
            let byHand = folder.source == .manual && !isWindow && !isDefault
            let lonelyDefault = isDefault && !signedInExists
            let key = dirPrefix + folder.id
            if (byHand || lonelyDefault), !forgotten.contains(key) {
                buckets[key, default: []].append(folder)
            } else {
                unsigned.append(folder)
            }
        }

        // ~/.claude's own choices (saved per folder, before identities) are
        // its owner's, not those of whoever the extension mirrored in.
        let owner = defaultOwner(defaultFolder: defaultFolder, identityKeys: Array(buckets.keys))
        let defaultIsMirrored = defaultFolder.map { keys[$0.id]?.corrected == true } ?? false
        func prefsSources(_ key: String, _ ordered: [ClaudeAccount]) -> [ClaudeAccount] {
            guard let defaultFolder, defaultIsMirrored else { return ordered }
            var sources = ordered.filter { $0.id != defaultFolder.id }
            if key == owner { sources.insert(defaultFolder, at: 0) }
            return sources
        }

        var allPrefs = prefs
        var identities: [ClaudeIdentityAccount] = []
        var identityOfFolder: [String: String] = [:]
        // Derive missing prefs in a stable order so colours don't depend on
        // dictionary order: identities holding the default first, then by key.
        let orderedKeys = buckets.keys.sorted { lhs, rhs in
            let lhsDefault = buckets[lhs]?.contains { AccountRegistry.isDefault($0, home: home) } ?? false
            let rhsDefault = buckets[rhs]?.contains { AccountRegistry.isDefault($0, home: home) } ?? false
            if lhsDefault != rhsDefault { return lhsDefault }
            return lhs < rhs
        }
        for key in orderedKeys {
            guard let members = buckets[key] else { continue }
            let ordered = orderedFolders(members, home: home)
            if allPrefs[key] == nil {
                let sources = prefsSources(key, ordered)
                let saved = sources.filter { savedFolders.contains($0.id) }
                // Colours of the identities here now (not of ones long gone).
                let taken = buckets.keys.compactMap { allPrefs[$0]?.colorIndex }
                allPrefs[key] = derivedPrefs(from: saved + sources.filter { !savedFolders.contains($0.id) }, taken: taken)
            }
            let chosen = allPrefs[key] ?? IdentityPrefs(customLabel: nil, colorIndex: 0, isHidden: false)
            // Identity fields from the most trustworthy folder that has each:
            // stores (the account's own copy), windows, standalone folders,
            // then ~/.claude, a corrected (mirrored) folder last.
            let trusted = members.sorted { trustRank($0, keys: keys, home: home) < trustRank($1, keys: keys, home: home) }
            func field(_ value: (ClaudeAccount) -> String?) -> String? {
                trusted.lazy.compactMap(value).first { !$0.isEmpty }
            }
            let runDirs = ordered.filter { $0.kind == .run }
            let storeDirs = ordered.filter { $0.kind == .store }
            let identity = ClaudeIdentityAccount(
                id: key,
                ringID: ringID(identityKey: key, home: home),
                email: field(\.email),
                displayName: field(\.displayName),
                organizationName: field(\.organizationName),
                organizationUuid: organization(ofKey: key).map { scope in
                    trusted.lazy.compactMap(\.organizationUuid).first { $0.lowercased() == scope } ?? scope
                } ?? field(\.organizationUuid),
                accountUuid: accountUuid(ofKey: key) ?? field(\.accountUuid),
                subscriptionType: field(\.subscriptionType),
                rateLimitTier: field(\.rateLimitTier),
                runDirs: runDirs,
                storeDirs: storeDirs,
                customLabel: chosen.customLabel,
                colorIndex: chosen.colorIndex,
                isHidden: chosen.isHidden,
                includesDefault: runDirs.contains { AccountRegistry.isDefault($0, home: home) },
                organizationScope: organization(ofKey: key),
                windowDirIds: runDirs.filter { ParallelProfiles.isWindowDir($0.configDir, home: home) }.map(\.id)
            )
            identities.append(identity)
            for folder in members { identityOfFolder[folder.id] = key }
        }

        // Names told apart among tracked identities (an untracked one is
        // named as if it joined them), the same rule folders had.
        let representatives = identities.map(\.representative)
        let byRepresentative = Dictionary(zip(representatives.map(\.id), identities.map(\.id)), uniquingKeysWith: { first, _ in first })
        let names = AccountRegistry.named(representatives)
        for named in names {
            guard let identityId = byRepresentative[named.id],
                  let index = identities.firstIndex(where: { $0.id == identityId }) else { continue }
            identities[index].defaultLabel = named.defaultLabel
            identities[index].defaultMonogram = named.defaultMonogram
        }
        // By name, not by which one ~/.claude runs as: Claude Parallel
        // Profiles mirrors the focused window's account into ~/.claude, and
        // the rings must not swap places every time another window is focused.
        identities.sort { lhs, rhs in
            switch lhs.label.localizedCaseInsensitiveCompare(rhs.label) {
            case .orderedAscending: return true
            case .orderedDescending: return false
            case .orderedSame: return lhs.id < rhs.id
            }
        }
        return Result(
            identities: identities,
            identityOfFolder: identityOfFolder,
            unsignedFolders: orderedFolders(unsigned, home: home),
            forgottenFolders: forgottenFolders,
            prefs: allPrefs,
            correctedFolders: Set(keys.filter(\.value.corrected).map(\.key)),
            folderKeys: keys.mapValues(\.key),
            defaultOwner: owner
        )
    }

    /// `~/.claude` first, then VS Code windows, then other run folders, then
    /// stores; by path within each.
    static func orderedFolders(_ folders: [ClaudeAccount], home: String) -> [ClaudeAccount] {
        func rank(_ folder: ClaudeAccount) -> Int {
            if folder.kind == .store { return 3 }
            if AccountRegistry.isDefault(folder, home: home) { return 0 }
            if ParallelProfiles.isWindowDir(folder.configDir, home: home) { return 1 }
            return 2
        }
        return folders.sorted { lhs, rhs in
            let (left, right) = (rank(lhs), rank(rhs))
            if left != right { return left < right }
            return lhs.configDir < rhs.configDir
        }
    }

    private static func trustRank(_ folder: ClaudeAccount, keys: [String: Resolution], home: String) -> Int {
        if keys[folder.id]?.corrected == true { return 9 }
        if folder.kind == .store { return 0 }
        if ParallelProfiles.isWindowDir(folder.configDir, home: home) { return 1 }
        if AccountRegistry.isDefault(folder, home: home) { return 3 }
        return 2
    }

    /// An identity's prefs from its folders' old per-folder choices: the
    /// first folder's (default first); its colour unless another identity
    /// has it, else the least used one.
    static func derivedPrefs(from ordered: [ClaudeAccount], taken: [Int]) -> IdentityPrefs {
        let first = ordered.first
        let label = ordered.lazy.compactMap(\.customLabel).first { !$0.isEmpty }
        var color = first?.colorIndex ?? AccountRegistry.nextColorIndex(used: taken)
        let size = AccountRegistry.paletteSize
        if taken.contains(where: { (($0 % size) + size) % size == ((color % size) + size) % size }) {
            color = AccountRegistry.nextColorIndex(used: taken)
        }
        return IdentityPrefs(customLabel: label, colorIndex: color, isHidden: first?.isHidden ?? false)
    }
}

extension ClaudeRingIdentity {
    /// `claude-acct-<first 12 hex digits of sha256(key)>` for an account's
    /// UUID (or email, when it has none), lowercased.
    nonisolated public static func ringID(accountKey: String) -> String {
        let digest = SHA256.hash(data: Data(accountKey.lowercased().utf8))
        return "claude-acct-" + digest.prefix(6).map { String(format: "%02x", $0) }.joined()
    }
}

/// Which ring each folder's sessions go to, and which sessions belong to an
/// untracked or forgotten account, readable from any thread (the shared
/// registry keeps it current; pure mappers and the socket server read it).
nonisolated enum FolderRings {
    /// What the registry published last.
    struct Snapshot: Sendable {
        var rings: [String: String] = [:]
        /// Folder → identity it names now (forgotten identities included).
        var identityOfFolder: [String: String] = [:]
        var ringOfIdentity: [String: String] = [:]
        /// Identities that are untracked or forgotten.
        var untrackedIdentities: Set<String> = []
        /// Folders whose account is untracked or forgotten now.
        var untrackedFolders: Set<String> = []
        /// `~/.claude`, and whether Claude Parallel Profiles mirrors into it.
        var defaultFolder: String?
        var mirrorsDefault = false
        var defaultTimeline = FolderIdentityTimeline()

        /// Who a session in `folder`, started at `startedAt`, runs as.
        func attribution(folder: String, startedAt: Date?) -> FolderAttribution {
            let folder = AccountPaths.normalize(folder)
            return FolderAttribution.attribute(current: identityOfFolder[folder],
                                               timeline: folder == defaultFolder ? defaultTimeline : nil,
                                               startedAt: startedAt, mirrored: mirrorsDefault && folder == defaultFolder)
        }

        /// The session's account is untracked or forgotten (when known; else
        /// the folder's current account decides).
        func isUntracked(folder: String, startedAt: Date?) -> Bool {
            switch attribution(folder: folder, startedAt: startedAt) {
            case .known(let identity?): return untrackedIdentities.contains(identity)
            case .known(nil), .unsure: return untrackedFolders.contains(AccountPaths.normalize(folder))
            }
        }
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var snapshot = Snapshot()

    static func set(_ byFolder: [String: String]) {
        lock.lock()
        snapshot.rings = byFolder
        lock.unlock()
    }

    static func set(_ new: Snapshot) {
        lock.lock()
        snapshot = new
        lock.unlock()
    }

    static var current: Snapshot {
        lock.lock()
        defer { lock.unlock() }
        return snapshot
    }

    static func ring(forFolder folder: String) -> String? {
        lock.lock()
        defer { lock.unlock() }
        return snapshot.rings[AccountPaths.normalize(folder)]
    }

    /// A hook event from this folder and process belongs to an untracked or
    /// forgotten account: nothing shows it, so nothing may hold it (a
    /// permission request is answered at once, with no decision, and the
    /// session asks in its own terminal).
    static func isUntracked(folder: String, pid: Int?) -> Bool {
        let snapshot = current
        guard !snapshot.untrackedFolders.isEmpty || !snapshot.untrackedIdentities.isEmpty else { return false }
        let startedAt = pid.flatMap { ProcessInspector.startDate(pid: $0) }
        return snapshot.isUntracked(folder: folder, startedAt: startedAt)
    }
}
