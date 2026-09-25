import AppKit
import ClaudeControl
import Combine
import SwiftUI

/// The sessions panel's window: a non-activating `ClaudePanel` anchored to
/// the clicked ring on any edge (and beside the camera), holding
/// `ClaudeSessionsPanel` in Codenotch's hover-card chrome (design §7).
///
/// - **Opening.** A ring click toggles the panel for that ring, or moves it to
///   another ring. Clicks, the hot key, a banner and Settings open it as key
///   (without activating the app, so the terminal stays in front); an
///   auto-open never takes key, and never disturbs a panel already open.
/// - **While open.** Codenotch's hover cards are off on every notch, and the
///   anchor notch is pinned open (`ClaudeNotchHold`) so the ring the tail
///   points at stays out.
/// - **Closing.** Esc (after backing out of a chat), the close button, the
///   same ring, a click outside (unless "Keep open" is on), a jump to the
///   terminal, a space change; an untouched auto-opened panel after its
///   session is resolved or its time is up (`ClaudePanelPolicy`). The pin and
///   the hover cards are then restored.
/// - **Placement.** `ClaudePanelGeometry`, from the anchor notch's own hover
///   card geometry, redone when the notch moves (edge, size, nudge, rings,
///   screens) and when the content changes width (list or chat) or height.
///
/// Fork-only file. Owned by WP-D. WP-C calls it only through `configure`,
/// `toggle`, `open` and `close`.
@MainActor
final class ClaudePanelController {
    static let shared = ClaudePanelController()

    enum OpenReason { case ringClick, hoverRow, peekClick, notification, hotKey, auto, settings }

    private(set) var isOpen = false
    /// The ring the panel hangs off; nil while it floats.
    private(set) var ringID: String?
    /// What the panel shows while open.
    var route: ClaudePanelRoute? { isOpen ? state?.route : nil }

    private weak var fleet: NotchFleet?
    private weak var preferences: Preferences?
    private weak var hub: ClaudeControlHub?

    private var panel: ClaudePanel?
    private var state: ClaudePanelState?
    private let chrome = ClaudePanelChromeModel()
    private weak var anchorController: NotchWindowController?
    private var anchorGeometry: ClaudePanelAnchor?
    /// While floating: the frame of the screen it floats on, so a content
    /// change or a re-anchor later keeps it there rather than following
    /// the pointer to another display.
    private var floatingScreenFrame: CGRect?
    private(set) var placement: ClaudePanelPlacement?
    /// The row a banner or an auto-open points out in the list
    /// (`ClaudePanelPolicy.presentation`).
    private(set) var highlightedSessionID: String?
    /// Sealed launch hook only: whether a scripted run is driving the panel
    /// (the clicks and space changes of whoever is using the Mac meanwhile
    /// are then logged, not acted on), and for the self-test the window
    /// before a change it expects to move it.
    private var isScriptedRun = false
    private var pendingSelfTestFrame: CGRect?

    /// Set while the open panel is one that opened by itself.
    private var autoOpen: (policy: ClaudePanelPolicy.AutoOpen, cause: ClaudePanelPolicy.AutoOpenCause?,
                           sessionID: String?)?
    private var autoCloseWork: DispatchWorkItem?
    /// Opened by a click, the hot key, a banner or Settings: meant to be key.
    private var wantsKey = false
    /// The last app other than this one to come to the front, and the one to
    /// give the front back to when a panel opened from a banner closes: the
    /// banner click activated this app, so "the app in front keeps key" no
    /// longer holds on its own (BHV-6).
    private var lastOtherApp: NSRunningApplication?
    private var returnTo: NSRunningApplication?
    private var reanchorWork: DispatchWorkItem?
    private var mouseMonitors: [Any] = []
    private var cancellables = Set<AnyCancellable>()
    private var stateCancellables = Set<AnyCancellable>()
    private var panelCancellables = Set<AnyCancellable>()
    private var anchorCancellables = Set<AnyCancellable>()

    /// Codenotch's hover-card metrics, which the panel's chrome copies.
    static let chromeMetrics = ClaudePanelChrome(tailLength: NotchLayout.tailLength,
                                                 tailWidth: NotchLayout.tailHeight,
                                                 corner: NotchLayout.cardCorner)
    /// How long after the notch moves the panel follows it: after Codenotch's
    /// own relocation (an edge change crossfades for 0.16 s, then opens).
    static let reanchorDelay: TimeInterval = 0.35
    /// A content-height or width change animates the window this long.
    static let resizeDuration: TimeInterval = 0.18
    /// The launch hook waits this long, so the notches exist and show fixtures.
    static let launchHookDelay: TimeInterval = 2

    private init() {}

    // MARK: - Wiring

