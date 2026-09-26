//
//  DesktopHostedSessions.swift
//  ClaudeControl
//
//  Which account a Claude Code session that Claude Desktop hosts runs as.
//  Claude Desktop runs the Claude Code sessions it hosts as whoever Desktop
//  is signed in as, whatever the config folder they run in (usually
//  `~/.claude`) names, so the folder says nothing about them.
//
//  Claude Code marks such a session in its registry entry
//  (`<configDir>/sessions/<pid>.json`, see `SessionRegistryEntry`): an
//  entrypoint of `claude-desktop`, `claude-desktop-3p` or `local-agent`, and
//  `hostSessionId`, Desktop's own id for the session (`local_` and hex
//  digits and dashes, from `CLAUDE_CODE_HOST_SESSION_ID`; Claude Code writes
//  it for those entrypoints only). Claude Desktop keeps its record of each
//  session it hosts under the account and organization it ran as:
//
//      ~/Library/Application Support/Claude/claude-code-sessions/
//          <accountUuid>/<organizationUuid>/<hostSessionId>.json
//
//  So the session is the known identity whose record exists: a few `lstat`s
//  per identity (its account UUID and organization; for one whose
//  organization isn't known, a listing of its account's folder), and no
//  file there is ever opened. Desktop writes plain files and folders, so a
//  link anywhere from the root down (the root, an account's or an
//  organization's folder, or the record) is never followed: it is no
//  record. No record, records under more than one identity, or a
//  Desktop-hosted entry without a usable `hostSessionId`: the session can't
//  be attributed, and nothing is guessed. The hub asks only when not sealed.
//  Checked against Claude Code 2.1.282.
//

import Foundation

nonisolated enum DesktopHostedSessions {
    /// The entrypoints Claude Code writes a `hostSessionId` for.
    static let entrypoints: Set<String> = ["claude-desktop", "claude-desktop-3p", "local-agent"]

    /// A registry entry (or hook) entrypoint of a session Claude Desktop
    /// hosts. Any other entrypoint naming Desktop counts too: it carries no
    /// `hostSessionId`, so such a session is never attributed.
    static func isDesktopHosted(entrypoint: String?) -> Bool {
        guard let value = entrypoint?.trimmingCharacters(in: .whitespaces).lowercased(), !value.isEmpty else { return false }
        return entrypoints.contains(value) || value.contains("desktop")
    }

    /// Claude Code's own check of the id (`^local_[0-9a-f-]{8,72}$`), which
    /// also keeps it a plain file name.
    static func isHostSessionId(_ id: String) -> Bool {
        let prefix = "local_"
        guard id.hasPrefix(prefix) else { return false }
        let rest = id.utf8.dropFirst(prefix.utf8.count)
        return (8...72).contains(rest.count)
            && rest.allSatisfy { ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x61 && $0 <= 0x66) || $0 == 0x2D }
    }

    /// Where Claude Desktop keeps its records of the sessions it hosts.
    static func root(home: String) -> String {
        (home as NSString).appendingPathComponent("Library/Application Support/Claude/claude-code-sessions")
    }

    /// A known identity, as Desktop's folders name it.
    nonisolated struct Candidate: Equatable, Sendable {
        var identityId: String
        var accountUuid: String
        /// Nil when not known: every organization folder of the account is looked in.
        var organizationUuid: String?
    }

    /// The identity whose Desktop record of `hostSessionId` exists under
    /// `root`; nil when none does, when more than one does, or when the id
    /// isn't one. `lstat`s and folder listings only (injectable for tests).
    static func identity(hostSessionId: String?, candidates: [Candidate], root: String,
                         exists: (String) -> Bool = DesktopHostedSessions.isRecord,
                         list: (String) -> [String] = DesktopHostedSessions.listing) -> String? {
        guard let hostSessionId, isHostSessionId(hostSessionId) else { return nil }
        let file = hostSessionId + ".json"
        let holders = candidates.filter { candidate in
            let account = candidate.accountUuid.trimmingCharacters(in: .whitespaces)
            guard CloudKeys.isUUID(account) else { return false }
            return spellings(account).contains { accountName in
                let accountFolder = (root as NSString).appendingPathComponent(accountName)
                let organizations: [String]
                if let organization = candidate.organizationUuid?.trimmingCharacters(in: .whitespaces),
                   CloudKeys.isUUID(organization) {
                    organizations = spellings(organization)
                } else {
                    organizations = list(accountFolder).filter(CloudKeys.isUUID).sorted()
                }
                return organizations.contains { organization in
                    exists(((accountFolder as NSString).appendingPathComponent(organization) as NSString)
                        .appendingPathComponent(file))
                }
            }
        }
        let identities = Set(holders.map(\.identityId))
        return identities.count == 1 ? identities.first : nil
    }

    /// A UUID as Desktop names its folder (lowercase), and as given if that
    /// differs (a case-sensitive volume).
    private static func spellings(_ uuid: String) -> [String] {
        let lower = uuid.lowercased()
        return lower == uuid ? [uuid] : [lower, uuid]
    }

    /// Desktop's record `<root>/<account>/<organization>/<id>.json` is a
    /// regular file reached through real folders: the root, then the
    /// account's and the organization's folders, then the record, each
    /// `lstat`ed in that order and only while the ones above are real, so a
    /// link is never followed (not even to look further down). Never opened.
    static func isRecord(_ path: String) -> Bool {
        let organization = (path as NSString).deletingLastPathComponent
        let account = (organization as NSString).deletingLastPathComponent
        let root = (account as NSString).deletingLastPathComponent
        return isPlain(root, S_IFDIR) && isPlain(account, S_IFDIR) && isPlain(organization, S_IFDIR)
            && isPlain(path, S_IFREG)
    }

    /// An account's folder's entries, when the root and the folder are real
    /// folders (never a link, which isn't followed); empty otherwise, or when
    /// it can't be listed.
    static func listing(_ path: String) -> [String] {
        guard isPlain((path as NSString).deletingLastPathComponent, S_IFDIR), isPlain(path, S_IFDIR) else { return [] }
        return (try? FileManager.default.contentsOfDirectory(atPath: path)) ?? []
    }

    /// What is at `path` itself (a link is not what it points to) is of
    /// `kind` (`S_IFDIR`, `S_IFREG`): `lstat` only.
    private static func isPlain(_ path: String, _ kind: mode_t) -> Bool {
        var info = stat()
        return lstat(path, &info) == 0 && (info.st_mode & S_IFMT) == kind
    }

    /// A session's attribution once Claude Desktop hosting it is taken into
    /// account: its Desktop record's identity when found, else unsure
    /// (shown under the folder's account, never recorded as anyone's). A
    /// session Desktop doesn't host keeps its folder's. Pure.
    static func attribution(folder: FolderAttribution, isDesktopHosted: Bool, desktopIdentity: String?) -> FolderAttribution {
        guard isDesktopHosted else { return folder }
        if let desktopIdentity { return .known(desktopIdentity) }
        return .unsure(current: folder.bestGuess)
    }
}

