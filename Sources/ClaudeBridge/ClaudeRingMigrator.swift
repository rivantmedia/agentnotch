import ClaudeControl
import Foundation

/// Moves Codenotch's choices for the per-folder Claude rings of earlier
/// versions (`claude`, `claude-paras`, `claude-shared`, …) to the account
/// rings (`claude-acct-…`) that replaced them: nickname, on/off, place in
/// the order, archived reading, muted alerts and the menu-bar choice, the
/// first old ring that had one winning; then drops the old ids from the
/// order, the nicknames, the archive, the muted alerts and the menu bar
/// (`ClaudeRingMigration.plan`, which holds the rules and is tested in the
/// package). Each old id once: the ids handled are remembered in this app's
/// own defaults, so a choice made on an account ring later stands.
///
/// Fork-only file.
@MainActor
enum ClaudeRingMigrator {
    /// Old ring ids already moved over (the fork's own defaults domain).
    static let migratedKey = "claudeControl.migratedRingIDs"

    /// At launch, before the store exists (it reads the archive when made):
    /// the archived readings. Nothing else is written here.
    static func migrateArchive(hub: ClaudeControlHub, defaults: UserDefaults = .standard, log: (String) -> Void = { _ in }) {
        let (accounts, retired) = hub.ringMigrationSources()
        migrateArchive(accounts: accounts, retired: retired, defaults: defaults, log: log)
    }

    /// The archived readings of `accounts`' former rings, moved (see `migrateArchive(hub:)`).
    static func migrateArchive(accounts: [ClaudeAccountSummary], retired: Set<String>,
                               defaults: UserDefaults = .standard, log: (String) -> Void = { _ in }) {
        guard !accounts.isEmpty || !retired.isEmpty else { return }
        let archive = UsageArchive(defaults: defaults)
        var readings = archive.load()
        let stored = ClaudeRingMigration.Stored(archived: Set(readings.keys),
                                                migrated: Set(defaults.stringArray(forKey: migratedKey) ?? []))
        let plan = ClaudeRingMigration.plan(accounts: accounts, retired: retired, stored: stored)
        guard !plan.archiveMoves.isEmpty || !plan.archiveDrops.isEmpty else { return }
        for (old, ring) in plan.archiveMoves {
            guard let entry = readings.removeValue(forKey: old), readings[ring] == nil else { continue }
            readings[ring] = (renamed(entry.snapshot, to: ring), entry.fetchedAt)
        }
        for old in plan.archiveDrops { readings.removeValue(forKey: old) }
        archive.save(readings)
        log("claude archive moved \(plan.archiveMoves.sorted { $0.key < $1.key }.map { "\($0.key)→\($0.value)" }.joined(separator: ", ")), dropped \(plan.archiveDrops.sorted().joined(separator: ", "))")
    }

    /// With Codenotch's preferences: nicknames, on/off, order, muted alerts
    /// and the menu bar. Returns whether anything changed.
    @discardableResult
    static func migratePreferences(accounts: [ClaudeAccountSummary], retired: Set<String>,
                                   preferences: Preferences, store: UsageStore?,
                                   defaults: UserDefaults = .standard, log: (String) -> Void = { _ in }) -> Bool {
        let migrated = Set(defaults.stringArray(forKey: migratedKey) ?? [])
        let seen = preferences.seenProviders
        let stored = ClaudeRingMigration.Stored(
            nicknames: preferences.accountNicknames,
            seen: seen,
            connected: Set((seen.union(accounts.map(\.ringID))).filter(preferences.isConnected)),
            order: preferences.providerOrder,
            archived: [],
            migrated: migrated,
            muted: preferences.mutedAlertProviders,
            menuBar: preferences.menuBarLimits.chosen
        )
        let plan = ClaudeRingMigration.plan(accounts: accounts, retired: retired, stored: stored)
        guard !plan.isEmpty else { return false }
        for (id, nickname) in plan.nicknames.sorted(by: { $0.key < $1.key }) {
            preferences.setNickname(nickname ?? "", for: id)
        }
        for (ring, on) in plan.connected.sorted(by: { $0.key < $1.key }) {
            preferences.setConnected(on, for: ring)
        }
        for (id, muted) in plan.muted.sorted(by: { $0.key < $1.key }) {
            preferences.setAlertsMuted(muted, for: id)
        }
        // Only once someone chose (else the defaults stand): `listed` is
        // then never read.
        for (id, shown) in plan.menuBar.sorted(by: { $0.key < $1.key }) {
            preferences.setInMenuBar(shown, for: id, among: [])
        }
        if let order = plan.order {
            // Assigned, not `setProviderOrder`: that keeps ids it can't see,
            // and these are gone for good.
            preferences.providerOrder = order
        }
        defaults.set(Array(migrated.union(plan.migrated)).sorted(), forKey: migratedKey)
        if let store {
            store.disconnected = preferences.disconnectedIDs(among: store.knownIDs)
        }
        log("claude rings migrated: \(plan.migrated.sorted().joined(separator: ", "))"
            + (plan.connected.isEmpty ? "" : "; on/off \(plan.connected.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: ", "))")
            + (plan.order == nil ? "" : "; order rewritten"))
        return true
    }

    /// An archived reading under another ring id.
    private static func renamed(_ snapshot: ProviderSnapshot, to id: String) -> ProviderSnapshot {
        ProviderSnapshot(
            id: id,
            displayName: snapshot.displayName,
            glyph: snapshot.glyph,
            fidelity: snapshot.fidelity,
            status: snapshot.status,
            windows: snapshot.windows,
            headlineID: snapshot.headlineID,
            weeklyID: snapshot.weeklyID,
            tokenUsage: snapshot.tokenUsage,
            usageDetail: snapshot.usageDetail
        )
    }
}