    /// Wire the panel to the notch and the engine (WP-C, from
    /// `ClaudeBridge.attach`, before any notch exists). Also starts the hold
    /// (`ClaudeNotchHold`), the settings navigation (`ClaudeSettingsNavigation`)
    /// and the hot key, and in sealed runs the `SPCN_OPEN_PANEL_ON_LAUNCH`
    /// development hook.
    func configure(fleet: NotchFleet, preferences: Preferences, hub: ClaudeControlHub) {
        self.fleet = fleet
        self.preferences = preferences
        self.hub = hub
        cancellables.removeAll()

        ClaudeNotchHold.shared.configure(fleet: fleet, preferences: preferences)
        ClaudeSettingsNavigation.configure(fleet: fleet)
        ClaudeHotKey.shared.start { [weak self] in self?.hotKeyPressed() }

        NotificationCenter.default.publisher(for: NSApplication.didChangeScreenParametersNotification)
            .sink { [weak self] _ in self?.scheduleReanchor() }
            .store(in: &cancellables)
        NSWorkspace.shared.notificationCenter
            .publisher(for: NSWorkspace.didActivateApplicationNotification)
            .compactMap { $0.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication }
            .filter { $0.processIdentifier != ProcessInfo.processInfo.processIdentifier }
            .sink { [weak self] app in self?.lastOtherApp = app }
            .store(in: &cancellables)
        if let front = NSWorkspace.shared.frontmostApplication,
           front.processIdentifier != ProcessInfo.processInfo.processIdentifier {
            lastOtherApp = front
        }
        NSWorkspace.shared.notificationCenter.publisher(for: NSWorkspace.activeSpaceDidChangeNotification)
            .sink { [weak self] _ in
                guard let self, self.isOpen, self.state?.isPinned != true else { return }
                guard !self.isScriptedRun else {
                    self.trace("space changed; left open during a scripted run")
                    return
                }
                self.close(because: "space changed")
            }
            .store(in: &cancellables)
        // Hidden or shown again: float, or hang off the notch again.
        preferences.$notchVisibility
            .dropFirst()
            .removeDuplicates()
            .sink { [weak self] _ in self?.scheduleReanchor() }
            .store(in: &cancellables)
        // A session resolved or gone decides when an auto-opened panel goes.
        hub.$sessions
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in self?.evaluateAutoClose() }
            .store(in: &cancellables)

