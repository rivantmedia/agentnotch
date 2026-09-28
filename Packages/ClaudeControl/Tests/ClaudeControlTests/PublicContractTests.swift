// A plain import on purpose: this file sees only what the app's bridge sees.
// If a §12 contract member stops being public, or changes signature, this
// stops compiling. Nothing here runs the engine (no bootstrap).
import ClaudeControl
import Combine
import CoreGraphics
import Foundation
import SwiftUI
import Testing

@MainActor
struct PublicContractTests {
    @Test func configurationSurface() {
        let live: (String, String, String, [String: String], [String]) -> ClaudeControlConfiguration =
            ClaudeControlConfiguration.live(appDisplayName:bundleIdentifier:supportFolderName:environment:arguments:)
        let sealed: (String, String) -> ClaudeControlConfiguration = ClaudeControlConfiguration.sealed
        var configuration = sealed("Test", "com.example.test")
        #expect(configuration.mode == .sealed)
        #expect(!configuration.installsAllowed && !configuration.notificationsAllowed && !configuration.probesAllowed)
        configuration.externalTabFocus = { (_: ClaudeExternalTabRequest) in false }
        configuration.externalUsageSource = nil
        configuration.websiteURL = nil
        let _: [String] = configuration.extraConfigDirs
        let _: String? = configuration.websiteURL
        let _: String = ClaudeControlConfiguration.websiteURLInfoKey
        let _: ([String: Any]?) -> String? = ClaudeControlConfiguration.websiteURL(infoDictionary:)
        let _: UserDefaults = configuration.defaults
        let _: (URL, String, String, String, String) = (configuration.supportDirectory, configuration.socketPath,
                                                        configuration.homeDirectory, configuration.hookScriptName,
                                                        configuration.statusLineScriptName)
        let other = live("Test", "com.example.test", "Test", ["HOME": "/tmp/agentnotch-contract-home"], [])
        #expect(other.mode == .live)
        let _ = ClaudeExternalUsageReading(windows: [], observedAt: Date())
    }

    @Test func hubSurface() {
        let _: (ClaudeControlConfiguration) -> ClaudeControlHub = ClaudeControlHub.bootstrap
        let _: KeyPath<ClaudeControlHub, [ClaudeAccountSummary]> = \.accounts
        let _: KeyPath<ClaudeControlHub, [ClaudeSessionSummary]> = \.sessions
        let _: KeyPath<ClaudeControlHub, [String: ClaudeAttentionCounts]> = \.ringCounts
        let _: KeyPath<ClaudeControlHub, ClaudeAttentionCounts> = \.totalCounts
        let _: KeyPath<ClaudeControlHub, [String: ClaudeRingReading]> = \.ringReadings
        let _: KeyPath<ClaudeControlHub, [String: Date]> = \.freshSuccessUntil
        let _: KeyPath<ClaudeControlHub, ClaudeSetupState> = \.setup
        let _: KeyPath<ClaudeControlHub, Bool> = \.isBusy
        let _: KeyPath<ClaudeControlHub, AnyPublisher<ClaudeAttentionTransition, Never>> = \.transitions
        let _: KeyPath<ClaudeControlHub, AnyPublisher<ClaudePanelRoute, Never>> = \.panelRequests
        let _: (ClaudeControlHub) -> () -> [ClaudeAccountSummary] = ClaudeControlHub.launchAccounts
        let _: (ClaudeControlHub) -> () -> Void = ClaudeControlHub.start
        let _: (ClaudeControlHub) -> () -> Void = ClaudeControlHub.stop
        let _: (ClaudeControlHub) -> (String, Date) -> [ClaudeActivityRow] = ClaudeControlHub.activityRows(ringID:now:)
        let _: (ClaudeControlHub) -> (Int32) -> ClaudeSessionSummary? = ClaudeControlHub.session(pid:)
        let _: (ClaudeControlHub) -> (String) async -> Bool = ClaudeControlHub.focus(sessionId:)
        let _: (ClaudeControlHub) -> (String) -> Void = ClaudeControlHub.markReviewed(sessionId:)
        let _: (ClaudeControlHub) -> (String) async -> Bool = ClaudeControlHub.isTerminalFocused(sessionId:)
        let _: (ClaudeControlHub) -> () async -> Bool = ClaudeControlHub.isAnyTerminalVisible
        let _: (ClaudeControlHub) -> (ClaudePanelRoute) -> Void = ClaudeControlHub.requestPanel
        let _: (ClaudeControlHub) -> (String, ClaudeRefreshReason) async -> Void = ClaudeControlHub.refreshUsage(ringID:reason:)
        let _: (ClaudeControlHub) -> (Set<String>) -> Void = ClaudeControlHub.setShownRings
        let _: (ClaudeControlHub) -> ([String: String]) -> Void = ClaudeControlHub.setNicknames
    }

