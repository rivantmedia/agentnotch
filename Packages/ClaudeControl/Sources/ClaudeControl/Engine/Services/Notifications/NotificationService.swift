//
//  NotificationService.swift
//  ClaudeControl
//
//  macOS notifications for sessions that need the user or finished work to
//  review, driven by AttentionTracker's per-session transitions.
//
//  - Identifiers are `agentnotch.needsInput.<session id>` and
//    `agentnotch.review.<session id>` (thread = the session), so a newer banner
//    replaces the older; each is withdrawn once the session no longer needs
//    input, is reviewed, or goes away.
//  - Sessions stopped by a usage limit share one banner per account,
//    `agentnotch.limit.<ring id>`: "Work: 3 sessions hit the limit · resets 14:05",
//    naming the window that ran out. It is posted once per limit
//    (`LimitAnnouncements`: until the window resets, across relaunches, and
//    not at all when Codenotch's own "limit reached" came first): a retry,
//    wake-up or /loop tick that fails again, another session stopped by the
//    same limit, or a session reopened with its old failure post nothing.
//    Withdrawn while no session is stopped by it.
//  - Any other failed turn (overloaded, sign-in, billing, …) gets its own
//    kind of banner, `agentnotch.failed.<session id>`: "<title> stopped", with
//    what to do, never "needs you" (there is nothing to answer).
//  - Quiet completions (a turn waiting on background agents, a /loop or
//    cron tick) and completions from before launch are never announced.
//  - Nothing is posted while the user is looking at that session's own tab
//    or pane (a session-precise check, made once per session per burst).
//  - Clicking opens the sessions panel on that session; review banners also
//    offer "Mark Reviewed".
//  - Private: a banner names the session by its title (never the first
//    prompt) and says what it wants in the words of the hover rows (a tool
//    and a short input preview, a question's header), never Claude's
//    messages, questions or plans. Banners show on screen, on the lock
//    screen and in Notification Center; the panel is where text is read.
//  - Banners are silent: the notch chimes with Codenotch's own sounds.
//  - This is the app's only notification delegate. Codenotch's own
//    notifications (usage thresholds, limits) pass through untouched: not
//    presented while the app is frontmost and nothing done on a click,
//    exactly as without a delegate.
//

import AppKit
import Combine
import Foundation
import UserNotifications
import os.log

// MARK: - Content

