//
//  ClaudePanelPolicy.swift
//  ClaudeControl
//
//  The decisions behind the sessions panel's window (design §6, §7), kept
//  pure so they can be tested without AppKit: what a ring click does, which
//  notch the panel hangs off, which clicks close it, when a panel that opened
//  by itself goes away again, which notches stay open while a session needs
//  you, which edit keys the panel answers itself, and the global shortcut's
//  chord. The app's ClaudePanelController, ClaudeNotchHold and ClaudeHotKey
//  only carry these out.
//
//  Owned by WP-D.
//

import CoreGraphics
import Foundation

public nonisolated enum ClaudePanelPolicy {
    // MARK: - Ring clicks

    public enum RingClickOutcome: Equatable, Sendable {
        /// Nothing is open: open the list for the clicked ring.
        case open
        /// The panel already hangs off this ring: put it away.
        case close
        /// It is open for another ring, or for the same ring on another
        /// display's notch: move it there and filter to it.
        case switchRing
    }

    /// A Claude ring was clicked while the panel was `isOpen`, anchored to
    /// `anchorRingID`. `onAnchorNotch` is whether the click landed on the notch
    /// the panel hangs off: with a notch on every display each shows the same
    /// rings, and the same ring clicked on another display moves the panel
    /// there rather than closing it.
    public static func ringClick(isOpen: Bool, anchorRingID: String?, clicked: String,
                                 onAnchorNotch: Bool = true) -> RingClickOutcome {
        guard isOpen else { return .open }
        return anchorRingID == clicked && onAnchorNotch ? .close : .switchRing
    }

    // MARK: - What opens

    /// What the panel shows for a request, and the row it points out.
    public struct Presentation: Equatable, Sendable {
        public var route: ClaudePanelRoute
        /// The session whose row the list highlights and selects.
        public var highlightedSessionID: String?

        public init(route: ClaudePanelRoute, highlightedSessionID: String? = nil) {
            self.route = route
            self.highlightedSessionID = highlightedSessionID
        }
    }

    /// A banner click or an auto-open (`landsOnList`) that names a session
    /// shows the list of every account with that session's row highlighted
    /// (design §7), not the session's chat: its answer is one keystroke away
    /// there, and opening a chat marks the session reviewed, which would
    /// resolve (and so close) a panel auto-opened for a finished session the
    /// moment it appeared. Everything else shows what it asks for.
    public static func presentation(for route: ClaudePanelRoute, landsOnList: Bool) -> Presentation {
        guard landsOnList, case .session(let id) = route else { return Presentation(route: route) }
        return Presentation(route: .sessions(ringID: nil), highlightedSessionID: id)
    }

    /// Whether the content's request to make the panel key is granted. A
    /// panel that opened by itself and has not been touched never takes key:
    /// the chat's composer asks as it appears, and granting that would take
    /// the keyboard from the terminal. A click in the panel engages it first
    /// (the window sees the mouse-down before the view does), so a text
    /// field that was clicked still gets the keyboard.
    public static func grantsKeyRequest(autoOpen: AutoOpen?) -> Bool {
        autoOpen?.isEngaged ?? true
    }

    // MARK: - Anchoring

    /// Which of the notch's Claude rings the panel points at for `route`: the
    /// requested ring, the session's ring, or the first Claude ring. Nil when
    /// the wanted ring is not in this notch (switched off, or no Claude ring
    /// at all), and the panel floats instead.
    public static func anchorRing(for route: ClaudePanelRoute, sessionRingID: String?,
                                  ringsInNotch: [String]) -> String? {
        switch route {
        case .sessions(let ringID?):
            return ringsInNotch.contains(ringID) ? ringID : nil
        case .sessions(nil), .setup:
            return ringsInNotch.first
        case .session:
            guard let sessionRingID else { return ringsInNotch.first }
            return ringsInNotch.contains(sessionRingID) ? sessionRingID : nil
        }
    }

    /// Which notch the panel hangs off, by index: the one whose window holds
    /// the pointer, else the one on the pointer's screen, else the one on the
    /// menu-bar screen, else the first. `notchFrames[i]` is notch i's window
    /// and `screenFrames[i]` the screen it is on.
    public static func anchorIndex(notchFrames: [CGRect], screenFrames: [CGRect], pointer: CGPoint?,
                                   mainScreen: CGRect?) -> Int? {
        guard !notchFrames.isEmpty else { return nil }
        if let pointer {
            if let index = notchFrames.firstIndex(where: { $0.contains(pointer) }) { return index }
            if let index = screenFrames.firstIndex(where: { $0.contains(pointer) }),
               notchFrames.indices.contains(index) { return index }
        }
        if let mainScreen, let index = screenFrames.firstIndex(of: mainScreen),
           notchFrames.indices.contains(index) { return index }
        return 0
    }

    // MARK: - Closing

    /// Where a mouse-down landed, as far as the panel is concerned.
    public enum ClickTarget: Equatable, Sendable {
        /// On the panel itself.
        case panel
        /// On a notch: the notch decides (a ring click toggles or switches).
        case notch
        /// Another of the app's windows (Settings).
        case otherOwnWindow
        /// Another app.
        case elsewhere
    }

    /// Whether a mouse-down closes the panel. The header's "Keep open" pin
    /// turns closing on outside clicks off.
    public static func closesOnMouseDown(_ target: ClickTarget, isPinned: Bool) -> Bool {
        switch target {
        case .panel, .notch: return false
        case .otherOwnWindow, .elsewhere: return !isPinned
        }
    }

    /// A panel that opened by itself (auto-open), until the user has touched it.
    public struct AutoOpen: Equatable, Sendable {
        public var openedAt: Date
        /// When the attention that opened it (needs input, or ready for
        /// review) was resolved, or the session went away.
        public var resolvedAt: Date?
        /// The pointer has entered the panel, or it has become key. From then
        /// on it is the user's panel and closes only the ordinary ways.
        public var isEngaged: Bool

        public init(openedAt: Date, resolvedAt: Date? = nil, isEngaged: Bool = false) {
            self.openedAt = openedAt
            self.resolvedAt = resolvedAt
            self.isEngaged = isEngaged
        }
    }

    /// What an auto-opened panel was opened for.
    public enum AutoOpenCause: Equatable, Sendable {
        case needsInput
        case readyForReview
    }

    /// The cause a session's attention can auto-open the panel for; nil for
    /// working and idle, which never do.
    public static func autoOpenCause(_ attention: ClaudeAttention) -> AutoOpenCause? {
        switch attention {
        case .needsInput: return .needsInput
        case .readyForReview: return .readyForReview
        case .working, .idle: return nil
        }
    }

    /// Whether what auto-opened the panel is over: the session has left that
    /// attention (answered, reviewed, went back to work), or it is gone (nil).
    /// A different question on the same session still needs the user.
    public static func isResolved(_ cause: AutoOpenCause, attention: ClaudeAttention?) -> Bool {
        guard let attention else { return true }
        return autoOpenCause(attention) != cause
    }

    /// How long after its session is resolved an auto-opened panel lingers.
    public static let autoCloseAfterResolved: TimeInterval = 1
    /// The least an untouched auto-opened panel stays up.
    public static let autoCloseMinimumTimeout: TimeInterval = 8

    /// When an auto-opened panel closes by itself: 1 s after its session is
    /// resolved, or `max(peekDuration, 8 s)` after it opened if nobody
    /// touched it, whichever comes first. Nil once the user has engaged.
    public static func autoCloseDeadline(_ state: AutoOpen, peekDuration: TimeInterval) -> Date? {
        guard !state.isEngaged else { return nil }
        let timeout = state.openedAt.addingTimeInterval(max(peekDuration, autoCloseMinimumTimeout))
        guard let resolvedAt = state.resolvedAt else { return timeout }
        return min(timeout, resolvedAt.addingTimeInterval(autoCloseAfterResolved))
    }

    // MARK: - Holding notches open

    /// Whether a notch is held open (shown as if "Always show" were on)
    /// while a session needs you. `.auto` holds only notches whose folded
    /// state cannot show the resting marks: flush with the camera, where the
    /// open rings sit in the menu-bar band and cover nothing. A notch the user
    /// hid stays hidden, and one already always shown needs no hold.
    public static func holdsNotchOpen(policy: HoldOpenPolicy, needsYou: Bool, isFlushWithHardware: Bool,
                                      userHidesNotch: Bool, userAlwaysShowsNotch: Bool) -> Bool {
        guard needsYou, !userHidesNotch, !userAlwaysShowsNotch else { return false }
        switch policy {
        case .never: return false
        case .always: return true
        case .auto: return isFlushWithHardware
        }
    }

    // MARK: - Keys

    /// The standard editing commands the panel handles itself. A
    /// non-activating panel can be key while the app is not active, and then
    /// the main menu's key equivalents never fire.
    public enum EditCommand: Equatable, Sendable {
        case selectAll, copy, paste, cut, undo, redo
    }

    public struct KeyModifiers: OptionSet, Equatable, Sendable {
        public let rawValue: Int
        public init(rawValue: Int) { self.rawValue = rawValue }
        public static let command = KeyModifiers(rawValue: 1 << 0)
        public static let shift = KeyModifiers(rawValue: 1 << 1)
        public static let option = KeyModifiers(rawValue: 1 << 2)
        public static let control = KeyModifiers(rawValue: 1 << 3)
    }

    /// ⌘A ⌘C ⌘V ⌘X ⌘Z and ⇧⌘Z, from the key's characters ignoring modifiers.
    /// Anything with ⌥ or ⌃ is someone else's shortcut.
    public static func editCommand(charactersIgnoringModifiers characters: String,
                                   modifiers: KeyModifiers) -> EditCommand? {
        guard modifiers.contains(.command), modifiers.isDisjoint(with: [.option, .control]) else { return nil }
        let shifted = modifiers.contains(.shift)
        switch characters.lowercased() {
        case "a" where !shifted: return .selectAll
        case "c" where !shifted: return .copy
        case "v" where !shifted: return .paste
        case "x" where !shifted: return .cut
        case "z": return shifted ? .redo : .undo
        default: return nil
        }
    }

    // MARK: - Sealed launch hook

    /// What `SPCN_OPEN_PANEL_ON_LAUNCH` asks a sealed run to open.
    public struct LaunchRequest: Equatable, Sendable {
        public var route: ClaudePanelRoute
        /// Open the way an auto-open does (without key) rather than a click.
        public var isAuto: Bool
    }

    /// `sessions`, `sessions:<ring>`, `session:<id>` or `setup`, optionally
    /// prefixed `auto:`. Nil for anything else.
    public static func launchRequest(_ value: String) -> LaunchRequest? {
        var text = value.trimmingCharacters(in: .whitespaces)
        let isAuto = text.hasPrefix("auto:")
        if isAuto { text.removeFirst("auto:".count) }
        let parts = text.split(separator: ":", maxSplits: 1, omittingEmptySubsequences: false)
        let argument = parts.count == 2 ? String(parts[1]) : nil
        let route: ClaudePanelRoute
        switch (parts.first.map(String.init), argument) {
        case ("sessions"?, nil): route = .sessions(ringID: nil)
        case ("sessions"?, let ring?) where !ring.isEmpty: route = .sessions(ringID: ring)
        case ("session"?, let id?) where !id.isEmpty: route = .session(id: id)
        case ("setup"?, nil): route = .setup
        default: return nil
        }
        return LaunchRequest(route: route, isAuto: isAuto)
    }

    // MARK: - Hot key

    /// A global shortcut as Carbon's `RegisterEventHotKey` takes it: a virtual
    /// key code and Carbon modifier bits (`cmdKey`, `optionKey`, …), spelled
    /// out so the engine needs no Carbon import. Nil for `.off`.
    public struct HotKeyChord: Equatable, Sendable {
        public var keyCode: UInt32
        public var modifiers: UInt32
    }

    /// Carbon's values: `kVK_Space`, `kVK_ANSI_J`; `cmdKey`, `optionKey`, `controlKey`.
    static let keySpace: UInt32 = 0x31
    static let keyJ: UInt32 = 0x26
    static let carbonCommand: UInt32 = 1 << 8
    static let carbonOption: UInt32 = 1 << 11
    static let carbonControl: UInt32 = 1 << 12

    public static func hotKeyChord(_ choice: PanelHotKey) -> HotKeyChord? {
        switch choice {
        case .off: return nil
        case .controlOptionSpace: return HotKeyChord(keyCode: keySpace, modifiers: carbonControl | carbonOption)
        case .optionCommandJ: return HotKeyChord(keyCode: keyJ, modifiers: carbonOption | carbonCommand)
        }
    }
}