    @Test func summariesSurface() {
        let account = ClaudeAccountSummary(id: "/h/.claude", ringID: "claude", configDir: "/h/.claude", label: "me",
                                           email: nil, planName: "Max", organizationUuid: nil, isDefault: true,
                                           isTracked: true, colorIndex: 0, hooks: ClaudeHookStatus(), launchCommand: "claude")
        let session = ClaudeSessionSummary(
            id: "s", ringID: account.ringID, pid: 1, title: "t", projectName: "p", hostApp: nil,
            attention: .needsInput(ClaudeNeedsInput(kind: .permission, summary: "Allow Bash")),
            attentionSince: Date(), tasks: ClaudeTaskProgress(completed: 1, total: 2, active: nil),
            contextPercent: 42, backgroundTasks: 0)
        let counts = ClaudeAttentionCounts(needsYou: 1, review: 0, working: 0, idle: 0)
        #expect(ClaudeAttentionCounts.of([session]) == counts)
        let _ = ClaudeAttentionTransition(kind: .needsInput, session: session)
        let _ = ClaudeSetupState(needsHookConsent: true, vibeNotchRunning: false, legacyHooksFound: false, socketError: nil)
        let routes: [ClaudePanelRoute] = [.sessions(ringID: nil), .session(id: "s"), .setup]
        #expect(routes.count == 3)
        let _: [ClaudeRefreshReason] = [.ringClick, .forced]
        #expect(ClaudeRingIdentity.ringID(configDir: "/h/.claude", home: "/h") == "claude")
        #expect(ClaudeRingIdentity.ringID(configDir: "/h/.claude-work", home: "/h") == "claude-work")
        #expect(ClaudeRingIdentity.ringID(configDir: "/elsewhere/claude", home: "/h").hasPrefix("claude-dir-"))
        let window = ClaudeRingReading.Window(id: "extra_usage", label: nil, usedFraction: 0.2, resetsAt: nil, duration: nil,
                                              money: .init(currency: "USD", spent: 1, remaining: 4))
        let statuses: [ClaudeRingReading.Status] = [.ok, .waitingForFirstReading, .signInNeeded("x"), .unavailable("x"), .failed("x")]
        let reading = ClaudeRingReading(windows: [window], plan: "Max", updatedAt: Date(), status: statuses[0])
        #expect(reading.windows.first?.money?.remaining == 4)
        let row = ClaudeActivityRow(id: "s", name: "t", detail: "d", state: .waiting, waitingFor: "Allow Bash", since: Date(), pid: 1)
        #expect(row.state == .waiting)
    }

    @Test func geometrySurface() {
        let chrome = ClaudePanelChrome(tailLength: 8, tailWidth: 16, corner: 18)
        let anchor = ClaudePanelAnchor(edge: .left, notchWindowFrame: CGRect(x: 0, y: 300, width: 60, height: 300),
                                       ringAlong: 100, tailTipInset: 10, visibleFrame: CGRect(x: 0, y: 0, width: 1512, height: 944))
        let placement: ClaudePanelPlacement = ClaudePanelGeometry.place(
            anchor: anchor, mode: ClaudePanelMode.list, idealContentHeight: 300, chrome: chrome,
            fallbackVisibleFrame: anchor.visibleFrame)
        #expect(placement.hasTail && placement.cardRect.minX >= anchor.notchWindowFrame.minX)
        let _: CGPoint = ClaudeRingBadgeLayout.center(ClaudeRingBadgeSlot.needsYou, edge: ClaudePanelEdge.right,
                                                      compact: false, ringDiameter: 30)
    }

