//
//  AccountLabels.swift
//  ClaudeControl
//
//  What each account is called in the panel. The engine already gives every
//  account a distinct default name (`AccountNaming`, the same the ring
//  uses), so what can still collide is a name the user chose: two
//  Codenotch nicknames or custom names that read the same, and colour alone
//  must never be what tells them apart. Colliding labels get the first
//  qualifier that separates them: the plan ("Max 20x" / "Team"), then the
//  email, then the config folder, which always differs.
//

import Foundation

nonisolated enum AccountLabels {
    /// Display label per account id.
    static func disambiguated(_ accounts: [ClaudeAccountSummary], home: String) -> [String: String] {
        var result: [String: String] = [:]
        let groups = Dictionary(grouping: accounts) { $0.label.lowercased() }
        for group in groups.values {
            guard group.count > 1 else {
                if let only = group.first { result[only.id] = only.label }
                continue
            }
            let qualifiers: [(ClaudeAccountSummary) -> String?] = [
                { $0.planName },
                { account in account.email.flatMap { $0.caseInsensitiveCompare(account.label) == .orderedSame ? nil : $0 } },
                { folderName($0.configDir, home: home) },
            ]
            let chosen = qualifiers.first { qualifier in
                let values = group.map(qualifier)
                guard values.allSatisfy({ $0?.isEmpty == false }) else { return false }
                return Set(values.compactMap { $0?.lowercased() }).count == group.count
            } ?? { folderName($0.configDir, home: home) }
            for account in group {
                result[account.id] = "\(account.label) · \(chosen(account) ?? account.configDir)"
            }
        }
        return result
    }

    /// `~/.claude-work` → "work", `~/.claude` → "~/.claude", else `~/…`.
    static func folderName(_ configDir: String, home: String) -> String {
        let name = (configDir as NSString).lastPathComponent
        let parent = (configDir as NSString).deletingLastPathComponent
        let normalizedHome = home.hasSuffix("/") ? String(home.dropLast()) : home
        if parent == normalizedHome, name.hasPrefix(".claude-") || name.hasPrefix(".claude_") {
            let slug = String(name.dropFirst(".claude-".count))
            if !slug.isEmpty { return slug }
        }
        return AccountPathDisplay.abbreviated(configDir, home: home)
    }
}
