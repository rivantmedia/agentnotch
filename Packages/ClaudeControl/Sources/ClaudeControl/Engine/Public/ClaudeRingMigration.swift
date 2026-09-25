//
//  ClaudeRingMigration.swift
//  ClaudeControl
//
//  Moving Codenotch's choices from per-folder rings to per-account rings.
//  Before accounts were identities every config folder had a ring
//  (`claude`, `claude-paras`, `claude-shared`, …); now an account has one
//  (`claude-acct-…`) whatever folders it lives in. What the user set up for
//  the old rings (a nickname, its place in the order, its archived reading,
//  muted alerts) moves to the account's ring, the first old ring that has
//  it winning ("first" by the user's order, then the account's folder
//  order; an old ring that may not be the account's, `uncertainFormerRingIDs`,
//  last). The ring is on when any of its old rings was on (a duplicate store
//  ring switched off can't switch the account off), and in the menu bar
//  when any was chosen for it. The old ids then disappear from the order,
//  the nicknames, the archive, the muted alerts and the menu bar, so no
//  stale "Claude (shared)" row is left anywhere. Each old id is migrated
//  once (the app remembers which), so a choice made on the new ring later
//  is never overwritten. Pure; the app applies it.
//

import Foundation

public nonisolated enum ClaudeRingMigration {
    /// What Codenotch has stored, as far as the migration needs it.
    public struct Stored: Equatable, Sendable {
        public var nicknames: [String: String]
        /// Ids Codenotch has seen, and of those the ones switched on.
        public var seen: Set<String>
        public var connected: Set<String>
        public var order: [String]
        /// Ids with an archived reading.
        public var archived: Set<String>
        /// Old ids already migrated in an earlier run.
        public var migrated: Set<String>
        /// Ids whose threshold alerts are muted.
        public var muted: Set<String> = []
        /// Ids chosen for the menu bar; nil while nobody has chosen (the
        /// defaults apply, nothing to move).
        public var menuBar: Set<String>?

        public init(nicknames: [String: String] = [:], seen: Set<String> = [], connected: Set<String> = [],
                    order: [String] = [], archived: Set<String> = [], migrated: Set<String> = [],
                    muted: Set<String> = [], menuBar: Set<String>? = nil) {
            self.nicknames = nicknames
            self.seen = seen
            self.connected = connected
            self.order = order
            self.archived = archived
            self.migrated = migrated
            self.muted = muted
            self.menuBar = menuBar
        }
    }

    /// What to change.
    public struct Plan: Equatable, Sendable {
        /// New nicknames for rings; nil removes an old ring's.
        public var nicknames: [String: String?] = [:]
        /// Rings to switch on or off.
        public var connected: [String: Bool] = [:]
        /// The new order, when it changes.
        public var order: [String]?
        /// Archived readings to move (old id → ring) and to drop.
        public var archiveMoves: [String: String] = [:]
        public var archiveDrops: Set<String> = []
        /// Old ids handled now (to remember).
        public var migrated: Set<String> = []
        /// Alerts to mute (true) or unmute (false), per id.
        public var muted: [String: Bool] = [:]
        /// Ids to put in (true) or take out of (false) the menu bar.
        public var menuBar: [String: Bool] = [:]

        public init() {}

        public var isEmpty: Bool {
            nicknames.isEmpty && connected.isEmpty && order == nil && archiveMoves.isEmpty
                && archiveDrops.isEmpty && migrated.isEmpty && muted.isEmpty && menuBar.isEmpty
        }
    }

    /// - Parameters:
    ///   - accounts: the accounts now, each with the ring ids its folders had.
    ///   - retired: old ring ids no ring has any more (folders that are not
    ///     accounts, like `~/.claude-shared`, included).
    public static func plan(accounts: [ClaudeAccountSummary], retired: Set<String>, stored: Stored) -> Plan {
        var plan = Plan()
        let current = Set(accounts.map(\.ringID))
        var order = stored.order
        var claimed = Set<String>()

        func position(_ id: String) -> Int { order.firstIndex(of: id) ?? Int.max }
        /// An old id gone from everywhere.
        func retire(_ old: String) {
            if stored.nicknames[old] != nil { plan.nicknames[old] = .some(nil) }
            if stored.archived.contains(old), plan.archiveMoves[old] == nil { plan.archiveDrops.insert(old) }
            if stored.muted.contains(old) { plan.muted[old] = false }
            if stored.menuBar?.contains(old) == true { plan.menuBar[old] = false }
            order.removeAll { $0 == old }
            plan.migrated.insert(old)
        }

        for account in accounts {
            let ring = account.ringID
            let candidates = account.formerRingIDs
                .filter { $0 != ring && !current.contains($0) && !stored.migrated.contains($0) && !claimed.contains($0) }
            guard !candidates.isEmpty else { continue }
            claimed.formUnion(candidates)
            // First: where the user put them, then the account's folder
            // order; one that may not be this account's last.
            let uncertain = account.uncertainFormerRingIDs
            let ranked = candidates.enumerated()
                .sorted { lhs, rhs in
                    let left = (uncertain.contains(lhs.element) ? 1 : 0, position(lhs.element), lhs.offset)
                    let right = (uncertain.contains(rhs.element) ? 1 : 0, position(rhs.element), rhs.offset)
                    return left < right
                }
                .map(\.element)
            // Choices that are surely this account's, else the uncertain ones.
            let sure = ranked.filter { !uncertain.contains($0) && stored.seen.contains($0) }
            let seenCandidates = sure.isEmpty ? ranked.filter(stored.seen.contains) : sure

            if stored.nicknames[ring] == nil,
               let nickname = ranked.lazy.compactMap({ stored.nicknames[$0] }).first(where: { !$0.isEmpty }) {
                plan.nicknames[ring] = .some(nickname)
            }
            if !seenCandidates.isEmpty {
                // On when any of its old rings was: a second store's ring
                // the user switched off doesn't switch the account off.
                let on = seenCandidates.contains(where: stored.connected.contains)
                if stored.connected.contains(ring) != on || !stored.seen.contains(ring) {
                    plan.connected[ring] = on
                }
            }
            if let first = seenCandidates.first ?? ranked.first, !stored.muted.contains(ring), stored.muted.contains(first) {
                plan.muted[ring] = true
            }
            if let chosen = stored.menuBar, !chosen.contains(ring), candidates.contains(where: chosen.contains) {
                plan.menuBar[ring] = true
            }
            if !order.contains(ring), let place = ranked.compactMap({ order.firstIndex(of: $0) }).min() {
                order.insert(ring, at: place)
            }
            if !stored.archived.contains(ring), let source = ranked.first(where: stored.archived.contains) {
                plan.archiveMoves[source] = ring
            }
            for old in ranked {
                retire(old)
            }
        }

        // Old rings no account took over (the shared history's, a folder
        // nobody signs in to any more): gone from everywhere.
        for old in retired.subtracting(current).subtracting(claimed).subtracting(stored.migrated).sorted() {
            retire(old)
        }

        if order != stored.order { plan.order = order }
        return plan
    }
}