    @Test func uiSurface() {
        let theme: ClaudeControlTheme = .codenotchDark
        let _ = Text("x").claudeControlTheme(theme)
        let state = ClaudePanelState(route: .sessions(ringID: "claude"))
        #expect(state.ringFilter == "claude" && state.mode == .list)
        state.route = .session(id: "s")
        #expect(state.mode == .chat)
        state.isPinned = true
        state.contentWidth = 520
        let _: CGFloat = state.idealContentHeight
        state.onClose = {}; state.onRequestKey = {}; state.onOpenSettings = {}; state.onJumped = {}
        let _: (ClaudeControlHub, ClaudePanelState) -> ClaudeSessionsPanel = ClaudeSessionsPanel.init(hub:state:)
        let _: (ClaudeControlHub, ClaudeSettingsHost) -> ClaudeSettingsPane = ClaudeSettingsPane.init(hub:host:)
        let _ = SealedModeBadge()
        let _: any ObservableObject.Type = ClaudePanelState.self
        let host: ClaudeSettingsHost = ContractSettingsHost()
        #expect(host.isRingShown("claude"))
        // The browser step of the website sign-in is the app's.
        let _: (any ClaudeSettingsHost) -> (URL) async throws -> URL = { host in host.presentWebsiteSignIn }
        // ...and optional: a host written before it existed still conforms.
        let older: ClaudeSettingsHost = OlderSettingsHost()
        #expect(older.isRingShown("claude"))
    }

