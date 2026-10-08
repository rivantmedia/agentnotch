//
//  LimitAnnouncements.swift
//  ClaudeControl
//
//  Which usage limits of each account have been announced, so a limit is
//  told once per account until it lifts, however often it is seen again.
//
//  Three things see an account hit its limit: the account's limit banner
//  (sessions stopped by the limit, `NotificationService`), the notch's chime
//  and peek for a failed turn (the hub's transitions), and Codenotch's own
//  "limit reached" card or banner for the account's ring (the bridge, seam
//  LIM2). Each used to announce the same limit, and the first two again on
//  every retry, wake-up, /loop tick and reopened session. All three key an
//  account by the ring the hub shows it on.
//
//  - An incident is one account (its ring) and one window: the 5-hour, the
//    weekly, a model's weekly, or not known yet. It lasts until that window
//    resets, or `unknownLifetime` when no reset time is known, or until the
//    readings show the window back under its limit (an early reset keeps
//    the reset time): then using it up again is news.
//  - Each channel fires once per incident: a notification (a banner or a
//    card) and a reaction (the chime and peek). A notification that brings
//    its own sound (Codenotch's card) takes the reaction with it.
//  - An incident of a window not known yet covers every window for
//    `unknownAbsorbsFor`, and takes the first window and reset time it is
//    told, by a claim or by the readings (`learn`): a turn fails on the limit
//    before the readings say which window ran out. A window that runs out
//    later than that is a limit of its own (the first failure was a burst
//    of requests turned away, not a window running out).
//  - Kept in `<support>/limit-announcements.json` (0600), so a relaunch
//    doesn't tell a limit again. A sealed run keeps it in memory.
//

import Foundation
import os.log

/// The announced incidents, per ring. Pure, so the rule is unit-tested.
nonisolated struct LimitAnnouncements: Codable, Equatable, Sendable {
    nonisolated enum Channel: String, Codable, Sendable, CaseIterable {
        /// A banner, or Codenotch's card in the notch.
        case notification
        /// The notch's chime and peek for a turn stopped by the limit.
        case reaction
    }

    nonisolated struct Incident: Codable, Equatable, Sendable {
        /// `windowKey(_:)`: "session", "weekly", "scoped:<model>", or "unknown".
        var window: String
        /// When the window resets; nil when nobody said.
        var resetsAt: Date?
        var startedAt: Date
        var channels: Set<Channel>

        func expiresAt(unknownLifetime: TimeInterval) -> Date {
            resetsAt ?? startedAt.addingTimeInterval(unknownLifetime)
        }
    }

    static let unknownWindow = "unknown"
    /// How long an incident without a reset time lasts. A limit the usage
    /// readings don't show is most likely brief (a burst of requests turned
    /// away), so an hour, not a whole window.
    static let unknownLifetime: TimeInterval = 60 * 60
    /// How long an incident of a window not known yet waits for the readings
    /// to name it: the longest automatic probe interval.
    static let unknownAbsorbsFor: TimeInterval = 30 * 60
    /// Incidents kept per ring at most (one per window is the most there
    /// can be at once).
    static let maxIncidentsPerRing = 8

    var version = 1
    var incidents: [String: [Incident]] = [:]

    static func windowKey(_ window: UsageLimitHit.Window?) -> String {
        switch window {
        case .session?: return "session"
        case .weekly?: return "weekly"
        case .scoped(let name)?: return "scoped:\(name)"
        case nil: return unknownWindow
        }
    }

    /// `incident` speaks for a claim on the window `claimed` at `now`: the
    /// same window; a claim that names none; or an incident that names none
    /// yet, while the readings may still be catching up with it.
    static func covers(_ incident: Incident, _ claimed: String, now: Date) -> Bool {
        if incident.window == claimed || claimed == unknownWindow { return true }
        guard incident.window == unknownWindow else { return false }
        return now.timeIntervalSince(incident.startedAt) <= unknownAbsorbsFor
    }

    /// Whether `channel` may announce `ring`'s limit on `window` (a
    /// `windowKey`), resetting at `resetsAt`: true the first time in an
    /// incident, which it then records, with `extra` channels marked too.
    /// A reset time that has already passed counts as unknown.
    mutating func claim(
        _ channel: Channel,
        ring: String,
        window: String,
        resetsAt: Date?,
        now: Date,
        alsoMarking extra: Set<Channel> = []
    ) -> Bool {
        prune(now: now)
        let resetsAt = resetsAt.flatMap { $0 > now ? $0 : nil }
        var list = incidents[ring] ?? []
        let covering = list.indices.filter { Self.covers(list[$0], window, now: now) }
        guard let first = covering.first else {
            list.append(Incident(window: window, resetsAt: resetsAt, startedAt: now,
                                 channels: extra.union([channel])))
            incidents[ring] = Array(list.suffix(Self.maxIncidentsPerRing))
            return true
        }
        let isNew = !covering.contains { list[$0].channels.contains(channel) }
        var incident = list[first]
        if incident.window == Self.unknownWindow, window != Self.unknownWindow {
            incident.window = window
            incident.resetsAt = resetsAt ?? incident.resetsAt
        } else if incident.window == window, incident.resetsAt == nil, let resetsAt {
            incident.resetsAt = resetsAt
        }
        if isNew { incident.channels.formUnion(extra.union([channel])) }
        list[first] = incident
        incidents[ring] = list
        return isNew
    }

    /// The readings now say which window ran out: an incident of a window not
    /// known yet (and still waiting for one) takes it and its reset time,
    /// announcing nothing, so it lasts until that reset rather than an hour.
    mutating func learn(ring: String, window: String, resetsAt: Date?, now: Date) {
        prune(now: now)
        guard window != Self.unknownWindow, var list = incidents[ring],
              let index = list.lastIndex(where: { $0.window == Self.unknownWindow }),
              now.timeIntervalSince(list[index].startedAt) <= Self.unknownAbsorbsFor else { return }
        list[index].window = window
        list[index].resetsAt = resetsAt.flatMap { $0 > now ? $0 : nil } ?? list[index].resetsAt
        incidents[ring] = list
    }

    /// The readings show `window` back under its limit before its reset time
    /// (an early reset, a reset credit): its incident is over, and using the
    /// window up again is a new limit.
    mutating func end(ring: String, window: String) {
        guard var list = incidents[ring] else { return }
        list.removeAll { $0.window == window }
        incidents[ring] = list.isEmpty ? nil : list
    }

    /// Drops the incidents whose window has reset (or that outlived
    /// `unknownLifetime`).
    mutating func prune(now: Date) {
        for (ring, list) in incidents {
            let live = list.filter { $0.expiresAt(unknownLifetime: Self.unknownLifetime) > now }
            incidents[ring] = live.isEmpty ? nil : live
        }
    }
}