/// The hub's lookups of Desktop records, remembered: a record found holds
/// for as long as the session runs (Desktop doesn't move it) and the known
/// identities stay the same; a miss is looked at again after `retryAfter`
/// (the record may be written just after the session starts).
@MainActor
final class DesktopSessionAttributor {
    static let retryAfter: TimeInterval = 15

    private let root: String
    private let exists: (String) -> Bool
    private let list: (String) -> [String]
    private var answers: [String: (identity: String?, candidates: [DesktopHostedSessions.Candidate], at: Date)] = [:]

    init(root: String, exists: @escaping (String) -> Bool = DesktopHostedSessions.isRecord,
         list: @escaping (String) -> [String] = DesktopHostedSessions.listing) {
        self.root = root
        self.exists = exists
        self.list = list
    }

    /// The identity of the Desktop-hosted session `hostSessionId`, or nil.
    func identity(hostSessionId: String?, candidates: [DesktopHostedSessions.Candidate], now: Date) -> String? {
        guard let hostSessionId, DesktopHostedSessions.isHostSessionId(hostSessionId) else { return nil }
        if let known = answers[hostSessionId], known.candidates == candidates,
           known.identity != nil || now.timeIntervalSince(known.at) < Self.retryAfter {
            return known.identity
        }
        let identity = DesktopHostedSessions.identity(hostSessionId: hostSessionId, candidates: candidates, root: root,
                                                      exists: exists, list: list)
        answers[hostSessionId] = (identity, candidates, now)
        return identity
    }

    /// Forget the sessions no longer running.
    func retain(_ hostSessionIds: Set<String>) {
        answers = answers.filter { hostSessionIds.contains($0.key) }
    }
}
