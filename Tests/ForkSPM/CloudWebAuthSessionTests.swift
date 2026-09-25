import AuthenticationServices
import Foundation
import Testing
@testable import Codenotch

/// The website sign-in's browser step: what a finished session means for the
/// engine, and which window it is shown over. Nothing is presented here.
@MainActor
struct CloudWebAuthSessionTests {
    private let callback = URL(string: "agentnotch://auth-callback?code=abc")!

    @Test func theCallbackURLIsHandedBack() throws {
        let result = CloudWebAuthSession.outcome(callback: callback, error: nil)
        #expect(try result.get() == callback)
        #expect(CloudWebAuthSession.callbackScheme == "agentnotch")
    }

    /// Closing the sheet is a quiet cancel: the engine goes back to signed
    /// out without an error.
    @Test func closingTheSheetIsACancelNotAnError() {
        let closed = ASWebAuthenticationSessionError(.canceledLogin)
        for error in [closed as Error, CancellationError()] {
            guard case .failure(let failure) = CloudWebAuthSession.outcome(callback: nil, error: error) else {
                Issue.record("expected a failure"); continue
            }
            #expect(failure is CancellationError)
        }
    }

    @Test func otherFailuresAreErrors() {
        let invalid = ASWebAuthenticationSessionError(.presentationContextInvalid)
        guard case .failure(let failure) = CloudWebAuthSession.outcome(callback: nil, error: invalid) else {
            Issue.record("expected a failure"); return
        }
        #expect(!(failure is CancellationError))
        guard case .failure(let empty) = CloudWebAuthSession.outcome(callback: nil, error: nil) else {
            Issue.record("expected a failure"); return
        }
        #expect(empty as? CloudWebAuthError == .noCallback)
    }

    private final class FakeWindow {
        let name: String
        let settings: Bool
        let visibleTitled: Bool
        init(_ name: String, settings: Bool = false, visibleTitled: Bool = true) {
            self.name = name
            self.settings = settings
            self.visibleTitled = visibleTitled
        }
    }

    private func anchor(key: FakeWindow?, _ windows: [FakeWindow]) -> String? {
        CloudWebAuthSession.chooseAnchor(key: key, windows: windows, isSettings: { $0.settings },
                                         isVisibleTitled: { $0.visibleTitled })?.name
    }

    @Test func theSheetGoesOverTheSettingsWindow() {
        let notch = FakeWindow("notch", visibleTitled: false)
        let other = FakeWindow("other")
        let settings = FakeWindow("settings", settings: true)
        #expect(anchor(key: settings, [notch, other, settings]) == "settings")
        // Not key: still the settings window when it is open.
        #expect(anchor(key: other, [notch, other, settings]) == "settings")
        // No settings window: the first visible titled one, never the notch.
        #expect(anchor(key: notch, [notch, other]) == "other")
        #expect(anchor(key: nil, [notch]) == nil)
    }
}
