//
//  TerminalVisibilityDetector.swift
//  ClaudeControl
//
//  Two questions the attention reactions ask:
//
//  - Is the user looking at *this* session? Only a session-precise answer
//    counts: its tmux pane is the active pane of a client in the frontmost
//    terminal; its iTerm2 / Terminal tab is the selected one in the front
//    window (a read-only AppleScript query, made only when Automation access
//    was already granted, so it never prompts); or the frontmost app hosts
//    this session and no other. Being frontmost alone is not enough: with four
//    Claude tabs in one iTerm2 window, activating iTerm2 says nothing about
//    which tab the user reads.
//  - Is a terminal window actually visible? Only a terminal or editor window
//    that other windows don't cover counts, not one buried under a browser.
//
//  Answers come from the cached app index and host cache, never from a
//  LaunchServices lookup per window or per session.
//

import AppKit
import CoreGraphics
import Foundation

/// What the precise focus check needs about one session.
nonisolated struct SessionFocusProbe: Sendable, Equatable {
    let pid: Int
    let tty: String?
    let isInTmux: Bool
    /// The other tracked sessions outside tmux, whose host apps tell whether
    /// this one is alone in its app.
    let otherSessionPids: Set<Int>
}

/// An on-screen window, front to back as the window server lists them.
nonisolated struct OnScreenWindow: Sendable, Equatable {
    let ownerPid: Int32
    let ownerName: String?
    let layer: Int
    let bounds: CGRect
    let alpha: Double
}

@MainActor
enum TerminalVisibilityDetector {
    // MARK: - Visible terminal

