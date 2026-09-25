import ClaudeControl
import SwiftUI

/// The "Claude Code" settings pane (U11): ClaudeControl's pane, given what
/// Codenotch owns (nicknames, connected rings, its Notifications pane,
/// opening the panel) through `Preferences`, and the website sign-in's
/// browser step (`CloudWebAuthSession`).
///
/// Observing `preferences` redraws the pane whenever a nickname or the
/// connected set changes, including from the Accounts pane: both panes read
/// and write the same two preferences, so a rename or a switch in either
/// shows in the other.
///
/// Fork-only file. Owned by WP-D.
struct ClaudeCodeSettingsHost: View {
    @ObservedObject var preferences: Preferences

    init(preferences: Preferences) {
        self.preferences = preferences
    }

    var body: some View {
        if let hub = ClaudeControlHub.shared {
            // A new host on every redraw, so the pane (which cannot observe a
            // protocol) is handed a different value and reads the new names.
            ClaudeSettingsPane(hub: hub, host: PreferencesClaudeSettingsHost(preferences: preferences))
                .claudeControlTheme(.codenotch(surfaceStyle: .solid, colorScheme: .dark,
                                               reduceTransparency: false,
                                               accent: preferences.accentColor.color))
        } else {
            Form {
                Section {
                    Text("Claude Code control is not running.")
                        .foregroundStyle(.secondary)
                }
            }
            .formStyle(.grouped)
        }
    }
}

/// `ClaudeSettingsHost` over Codenotch's preferences.
@MainActor
final class PreferencesClaudeSettingsHost: ClaudeSettingsHost {
    private let preferences: Preferences

    init(preferences: Preferences) {
        self.preferences = preferences
    }

    func nickname(ringID: String) -> String? {
        preferences.nickname(for: ringID)
    }

    /// Nil or blank goes back to the account's own name, as in Accounts.
    func setNickname(_ nickname: String?, ringID: String) {
        preferences.setNickname(nickname ?? "", for: ringID)
    }

    func isRingShown(_ ringID: String) -> Bool {
        preferences.isConnected(ringID)
    }

    func setRingShown(_ on: Bool, ringID: String) {
        preferences.setConnected(on, for: ringID)
    }

    /// Codenotch's Notifications pane: the chimes and the peek Claude
    /// sessions share with every other provider.
    func openNotificationsSettings() {
        ClaudeSettingsNavigation.showNotifications()
    }

    func openSessionsPanel() {
        ClaudePanelController.shared.open(.sessions(ringID: nil), reason: .settings)
    }

    /// "Sign in with Google" in the Cloud section: the browser step, over
    /// this settings window.
    func presentWebsiteSignIn(_ url: URL) async throws -> URL {
        try await CloudWebAuthSession.present(url)
    }
}
