import ClaudeControl
import Foundation

/// One Claude ring, as a Codenotch usage provider. It does no I/O and reads
/// no token: ClaudeControl reads usage (the `get_usage` probe, `.claude.json`,
/// the status line, Claude Desktop's cache) and this copies the hub's reading
/// into a `ProviderSnapshot`.
///
/// Readings reach the ring two ways. `ClaudeProviderSync` pushes each new one
/// with `UsageStore.ingest` as it arrives (no refetch, no spinning ring), and
/// the store's own polling asks `fetchSnapshot`, which answers from the same
/// reading.
///
/// Fork-only file (design §4.4). Owned by WP-C.
actor ClaudeUsageProvider: UsageProvider {
    nonisolated let id: String
    nonisolated let glyph: ProviderGlyph = .claude

    /// The ring's own name; the store swaps in a nickname where there is one.
    nonisolated var displayName: String { ClaudeRingNames.shared.name(for: id) }

    nonisolated var signInRoute: SignInRoute {
        .guidance(Self.signInGuidance(launchCommand: ClaudeRingNames.shared.entry(id)?.launchCommand))
    }

    init(ringID: String) {
        self.id = ringID
    }

    func fetchSnapshot() async throws -> ProviderSnapshot {
        let ringID = id
        let (reading, resets) = await MainActor.run {
            (ClaudeControlHub.shared?.ringReadings[ringID],
             DesktopUsageSource.shared.resetCredits(
                organizationUuid: ClaudeRingNames.shared.entry(ringID)?.organizationUuid, now: Date()))
        }
        let current = reading ?? ClaudeRingReading(status: .waitingForFirstReading)
        if case .waitingForFirstReading = current.status {
            // Not an answer yet, and not a failure either. Thrown rather than
            // returned as an empty snapshot: the store would take an empty
            // one as a good fetch and overwrite the reading it archived at the
            // last launch. `credentialExpired` is the store's "keep the last
            // reading and let it age" path, and with nothing archived it shows
            // "Waiting for the first reading…" — which is what this is.
            Log.usage.debug("\(ringID, privacy: .public): no ClaudeControl reading yet; keeping the last one")
            throw UsageProviderError.credentialExpired
        }
        return try Self.snapshot(ringID: ringID, displayName: displayName, reading: current,
                                 resetCredits: resets, now: Date())
    }

    /// Whose usage this is. Nil until the account has signed in, as upstream's
    /// Claude provider does: the settings row then shows the sign-in guidance.
    nonisolated func account() -> ProviderAccount? {
        guard let entry = ClaudeRingNames.shared.entry(id), let email = entry.email, !email.isEmpty else { return nil }
        return ProviderAccount(label: email, plan: entry.plan, source: entry.source, manageURL: Self.manageURL)
    }

    /// The credential is Claude Code's; there is nothing of ours to sign out of.
    func signOut() async {}

    /// Nothing to present: Claude Code signs in from the terminal. The
    /// Claude Code settings pane says how, per account.
    nonisolated func presentSignIn() {
        Task { @MainActor in ClaudeSettingsNavigation.showClaudeCode() }
    }

    /// No credential is ever held.
    nonisolated func forgetCachedCredential() {}

    // MARK: - Field copies (pure)

    static let manageURL = URL(string: "https://claude.ai/settings/usage")

    static func signInGuidance(launchCommand: String?) -> String {
        "Run \(launchCommand ?? "claude") once, then /login."
    }

    /// A ring snapshot from a hub reading:
    ///
    /// | Reading                          | Snapshot                           |
    /// |----------------------------------|------------------------------------|
    /// | within `staleThreshold`          | `.ok`                              |
    /// | older (and not used up)          | `.stale(since:)`                   |
    /// | waiting for the first reading    | no windows, `.stale(.distantPast)` |
    /// | sign-in needed, or unavailable   | `.unsupported(text)`               |
    /// | failed with no windows           | throws `apiError(text)`            |
    ///
    /// A failed reading that still has windows is its windows, aged like any
    /// other.
    static func snapshot(
        ringID: String,
        displayName: String,
        reading: ClaudeRingReading,
        resetCredits: UsageResetCredits? = nil,
        now: Date
    ) throws -> ProviderSnapshot {
        let status: ProviderStatus
        // Codenotch's order whatever the reading's: the card lists them as
        // they come, and the menu bar and alerts expect the session first.
        var windows = reading.windows.map(window).sorted(by: UsageResponse.displayOrder)
        switch reading.status {
        case .ok:
            status = age(of: reading, now: now)
        case .failed(let text):
            guard !windows.isEmpty else { throw UsageProviderError.apiError(text) }
            status = age(of: reading, now: now)
        case .waitingForFirstReading:
            windows = []
            status = .stale(since: .distantPast)
        case .signInNeeded(let text), .unavailable(let text):
            status = .unsupported(text)
        }
        return ProviderSnapshot(
            id: ringID,
            displayName: displayName,
            glyph: .claude,
            fidelity: .official,
            status: status,
            windows: windows,
            headlineID: "session",
            weeklyID: "weekly_all",
            plan: reading.plan.flatMap { $0.isEmpty ? nil : $0 },
            resetCredits: resetCredits
        )
    }

    /// What `ClaudeProviderSync` pushes into the store for a reading, or nil
    /// when the reading says nothing the ring should change for: still
    /// waiting (the placeholder or the archived reading already says so), or
    /// failed with nothing to show (the store's own next poll takes the
    /// failure through its degrade path, which keeps the last good reading).
    static func pushed(
        ringID: String,
        displayName: String,
        reading: ClaudeRingReading,
        resetCredits: UsageResetCredits? = nil,
        now: Date
    ) -> ProviderSnapshot? {
        if case .waitingForFirstReading = reading.status { return nil }
        return try? snapshot(ringID: ringID, displayName: displayName, reading: reading,
                             resetCredits: resetCredits, now: now)
    }

    /// Stale past the reading's own threshold (the engine scales it with the
    /// probe interval), and never while a used-up window waits for its reset.
    private static func age(of reading: ClaudeRingReading, now: Date) -> ProviderStatus {
        // A failed check that still has windows ages like any other reading.
        var measured = reading
        measured.status = .ok
        guard measured.isStale(now: now), let updated = reading.updatedAt else { return .ok }
        return .stale(since: updated)
    }

    static func window(_ window: ClaudeRingReading.Window) -> LimitWindow {
        LimitWindow(
            id: window.id,
            label: window.label ?? UsageResponse.label(forKind: window.id),
            usedFraction: window.usedFraction,
            money: window.money.map { UsageMoneyBreakdown(currency: $0.currency, spent: $0.spent, remaining: $0.remaining) },
            resetsAt: window.resetsAt,
            duration: window.duration
        )
    }
}