/// Title and text of a session notification. Pure, so it is unit-tested.
nonisolated struct SessionNotificationContent: Equatable, Sendable {
    nonisolated enum Kind: String, Sendable, CaseIterable {
        case needsInput = "needsInput"
        case readyForReview = "review"
        /// A failed turn other than a usage limit (those share one banner
        /// per account).
        case failed = "failed"

        var categoryIdentifier: String {
            switch self {
            case .needsInput: return NotificationService.needsInputCategory
            case .readyForReview: return NotificationService.reviewCategory
            case .failed: return NotificationService.failedCategory
            }
        }
    }

    let kind: Kind
    let sessionId: String
    let title: String
    let subtitle: String?
    let body: String

    /// Stable per session and kind: posting again replaces the previous one.
    var identifier: String { Self.identifier(kind: kind, sessionId: sessionId) }

    /// Every identifier this app posts starts with this.
    nonisolated static let identifierPrefix = "agentnotch."

    nonisolated static func identifier(kind: Kind, sessionId: String) -> String {
        "\(identifierPrefix)\(kind.rawValue).\(sessionId)"
    }

    /// The kind and session of one of our per-session identifiers.
    nonisolated static func parse(identifier: String) -> (kind: Kind, sessionId: String)? {
        guard identifier.hasPrefix(identifierPrefix) else { return nil }
        let rest = identifier.dropFirst(identifierPrefix.count)
        guard let dot = rest.firstIndex(of: "."),
              let kind = Kind(rawValue: String(rest[..<dot])) else { return nil }
        let sessionId = String(rest[rest.index(after: dot)...])
        return sessionId.isEmpty ? nil : (kind, sessionId)
    }

    /// Whether a notification of `kind` still describes a session whose
    /// attention is now `attention` (nil: the session isn't known).
    nonisolated static func stillApplies(kind: Kind, attention: SessionAttention?) -> Bool {
        switch kind {
        case .needsInput: return attention?.bucket == .needsInput
        case .readyForReview: return attention == .readyForReview
        case .failed: return attention?.isError == true
        }
    }

    static let maxTitleLength = 60
    static let maxBodyLength = 220

    /// "<title> needs you", with why and what: the tool and its input for a
    /// permission, the question's header, a plan to approve, or the reason
    /// (with when the limit lifts, for a rate-limited turn).
    static func needsInput(
        session: SessionState,
        reason: NeedsInputReason,
        accountLabel: String?,
        limitReset: String? = nil
    ) -> SessionNotificationContent {
        SessionNotificationContent(
            kind: .needsInput,
            sessionId: session.sessionId,
            title: "\(truncated(ClaudeHostProjections.publicTitle(session), to: maxTitleLength)) needs you",
            subtitle: accountLabel,
            body: truncated(needsInputBody(session: session, reason: reason, limitReset: limitReset), to: maxBodyLength)
        )
    }

    /// "Done: <title>", "Ready for review · <project>", and how many
    /// background tasks still run. Claude's final message stays in the panel.
    static func readyForReview(session: SessionState, accountLabel: String?) -> SessionNotificationContent {
        var parts = ["Ready for review"]
        if let project = preview(session.displayProjectName) { parts.append(project) }
        let background = session.backgroundTaskCount
        if background > 0 {
            parts.append("\(background) background task\(background == 1 ? "" : "s") running")
        }
        return SessionNotificationContent(
            kind: .readyForReview,
            sessionId: session.sessionId,
            title: "Done: \(truncated(ClaudeHostProjections.publicTitle(session), to: maxTitleLength))",
            subtitle: accountLabel,
            body: truncated(parts.joined(separator: " · "), to: maxBodyLength)
        )
    }

    /// "<title> stopped", with why and what to do: "Overloaded · retry in
    /// its terminal", "Sign-in failed · run /login in its terminal".
    static func failed(session: SessionState, reason: NeedsInputReason, accountLabel: String?) -> SessionNotificationContent {
        var parts = [reason.displayText]
        if let hint = failureHint(session.stopErrorKind) { parts.append(hint) }
        return SessionNotificationContent(
            kind: .failed,
            sessionId: session.sessionId,
            title: "\(truncated(ClaudeHostProjections.publicTitle(session), to: maxTitleLength)) stopped",
            subtitle: accountLabel,
            body: truncated(parts.joined(separator: " · "), to: maxBodyLength)
        )
    }

    /// What the user can do about a failed turn.
    static func failureHint(_ kind: StopErrorKind?) -> String? {
        switch kind {
        case .overloaded, .serverError: return "retry in its terminal"
        case .authentication: return "run /login in its terminal"
        case .billing: return "check the account's billing"
        case .maxOutputTokens: return "ask Claude to go on in smaller steps"
        case .invalidRequest, .other, .rateLimit, nil: return nil
        }
    }

    private static func needsInputBody(session: SessionState, reason: NeedsInputReason, limitReset: String?) -> String {
        let permission = session.activePermission
        switch reason {
        case .permission(let tool):
            let toolName = permission?.toolName ?? tool
            let label = toolName.isEmpty ? reason.displayText : "Approve \(MCPToolFormatter.formatToolName(toolName))"
            if let input = preview(permission?.offPanelInput) {
                return "\(label): \(input)"
            }
            return label
        case .question:
            let questions = ChatQuestion.parse(toolInput: permission?.toolInput)
            guard !questions.isEmpty else { return reason.displayText }
            let more = questions.count > 1 ? " (+\(questions.count - 1) more)" : ""
            if let header = ClaudeHostProjections.questionHeader(permission?.toolInput) {
                return "Question · \(header)\(more)"
            }
            return "Question for you\(more)"
        case .planApproval:
            return "Plan ready for approval"
        case .elicitation, .dialog:
            return reason.displayText
        case .error:
            if let limitReset, ClaudeHostProjections.isRateLimit(reason) {
                return "\(reason.displayText) · \(limitReset)"
            }
            return reason.displayText
        }
    }

    /// Whitespace collapsed to single spaces; nil when empty.
    static func preview(_ text: String?) -> String? {
        guard let text else { return nil }
        let collapsed = text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
        return collapsed.isEmpty ? nil : collapsed
    }

    /// Shortens to `limit` characters with an ellipsis.
    static func truncated(_ text: String, to limit: Int) -> String {
        guard limit > 1, text.count > limit else { return text }
        return String(text.prefix(limit - 1)).trimmingCharacters(in: .whitespaces) + "…"
    }
}

