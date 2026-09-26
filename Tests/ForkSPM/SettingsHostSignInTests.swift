import ClaudeControl
import Foundation
import Testing
@testable import Codenotch

/// The bridge's settings host runs the website sign-in's browser step
/// itself. `ClaudeSettingsHost` has a default `presentWebsiteSignIn` (for
/// hosts written before the website existed) that only says the app can't
/// open the sign-in, so if the bridge's method stopped matching the
/// requirement (a rename, a new label after a merge), the app would still
/// build and every sign-in would fail. Nothing is presented here.
@MainActor
struct SettingsHostSignInTests {
    private let suite = "agentnotch-settings-host-\(UUID().uuidString)"

    private final class Opened {
        var urls: [URL] = []
    }

    /// Regression (review): the pane, which sees only the protocol, reaches
    /// the bridge's own browser step, never the default.
    @Test func thePaneGetsTheBridgesBrowserStep() async throws {
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let opened = Opened()
        let callback = try #require(URL(string: "agentnotch://auth-callback?code=abc"))
        let host: any ClaudeSettingsHost = PreferencesClaudeSettingsHost(preferences: Preferences(defaults: defaults)) { url in
            opened.urls.append(url)
            return callback
        }
        let authorize = try #require(URL(string: "https://example.supabase.co/auth/v1/authorize?provider=google"))
        do {
            #expect(try await host.presentWebsiteSignIn(authorize) == callback)
        } catch {
            Issue.record("the protocol's default answered instead of the bridge: \(error)")
        }
        #expect(opened.urls == [authorize])
    }
}