        if Fork.isSealed { runLaunchHook() }
    }

    // MARK: - Opening and closing

    /// A Claude ring was clicked: open for that ring, close if it is already
    /// showing it, switch if another ring's list is showing (or the same ring
    /// on another display's notch).
    func toggle(ringID clicked: String, at screenPoint: CGPoint) {
        let onAnchorNotch = anchorController?.panelFrameForTesting?.contains(screenPoint) ?? true
        switch ClaudePanelPolicy.ringClick(isOpen: isOpen, anchorRingID: ringID, clicked: clicked,
                                           onAnchorNotch: onAnchorNotch) {
        case .open:
            open(.sessions(ringID: clicked), reason: .ringClick, pointer: screenPoint)
        case .close:
            close(because: "same ring clicked")
        case .switchRing:
            open(.sessions(ringID: clicked), reason: .ringClick, pointer: screenPoint)
        }
    }

    func open(_ route: ClaudePanelRoute, reason: OpenReason) {
        open(route, reason: reason, pointer: NSEvent.mouseLocation)
    }

    func close() {
        close(because: "closed")
    }

    /// Whether closing the panel should give the front back to the app the
    /// user was in: when a banner click opened it and so activated this app
    /// (a click on a notification brings its app forward whatever its
    /// options), and there is an app to go back to (BHV-6). Pure.
    static func returnsFrontOnClose(reason: OpenReason, frontmostIsSelf: Bool, hasOtherApp: Bool) -> Bool {
        reason == .notification && frontmostIsSelf && hasOtherApp
    }

    private func open(_ route: ClaudePanelRoute, reason: OpenReason, pointer: CGPoint) {
        guard let fleet, let hub else {
            trace("open \(route) (\(reason)) ignored: not configured")
            return
        }
        // Something opening by itself never takes over a panel in use.
        if isOpen, reason == .auto { return }
        let wasOpen = isOpen
        let frontmostBefore = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "none"
        // A banner or an auto-open shows the list with the session's row
        // highlighted; the anchor and the auto-close still follow the
        // session it was opened for (`route`).
        let presentation = ClaudePanelPolicy.presentation(for: route,
                                                          landsOnList: reason == .notification || reason == .auto)
        let state = self.state ?? makeState(route: presentation.route)
        state.route = presentation.route
        if case .sessions(let ring) = presentation.route {
            state.ringFilter = ring
        } else {
            // A hover row or a peek click shows every account.
            state.ringFilter = nil
        }
        highlightedSessionID = presentation.highlightedSessionID
        // The list scrolls to, unfolds and selects that row.
        state.highlightedSessionId = presentation.highlightedSessionID

        let sessionRing = sessionRingID(for: route, hub: hub)
        let anchor = fleet.claudeAnchor(for: route, sessionRingID: sessionRing, pointer: pointer,
                                        notchHidden: isNotchHidden)
        let panel = self.panel ?? makePanel(state: state, hub: hub)
        adopt(anchor, pointer: pointer)
        place(animated: wasOpen)
        if !wasOpen {
            // Lay the content out at its width before it is shown, so the
            // window opens at the content's height rather than growing to it.
            panel.contentView?.layoutSubtreeIfNeeded()
            place(animated: false)
        }
        setHoverCardsSuppressed(true)
        ClaudeNotchHold.shared.setPanel(anchor: anchor?.controller)

        if reason == .auto {
            let session = sessionID(of: route)
            let cause = session.flatMap { id in hub.sessions.first { $0.id == id } }
                .flatMap { ClaudePanelPolicy.autoOpenCause($0.attention) }
            autoOpen = (ClaudePanelPolicy.AutoOpen(openedAt: Date()), cause, session)
            wantsKey = false
            panel.orderFrontRegardless()
        } else {
            autoOpen = nil
            wantsKey = true
            let frontmostIsSelf = NSWorkspace.shared.frontmostApplication?.processIdentifier
                == ProcessInfo.processInfo.processIdentifier
            if Self.returnsFrontOnClose(reason: reason, frontmostIsSelf: frontmostIsSelf,
                                        hasOtherApp: lastOtherApp != nil) {
                returnTo = lastOtherApp
            }
            // Key without activating the app: the panel is non-activating.
            // The window server grants it on the heels of the user's click or
            // key press; `mouseDown(on: .panel)` asks again on the next one.
            panel.makeKeyAndOrderFront(nil)
        }
        if !wasOpen {
            isOpen = true
            state.isPresented = true
            installMouseMonitors()
        }
        evaluateAutoClose()
        // Key status settles with the window server; read it a turn later.
        // "Without activating the app" is about the frontmost app, which
        // keeps the menu bar: AppKit's own `isActive` reads true while a
        // non-activating panel of ours is key, though nothing else changes.
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.isOpen else { return }
                let frontmost = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "none"
                self.trace("open \(route) (\(reason)) \(self.anchorDescription) "
                           + "highlight=\(presentation.highlightedSessionID ?? "none") wantsKey=\(self.wantsKey) "
                           + "key=\(panel.isKeyWindow) frontmostBefore=\(frontmostBefore) frontmost=\(frontmost) "
                           + "isActive=\(NSApp.isActive) hoverCardsSuppressed=\(self.suppressedCount) "
                           + "pinnedForPanel=\(ClaudeNotchHold.shared.pinnedForPanel) "
                           + "anchorPinned=\(anchor?.controller.model.isPinned ?? false)")
            }
        }
    }

    private func close(because why: String) {
        guard isOpen else { return }
        isOpen = false
        autoOpen = nil
        wantsKey = false
        autoCloseWork?.cancel()
        autoCloseWork = nil
        reanchorWork?.cancel()
        reanchorWork = nil
        removeMouseMonitors()
        anchorCancellables.removeAll()
        state?.isPresented = false
        // Not activated, so the app that was in front keeps (or gets back) key.
        panel?.orderOut(nil)
        // Opened from a banner, which did activate this app: hand the front
        // back to the app the user was in (a terminal, usually).
        if let app = returnTo, !app.isTerminated,
           NSWorkspace.shared.frontmostApplication?.processIdentifier == ProcessInfo.processInfo.processIdentifier {
            NSApp.yieldActivation(to: app)
            app.activate()
            trace("gave the front back to \(app.bundleIdentifier ?? "?")")
        }
        returnTo = nil

        let controller = anchorController
        let pinnedForPanel = ClaudeNotchHold.shared.pinnedForPanel
        // Restore the pin first, then let the hover fold take over.
        ClaudeNotchHold.shared.setPanel(anchor: nil)
        setHoverCardsSuppressed(false)
        anchorController = nil
        anchorGeometry = nil
        floatingScreenFrame = nil
        highlightedSessionID = nil
        ringID = nil
        trace("close (\(why)): pin restored to \(controller?.model.isPinned ?? false) "
              + "(the panel had pinned it: \(pinnedForPanel)), hoverCardsSuppressed=\(suppressedCount)")
    }

    private func hotKeyPressed() {
        guard let panel, isOpen else {
            open(.sessions(ringID: nil), reason: .hotKey)
            return
        }
        if panel.isKeyWindow {
            close(because: "hot key")
        } else {
            markEngaged()
            wantsKey = true
            panel.makeKeyAndOrderFront(nil)
        }
    }

    // MARK: - Anchoring and placement

    private var isNotchHidden: Bool { preferences?.notchVisibility == .hidden }

    /// Hang off `anchor`, or float (nil) on the screen `pointer` is on.
    private func adopt(_ anchor: ClaudeNotchAnchor?, pointer: CGPoint) {
        if anchor?.controller !== anchorController || anchor == nil {
            observe(anchor?.controller)
        }
        anchorController = anchor?.controller
        anchorGeometry = anchor?.geometry
        ringID = anchor?.ringID
        floatingScreenFrame = anchor == nil
            ? (Self.screen(containing: pointer) ?? NSScreen.screens.first)?.frame
            : nil
    }

    /// Follow the anchor notch as Codenotch moves or rebuilds it.
    private func observe(_ controller: NotchWindowController?) {
        anchorCancellables.removeAll()
        guard let model = controller?.model else { return }
        // Anything that moves a ring or the notch's window: the edge, the
        // size, the ⌥-drag nudge, the readings (the ring count, and the card
        // heights the window reserves room for), the display.
        func changes<P: Publisher>(_ publisher: P) -> AnyPublisher<Void, Never> where P.Failure == Never {
            publisher.dropFirst().map { _ in () }.eraseToAnyPublisher()
        }
        Publishers.MergeMany(
            changes(model.$edge),
            changes(model.$sizeScale),
            changes(model.$alongOffset),
            changes(model.$snapshots),
            changes(model.$hardwareNotch),
            changes(model.$screenSize),
            changes(model.$showsNotchReadings)
        )
        .sink { [weak self] in self?.scheduleReanchor() }
        .store(in: &anchorCancellables)
        model.$surfaceStyle
            .dropFirst()
            .sink { [weak self] style in self?.applySurface(style) }
            .store(in: &anchorCancellables)
        model.$accentColor
            .dropFirst()
            .sink { [weak self] choice in self?.chrome.accent = choice.color }
            .store(in: &anchorCancellables)
    }

    private func scheduleReanchor() {
        guard isOpen else { return }
        reanchorWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated { self?.reanchor() }
        }
        reanchorWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.reanchorDelay, execute: work)
    }

    /// The notch moved, was rebuilt, hidden or shown: hang off it again. The
    /// panel stays on the notch it opened on; if that notch is gone (its
    /// display was unplugged) the panel closes.
    private func reanchor() {
        reanchorWork = nil
        guard isOpen, let fleet, let hub, let state else { return }
        // Where the panel is now: its notch, or the screen it floats on (if
        // that screen is still there), else where the pointer is. A panel
        // that has to float, or can hang off a notch again, stays there
        // rather than following the pointer to another display.
        let pointer = anchorGeometry.map { CGPoint(x: $0.notchWindowFrame.midX, y: $0.notchWindowFrame.midY) }
            ?? floatingScreen.map { CGPoint(x: $0.frame.midX, y: $0.frame.midY) }
            ?? NSEvent.mouseLocation
        let anchor: ClaudeNotchAnchor?
        if anchorGeometry != nil {
            // Hanging off a notch: stay on it, or close if it is gone (its
            // display unplugged; the fleet drops the controller).
            guard let controller = anchorController,
                  fleet.controllersForTesting.contains(where: { $0 === controller }) else {
                close(because: "its notch went away")
                return
            }
            if isNotchHidden {
                anchor = nil
            } else if let ringID, let same = controller.claudeAnchor(ringID: ringID) {
                anchor = same
            } else {
                let rings = controller.model.snapshots.map(\.id).filter(ClaudeBridge.ownsProvider)
                anchor = ClaudePanelPolicy.anchorRing(for: state.route, sessionRingID: sessionRingID(for: state.route, hub: hub),
                                                      ringsInNotch: rings)
                    .flatMap(controller.claudeAnchor(ringID:))
            }
        } else {
            // Floating: hang off the notch of the screen it floats on, if
            // that notch can take it now.
            anchor = fleet.claudeAnchor(for: state.route, sessionRingID: sessionRingID(for: state.route, hub: hub),
                                        pointer: pointer, notchHidden: isNotchHidden)
        }
        adopt(anchor, pointer: pointer)
        ClaudeNotchHold.shared.setPanel(anchor: anchor?.controller)
        setHoverCardsSuppressed(true)
        if place(animated: true) {
            trace("re-anchored: \(anchorDescription)")
        }
    }

    /// Put the window where `ClaudePanelGeometry` says, sized to the content.
    /// Returns whether the window moved or changed size.
    @discardableResult
    private func place(animated: Bool) -> Bool {
        guard let panel, let state else { return false }
        let fallback = (floatingScreen ?? Self.screen(containing: NSEvent.mouseLocation) ?? NSScreen.screens.first)?
            .visibleFrame ?? .zero
        let placement = ClaudePanelGeometry.place(anchor: anchorGeometry, mode: state.mode,
                                                  idealContentHeight: state.idealContentHeight,
                                                  chrome: Self.chromeMetrics, fallbackVisibleFrame: fallback)
        self.placement = placement
        if state.contentWidth != placement.contentWidth { state.contentWidth = placement.contentWidth }
        // The content keeps its ideal height under what this screen allows.
        if state.maxContentHeight != placement.maxContentHeight { state.maxContentHeight = placement.maxContentHeight }

        // From the geometry the window was placed by, not the notch's edge
        // now: between an edge change and the re-anchor the two differ, and
        // the tail must stay on the side the card was placed for.
        let direction = placement.hasTail ? anchorGeometry?.edge.notchEdge.tooltipDirection : nil
        let style = anchorController?.model.surfaceStyle ?? preferences?.notchSurfaceStyle ?? .glass
        let accent = (anchorController?.model.accentColor ?? preferences?.accentColor)?.color ?? .accentColor
        let changes = {
            self.chrome.direction = direction
            self.chrome.tailOffset = placement.tailOffset
        }
        if animated, panel.isVisible, chrome.direction == direction {
            withAnimation(.easeOut(duration: Self.resizeDuration)) { changes() }
        } else {
            changes()
        }
        chrome.accent = accent
        applySurface(style)

        let frame = placement.windowFrame.integral
        guard panel.frame != frame else { return false }
        if animated, panel.isVisible {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = Self.resizeDuration
                context.timingFunction = CAMediaTimingFunction(name: .easeOut)
                panel.animator().setFrame(frame, display: true)
            }
        } else {
            panel.setFrame(frame, display: true)
        }
        return true
    }

    private func applySurface(_ style: NotchSurfaceStyle) {
        if chrome.surfaceStyle != style { chrome.surfaceStyle = style }
        // As on the notch: nil (follow the Mac) for glass, dark otherwise.
        panel?.appearance = style.panelAppearance(
            reduceTransparency: NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency)
    }

    private static func screen(containing point: CGPoint) -> NSScreen? {
        NSScreen.screens.first { $0.frame.contains(point) }
    }

    /// The screen a floating panel floats on, while it is still attached.
    private var floatingScreen: NSScreen? {
        floatingScreenFrame.flatMap { frame in NSScreen.screens.first { $0.frame == frame } }
    }

    // MARK: - Hover cards

    /// Codenotch's hover cards stay away while the panel is open, on every
    /// notch: the panel is the card, and a second one would show through it.
    private func setHoverCardsSuppressed(_ on: Bool) {
        for controller in fleet?.controllersForTesting ?? [] where controller.model.suppressesTooltips != on {
            controller.model.suppressesTooltips = on
            // Rebuilds the notch's hit regions without the card's.
            controller.cursorMoved()
        }
    }

    private var suppressedCount: String {
        let controllers = fleet?.controllersForTesting ?? []
        return "\(controllers.filter(\.model.suppressesTooltips).count)/\(controllers.count)"
    }

    // MARK: - Outside clicks

    /// A mouse-down anywhere else closes the panel, unless "Keep open" is on.
    /// Two monitors, neither of which needs a permission: a global one for
    /// other apps, a local one for the app's own windows.
    private func installMouseMonitors() {
        guard mouseMonitors.isEmpty else { return }
        let events: NSEvent.EventTypeMask = [.leftMouseDown, .rightMouseDown]
        if let global = NSEvent.addGlobalMonitorForEvents(matching: events, handler: { [weak self] _ in
            MainActor.assumeIsolated { self?.mouseDown(on: .elsewhere, fromMonitor: true) }
        }) {
            mouseMonitors.append(global)
        }
        if let local = NSEvent.addLocalMonitorForEvents(matching: events, handler: { [weak self] event in
            MainActor.assumeIsolated {
                guard let self else { return }
                let target: ClaudePanelPolicy.ClickTarget
                if event.window != nil, event.window === self.panel {
                    target = .panel
                } else if event.window is NotchPanel {
                    target = .notch
                } else {
                    target = .otherOwnWindow
                }
                self.mouseDown(on: target, fromMonitor: true)
            }
            return event
        }) {
            mouseMonitors.append(local)
        }
    }

    private func removeMouseMonitors() {
        mouseMonitors.forEach(NSEvent.removeMonitor)
        mouseMonitors.removeAll()
    }

    private func mouseDown(on target: ClaudePanelPolicy.ClickTarget, fromMonitor: Bool = false) {
        guard isOpen else { return }
        if target == .panel {
            markEngaged()
            // A panel opened to type into takes key on the user's first click
            // in it if it did not get key when it opened. One that opened by
            // itself only does for a text field (`becomesKeyOnlyIfNeeded`),
            // so clicking Allow leaves the terminal with the keyboard.
            if wantsKey, let panel, !panel.isKeyWindow { panel.makeKey() }
        }
        if ClaudePanelPolicy.closesOnMouseDown(target, isPinned: state?.isPinned ?? false) {
            guard !(fromMonitor && isScriptedRun) else {
                trace("clicked outside (\(target)); left open during a scripted run")
                return
            }
            close(because: "clicked outside")
        }
    }

    // MARK: - Auto-open

    private func markEngaged() {
        guard var auto = autoOpen, !auto.policy.isEngaged else { return }
        auto.policy.isEngaged = true
        autoOpen = auto
        evaluateAutoClose()
    }

    /// Note a resolution, and (re)schedule the auto-close for the deadline
    /// `ClaudePanelPolicy.autoCloseDeadline` gives.
    private func evaluateAutoClose() {
        autoCloseWork?.cancel()
        autoCloseWork = nil
        guard isOpen, var auto = autoOpen else { return }
        if auto.policy.resolvedAt == nil, let cause = auto.cause {
            let attention = auto.sessionID.flatMap { id in hub?.sessions.first { $0.id == id } }?.attention
            if ClaudePanelPolicy.isResolved(cause, attention: attention) {
                auto.policy.resolvedAt = Date()
                autoOpen = auto
            }
        }
        let peek = preferences?.peekDuration.seconds ?? ClaudePanelPolicy.autoCloseMinimumTimeout
        guard let deadline = ClaudePanelPolicy.autoCloseDeadline(auto.policy, peekDuration: peek) else { return }
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.autoOpen != nil else { return }
                self.close(because: "auto-open timed out or was resolved")
            }
        }
        autoCloseWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + max(0, deadline.timeIntervalSinceNow), execute: work)
    }

    // MARK: - Building

    private func makeState(route: ClaudePanelRoute) -> ClaudePanelState {
        let state = ClaudePanelState(route: route)
        state.isPinned = ClaudeControlSettings.panelPinned
        state.onClose = { [weak self] in self?.close(because: "close button or Esc") }
        state.onRequestKey = { [weak self] in
            guard let self, self.isOpen, let panel = self.panel else { return }
            guard ClaudePanelPolicy.grantsKeyRequest(autoOpen: self.autoOpen?.policy) else {
                self.trace("key request ignored: the panel opened by itself and is untouched")
                return
            }
            self.markEngaged()
            panel.makeKey()
        }
        state.onOpenSettings = { ClaudeSettingsNavigation.showClaudeCode() }
        state.onJumped = { [weak self] in
            guard let self, self.state?.isPinned != true else { return }
            self.close(because: "jumped to the terminal")
        }
        // The window follows the content: list or chat width, natural height.
        state.$idealContentHeight
            .removeDuplicates()
            .dropFirst()
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in
                guard let self, self.isOpen else { return }
                self.place(animated: true)
            }
            .store(in: &stateCancellables)
        state.$route
            .map { route -> ClaudePanelMode in if case .session = route { return .chat } else { return .list } }
            .removeDuplicates()
            .dropFirst()
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in
                guard let self, self.isOpen else { return }
                self.place(animated: true)
            }
            .store(in: &stateCancellables)
        state.$isPinned
            .dropFirst()
            .removeDuplicates()
            .sink { ClaudeControlSettings.panelPinned = $0 }
            .store(in: &stateCancellables)
        self.state = state
        return state
    }

    private func makePanel(state: ClaudePanelState, hub: ClaudeControlHub) -> ClaudePanel {
        let frame = CGRect(x: 0, y: 0, width: 440, height: ClaudePanelGeometry.minimumHeight)
        let panel = ClaudePanel(contentRect: frame)
        panel.onCancel = { [weak self] in self?.cancel() }
        panel.onEditCommand = { [weak self] command, handled in
            let frontmost = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "none"
            self?.trace("\(command) via the key-equivalent fallback, handled=\(handled), frontmost=\(frontmost)")
        }
        let hosting = NSHostingView(rootView: ClaudePanelChromeView(chrome: chrome) {
            ClaudeSessionsPanel(hub: hub, state: state)
        })
        hosting.sizingOptions = []
        let container = ClaudePanelContainerView(frame: CGRect(origin: .zero, size: frame.size))
        container.autoresizingMask = [.width, .height]
        container.onPointerEntered = { [weak self] in self?.markEngaged() }
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        panel.contentView = container
        NotificationCenter.default.publisher(for: NSWindow.didBecomeKeyNotification, object: panel)
            .sink { [weak self] _ in self?.markEngaged() }
            .store(in: &panelCancellables)
        self.panel = panel
        return panel
    }

    /// Esc that nothing in the content took: back from the chat, else close
    /// (through `onClose`), the same rule as the content's own Esc.
    private func cancel() {
        state?.escape()
    }

    // MARK: - Helpers

    private func sessionID(of route: ClaudePanelRoute) -> String? {
        if case .session(let id) = route { return id }
        return nil
    }

    private func sessionRingID(for route: ClaudePanelRoute, hub: ClaudeControlHub) -> String? {
        sessionID(of: route).flatMap { id in hub.sessions.first { $0.id == id }?.ringID }
    }

    private var anchorDescription: String {
        guard let placement else { return "unplaced" }
        let window = NSStringFromRect(placement.windowFrame)
        guard let anchorGeometry, let ringID else { return "floating window=\(window)" }
        return "ring=\(ringID) edge=\(anchorGeometry.edge) notch=\(NSStringFromRect(anchorGeometry.notchWindowFrame)) "
            + "window=\(window) tailOffset=\(placement.tailOffset.rounded())"
    }

    /// The panel's log line; sealed runs also print it, so a smoke run's log
    /// shows what the panel did.
    private func trace(_ message: String) {
        Log.sessions.info("claude panel: \(message, privacy: .public)")
        if Fork.isSealed {
            FileHandle.standardError.write(Data("[spcn-panel] \(message)\n".utf8))
        }
    }

    // MARK: - Sealed launch hook

    /// Sealed runs only: `SPCN_OPEN_PANEL_ON_LAUNCH=sessions|sessions:<ring>|
    /// session:<id>|setup`, optionally prefixed `auto:` to open the way an
    /// auto-open does, opens the panel once the notches are up; with
    /// `SPCN_PANEL_CLOSE_AFTER=<seconds>` it is closed again after that long,
    /// to show the notch's pin and hover cards coming back. With
    /// `SPCN_PANEL_SELF_TEST=1` it is driven through `runSelfTest` instead.
    private func runLaunchHook() {
        let environment = ProcessInfo.processInfo.environment
        guard let value = environment["SPCN_OPEN_PANEL_ON_LAUNCH"],
              let request = ClaudePanelPolicy.launchRequest(value) else { return }
        let closeAfter = environment["SPCN_PANEL_CLOSE_AFTER"].flatMap(TimeInterval.init)
        let selfTest = environment["SPCN_PANEL_SELF_TEST"] == "1"
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.launchHookDelay) { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.isScriptedRun = selfTest || closeAfter != nil
                self.open(request.route, reason: request.isAuto ? .auto : .hotKey)
                if selfTest {
                    self.runSelfTest()
                } else if let closeAfter {
                    DispatchQueue.main.asyncAfter(deadline: .now() + closeAfter) {
                        MainActor.assumeIsolated {
                            self.close(because: "SPCN_PANEL_CLOSE_AFTER")
                            self.isScriptedRun = false
                        }
                    }
                }
            }
        }
    }
}