/// The one banner for an account's sessions stopped by a usage limit. Pure.
nonisolated struct LimitNotificationContent: Equatable, Sendable {
    let ringID: String
    let title: String
    let body: String

    var identifier: String { Self.identifier(ringID: ringID) }

    static let prefix = SessionNotificationContent.identifierPrefix + "limit."

    static func identifier(ringID: String) -> String { prefix + ringID }

    /// The ring of one of our limit identifiers.
    static func parse(identifier: String) -> String? {
        guard identifier.hasPrefix(prefix) else { return nil }
        let ring = String(identifier.dropFirst(prefix.count))
        return ring.isEmpty ? nil : ring
    }

    /// "Work: 3 sessions hit the limit" / "Refactor the parser hit the
    /// limit", with when it lifts: "Rate limited · resets 14:05".
    static func make(ringID: String, accountLabel: String?, sessionTitles: [String], limitReset: String?) -> LimitNotificationContent {
        let count = sessionTitles.count
        let subject: String
        if count == 1, let only = sessionTitles.first {
            subject = SessionNotificationContent.truncated(only, to: SessionNotificationContent.maxTitleLength)
        } else {
            subject = "\(count) sessions"
        }
        let title = [accountLabel.map { "\($0): " }, "\(subject) hit the limit"].compactMap { $0 }.joined()
        let reason = NeedsInputReason.humanizedStopError("rate_limit")
        let body = limitReset.map { "\(reason) · \($0)" } ?? "\(reason). Claude Code waits for you once it lifts."
        return LimitNotificationContent(ringID: ringID, title: title, body: body)
    }
}

// MARK: - Routing

/// Which notifications are ours and what a click on one means. Pure.
nonisolated enum NotificationRouting {
    enum Response: Equatable, Sendable {
        /// Open the sessions panel on this session.
        case openSession(String)
        /// Open the sessions panel on this ring's list.
        case openRing(String)
        case markReviewed(String)
        /// Dismissed: nothing to do.
        case none
    }

    static func isOurs(identifier: String) -> Bool {
        identifier.hasPrefix(SessionNotificationContent.identifierPrefix)
    }

    /// Ours show as silent banners even while this (accessory) app is
    /// active; anybody else's are left as the system handles them without a
    /// delegate: not presented in the foreground.
    static func presentationOptions(forIdentifier identifier: String) -> UNNotificationPresentationOptions {
        isOurs(identifier: identifier) ? [.banner, .list] : []
    }

    /// What a click means; nil for a notification that isn't ours.
    static func response(identifier: String, actionIdentifier: String) -> Response? {
        guard isOurs(identifier: identifier) else { return nil }
        if actionIdentifier == UNNotificationDismissActionIdentifier { return Response.none }
        if let ring = LimitNotificationContent.parse(identifier: identifier) {
            return .openRing(ring)
        }
        guard let (_, sessionId) = SessionNotificationContent.parse(identifier: identifier) else { return Response.none }
        if actionIdentifier == NotificationService.markReviewedAction { return .markReviewed(sessionId) }
        return .openSession(sessionId)
    }
}

// MARK: - Service

@MainActor
final class NotificationService: NSObject, ObservableObject {
    static let shared = NotificationService()