/// The app's one `LimitAnnouncements`, kept on disk.
@MainActor
final class LimitAnnouncementStore {
    static let fileName = "limit-announcements.json"

    /// A sealed run keeps its announcements in memory only.
    static let shared = LimitAnnouncementStore(
        fileURL: AppIdentity.supportDirectory.appendingPathComponent(fileName),
        persists: !DevFlags.isSealed
    )

    private static var logger: Logger { EngineLog.logger("LimitAnnouncements") }

    private(set) var state: LimitAnnouncements
    private let fileURL: URL
    private let persists: Bool
    /// Tests say no, so a write landing after a test removed its temporary
    /// folder doesn't bring the folder back.
    private let createsFolder: Bool

    init(fileURL: URL, persists: Bool = true, createsFolder: Bool = true, now: Date = Date()) {
        self.fileURL = fileURL
        self.persists = persists
        self.createsFolder = createsFolder
        var loaded = persists ? Self.load(from: fileURL) : LimitAnnouncements()
        loaded.prune(now: now)
        state = loaded
    }

    /// `LimitAnnouncements.claim`, saved when it changed anything.
    func claim(
        _ channel: LimitAnnouncements.Channel,
        ring: String,
        window: UsageLimitHit.Window?,
        resetsAt: Date?,
        now: Date = Date(),
        alsoMarking extra: Set<LimitAnnouncements.Channel> = []
    ) -> Bool {
        var next = state
        let granted = next.claim(channel, ring: ring, window: LimitAnnouncements.windowKey(window),
                                 resetsAt: resetsAt, now: now, alsoMarking: extra)
        if next != state {
            state = next
            save()
        }
        return granted
    }

    /// `LimitAnnouncements.learn`, saved when it changed anything.
    func learn(ring: String, window: UsageLimitHit.Window, resetsAt: Date?, now: Date = Date()) {
        update { $0.learn(ring: ring, window: LimitAnnouncements.windowKey(window), resetsAt: resetsAt, now: now) }
    }

    /// `LimitAnnouncements.end`, saved when it changed anything.
    func end(ring: String, window: UsageLimitHit.Window) {
        update { $0.end(ring: ring, window: LimitAnnouncements.windowKey(window)) }
    }

    private func update(_ change: (inout LimitAnnouncements) -> Void) {
        var next = state
        change(&next)
        if next != state {
            state = next
            save()
        }
    }

    private func save() {
        guard persists else { return }
        do {
            let encoder = JSONEncoder()
            encoder.dateEncodingStrategy = .iso8601
            encoder.outputFormatting = [.sortedKeys]
            try CloudFiles.writeAtomically(try encoder.encode(state), to: fileURL, createsFolder: createsFolder)
        } catch {
            Self.logger.error("Saving announced limits failed: \(String(describing: error), privacy: .public)")
        }
    }

    private static func load(from url: URL) -> LimitAnnouncements {
        guard let data = try? Data(contentsOf: url) else { return LimitAnnouncements() }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        do {
            return try decoder.decode(LimitAnnouncements.self, from: data)
        } catch {
            logger.error("Unreadable \(fileName, privacy: .public), starting afresh: \(String(describing: error), privacy: .public)")
            return LimitAnnouncements()
        }
    }
}
