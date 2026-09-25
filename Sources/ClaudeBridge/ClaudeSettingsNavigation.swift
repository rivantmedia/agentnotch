import AppKit
import ClaudeControl
import Combine
import SwiftUI

/// Opens Codenotch's settings at a given pane: "Claude Code" (the panel's
/// gear, the consent banner) or "Notifications" (the Claude Code pane's link
/// to the sounds and peek it shares with Codenotch).
///
/// SettingsView selects the pane (U11). Its sections are private to it, so a
/// request says which pane by `Target`, and SettingsView maps that to its own
/// section.
///
/// A request can arrive before anyone listens: the first `openSettings()`
/// builds the settings window, and SwiftUI subscribes the view to these
/// publishers after this call has returned. `selections` holds such a
/// request (`ClaudeHeldRequests`, for a few seconds) and hands it to the
/// first subscriber, so the first click of the gear lands on the pane too.
///
/// The window is opened the way the notch's own gear opens it
/// (`fleet.onOpenSettings`, which `ClaudePanelController.configure` hands
/// over). `NSApp.delegate` is no way to it: under SwiftUI's
/// `@NSApplicationDelegateAdaptor` it is SwiftUI's own delegate object, not
/// the app's `AppDelegate`, so a cast to `AppDelegate` finds nothing.
///
/// Fork-only file. Owned by WP-D.
enum ClaudeSettingsNavigation {
    /// A settings pane the fork opens.
    enum Target: Equatable {
        case claudeCode
        case notifications
    }

    /// Requests for the Claude Code pane, sent as they are made (the U11
    /// listener as Step 0 wrote it).
    @available(*, deprecated, message: "Listen to `selections`; kept only while design §12 lists it (CS-8).")
    static let requests = PassthroughSubject<Void, Never>()

    /// Whether the settings window opens on "Claude Code" rather than
    /// Codenotch's Accounts: while "Turn on Claude Code control" is still
    /// unanswered, so the consent card is what a first launch (or a reopen)
    /// shows (GUX-5). SettingsView reads it for its first selection (U11).
    static var opensOnClaudeCode: Bool {
        opensOnClaudeCode(hookConsent: ClaudeControlSettings.hookConsent, sealed: Fork.isSealed)
    }

    /// Pure, for tests.
    static func opensOnClaudeCode(hookConsent: Bool?, sealed: Bool) -> Bool {
        !sealed && hookConsent == nil
    }

    /// Every pane request, including one made while the settings window was
    /// being built: SettingsView listens here (U11).
    @MainActor static var selections: AnyPublisher<Target, Never> { held.publisher }

    @MainActor private static let held = ClaudeHeldRequests<Target>()

    /// The fleet whose `onOpenSettings` (the notch's gear) opens the window;
    /// read when a pane is asked for, so it does not matter whether the app
    /// delegate had wired it yet when this was set.
    @MainActor private static weak var fleet: NotchFleet?

    /// Called from `ClaudePanelController.configure`.
    @MainActor
    static func configure(fleet: NotchFleet) {
        self.fleet = fleet
    }

    @MainActor
    static func showClaudeCode() {
        show(.claudeCode)
    }

    @MainActor
    static func showNotifications() {
        show(.notifications)
    }

    @MainActor
    private static func show(_ target: Target) {
        // The gear toggles: it would put away a settings window that is
        // already the key window (the Notifications link inside the Claude
        // Code pane is in it). That one only needs its pane changed.
        if !isSettingsWindowKey {
            if let open = fleet?.onOpenSettings {
                open()
            } else {
                Log.sessions.error("claude settings: no way to open the settings window (sealed, or not configured)")
            }
        }
        held.send(target)
    }

    /// Whether Codenotch's settings window is the key window: its content
    /// view is the `SettingsView` host `SettingsWindowController` builds.
    @MainActor
    private static var isSettingsWindowKey: Bool {
        NSApp.keyWindow?.contentView is NSHostingView<SettingsView>
    }
}