    private static var logger: Logger { EngineLog.logger("Notifications") }

    nonisolated static let needsInputCategory = "agentnotch.session.needsInput"
    nonisolated static let reviewCategory = "agentnotch.session.review"
    nonisolated static let limitCategory = "agentnotch.account.limit"
    nonisolated static let failedCategory = "agentnotch.session.failed"
    nonisolated static let openAction = "agentnotch.open"
    nonisolated static let markReviewedAction = "agentnotch.markReviewed"
    nonisolated static let sessionIdKey = "sessionId"

    /// Transitions arriving within this long are handled together (one
    /// focus check per session, one limit banner per account).
    static let burstWindow: Duration = .milliseconds(150)

    /// The user turned the app's notifications off in System Settings.
    @Published private(set) var isAuthorizationDenied = false

    private var cancellables = Set<AnyCancellable>()
    private var started = false
    private var knownSessionIds: Set<String> = []
    private var pending: [AttentionTransition] = []
    private var flushTask: Task<Void, Never>?
    /// Rate-limited sessions per ring that the limit banner counted.
    private var limitedSessions: [String: Set<String>] = [:]

    private override init() {
        super.init()
    }

    /// UNUserNotificationCenter only works inside an app bundle (it throws
    /// for a bare `swift run` binary or the test runner). Sealed runs and
    /// `AGENTNOTCH_NO_NOTIFICATIONS=1` (see ClaudeControlConfiguration) turn it off.
    nonisolated static var isAvailable: Bool {
        if DevFlags.notificationsDisabled { return false }
        return Bundle.main.bundleIdentifier != nil && Bundle.main.bundleURL.pathExtension == "app"
    }

    // MARK: - Lifecycle

