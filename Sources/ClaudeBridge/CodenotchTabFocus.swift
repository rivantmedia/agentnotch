import ClaudeControl
import Foundation

/// The "external" step of ClaudeControl's focus chain
/// (`ClaudeControlConfiguration.externalTabFocus`): selecting a session's tab
/// in the terminals ClaudeControl cannot script itself, through Codenotch's
/// `TerminalTabFocus`.
///
/// ClaudeControl's own steps come first (tmux, then iTerm2 and Terminal by
/// tty) and the editor families after, so this answers only for the
/// terminals in between: cmux, which it finds by the surface id the session's
/// process tree carries or by working directory, and Ghostty, by working
/// directory. Anything else is false at once, so no terminal is scripted
/// twice.
///
/// Blocks for as long as `osascript` takes (up to a few seconds, the first
/// time behind an Automation prompt); ClaudeControl calls it off the main
/// actor. Never in sealed runs.
///
/// Fork-only file. Owned by WP-C.
enum CodenotchTabFocus {
    /// The terminals this step handles, by bundle id.
    static let bundleIDs: Set<String> = ["com.cmuxterm.app", "com.mitchellh.ghostty"]

    /// For `ClaudeControlConfiguration.externalTabFocus`.
    static let select: @Sendable (ClaudeExternalTabRequest) -> Bool = { request in
        guard handles(request, sealed: Fork.isSealed) else { return false }
        if Thread.isMainThread {
            Log.sessions.fault("tab focus asked for on the main thread; it runs osascript and blocks")
        }
        let selected = TerminalTabFocus.selectTab(bundleID: request.bundleID, pid: request.pid,
                                                  tty: request.tty, cwd: request.cwd)
        Log.sessions.info("tab focus \(request.bundleID ?? "?", privacy: .public) pid \(request.pid, privacy: .public): \(selected ? "selected" : "not found", privacy: .public)")
        return selected
    }

    /// Whether this step answers for `request` at all. Pure.
    static func handles(_ request: ClaudeExternalTabRequest, sealed: Bool) -> Bool {
        guard !sealed, request.pid > 0, let bundleID = request.bundleID else { return false }
        return bundleIDs.contains(bundleID)
    }
}