// MARK: - Sealed self-test

extension ClaudePanelController {
    /// Sealed runs only (`SPCN_PANEL_SELF_TEST=1`, after the launch hook
    /// opened the panel on a session's chat): the panel's behaviour driven
    /// in-process, the way the user's keys and clicks reach it, each step
    /// logged `self-test <step>: ok|FAIL (...)`. Nothing leaves the process:
    /// the key event is handed to the panel, not posted to the system. The
    /// whole test takes about 5 s, so it fits a sealed run of 9 s.
    private func runSelfTest() {
        /// Each step runs `delay` after the one before it: a quarter of a
        /// second for the direct ones, longer where Codenotch relocates the
        /// notch first and the panel follows `reanchorDelay` later.
        var steps: [(name: String, delay: TimeInterval, check: () -> String?)] = []
        func step(_ name: String, after delay: TimeInterval = 0.25, _ check: @escaping () -> String?) {
            steps.append((name, delay, check))
        }
        // A click on a ring lands on its notch: the point its tail aims at.
        let pointOnRing = { [weak self] (ring: String) -> CGPoint in
            let controller = self?.anchorController ?? self?.fleet?.claudeControllers.first
            return controller?.claudeAnchor(ringID: ring).map { ClaudePanelGeometry.ringPoint($0.geometry) }
                ?? NSEvent.mouseLocation
        }
        // Whether the window is where the geometry puts it for the notch as it
        // is now: what re-anchoring promises, whether or not the change moved it.
        let isWhereTheNotchSays = { [weak self] () -> String? in
            guard let self, let controller = self.anchorController, let ringID = self.ringID,
                  let anchor = controller.claudeAnchor(ringID: ringID), let state = self.state,
                  let placement = self.placement else { return "not anchored" }
            let expected = ClaudePanelGeometry.place(anchor: anchor.geometry, mode: state.mode,
                                                     idealContentHeight: state.idealContentHeight,
                                                     chrome: Self.chromeMetrics, fallbackVisibleFrame: .zero)
            return expected.windowFrame == placement.windowFrame
                ? nil : "window \(NSStringFromRect(placement.windowFrame)), notch says \(NSStringFromRect(expected.windowFrame))"
        }
        let otherRing = { [weak self] () -> String? in
            let rings = self?.anchorController?.model.snapshots.map(\.id).filter(ClaudeBridge.ownsProvider) ?? []
            return rings.first { $0 != self?.ringID }
        }

        step("cmd-V reaches the key-equivalent fallback", after: 0.5) { [weak self] in
            guard let self, let panel = self.panel,
                  let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command,
                                               timestamp: ProcessInfo.processInfo.systemUptime,
                                               windowNumber: panel.windowNumber, context: nil,
                                               characters: "v", charactersIgnoringModifiers: "v",
                                               isARepeat: false, keyCode: 9)
            else { return "no panel" }
            // AppKit sends key equivalents only to the key window. Key is the
            // window server's to grant; without it there is nothing to test.
            panel.makeKey()
            guard panel.isKeyWindow else { return "skipped (the panel is not key)" }
            var reached: ClaudePanelPolicy.EditCommand?
            let previous = panel.onEditCommand
            panel.onEditCommand = { command, handled in reached = command; previous?(command, handled) }
            _ = panel.performKeyEquivalent(with: event)
            panel.onEditCommand = previous
            return reached == .paste ? nil : "fallback not reached (panel key: \(panel.isKeyWindow))"
        }
        step("Esc in a chat goes back to the list") { [weak self] in
            self?.panel?.cancelOperation(nil)
            guard let self, self.isOpen, case .sessions = self.state?.route else {
                return "route \(String(describing: self?.route))"
            }
            return nil
        }
        step("Esc in the list closes") { [weak self] in
            self?.panel?.cancelOperation(nil)
            return self?.isOpen == false ? nil : "still open"
        }
        step("a ring click opens for that ring") { [weak self] in
            guard let self, let ring = self.fleet?.claudeControllers.first?.model.snapshots.map(\.id)
                .first(where: ClaudeBridge.ownsProvider) else { return "no Claude ring" }
            self.toggle(ringID: ring, at: pointOnRing(ring))
            return self.isOpen && self.ringID == ring ? nil : "open \(self.isOpen) ring \(self.ringID ?? "none")"
        }
        step("another ring's click switches to it") { [weak self] in
            guard let self, let ring = otherRing() else { return "only one Claude ring" }
            self.toggle(ringID: ring, at: pointOnRing(ring))
            guard self.isOpen, self.ringID == ring, case .sessions(ring?) = self.state?.route else {
                return "open \(self.isOpen) ring \(self.ringID ?? "none") route \(String(describing: self.route))"
            }
            return nil
        }
        step("the same ring's click closes") { [weak self] in
            guard let self, let ring = self.ringID else { return "no ring" }
            self.toggle(ringID: ring, at: pointOnRing(ring))
            return self.isOpen ? "still open" : nil
        }
        step("an outside click closes") { [weak self] in
            guard let self else { return "gone" }
            self.open(.sessions(ringID: nil), reason: .hotKey)
            self.mouseDown(on: .elsewhere)
            return self.isOpen ? "still open" : nil
        }
        step("an outside click leaves a kept-open panel") { [weak self] in
            guard let self else { return "gone" }
            self.open(.sessions(ringID: nil), reason: .hotKey)
            self.state?.isPinned = true
            self.mouseDown(on: .elsewhere)
            let stayed = self.isOpen
            self.state?.isPinned = false
            return stayed ? nil : "closed"
        }
        step("a size change re-anchors") { [weak self] in
            guard let self, let fleet = self.fleet else { return "gone" }
            self.pendingSelfTestFrame = self.placement?.windowFrame
            fleet.apply(scale: 1.25)
            return nil
        }
        step("the panel followed the size change", after: 0.8) { [weak self] in
            guard let self else { return "gone" }
            if let failure = isWhereTheNotchSays() { return failure }
            // Beside the camera the notch keeps the hardware's size.
            let moved = self.placement?.windowFrame != self.pendingSelfTestFrame
            let hardware = self.anchorController?.model.splitsAroundHardwareNotch == true
            return moved || hardware ? nil : "window did not move"
        }
        step("an edge change re-anchors") { [weak self] in
            guard let self, let fleet = self.fleet, let edge = self.anchorController?.model.edge else { return "gone" }
            fleet.apply(edge: edge == .right ? .left : .right)
            return nil
        }
        // The notch fades out, lands on the other edge and opens again first.
        step("the panel followed the edge change", after: 1.3) { [weak self] in
            guard let self, let geometry = self.anchorGeometry else { return "floating" }
            let edge = self.anchorController?.model.edge.claudePanelEdge
            guard geometry.edge == edge else { return "anchored to \(geometry.edge), notch on \(String(describing: edge))" }
            return isWhereTheNotchSays()
        }
        step("close") { [weak self] in
            self?.close(because: "self-test done")
            return self?.isOpen == false ? nil : "still open"
        }

        var at: TimeInterval = 0
        for (index, step) in steps.enumerated() {
            at += step.delay
            DispatchQueue.main.asyncAfter(deadline: .now() + at) { [weak self] in
                MainActor.assumeIsolated {
                    let failure = step.check()
                    let verdict = failure.map { $0.hasPrefix("skipped") ? $0 : "FAIL (\($0))" } ?? "ok"
                    self?.trace("self-test \(step.name): " + verdict)
                    if index == steps.count - 1 { self?.isScriptedRun = false }
                }
            }
        }
    }
}