    /// Become the notification centre's delegate and follow attention changes. Idempotent.
    func start() {
        guard !started else { return }
        started = true
        guard Self.isAvailable else {
            Self.logger.info("Notifications unavailable (not an app bundle, or disabled for this run)")
            return
        }

        let center = UNUserNotificationCenter.current()
        center.delegate = self
        center.setNotificationCategories(Self.categories())

        if ClaudeControlSettings.notifyNeedsInput || ClaudeControlSettings.notifyReadyForReview {
            Task { _ = await self.ensureAuthorized() }
        }

        AttentionTracker.shared.transitions
            .sink { [weak self] transition in
                self?.enqueue(transition)
            }
            .store(in: &cancellables)

        // Sessions that end take their notifications with them.
        ClaudeSessionMonitor.shared.$instances
            .map { Set($0.map(\.sessionId)) }
            .removeDuplicates()
            .sink { [weak self] ids in
                self?.sessionsChanged(ids)
            }
            .store(in: &cancellables)

        // Notifications left in Notification Center by an earlier run are
        // withdrawn once sessions have been rediscovered, unless they still
        // apply (the transitions above only cover changes seen this run).
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.launchReconcileDelay) { [weak self] in
            self?.withdrawStaleNotifications()
        }
    }

    func stop() {
        cancellables.removeAll()
        flushTask?.cancel()
        flushTask = nil
        pending.removeAll()
        started = false
    }

    /// Long enough for the registry scan and hook events to bring back the
    /// sessions that were running before launch.
    private static let launchReconcileDelay: TimeInterval = 10

    private static func categories() -> Set<UNNotificationCategory> {
        let open = UNNotificationAction(identifier: openAction, title: "Open", options: [.foreground])
        let markReviewed = UNNotificationAction(identifier: markReviewedAction, title: "Mark Reviewed", options: [])
        return [
            UNNotificationCategory(identifier: needsInputCategory, actions: [open], intentIdentifiers: [], options: []),
            UNNotificationCategory(identifier: reviewCategory, actions: [open, markReviewed], intentIdentifiers: [], options: []),
            UNNotificationCategory(identifier: limitCategory, actions: [open], intentIdentifiers: [], options: []),
            UNNotificationCategory(identifier: failedCategory, actions: [open], intentIdentifiers: [], options: []),
        ]
    }

    // MARK: - Transitions

    private func enqueue(_ transition: AttentionTransition) {
        pending.append(transition)
        guard flushTask == nil else { return }
        flushTask = Task { [weak self] in
            try? await Task.sleep(for: Self.burstWindow)
            guard !Task.isCancelled else { return }
            await self?.flush()
        }
    }

    /// Handle a burst: withdraw what no longer applies, then post what is
    /// new, checking each session's focus once.
    private func flush() async {
        let burst = pending
        pending.removeAll()
        flushTask = nil

        var focused: [String: Bool] = [:]
        func isFocused(_ session: SessionState) async -> Bool {
            if let known = focused[session.sessionId] { return known }
            var result = false
            if let probe = TerminalVisibilityDetector.probe(for: session) {
                result = await TerminalVisibilityDetector.isSessionFocused(probe)
            }
            focused[session.sessionId] = result
            return result
        }

        var limitRings: Set<String> = []
        for transition in burst {
            let session = transition.session
            let sessionId = session.sessionId

            if transition.from?.bucket == .needsInput && transition.to.bucket != .needsInput {
                remove(.needsInput, sessionId: sessionId)
            }
            if transition.from?.isError == true && !transition.to.isError {
                remove(.failed, sessionId: sessionId)
            }
            if transition.from == .readyForReview && transition.to != .readyForReview {
                remove(.readyForReview, sessionId: sessionId)
            }
            if let reason = transition.from?.needsInputReason, ClaudeHostProjections.isRateLimit(reason),
               transition.to.needsInputReason.map(ClaudeHostProjections.isRateLimit) != true {
                limitRings.insert(ringID(for: session))
            }
            // An account whose tracking was just switched off keeps its
            // sessions until they end; they don't announce anything.
            if let accountId = session.accountId,
               AccountRegistry.shared.account(id: accountId)?.isHidden == true || AccountRegistry.shared.isForgotten(accountId) {
                continue
            }

            if transition.becameNeedsInput, ClaudeControlSettings.notifyNeedsInput, let reason = transition.to.needsInputReason {
                // A failure read back from disk was announced when it happened.
                if reason.isError, session.stopErrorIsRestored { continue }
                if ClaudeHostProjections.isRateLimit(reason) {
                    // One banner per account, below.
                    limitRings.insert(ringID(for: session))
                    continue
                }
                guard !(await isFocused(session)) else { continue }
                // A failed turn has nothing to answer: its own banner.
                let content = transition.isFailure
                    ? SessionNotificationContent.failed(session: session, reason: reason,
                                                        accountLabel: accountLabel(for: session))
                    : SessionNotificationContent.needsInput(session: session, reason: reason,
                                                            accountLabel: accountLabel(for: session))
                await post(content, expecting: transition.to)
            }
            // The tracker already holds back quiet and pre-launch
            // completions; checked here too so no path can announce one.
            if transition.becameReadyForReview, ClaudeControlSettings.notifyReadyForReview, !session.completionIsQuiet {
                guard !(await isFocused(session)) else { continue }
                let content = SessionNotificationContent.readyForReview(
                    session: session,
                    accountLabel: accountLabel(for: session)
                )
                await post(content, expecting: transition.to)
            }
        }
        for ring in limitRings.sorted() {
            await updateLimitNotification(ringID: ring)
        }
    }

    /// The ring the hub shows the session on: its account by attribution (a
    /// session that started while a mirrored folder named another account,
    /// one Claude Desktop runs as its own), as the notch's chime and
    /// Codenotch's limit card key it; the folder's account until the hub has
    /// placed it.
    private func ringID(for session: SessionState) -> String {
        ClaudeControlHub.shared?.limitPlacement(sessionId: session.sessionId)?.ringID
            ?? ClaudeHostProjections.ringID(for: session, home: AppIdentity.homeDirectory)
    }

    /// The account's name, only when several accounts are in use; with the
    /// organization or plan when another visible account has the same name
    /// (the same email in two organizations, say), so the two can be told
    /// apart without their colours.
    private func accountLabel(for session: SessionState) -> String? {
        let registry = AccountRegistry.shared
        let folderId = session.accountId ?? AccountPaths.defaultConfigDir
        // One account per signed-in identity, whatever folder the session runs
        // in: the one the hub shows it under (by attribution), else the
        // folder's current one.
        let placed = ClaudeControlHub.shared?.limitPlacement(sessionId: session.sessionId)
            .flatMap { placement in registry.identities.first { $0.ringID == placement.ringID } }
        if let identity = placed ?? registry.identity(forFolderId: folderId) {
            let visible = registry.visibleIdentities
            let labels = Dictionary(visible.map { ($0.id, displayLabel($0.id, fallback: $0.label)) },
                                    uniquingKeysWith: { first, _ in first })
            var account = identity.representative
            account.customLabel = nil
            return Self.subtitle(id: identity.id, account: account,
                                 label: displayLabel(identity.id, fallback: identity.label), allLabels: labels)
        }
        guard let account = registry.account(id: folderId) else { return nil }
        let visible = registry.visibleAccounts
        let labels = Dictionary(visible.map { ($0.id, displayLabel($0)) }, uniquingKeysWith: { first, _ in first })
        return Self.subtitle(for: account, label: displayLabel(account), allLabels: labels)
    }

    private func displayLabel(_ accountId: String, fallback: String) -> String {
        ClaudeControlHub.shared?.displayLabel(forAccountId: accountId) ?? fallback
    }

    private func displayLabel(_ account: ClaudeAccount) -> String {
        ClaudeControlHub.shared?.displayLabel(forAccountId: account.id) ?? account.label
    }

    /// Nil for a single account; the label, plus the organization or plan
    /// when it collides with another visible account's. Pure.
    nonisolated static func subtitle(for account: ClaudeAccount, label: String, allLabels: [String: String]) -> String? {
        subtitle(id: account.id, account: account, label: label, allLabels: allLabels)
    }

    /// `subtitle(for:…)` for an account whose id in `allLabels` is `id` (an identity's).
    nonisolated static func subtitle(id: String, account: ClaudeAccount, label: String, allLabels: [String: String]) -> String? {
        guard allLabels.count > 1 else { return nil }
        let collides = allLabels.contains { otherId, other in
            otherId != id && other.caseInsensitiveCompare(label) == .orderedSame
        }
        guard collides else { return label }
        if let organization = account.organizationName, !organization.isEmpty {
            return "\(label) · \(organization)"
        }
        if let plan = account.planName {
            return "\(label) · \(plan)"
        }
        return "\(label) · \(AccountPaths.shortName(forConfigDir: account.configDir))"
    }

    /// Post or withdraw one account's limit banner. It is posted when a
    /// session is newly stopped by the limit (not one restored with an old
    /// failure) and the limit hasn't been announced yet; it is never re-added
    /// for a changed count (macOS would show it again); zero withdraws it.
    private func updateLimitNotification(ringID: String) async {
        let registry = AccountRegistry.shared
        let limited = ClaudeSessionMonitor.shared.instances.filter { session in
            self.ringID(for: session) == ringID
                && session.attention.needsInputReason.map(ClaudeHostProjections.isRateLimit) == true
                && session.accountId.flatMap(registry.account(id:))?.isHidden != true
                && !(session.accountId.map(registry.isForgotten) ?? false)
        }
        let ids = Set(limited.map(\.sessionId))
        let before = limitedSessions[ringID] ?? []
        limitedSessions[ringID] = ids.isEmpty ? nil : ids
        let identifier = LimitNotificationContent.identifier(ringID: ringID)
        guard !ids.isEmpty else {
            removeDelivered([identifier])
            return
        }
        let restored = Set(limited.filter(\.stopErrorIsRestored).map(\.sessionId))
        guard !ids.subtracting(before).subtracting(restored).isEmpty,
              ClaudeControlSettings.notifyNeedsInput else { return }

        // The used-up window of the account the hub placed the sessions with;
        // the folder's account's until it has.
        let placed = limited.lazy.compactMap { ClaudeControlHub.shared?.limitPlacement(sessionId: $0.sessionId) }.first
        let hit = placed.map(\.limitHit) ?? limited.first?.accountId
            .map { registry.identityId(for: $0) ?? $0 }
            .flatMap { UsageStore.shared.usage[$0]?.announcedLimitHit() }
        // Claimed only for a banner macOS will show: otherwise the limit is
        // left to Codenotch's card in the notch, which needs no permission.
        guard await ensureAuthorized(),
              await UNUserNotificationCenter.current().notificationSettings().alertStyle != .none else { return }
        // Once per limit, whichever announcement came first.
        guard LimitAnnouncementStore.shared.claim(.notification, ring: ringID, window: hit?.window,
                                                  resetsAt: hit?.resetsAt) else { return }
        let reset = ClaudeHostProjections.resetPhrase(hit?.resetsAt, now: Date())
        let label = limited.first.flatMap(accountLabel(for:))
        let content = LimitNotificationContent.make(
            ringID: ringID,
            accountLabel: label,
            sessionTitles: limited.map(ClaudeHostProjections.publicTitle),
            limitReset: reset
        )
        let notification = UNMutableNotificationContent()
        notification.title = content.title
        notification.body = content.body
        notification.threadIdentifier = identifier
        notification.categoryIdentifier = Self.limitCategory
        notification.interruptionLevel = .active
        notification.sound = nil
        await deliver(UNNotificationRequest(identifier: identifier, content: notification, trigger: nil))
    }

    private func post(_ content: SessionNotificationContent, expecting attention: SessionAttention) async {
        guard await ensureAuthorized() else { return }
        // Things may have moved on while we checked.
        guard AttentionTracker.shared.attention(for: content.sessionId) == attention else { return }

        let notification = UNMutableNotificationContent()
        notification.title = content.title
        if let subtitle = content.subtitle {
            notification.subtitle = subtitle
        }
        notification.body = content.body
        notification.threadIdentifier = content.sessionId
        notification.categoryIdentifier = content.kind.categoryIdentifier
        notification.userInfo = [Self.sessionIdKey: content.sessionId]
        notification.interruptionLevel = .active
        notification.sound = nil
        await deliver(UNNotificationRequest(identifier: content.identifier, content: notification, trigger: nil))
        // Answered or reviewed while the banner was on its way (a later
        // burst withdrew it before it existed): take it back.
        let now = AttentionTracker.shared.attention(for: content.sessionId)
        if !SessionNotificationContent.stillApplies(kind: content.kind, attention: now) {
            removeDelivered([content.identifier])
        }
    }

    private func deliver(_ request: UNNotificationRequest) async {
        guard await ensureAuthorized() else { return }
        do {
            try await UNUserNotificationCenter.current().add(request)
        } catch {
            Self.logger.error("Posting a notification failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    private func remove(_ kind: SessionNotificationContent.Kind, sessionId: String) {
        removeDelivered([SessionNotificationContent.identifier(kind: kind, sessionId: sessionId)])
    }

    private func removeDelivered(_ identifiers: [String]) {
        guard !identifiers.isEmpty else { return }
        let center = UNUserNotificationCenter.current()
        center.removePendingNotificationRequests(withIdentifiers: identifiers)
        center.removeDeliveredNotifications(withIdentifiers: identifiers)
    }

    private func sessionsChanged(_ ids: Set<String>) {
        let gone = knownSessionIds.subtracting(ids)
        knownSessionIds = ids
        guard !gone.isEmpty else { return }
        removeDelivered(gone.flatMap { id in
            SessionNotificationContent.Kind.allCases.map { SessionNotificationContent.identifier(kind: $0, sessionId: id) }
        })
        let rings = limitedSessions.filter { !$0.value.isDisjoint(with: gone) }.map(\.key)
        Task {
            for ring in rings.sorted() { await updateLimitNotification(ringID: ring) }
        }
    }

    private func withdrawStaleNotifications() {
        Task {
            let center = UNUserNotificationCenter.current()
            let delivered = await center.deliveredNotifications()
            let limitedRings = Set(ClaudeSessionMonitor.shared.instances
                .filter { $0.attention.needsInputReason.map(ClaudeHostProjections.isRateLimit) == true }
                .map { self.ringID(for: $0) })
            let stale = delivered.map(\.request.identifier).filter { identifier in
                if let ring = LimitNotificationContent.parse(identifier: identifier) {
                    return !limitedRings.contains(ring)
                }
                guard let (kind, sessionId) = SessionNotificationContent.parse(identifier: identifier) else { return false }
                return !SessionNotificationContent.stillApplies(kind: kind, attention: AttentionTracker.shared.attention(for: sessionId))
            }
            guard !stale.isEmpty else { return }
            Self.logger.info("Withdrawing \(stale.count) notification(s) that no longer apply")
            center.removeDeliveredNotifications(withIdentifiers: stale)
        }
    }

    // MARK: - Authorization

    /// Re-read whether notifications are allowed (for the settings hint).
    func refreshAuthorizationStatus() {
        guard Self.isAvailable else { return }
        Task {
            let settings = await UNUserNotificationCenter.current().notificationSettings()
            isAuthorizationDenied = settings.authorizationStatus == .denied
        }
    }

    /// System Settings › Notifications, at this app's entry.
    func openSystemSettings() {
        let bundleId = Bundle.main.bundleIdentifier ?? AppIdentity.bundleIdentifier
        let candidates = [
            "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=\(bundleId)",
            "x-apple.systempreferences:com.apple.preference.notifications",
        ]
        for candidate in candidates {
            if let url = URL(string: candidate), NSWorkspace.shared.open(url) {
                return
            }
        }
    }

    /// Asks for permission the first time it's needed; true when allowed.
    private func ensureAuthorized() async -> Bool {
        guard Self.isAvailable else { return false }
        let center = UNUserNotificationCenter.current()
        let settings = await center.notificationSettings()
        switch settings.authorizationStatus {
        case .authorized, .provisional, .ephemeral:
            isAuthorizationDenied = false
            return true
        case .denied:
            isAuthorizationDenied = true
            return false
        case .notDetermined:
            do {
                // Banners only: the chime is Codenotch's.
                let granted = try await center.requestAuthorization(options: [.alert])
                isAuthorizationDenied = !granted
                return granted
            } catch {
                Self.logger.error("Notification authorization failed: \(error.localizedDescription, privacy: .public)")
                return false
            }
        @unknown default:
            return false
        }
    }

    // MARK: - Responses

    fileprivate func handle(_ response: NotificationRouting.Response) {
        switch response {
        case .markReviewed(let sessionId):
            ClaudeSessionMonitor.shared.markReviewed(sessionId: sessionId)
        case .openSession(let sessionId):
            AppEventBus.shared.panelRequests.send(.session(id: sessionId))
        case .openRing(let ringID):
            if let hub = ClaudeControlHub.shared {
                hub.requestPanel(.sessions(ringID: ringID))
            } else {
                AppEventBus.shared.panelRequests.send(.sessions)
            }
        case .none:
            break
        }
    }
}

// MARK: - UNUserNotificationCenterDelegate

extension NotificationService: UNUserNotificationCenterDelegate {
    /// Ours show as silent banners even while this accessory app is active;
    /// anybody else's are not presented in the foreground (the system's
    /// behaviour without a delegate).
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        NotificationRouting.presentationOptions(forIdentifier: notification.request.identifier)
    }

    /// Ours open the panel or mark reviewed; anybody else's are completed
    /// with nothing done.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        guard let routed = NotificationRouting.response(
            identifier: response.notification.request.identifier,
            actionIdentifier: response.actionIdentifier
        ) else { return }
        await MainActor.run {
            NotificationService.shared.handle(routed)
        }
    }
}