    /// Whether a terminal or editor window is visible on the current space,
    /// i.e. on screen and not covered by the windows in front of it.
    static func isTerminalVisibleOnCurrentSpace() -> Bool {
        let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
        guard let list = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
            return false
        }
        let windows = list.compactMap(onScreenWindow)
        let apps = RunningApps.shared.index
        return anyTerminalUncovered(windows) { window in
            if let bundleId = apps.byPid[window.ownerPid]?.bundleIdentifier {
                return TerminalAppRegistry.isTerminalBundle(bundleId)
            }
            return window.ownerName.map(TerminalAppRegistry.isTerminal) ?? false
        }
    }

    /// Whether any terminal window (per `isTerminal`) is visible: normal
    /// (layer 0), opaque enough to read, and not hidden under the windows in
    /// front of it. Coverage is sampled on a grid; a window counts as visible
    /// when at least `minimumVisibleFraction` of its samples show. Pure.
    nonisolated static func anyTerminalUncovered(
        _ windows: [OnScreenWindow],
        minimumVisibleFraction: Double = 0.15,
        isTerminal: (OnScreenWindow) -> Bool
    ) -> Bool {
        var occluders: [CGRect] = []
        for window in windows where window.layer == 0 && window.alpha > 0.05 {
            guard window.bounds.width >= 40, window.bounds.height >= 40 else { continue }
            if isTerminal(window) {
                let samples = gridPoints(in: window.bounds, count: 5)
                let visible = samples.filter { point in !occluders.contains { $0.contains(point) } }.count
                if Double(visible) / Double(samples.count) >= minimumVisibleFraction {
                    return true
                }
            }
            occluders.append(window.bounds)
        }
        return false
    }

    nonisolated private static func gridPoints(in rect: CGRect, count: Int) -> [CGPoint] {
        var points: [CGPoint] = []
        for row in 0..<count {
            for column in 0..<count {
                points.append(CGPoint(
                    x: rect.minX + rect.width * (CGFloat(column) + 0.5) / CGFloat(count),
                    y: rect.minY + rect.height * (CGFloat(row) + 0.5) / CGFloat(count)
                ))
            }
        }
        return points
    }

    nonisolated private static func onScreenWindow(_ info: [String: Any]) -> OnScreenWindow? {
        guard let pid = info[kCGWindowOwnerPID as String] as? Int32,
              let layer = info[kCGWindowLayer as String] as? Int,
              let boundsDict = info[kCGWindowBounds as String] as? NSDictionary,
              let bounds = CGRect(dictionaryRepresentation: boundsDict) else { return nil }
        return OnScreenWindow(
            ownerPid: pid,
            ownerName: info[kCGWindowOwnerName as String] as? String,
            layer: layer,
            bounds: bounds,
            alpha: (info[kCGWindowAlpha as String] as? Double) ?? 1
        )
    }

    // MARK: - Focused session

    /// What the focus check needs about `session`; nil without a pid.
    static func probe(for session: SessionState) -> SessionFocusProbe? {
        guard let pid = session.pid else { return nil }
        let others = ClaudeSessionMonitor.shared.instances.compactMap { other -> Int? in
            guard other.sessionId != session.sessionId, !other.isInTmux, let otherPid = other.pid, otherPid != pid else {
                return nil
            }
            return otherPid
        }
        return SessionFocusProbe(pid: pid, tty: session.tty, isInTmux: session.isInTmux, otherSessionPids: Set(others))
    }

    /// Whether the user is looking at this session's own terminal tab or pane
    /// (see the file comment for what counts).
    static func isSessionFocused(_ probe: SessionFocusProbe) async -> Bool {
        guard let frontmost = RunningApps.shared.frontmostPid ?? NSWorkspace.shared.frontmostApplication?.processIdentifier,
              ProcessID.isRunning(probe.pid) else {
            return false
        }
        if probe.isInTmux {
            return await isTmuxPaneFocused(claudePid: probe.pid, frontmost: frontmost)
        }
        guard let host = await SessionHostCache.shared.resolve(pid: probe.pid), host.pid == frontmost else {
            return false
        }
        var selected: String?
        if let terminal = host.scriptableTerminal, probe.tty != nil {
            selected = await selectedTTY(of: terminal)
        }
        // Hosts of the other sessions, resolved now: a cold cache must not
        // make a crowded app look like it holds this session alone.
        let others = selected == nil ? await SessionHostCache.shared.resolve(pids: probe.otherSessionPids) : [:]
        return isFocused(
            sessionTTY: probe.tty,
            selectedTTY: selected,
            hostPid: host.pid,
            otherHostPids: others.values.map { $0?.pid }
        )
    }

    /// The frontmost app hosts the session: it is looked at when its tab is
    /// the selected one (when the terminal can say), else only when no other
    /// session runs in that app. Pure.
    nonisolated static func isFocused(
        sessionTTY: String?,
        selectedTTY: String?,
        hostPid: Int32,
        otherHostPids: [Int32?]
    ) -> Bool {
        if let selectedTTY, let sessionTTY {
            return TerminalScript.devicePath(forTTY: selectedTTY) == TerminalScript.devicePath(forTTY: sessionTTY)
        }
        return !otherHostPids.contains(hostPid)
    }

    /// Claude's pane is the active pane of a tmux client whose terminal is
    /// frontmost (and, for iTerm2 / Terminal, whose tab is the selected one).
    private static func isTmuxPaneFocused(claudePid: Int, frontmost: Int32) async -> Bool {
        guard let target = await TmuxController.shared.findTmuxTarget(forClaudePid: claudePid) else { return false }
        for client in await TmuxController.shared.clients(ofSession: target.session) {
            guard let host = await SessionHostCache.shared.resolve(pid: client.pid), host.pid == frontmost,
                  await TmuxController.shared.activePane(ofClientTTY: client.tty) == target.targetString else {
                continue
            }
            if let terminal = host.scriptableTerminal, let selected = await selectedTTY(of: terminal) {
                if TerminalScript.devicePath(forTTY: selected) == TerminalScript.devicePath(forTTY: client.tty) {
                    return true
                }
                continue
            }
            return true
        }
        return false
    }

    /// The TTY of the selected tab in the front window, or nil when it can't
    /// be asked without prompting for Automation access (or can't be read).
    /// A burst of checks (several sessions finishing together) asks once:
    /// answers are reused for `selectedTTYLifetime`.
    private static func selectedTTY(of terminal: ScriptableTerminal) async -> String? {
        let now = Date()
        if let cached = selectedTTYCache[terminal], now.timeIntervalSince(cached.at) < selectedTTYLifetime {
            return cached.tty
        }
        var tty: String?
        if await AutomationPermission.isGranted(bundleIdentifier: terminal.bundleIdentifier) {
            let outcome = await TerminalScriptRunner.shared.query(TerminalScript.selectedTTY(terminal), label: "selected tab of \(terminal.displayName)")
            tty = outcome.flatMap { $0.isEmpty ? nil : $0 }
        }
        selectedTTYCache[terminal] = (tty, Date())
        return tty
    }

    static let selectedTTYLifetime: TimeInterval = 1
    private static var selectedTTYCache: [ScriptableTerminal: (tty: String?, at: Date)] = [:]
}

/// Whether this app may already send Apple events to another app, asked
/// without ever showing the consent prompt.
nonisolated enum AutomationPermission {
    @concurrent
    static func isGranted(bundleIdentifier: String) async -> Bool {
        var target = AEAddressDesc()
        let created = bundleIdentifier.withCString { pointer in
            AECreateDesc(DescType(typeApplicationBundleID), pointer, strlen(pointer), &target)
        }
        guard created == noErr else { return false }
        defer { AEDisposeDesc(&target) }
        let status = AEDeterminePermissionToAutomateTarget(&target, AEEventClass(typeWildCard), AEEventID(typeWildCard), false)
        return status == noErr
    }
}
