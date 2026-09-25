//
//  AccountNaming.swift
//  ClaudeControl
//
//  Default names and badge letters for accounts, so two accounts never read
//  the same before anyone renames them. Pure.
//
//  Names follow Codenotch's own rule for Claude rings (`ClaudeProfile`
//  `.displayName` / `.displayNames`): `Claude Gmail` after the signed-in
//  address's domain, `Claude (work)` after the folder when nobody is signed
//  in, and the whole address for the accounts whose short names collide.
//  Beyond Codenotch: the same address in two organizations adds the
//  organization (or plan), and anything still equal adds the folder.
//
//  Badge letters come from the domain (`GM`, `AC`), which is what tells a
//  personal login from a work one; when two collide they fall back to the
//  address's own letters, the organization's, the folder's, then a digit.
//

import Foundation

nonisolated enum AccountNaming {
    struct Names: Equatable, Sendable {
        var label: String
        var monogram: String
    }

    // MARK: - One account

    /// Codenotch's one word for an address: its domain's first label,
    /// capitalised (`someone@acme.co.uk` → `Acme`). Nil when there is none
    /// worth showing.
    static func domainLabel(forAddress address: String?) -> String? {
        guard let address, let at = address.lastIndex(of: "@") else { return nil }
        let domain = address[address.index(after: at)...]
        guard let first = domain.split(separator: ".", omittingEmptySubsequences: false).first.map(String.init),
              !first.isEmpty,
              first.rangeOfCharacter(from: .letters) != nil
        else { return nil }
        return first.prefix(1).uppercased() + first.dropFirst()
    }

    /// The folder's own word: `work` for `~/.claude-work` or `~/.claude_work`,
    /// the folder name without its leading dot otherwise; nil for `~/.claude`.
    static func folderWord(for account: ClaudeAccount) -> String? {
        let name = AccountPaths.shortName(forConfigDir: account.configDir)
        if name == ".claude" || AccountPaths.isDefaultConfigDir(account.configDir) { return nil }
        for prefix in [".claude-", ".claude_"] where name.hasPrefix(prefix) && name.count > prefix.count {
            return String(name.dropFirst(prefix.count))
        }
        let trimmed = name.hasPrefix(".") ? String(name.dropFirst()) : name
        return trimmed.isEmpty ? name : trimmed
    }

    /// The name before any collision is considered.
    static func baseLabel(for account: ClaudeAccount) -> String {
        if let email = account.email, !email.isEmpty {
            if let word = domainLabel(forAddress: email) { return "Claude \(word)" }
            return "Claude \(email)"
        }
        if let word = folderWord(for: account) { return "Claude (\(word))" }
        return "Claude"
    }

    /// Two uppercase letters (or digits) from `text`, if it has any.
    static func letters(_ text: String) -> String? {
        let characters = text.filter { $0.isLetter || $0.isNumber }
        guard !characters.isEmpty else { return nil }
        return String(characters.prefix(2)).uppercased()
    }

    /// Badge letters in order of preference.
    static func monogramCandidates(for account: ClaudeAccount) -> [String] {
        var candidates: [String] = []
        func add(_ text: String?) {
            guard let text, let pair = letters(text), !candidates.contains(pair) else { return }
            candidates.append(pair)
        }
        if let customLabel = account.customLabel, !customLabel.isEmpty { add(customLabel) }
        if let email = account.email, !email.isEmpty {
            add(domainLabel(forAddress: email))
            add(email.split(separator: "@").first.map(String.init))
        }
        if let organization = account.organizationName {
            let initials = organization.split(whereSeparator: { !$0.isLetter && !$0.isNumber }).compactMap(\.first)
            add(initials.count >= 2 ? String(initials) : organization)
        }
        add(account.displayName)
        add(folderWord(for: account))
        if candidates.isEmpty { candidates.append("CC") }
        return candidates
    }

    // MARK: - All accounts

    /// A distinct default name and badge for every account. Accounts with a
    /// custom name keep it (their badge still comes from it), but take part:
    /// a default name that equals someone's custom name is lengthened too.
    static func assign(_ accounts: [ClaudeAccount]) -> [String: Names] {
        let ordered = accounts.sorted { $0.id < $1.id }
        var labels: [String: String] = [:]
        var level: [String: Int] = [:]
        for account in ordered {
            labels[account.id] = account.customLabel.flatMap { $0.isEmpty ? nil : $0 } ?? baseLabel(for: account)
            level[account.id] = 0
        }
        let custom = Set(ordered.filter { !($0.customLabel ?? "").isEmpty }.map(\.id))

        // Lengthen only the names that clash, one step at a time (a step
        // with nothing to add for an account, like an address it doesn't
        // have, leaves its name as it was for the next one).
        for _ in 1...maxLevel {
            let clashing = clashes(labels, among: ordered.map(\.id)).subtracting(custom)
            guard !clashing.isEmpty else { break }
            for account in ordered where clashing.contains(account.id) {
                let next = (level[account.id] ?? 0) + 1
                if let longer = label(for: account, level: next) {
                    labels[account.id] = longer
                }
                level[account.id] = next
            }
        }

        var monograms: [String: String] = [:]
        var taken = Set<String>()
        let preferred = Dictionary(uniqueKeysWithValues: ordered.map { ($0.id, monogramCandidates(for: $0)) })
        func pick(_ options: [String]) -> String {
            if let free = options.first(where: { !taken.contains($0) }) { return free }
            let lead = String((options.first ?? "C").prefix(1))
            return (1...99).lazy.map { "\(lead)\($0)" }.first { !taken.contains($0) } ?? lead
        }
        // A name the user chose keeps its own letters: they pick first (a
        // second custom name with the same letters takes its next choice).
        for account in ordered where custom.contains(account.id) {
            let chosen = pick(preferred[account.id] ?? ["CC"])
            monograms[account.id] = chosen
            taken.insert(chosen)
        }
        let defaults = ordered.filter { !custom.contains($0.id) }
        let firstChoices = defaults.map { preferred[$0.id]?.first ?? "CC" }
        let clashingFirst = Set(firstChoices.filter { first in firstChoices.filter { $0 == first }.count > 1 })
        // Accounts whose first choice is theirs alone (and free) keep it.
        for account in defaults {
            let first = preferred[account.id]?.first ?? "CC"
            if !clashingFirst.contains(first), !taken.contains(first) {
                monograms[account.id] = first
                taken.insert(first)
            }
        }
        // The rest avoid the letters they would have shared: their other
        // candidates first, the shared ones only if nothing else is free.
        for account in defaults where monograms[account.id] == nil {
            let options = preferred[account.id] ?? ["CC"]
            let chosen = pick(Array(options.dropFirst()) + options.prefix(1))
            monograms[account.id] = chosen
            taken.insert(chosen)
        }

        var names: [String: Names] = [:]
        for account in ordered {
            names[account.id] = Names(label: labels[account.id] ?? baseLabel(for: account),
                                      monogram: monograms[account.id] ?? "CC")
        }
        return names
    }

    private static let maxLevel = 3

    /// The name at one step of lengthening, or nil when that step adds nothing.
    private static func label(for account: ClaudeAccount, level: Int) -> String? {
        let email = account.email.flatMap { $0.isEmpty ? nil : $0 }
        let place = AccountPathDisplayName.abbreviated(account.configDir)
        switch level {
        case 1:
            return email.map { "Claude \($0)" }
        case 2:
            guard let email else { return nil }
            if let organization = account.organizationName, !organization.isEmpty {
                return "Claude \(email) · \(organization)"
            }
            if let plan = account.planName { return "Claude \(email) · \(plan)" }
            return nil
        case 3:
            if let email { return "Claude \(email) · \(place)" }
            return "Claude (\(place))"
        default:
            return nil
        }
    }

    private static func clashes(_ labels: [String: String], among ids: [String]) -> Set<String> {
        var byName: [String: [String]] = [:]
        for id in ids {
            byName[(labels[id] ?? "").lowercased(), default: []].append(id)
        }
        return Set(byName.values.filter { $0.count > 1 }.flatMap { $0 })
    }
}

/// Paths under the home folder written `~/…`, for names and messages.
nonisolated enum AccountPathDisplayName {
    static func abbreviated(_ path: String, home: String = AccountPaths.homeDirectory) -> String {
        let normalized = AccountPaths.normalize(path)
        let homePath = AccountPaths.normalize(home)
        if normalized == homePath { return "~" }
        if normalized.hasPrefix(homePath + "/") { return "~" + normalized.dropFirst(homePath.count) }
        return normalized
    }
}