    /// Regression (M4): `presentWebsiteSignIn(_:)` was added to the
    /// protocol after hosts existed. A host that doesn't implement it gets a
    /// default that fails the sign-in with `ClaudeWebsiteSignInUnavailable`
    /// (saying why), while one that does keeps its own.
    @Test func theWebsiteSignInStepIsOptional() async throws {
        let url = try #require(URL(string: "https://example.com/auth"))
        let older: ClaudeSettingsHost = OlderSettingsHost()
        await #expect(throws: ClaudeWebsiteSignInUnavailable()) {
            _ = try await older.presentWebsiteSignIn(url)
        }
        #expect(ClaudeWebsiteSignInUnavailable().errorDescription == "This app can't open the website's sign-in.")
        func sendable<T: Sendable>(_: T.Type) {}
        sendable(ClaudeWebsiteSignInUnavailable.self)
        let current: ClaudeSettingsHost = ContractSettingsHost()
        #expect(try await current.presentWebsiteSignIn(url) == url)
    }

    /// The website side of the hub: sign-in, the sync and summary switches,
    /// syncing now, and the state the settings pane shows.
    @Test func cloudSurface() {
        let _: KeyPath<ClaudeControlHub, ClaudeCloudState> = \.cloud
        let _: KeyPath<ClaudeControlHub, URL?> = \.cloudDashboardURL
        let _: KeyPath<ClaudeControlHub, URL?> = \.cloudPoolsURL
        let _: KeyPath<ClaudeControlHub, URL?> = \.cloudSettingsURL
        let _: (ClaudeControlHub) -> (@escaping ClaudeCloudBrowser) async -> Bool = ClaudeControlHub.cloudSignIn(presentingBrowser:)
        let _: (ClaudeControlHub) -> () async -> Void = ClaudeControlHub.cloudSignOut
        let _: (ClaudeControlHub) -> (Bool) -> Void = ClaudeControlHub.setCloudSync
        let _: (ClaudeControlHub) -> (Bool) -> Void = ClaudeControlHub.setSessionSummaries
        let _: (ClaudeControlHub) -> () async -> Void = ClaudeControlHub.syncCloudNow
        let browser: ClaudeCloudBrowser = { url in url }
        _ = browser

        let empty = ClaudeCloudState()
        #expect(empty.auth == .signedOut && !empty.syncEnabled && !empty.summariesEnabled && empty.websiteURL == nil)
        let state = ClaudeCloudState(websiteURL: "https://example.com", websiteIsOverridden: false,
                                     auth: .signedIn(email: "me@example.com"), syncEnabled: true, summariesEnabled: false,
                                     summariesAvailable: true, isSyncing: false, lastSyncAt: Date(), lastError: nil,
                                     pendingSessions: 1, pendingUsage: 2, summarizedSessions: 3,
                                     dashboardURL: URL(string: "https://example.com/dashboard"))
        let _: (String?, Bool, ClaudeCloudState.Auth, Bool, Bool, Bool, Bool, Date?, String?, Int, Int, Int, URL?) =
            (state.websiteURL, state.websiteIsOverridden, state.auth, state.syncEnabled, state.summariesEnabled,
             state.summariesAvailable, state.isSyncing, state.lastSyncAt, state.lastError, state.pendingSessions,
             state.pendingUsage, state.summarizedSessions, state.dashboardURL)
        let _: (Bool, String?, URL?) = (state.isSignedIn, state.email, state.poolsURL)
        #expect(state.poolsURL?.absoluteString == "https://example.com/dashboard/pools")
        #expect(state.settingsURL?.absoluteString == "https://example.com/settings")
        func auth(_ v: ClaudeCloudState.Auth) -> Int {
            switch v { case .signedOut: return 0; case .signingIn: return 1; case .signedIn(email: _): return 2; case .error(let text): return text.count }
        }
        #expect(auth(.signedIn(email: nil)) == 2)

        // The website is the build's (`ClaudeControlConfiguration.websiteURL`), not a setting.
        let _: (String, String, String) = (ClaudeControlSettings.Key.cloudSyncEnabled,
                                           ClaudeControlSettings.Key.cloudSummariesEnabled, ClaudeControlSettings.Key.cloudDeviceId)
        let _: (Bool, Bool) = (ClaudeControlSettings.cloudSyncEnabled, ClaudeControlSettings.cloudSummariesEnabled)
        func sendable<T: Sendable>(_: T.Type) {}
        func hashable<T: Hashable>(_: T.Type) {}
        hashable(ClaudeCloudState.self); sendable(ClaudeCloudState.self)
        hashable(ClaudeCloudState.Auth.self); sendable(ClaudeCloudState.Auth.self)
    }

    /// Every stored field of the §12 value types, by name and type, and every
    /// configuration field settable (`public var`).
    @Test func fieldsAndMutability() {
        var c = ClaudeControlConfiguration.sealed(appDisplayName: "T", bundleIdentifier: "com.example.t")
        c.mode = .live; c.appDisplayName = "T"; c.bundleIdentifier = "com.example.t"
        c.supportDirectory = URL(fileURLWithPath: "/tmp/agentnotch-contract"); c.socketPath = "/tmp/agentnotch-contract.sock"
        c.homeDirectory = "/tmp/agentnotch-contract-home"; c.hookScriptName = "h.py"; c.statusLineScriptName = "s.py"
        c.defaults = .standard; c.installsAllowed = false; c.notificationsAllowed = false; c.probesAllowed = false
        c.extraConfigDirs = []; c.externalTabFocus = nil; c.externalUsageSource = ContractUsageSource()
        c.websiteURL = "https://agentnotch.example.com"
        #expect(c.mode == .live)

        let tab = ClaudeExternalTabRequest(bundleID: nil, pid: 1, tty: nil, cwd: nil)
        let _: (String?, Int32, String?, String?) = (tab.bundleID, tab.pid, tab.tty, tab.cwd)
        let external = ClaudeExternalUsageReading(windows: [], observedAt: Date())
        let _: ([ClaudeRingReading.Window], Date) = (external.windows, external.observedAt)

        let a = ClaudeAccountSummary(id: "/h/.claude", ringID: "claude", configDir: "/h/.claude", label: "me",
                                     isDefault: true, launchCommand: "claude")
        let _: (String, String, String, String, String?, String?, String?, Bool, Bool, Int, ClaudeHookStatus, String) =
            (a.id, a.ringID, a.configDir, a.label, a.email, a.planName, a.organizationUuid, a.isDefault, a.isTracked,
             a.colorIndex, a.hooks, a.launchCommand)

        let needs = ClaudeNeedsInput(kind: .question, summary: "Question")
        let _: (ClaudeNeedsInput.Kind, String) = (needs.kind, needs.summary)
        let tasks = ClaudeTaskProgress(completed: 1, total: 3, active: "Writing tests")
        let _: (Int, Int, String?) = (tasks.completed, tasks.total, tasks.active)
        let s = ClaudeSessionSummary(id: "s", ringID: "claude", title: "t", projectName: "p", attention: .working,
                                     attentionSince: Date())
        let _: (String, String, Int32?, String, String, String?, ClaudeAttention, Date, ClaudeTaskProgress?, Double?, Int) =
            (s.id, s.ringID, s.pid, s.title, s.projectName, s.hostApp, s.attention, s.attentionSince, s.tasks,
             s.contextPercent, s.backgroundTasks)
        let counts = ClaudeAttentionCounts()
        let _: (Int, Int, Int, Int) = (counts.needsYou, counts.review, counts.working, counts.idle)
        let transition = ClaudeAttentionTransition(kind: .resolved, session: s)
        let _: (ClaudeAttentionTransition.Kind, ClaudeSessionSummary) = (transition.kind, transition.session)
        let setup = ClaudeSetupState()
        let _: (Bool, Bool, Bool, String?) = (setup.needsHookConsent, setup.vibeNotchRunning, setup.legacyHooksFound,
                                              setup.socketError)

        let w = ClaudeRingReading.Window(id: "session", usedFraction: 0.5)
        let _: (String, String?, Double, Date?, TimeInterval?, ClaudeRingReading.Window.Money?) =
            (w.id, w.label, w.usedFraction, w.resetsAt, w.duration, w.money)
        let reading = ClaudeRingReading(status: .waitingForFirstReading)
        let _: ([ClaudeRingReading.Window], String?, Date?, ClaudeRingReading.Status) =
            (reading.windows, reading.plan, reading.updatedAt, reading.status)
        let row = ClaudeActivityRow(id: "s", name: "t", detail: "d", state: .busy, since: Date())
        let _: (String, String, String, ClaudeActivityRow.State, String?, Date, Int32?) =
            (row.id, row.name, row.detail, row.state, row.waitingFor, row.since, row.pid)

        let chrome = ClaudePanelChrome(tailLength: 8, tailWidth: 16, corner: 18)
        #expect(chrome.margin == 8)
        let _: (CGFloat, CGFloat, CGFloat, CGFloat) = (chrome.tailLength, chrome.tailWidth, chrome.corner, chrome.margin)
        let anchor = ClaudePanelAnchor(edge: .top, notchWindowFrame: .zero, ringAlong: 0, tailTipInset: 0, visibleFrame: .zero)
        let _: (ClaudePanelEdge, CGRect, CGFloat, CGFloat, CGRect) =
            (anchor.edge, anchor.notchWindowFrame, anchor.ringAlong, anchor.tailTipInset, anchor.visibleFrame)
        let place: (ClaudePanelAnchor?, ClaudePanelMode, CGFloat, ClaudePanelChrome, CGRect) -> ClaudePanelPlacement =
            ClaudePanelGeometry.place(anchor:mode:idealContentHeight:chrome:fallbackVisibleFrame:)
        let p = place(nil, .chat, 300, chrome, CGRect(x: 0, y: 0, width: 1512, height: 944))
        let _: (CGRect, CGRect, CGFloat, Bool, CGFloat, CGFloat) =
            (p.windowFrame, p.cardRect, p.tailOffset, p.hasTail, p.contentWidth, p.maxContentHeight)
        let _: (ClaudeRingBadgeSlot, ClaudePanelEdge, Bool, CGFloat) -> CGPoint =
            ClaudeRingBadgeLayout.center(_:edge:compact:ringDiameter:)
        let _: (String, String) -> String = ClaudeRingIdentity.ringID(configDir:home:)
        let _: () -> ClaudeControlHub? = { ClaudeControlHub.shared }
        let _: any ObservableObject.Type = ClaudeControlHub.self
    }

    /// The conformances §12 promises.
    @Test func conformances() {
        func sendable<T: Sendable>(_: T.Type) {}
        func hashable<T: Hashable>(_: T.Type) {}
        func identifiable<T: Identifiable>(_: T.Type) {}
        func equatable<T: Equatable>(_: T.Type) {}
        sendable(ClaudeControlConfiguration.Mode.self)
        sendable(ClaudeExternalTabRequest.self); sendable(ClaudeExternalUsageReading.self)
        identifiable(ClaudeAccountSummary.self); hashable(ClaudeAccountSummary.self); sendable(ClaudeAccountSummary.self)
        identifiable(ClaudeSessionSummary.self); hashable(ClaudeSessionSummary.self); sendable(ClaudeSessionSummary.self)
        hashable(ClaudeAttention.self); sendable(ClaudeAttention.self)
        hashable(ClaudeNeedsInput.self); sendable(ClaudeNeedsInput.self); sendable(ClaudeNeedsInput.Kind.self)
        hashable(ClaudeTaskProgress.self); sendable(ClaudeTaskProgress.self)
        hashable(ClaudeAttentionCounts.self); sendable(ClaudeAttentionCounts.self)
        sendable(ClaudeAttentionTransition.self); sendable(ClaudeAttentionTransition.Kind.self)
        hashable(ClaudeSetupState.self); sendable(ClaudeSetupState.self)
        hashable(ClaudePanelRoute.self); sendable(ClaudePanelRoute.self)
        sendable(ClaudeRefreshReason.self)
        hashable(ClaudeRingReading.self); sendable(ClaudeRingReading.self)
        hashable(ClaudeRingReading.Window.self); sendable(ClaudeRingReading.Window.self)
        hashable(ClaudeRingReading.Status.self); sendable(ClaudeRingReading.Status.self)
        hashable(ClaudeActivityRow.self); sendable(ClaudeActivityRow.self); sendable(ClaudeActivityRow.State.self)
        sendable(ClaudePanelEdge.self); sendable(ClaudePanelMode.self); sendable(ClaudeRingBadgeSlot.self)
        equatable(ClaudePanelAnchor.self); sendable(ClaudePanelAnchor.self)
        equatable(ClaudePanelChrome.self); sendable(ClaudePanelChrome.self)
        equatable(ClaudePanelPlacement.self); sendable(ClaudePanelPlacement.self)
    }

    /// Every contract enum, switched over without `default`: adding or
    /// removing a case stops this compiling.
    @Test func enumCases() {
        func mode(_ v: ClaudeControlConfiguration.Mode) -> Int { switch v { case .live: return 0; case .sealed: return 1 } }
        func attention(_ v: ClaudeAttention) -> Int {
            switch v { case .needsInput: return 0; case .working: return 1; case .readyForReview: return 2; case .idle: return 3 }
        }
        func kind(_ v: ClaudeNeedsInput.Kind) -> Int {
            switch v {
            case .permission: return 0; case .question: return 1; case .plan: return 2
            case .elicitation: return 3; case .dialog: return 4; case .error: return 5
            }
        }
        func transition(_ v: ClaudeAttentionTransition.Kind) -> Int {
            switch v { case .needsInput: return 0; case .readyForReview: return 1; case .resolved: return 2 }
        }
        func route(_ v: ClaudePanelRoute) -> Int {
            switch v { case .sessions(ringID: _): return 0; case .session(id: _): return 1; case .setup: return 2 }
        }
        func reason(_ v: ClaudeRefreshReason) -> Int { switch v { case .ringClick: return 0; case .forced: return 1 } }
        func status(_ v: ClaudeRingReading.Status) -> Int {
            switch v {
            case .ok: return 0; case .waitingForFirstReading: return 1; case .signInNeeded(let text): return text.count
            case .unavailable(let text): return text.count; case .failed(let text): return text.count
            }
        }
        func state(_ v: ClaudeActivityRow.State) -> Int {
            switch v { case .busy: return 0; case .waiting: return 1; case .success: return 2; case .idle: return 3 }
        }
        func edge(_ v: ClaudePanelEdge) -> Int {
            switch v { case .right: return 0; case .left: return 1; case .top: return 2; case .bottom: return 3 }
        }
        func panelMode(_ v: ClaudePanelMode) -> Int { switch v { case .list: return 0; case .chat: return 1 } }
        func slot(_ v: ClaudeRingBadgeSlot) -> Int { switch v { case .needsYou: return 0; case .review: return 1 } }
        #expect(mode(.sealed) + attention(.idle) + kind(.error) + transition(.resolved) + route(.setup) + reason(.forced)
                + status(.ok) + state(.idle) + edge(.bottom) + panelMode(.chat) + slot(.review) == 22)
    }
}

