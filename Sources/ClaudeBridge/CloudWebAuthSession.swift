import AppKit
import AuthenticationServices
import ClaudeControl
import SwiftUI

/// The browser step of signing in to the Agent Notch website (Google,
/// through the website's Supabase project): an `ASWebAuthenticationSession`
/// that opens the authorize URL the engine built and hands back the
/// `agentnotch://auth-callback…` URL the sign-in returned to. The engine
/// does the rest (PKCE exchange, saving the session in its 0600 file).
///
/// - Callback scheme `agentnotch`, matched by the session itself; nothing
///   is registered with Launch Services, so no other app's link can land here.
/// - `prefersEphemeralWebBrowserSession = false`: the browser's own Google
///   login is reused, so a signed-in browser needs one click.
/// - Presented over the settings window, where "Sign in with Google" is.
/// - The user closing the sheet (or the task being cancelled) throws
///   `CancellationError`, which the engine takes as a quiet cancel.
/// - Never in a sealed run: it opens nothing and cancels.
///
/// No Keychain, and nothing of Claude's: this is the app's own login to its
/// own website.
///
/// Fork-only file.
@MainActor
final class CloudWebAuthSession: NSObject, ASWebAuthenticationPresentationContextProviding {
    static let callbackScheme = "agentnotch"

    /// Sessions in flight, kept until they finish: the system holds its
    /// presentation context provider weakly.
    private static var running: Set<CloudWebAuthSession> = []

    private var session: ASWebAuthenticationSession?

    /// `ClaudeCloudBrowser`: open `url`, return the URL it came back to.
    static func present(_ url: URL) async throws -> URL {
        guard !Fork.isSealed else { throw CancellationError() }
        let runner = CloudWebAuthSession()
        running.insert(runner)
        defer { running.remove(runner) }
        return try await runner.run(url)
    }

    private func run(_ url: URL) async throws -> URL {
        try Task.checkCancellation()
        let once = ResumeOnce()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<URL, Error>) in
                once.set(continuation)
                let session = ASWebAuthenticationSession(url: url, callback: .customScheme(Self.callbackScheme)) { callback, error in
                    once.resume(with: Self.outcome(callback: callback, error: error))
                }
                session.presentationContextProvider = self
                session.prefersEphemeralWebBrowserSession = false
                self.session = session
                if !session.start() {
                    once.resume(with: .failure(CloudWebAuthError.couldNotStart))
                }
            }
        } onCancel: {
            Task { @MainActor in self.session?.cancel() }
        }
    }

    /// What the session's completion means for the engine. Pure, for tests.
    nonisolated static func outcome(callback: URL?, error: Error?) -> Result<URL, Error> {
        if let error {
            // Closed by the user, or cancelled by us: a quiet cancel.
            if (error as? ASWebAuthenticationSessionError)?.code == .canceledLogin || error is CancellationError {
                return .failure(CancellationError())
            }
            return .failure(error)
        }
        guard let callback else { return .failure(CloudWebAuthError.noCallback) }
        return .success(callback)
    }

    // MARK: Presentation

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        Self.chooseAnchor(key: NSApp.keyWindow, windows: NSApp.windows,
                          isSettings: Self.isSettingsWindow,
                          isVisibleTitled: { $0.isVisible && $0.styleMask.contains(.titled) })
            ?? ASPresentationAnchor()
    }

    /// The key window when it is the settings window, else the settings
    /// window if it is open, else the first visible titled window. Pure over
    /// its inputs, for tests.
    nonisolated static func chooseAnchor<Window: AnyObject>(key: Window?, windows: [Window],
                                                            isSettings: (Window) -> Bool,
                                                            isVisibleTitled: (Window) -> Bool) -> Window? {
        if let key, isSettings(key) { return key }
        let candidates = windows.filter(isVisibleTitled)
        return candidates.first(where: isSettings) ?? candidates.first
    }

    /// Codenotch's settings window: its content view is the `SettingsView`
    /// host `SettingsWindowController` builds (as `ClaudeSettingsNavigation`
    /// tells it).
    private static func isSettingsWindow(_ window: NSWindow) -> Bool {
        window.contentView is NSHostingView<SettingsView>
    }
}

/// Why the browser step failed, other than a cancel.
enum CloudWebAuthError: Error, Equatable, LocalizedError {
    case couldNotStart
    case noCallback

    var errorDescription: String? {
        switch self {
        case .couldNotStart: return "The sign-in window couldn't open."
        case .noCallback: return "The sign-in ended without an answer from the website."
        }
    }
}

/// A continuation resumed at most once: the session's completion, a failed
/// start and a cancel can race.
private final class ResumeOnce: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<URL, Error>?

    func set(_ continuation: CheckedContinuation<URL, Error>) {
        lock.lock()
        self.continuation = continuation
        lock.unlock()
    }

    func resume(with result: Result<URL, Error>) {
        lock.lock()
        let continuation = self.continuation
        self.continuation = nil
        lock.unlock()
        continuation?.resume(with: result)
    }
}
