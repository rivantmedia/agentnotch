import ClaudeControl
import Foundation

/// Claude Desktop's cached usage, for ClaudeControl
/// (`ClaudeControlConfiguration.externalUsageSource`).
///
/// A thin adapter over Codenotch's `ClaudeDesktopUsageCache`, which reads the
/// `GET /api/organizations/<id>/usage` response Claude Desktop left in its
/// own HTTP cache: no token, no cookie, no keychain, no request, no
/// subprocess and no write. ClaudeControl decides when to ask and how a
/// reading merges with the others; this only reads and translates.
///
/// It also remembers the unused-reset grants that come with a reading, per
/// organization, for the ring's hover card (`ClaudeUsageProvider`).
///
/// Live runs only; sealed runs never read Claude Desktop's files.
///
/// Fork-only file. Owned by WP-C.
final class DesktopUsageSource: ClaudeExternalUsageSource, @unchecked Sendable {
    static let shared = DesktopUsageSource()

    /// After a read that found nothing (Desktop closed, signed into another
    /// account, no cache), how long before the directory is scanned again.
    static let rescanAfterMiss: TimeInterval = 5 * 60

    private let cache: ClaudeDesktopUsageCache
    private let isEnabled: @Sendable () -> Bool
    private let lock = NSLock()
    private var resets: [String: ClaudeDesktopUsageCache.ResetReading] = [:]
    private var lastMiss: [String: Date] = [:]

    init(cache: ClaudeDesktopUsageCache = ClaudeDesktopUsageCache(),
         isEnabled: @escaping @Sendable () -> Bool = { ClaudeControlSettings.readsDesktopUsageCache }) {
        self.cache = cache
        self.isEnabled = isEnabled
    }

    func reading(organizationUuid: String, now: Date) async -> ClaudeExternalUsageReading? {
        guard !Fork.isSealed, isEnabled(), !organizationUuid.isEmpty else { return nil }
        if let miss = withLock({ lastMiss[organizationUuid] }), now.timeIntervalSince(miss) < Self.rescanAfterMiss {
            return nil
        }
        // Off whatever actor asked: a scan stats a few thousand entries.
        let cache = self.cache
        let found = await Task.detached(priority: .utility) {
            cache.read(organization: organizationUuid, now: now)
        }.value
        guard let found else {
            withLock { lastMiss[organizationUuid] = now }
            return nil
        }
        withLock {
            lastMiss[organizationUuid] = nil
            if let grant = found.resets { resets[organizationUuid] = grant }
        }
        return Self.translate(found, now: now)
    }

    /// The unused resets last seen for this organization, as the hover card
    /// shows them (dated by when Desktop saw them), or nil.
    func resetCredits(organizationUuid: String?, now: Date) -> UsageResetCredits? {
        guard let organizationUuid else { return nil }
        return withLock { resets[organizationUuid] }?.credits(at: now)
    }

    // MARK: - Translation (pure)

    /// A cache reading as ClaudeControl's, or nil when it describes a period
    /// that is over: a window whose reset time has passed makes the whole
    /// response out of date, however recently it was written (upstream's
    /// Claude provider rejects it for the same reason).
    static func translate(_ reading: ClaudeDesktopUsageCache.Reading, now: Date) -> ClaudeExternalUsageReading? {
        guard !reading.windows.contains(where: { ($0.resetsAt.map { $0 <= now }) ?? false }) else { return nil }
        let windows = reading.windows.compactMap(window)
        guard !windows.isEmpty else { return nil }
        return ClaudeExternalUsageReading(windows: windows, observedAt: reading.capturedAt)
    }

    /// One window. Codenotch's ids are ClaudeControl's; a label Codenotch
    /// would give the id anyway is left out, so it follows the app's language
    /// rather than being stored in the one it was read in.
    static func window(_ window: LimitWindow) -> ClaudeRingReading.Window? {
        guard let used = window.usedFraction else { return nil }
        let standard = UsageResponse.label(forKind: window.id)
        return ClaudeRingReading.Window(
            id: window.id,
            label: window.label == standard ? nil : window.label,
            usedFraction: used,
            resetsAt: window.resetsAt,
            duration: window.duration,
            money: window.money.map { .init(currency: $0.currency, spent: $0.spent, remaining: $0.remaining) }
        )
    }

    private func withLock<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }
}