/// `ClaudeSettingsHost` as the app implements it (§12): every requirement.
@MainActor
private final class ContractSettingsHost: ClaudeSettingsHost {
    func nickname(ringID: String) -> String? { nil }
    func setNickname(_ n: String?, ringID: String) {}
    func isRingShown(_ ringID: String) -> Bool { true }
    func setRingShown(_ on: Bool, ringID: String) {}
    func openNotificationsSettings() {}
    func openSessionsPanel() {}
    func presentWebsiteSignIn(_ url: URL) async throws -> URL { url }
}

/// A host written before the website existed: no `presentWebsiteSignIn(_:)`.
/// It must still compile as a `ClaudeSettingsHost` (M4).
@MainActor
private final class OlderSettingsHost: ClaudeSettingsHost {
    func nickname(ringID: String) -> String? { nil }
    func setNickname(_ n: String?, ringID: String) {}
    func isRingShown(_ ringID: String) -> Bool { true }
    func setRingShown(_ on: Bool, ringID: String) {}
    func openNotificationsSettings() {}
    func openSessionsPanel() {}
}

/// `ClaudeExternalUsageSource` as the app's Desktop-cache adapter implements it.
private nonisolated struct ContractUsageSource: ClaudeExternalUsageSource {
    func reading(organizationUuid: String, now: Date) async -> ClaudeExternalUsageReading? { nil }
}
