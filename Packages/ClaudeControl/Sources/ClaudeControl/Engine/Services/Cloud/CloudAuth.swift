//
//  CloudAuth.swift
//  ClaudeControl
//
//  Signing in to the website: Supabase Auth with Google, as a PKCE flow
//  (RFC 7636) whose browser step the host runs (an
//  ASWebAuthenticationSession with the `agentnotch://auth-callback`
//  redirect). This is the app's own login to its own website; it has
//  nothing to do with Claude's login, which the app never reads.
//
//  1. authorize: `{supabase}/auth/v1/authorize?provider=google&redirect_to=…
//     &code_challenge=<S256(verifier)>&code_challenge_method=s256`
//  2. the browser comes back to `agentnotch://auth-callback?code=…` (or
//     `error`/`error_description`, in the query or the fragment)
//  3. exchange: `POST {supabase}/auth/v1/token?grant_type=pkce`
//     `{auth_code, code_verifier}` with the publishable key as `apikey`
//  4. refresh: `POST …/token?grant_type=refresh_token {refresh_token}`, one
//     at a time. Supabase rotates the refresh token, so the new session is
//     saved before its access token is used. The session is forgotten only
//     when Supabase refuses the refresh token itself (400/401 naming an
//     invalid grant, a used, revoked or unknown refresh token, or an
//     ended session); anything else (a 5xx, a 429, the network) is tried
//     again later with the session kept.
//  5. sign out: `POST …/logout?scope=local` (this Mac's session only), then
//     the saved session is removed whatever the answer.
//
//  A session belongs to the website it was made through: an access token
//  is only ever handed out for that website (`accessGrant(for:)`), and a
//  request is bound to the sign-in it started with: the token its 401 retry
//  gets is of that same sign-in and website, never of one made since
//  (`refreshAfterUnauthorized`).
//
//  The session lives in `<support>/cloud-session.json` (0600 in the 0700
//  engine folder, written atomically), never when sealed or before
//  bootstrap. Tests use `CloudSessionMemoryStore`.
//

import CryptoKit
import Foundation
import os.log
import Security

/// The host's browser step: open the URL, return the callback URL.
public typealias ClaudeCloudBrowser = @MainActor @Sendable (URL) async throws -> URL

/// What a host with no browser step for the website sign-in throws (the
/// default `ClaudeSettingsHost.presentWebsiteSignIn`): the sign-in fails and
/// says so, where a cancellation would look like the user closed a window
/// that never opened.
public nonisolated struct ClaudeWebsiteSignInUnavailable: LocalizedError, Equatable, Sendable {
    public init() {}

    public var errorDescription: String? { "This app can't open the website's sign-in." }
}

// MARK: - Session

/// A Supabase session for the website, and where it came from.
nonisolated struct CloudAuthSession: Codable, Equatable, Sendable {
    var accessToken: String
    var refreshToken: String
    var expiresAt: Date
    var userId: String?
    var email: String?
    /// The Supabase project and its publishable (public) key, so a refresh
    /// or a sign-out needs no config request.
    var supabaseUrl: String
    var publishableKey: String
    /// The website it was made through; another website means signing in again.
    var websiteURL: String
}

/// An access token, the website it may be sent to, and the sign-in it
/// belongs to (`CloudAuth`'s count of sessions adopted or dropped): what a
/// request needs to ask for its 401 retry's token.
nonisolated struct CloudAccessGrant: Equatable, Sendable {
    var token: String
    var website: String
    var signIn: Int
}

/// Where the session is kept.
nonisolated protocol CloudSessionStoring: Sendable {
    func load() -> CloudAuthSession?
    func save(_ session: CloudAuthSession) throws
    func clear()
}

/// `<support>/cloud-session.json`.
nonisolated final class CloudSessionFileStore: CloudSessionStoring, @unchecked Sendable {
    static let fileName = "cloud-session.json"

    /// A bootstrapped live run only.
    static let defaultGate: @Sendable () -> Bool = { AppIdentity.isFrozen && !AppIdentity.isSealed }

    enum StoreError: Error, Equatable { case notAllowed }

    private let directory: @Sendable () -> URL
    private let isAllowed: @Sendable () -> Bool
    private let lock = NSLock()

    /// - Parameters:
    ///   - directory: the engine's folder (asked for only once allowed).
    ///   - isAllowed: false when sealed or before bootstrap: nothing is read or written then.
    init(directory: @escaping @Sendable () -> URL = { AppIdentity.supportDirectory },
         isAllowed: @escaping @Sendable () -> Bool = CloudSessionFileStore.defaultGate) {
        self.directory = directory
        self.isAllowed = isAllowed
    }

    private var fileURL: URL { directory().appendingPathComponent(Self.fileName) }

    func load() -> CloudAuthSession? {
        guard isAllowed() else { return nil }
        return lock.withLock {
            guard let data = try? Data(contentsOf: fileURL) else { return nil }
            return try? CloudJSON.makeDecoder().decode(CloudAuthSession.self, from: data)
        }
    }

    func save(_ session: CloudAuthSession) throws {
        guard isAllowed() else { throw StoreError.notAllowed }
        let data = try CloudJSON.makeEncoder().encode(session)
        try lock.withLock {
            try CloudFiles.writeAtomically(data, to: fileURL)
        }
    }

    func clear() {
        guard isAllowed() else { return }
        lock.withLock {
            _ = try? FileManager.default.removeItem(at: fileURL)
        }
    }
}

/// In memory only (tests; a sealed run never signs in).
nonisolated final class CloudSessionMemoryStore: CloudSessionStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var session: CloudAuthSession?
    private(set) var saveCount = 0

    init(_ session: CloudAuthSession? = nil) {
        self.session = session
    }

    func load() -> CloudAuthSession? { lock.withLock { session } }

    func save(_ session: CloudAuthSession) throws {
        lock.withLock {
            self.session = session
            saveCount += 1
        }
    }

    func clear() { lock.withLock { session = nil } }
}

// MARK: - PKCE

nonisolated enum PKCE {
    /// A verifier from 32 random bytes: 43 base64url characters (RFC 7636
    /// allows 43–128).
    static func makeVerifier() -> String {
        var bytes = [UInt8](repeating: 0, count: 32)
        let status = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        if status != errSecSuccess {
            // Never a predictable verifier: fall back to the system CSPRNG.
            bytes = (0..<32).map { _ in UInt8.random(in: .min ... .max) }
        }
        return verifier(fromBytes: bytes)
    }

    static func verifier(fromBytes bytes: [UInt8]) -> String {
        base64URL(Data(bytes))
    }

    /// `S256`: base64url(SHA-256(ASCII(verifier))), no padding.
    static func challenge(for verifier: String) -> String {
        base64URL(Data(SHA256.hash(data: Data(verifier.utf8))))
    }

    static func base64URL(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}

// MARK: - Errors

nonisolated enum CloudAuthError: Error, Equatable, Sendable, LocalizedError {
    /// The website's config names no usable Supabase project.
    case badConfig(String)
    /// The browser came back somewhere else, or without a code.
    case invalidCallback
    /// Supabase or Google said no.
    case provider(String)
    /// The user closed the sign-in window.
    case cancelled
    /// No session (never signed in, signed out, or the refresh token was refused).
    case signedOut
    /// The session was made through another website than the one asked for.
    case otherWebsite
    /// Supabase answered with an error (`code`: its `error_code` or `error`).
    case http(status: Int, code: String?, message: String?)
    case transport(String)
    case badResponse(String)

    var errorDescription: String? {
        switch self {
        case .badConfig(let detail): return "The website's sign-in settings aren't usable: \(detail)"
        case .invalidCallback: return "Sign-in didn't come back with a code."
        case .provider(let message): return message
        case .cancelled: return "Sign-in was cancelled."
        case .signedOut: return "Signed out. Sign in again."
        case .otherWebsite: return "Signed in to another website. Sign in again."
        case .http(let status, _, let message): return message ?? "Sign-in failed (\(status))."
        case .transport(let message): return "Couldn't reach the sign-in service: \(message)"
        case .badResponse(let message): return "Unexpected answer from the sign-in service: \(message)"
        }
    }
}

// MARK: - Auth

actor CloudAuth {
    private static var logger: Logger { EngineLog.logger("CloudAuth") }

    /// Refresh when the access token has less than this left.
    static let refreshMargin: TimeInterval = 60

    private let transport: any CloudTransport
    private let store: any CloudSessionStoring
    private let clock: @Sendable () -> Date
    private var session: CloudAuthSession?
    private var loaded = false
    private var refreshTask: Task<CloudAuthSession, Error>?
    /// Which sign-in `session` is: bumped whenever it is replaced or dropped
    /// (adopted, forgotten, set aside), never by a refresh.
    private var signInEpoch = 0

    init(transport: any CloudTransport, store: any CloudSessionStoring,
         clock: @escaping @Sendable () -> Date = { Date() }) {
        self.transport = transport
        self.store = store
        self.clock = clock
    }

    /// The saved session, if any.
    func currentSession() -> CloudAuthSession? {
        loadIfNeeded()
        return session
    }

    private func loadIfNeeded() {
        guard !loaded else { return }
        loaded = true
        session = store.load()
    }

    // MARK: Sign in

    /// Where the browser goes first.
    static func authorizeURL(supabaseUrl: URL, challenge: String,
                             redirect: String = CloudContract.redirectURL) -> URL? {
        guard var components = URLComponents(url: supabaseUrl.appendingPathComponent("auth/v1/authorize"),
                                             resolvingAgainstBaseURL: false) else { return nil }
        components.queryItems = [
            URLQueryItem(name: "provider", value: "google"),
            URLQueryItem(name: "redirect_to", value: redirect),
            URLQueryItem(name: "code_challenge", value: challenge),
            URLQueryItem(name: "code_challenge_method", value: "s256"),
        ]
        return components.url
    }

    /// The code in the callback, or the error it carries (query or fragment).
    static func authorizationCode(fromCallback url: URL) throws -> String {
        guard url.scheme?.lowercased() == CloudContract.callbackScheme,
              url.host?.lowercased() == CloudContract.callbackHost else { throw CloudAuthError.invalidCallback }
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        // Pairs parsed by hand: the callback is someone else's text, and
        // URLComponents traps on a malformed percent-encoded query set on it.
        let pairs = formPairs(components?.percentEncodedQuery) + formPairs(components?.percentEncodedFragment)
        func value(_ name: String) -> String? {
            pairs.first { $0.name == name && !$0.value.isEmpty }?.value
        }
        if let error = value("error") ?? value("error_code") {
            throw CloudAuthError.provider(String((value("error_description") ?? error).prefix(300)))
        }
        guard let code = value("code") else { throw CloudAuthError.invalidCallback }
        return code
    }

    /// `a=1&b=two+words` as (name, value) pairs, `+` as a space and
    /// percent escapes decoded (a malformed escape is kept as written). Pure.
    static func formPairs(_ encoded: String?) -> [(name: String, value: String)] {
        guard let encoded, !encoded.isEmpty else { return [] }
        func decode(_ part: Substring) -> String {
            let spaced = part.replacingOccurrences(of: "+", with: " ")
            return spaced.removingPercentEncoding ?? spaced
        }
        return encoded.split(separator: "&", omittingEmptySubsequences: true).map { pair in
            let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            return (decode(parts[0]), parts.count > 1 ? decode(parts[1]) : "")
        }
    }

    /// Google sign-in through Supabase, the browser step run by the host.
    /// Returns the new session without using it: the caller `adopt`s it
    /// (saved, and used from then on) once it knows it is still wanted, or
    /// `revoke`s it.
    func signIn(config: CloudConfig, website: String, presentBrowser: ClaudeCloudBrowser) async throws -> CloudAuthSession {
        guard let supabase = CloudWebsite.validatedLink(config.supabaseUrl) else {
            throw CloudAuthError.badConfig("supabaseUrl")
        }
        guard !config.supabasePublishableKey.isEmpty else { throw CloudAuthError.badConfig("supabasePublishableKey") }
        let verifier = PKCE.makeVerifier()
        guard let url = Self.authorizeURL(supabaseUrl: supabase, challenge: PKCE.challenge(for: verifier)) else {
            throw CloudAuthError.badConfig("supabaseUrl")
        }
        let callback: URL
        do {
            callback = try await presentBrowser(url)
        } catch let error as CloudAuthError {
            throw error
        } catch {
            throw Self.isCancellation(error) ? CloudAuthError.cancelled : CloudAuthError.provider(error.localizedDescription)
        }
        let code = try Self.authorizationCode(fromCallback: callback)
        let body = try JSONSerialization.data(withJSONObject: ["auth_code": code, "code_verifier": verifier])
        let answer = try await tokenRequest(supabase: supabase, grant: "pkce", body: body,
                                            publishableKey: config.supabasePublishableKey)
        return try Self.session(from: answer, supabaseUrl: supabase.absoluteString,
                                publishableKey: config.supabasePublishableKey, website: website, now: clock())
    }

    /// Use `new` from now on, and save it.
    func adopt(_ new: CloudAuthSession) {
        loaded = true
        session = new
        signInEpoch += 1
        do {
            try store.save(new)
        } catch {
            // Signed in for this run; the next launch asks again.
            Self.logger.error("Couldn't save the website session: \(String(describing: error), privacy: .public)")
        }
        Self.logger.notice("Signed in to the website")
    }

    /// End `unwanted` on Supabase (best effort), a session that was never
    /// adopted: the saved one, if any, is left alone.
    func revoke(_ unwanted: CloudAuthSession) async {
        await logout(unwanted)
    }

    /// A browser step that ended because the user closed it.
    static func isCancellation(_ error: Error) -> Bool {
        if error is CancellationError { return true }
        let error = error as NSError
        // ASWebAuthenticationSessionError.canceledLogin, without linking AuthenticationServices.
        return error.domain == "com.apple.AuthenticationServices.WebAuthenticationSession" && error.code == 1
    }

    // MARK: Tokens

    /// An access token with at least a minute left, refreshing first when needed.
    func validAccessToken() async throws -> String {
        loadIfNeeded()
        guard let current = session else { throw CloudAuthError.signedOut }
        if current.expiresAt.timeIntervalSince(clock()) > Self.refreshMargin {
            return current.accessToken
        }
        return try await refresh().accessToken
    }

    /// `validAccessToken()`, only for the website the session was made
    /// through: another website never sees this one's token.
    func validAccessToken(for website: String) async throws -> String {
        try await accessGrant(for: website).token
    }

    /// An access token for `website` with at least a minute left (refreshed
    /// first when needed), and the sign-in it belongs to. Only a session
    /// made through `website` gives one: another website never sees this
    /// one's token, even if the session was replaced during the refresh.
    func accessGrant(for website: String) async throws -> CloudAccessGrant {
        loadIfNeeded()
        guard let current = session else { throw CloudAuthError.signedOut }
        guard current.websiteURL == website else { throw CloudAuthError.otherWebsite }
        let epoch = signInEpoch
        if current.expiresAt.timeIntervalSince(clock()) > Self.refreshMargin {
            return CloudAccessGrant(token: current.accessToken, website: website, signIn: epoch)
        }
        let fresh = try await refresh()
        return CloudAccessGrant(token: try Self.check(fresh, website: website, epoch: epoch, now: signInEpoch),
                                website: website, signIn: epoch)
    }

    /// Whether Supabase's answer to a refresh says the refresh token itself
    /// is no good (used, revoked, expired, unknown, or its session ended),
    /// rather than that something failed on the way. Pure.
    static func refusesRefreshToken(status: Int, code: String?, message: String?) -> Bool {
        guard status == 400 || status == 401 else { return false }
        let refusals: Set<String> = ["invalid_grant", "refresh_token_not_found", "refresh_token_already_used",
                                     "session_not_found", "session_expired", "user_not_found", "user_banned"]
        if let code = code?.lowercased(), refusals.contains(code) { return true }
        return message?.lowercased().contains("invalid refresh token") == true
    }

    /// After `website` answered 401 to `failedToken`, which sign-in `signIn`
    /// gave (`CloudAccessGrant`): a fresh access token of that same sign-in,
    /// for that same website. When another call already refreshed past it,
    /// that one is used. The user may have signed out and in again while
    /// the request was out: a session made through another website is
    /// never handed out (`otherWebsite`), and neither is another sign-in's
    /// on the same website (`signedOut`: the request's sign-in is over).
    func refreshAfterUnauthorized(failedToken: String, website: String, signIn: Int) async throws -> String {
        loadIfNeeded()
        guard let current = session else { throw CloudAuthError.signedOut }
        let token = try Self.check(current, website: website, epoch: signIn, now: signInEpoch)
        if token != failedToken, current.expiresAt.timeIntervalSince(clock()) > Self.refreshMargin {
            return token
        }
        let fresh = try await refresh()
        return try Self.check(fresh, website: website, epoch: signIn, now: signInEpoch)
    }

    /// The session's access token, if it was made through `website` and is
    /// still the sign-in `epoch` (`now` is the current one). Pure.
    private static func check(_ session: CloudAuthSession, website: String, epoch: Int, now: Int) throws -> String {
        guard session.websiteURL == website else { throw CloudAuthError.otherWebsite }
        guard epoch == now else { throw CloudAuthError.signedOut }
        return session.accessToken
    }

    /// Refresh now. Concurrent callers share one request: the refresh token
    /// is single-use.
    func refresh() async throws -> CloudAuthSession {
        loadIfNeeded()
        if let refreshTask { return try await refreshTask.value }
        guard let current = session else { throw CloudAuthError.signedOut }
        let task = Task { try await self.performRefresh(current) }
        refreshTask = task
        defer { refreshTask = nil }
        return try await task.value
    }

    private func performRefresh(_ current: CloudAuthSession) async throws -> CloudAuthSession {
        guard let supabase = CloudWebsite.validatedLink(current.supabaseUrl) else {
            forget()
            throw CloudAuthError.signedOut
        }
        let body = try JSONSerialization.data(withJSONObject: ["refresh_token": current.refreshToken])
        let answer: [String: Any]
        do {
            answer = try await tokenRequest(supabase: supabase, grant: "refresh_token", body: body,
                                            publishableKey: current.publishableKey)
        } catch CloudAuthError.http(let status, let code, let message)
                    where Self.refusesRefreshToken(status: status, code: code, message: message) {
            // The refresh token was refused (used, revoked, expired): the
            // session is over (unless another one replaced it meanwhile).
            Self.logger.notice("Session refresh refused (\(status), \(code ?? "no code", privacy: .public)): signed out")
            if session?.refreshToken == current.refreshToken { forget() }
            throw CloudAuthError.signedOut
        }
        // Signed out (or signed in again) while the request was out: this
        // answer belongs to a session that is over.
        guard session?.refreshToken == current.refreshToken else { throw CloudAuthError.signedOut }
        var new = try Self.session(from: answer, supabaseUrl: current.supabaseUrl, publishableKey: current.publishableKey,
                                   website: current.websiteURL, now: clock())
        new.userId = new.userId ?? current.userId
        new.email = new.email ?? current.email
        // Saved before use: the old refresh token no longer works.
        do {
            try store.save(new)
        } catch {
            Self.logger.error("Couldn't save the refreshed session: \(String(describing: error), privacy: .public)")
        }
        session = new
        return new
    }

    /// Sign out on Supabase (best effort; this Mac's session only, not the
    /// user's other ones), then forget the session.
    func signOut() async {
        loadIfNeeded()
        if let current = session {
            forget()
            await logout(current)
        } else {
            forget()
        }
    }

    /// `POST …/logout?scope=local` with the session's own token.
    private func logout(_ ended: CloudAuthSession) async {
        guard let supabase = CloudWebsite.validatedLink(ended.supabaseUrl),
              var components = URLComponents(url: supabase.appendingPathComponent("auth/v1/logout"),
                                             resolvingAgainstBaseURL: false) else { return }
        components.queryItems = [URLQueryItem(name: "scope", value: "local")]
        guard let url = components.url else { return }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue(ended.publishableKey, forHTTPHeaderField: "apikey")
        request.setValue("Bearer \(ended.accessToken)", forHTTPHeaderField: "Authorization")
        _ = try? await transport.send(request)
    }

    /// Stop using the saved session without removing it: it was made
    /// through another website than the one set now (a dev run pointed
    /// elsewhere shares the engine's folder; the file stays the app's).
    func setAside() {
        loaded = true
        session = nil
        signInEpoch += 1
    }

    /// Forget the session here (no request).
    func forget() {
        loaded = true
        session = nil
        signInEpoch += 1
        store.clear()
    }

    // MARK: Requests

    private func tokenRequest(supabase: URL, grant: String, body: Data, publishableKey: String) async throws -> [String: Any] {
        guard var components = URLComponents(url: supabase.appendingPathComponent("auth/v1/token"),
                                             resolvingAgainstBaseURL: false) else { throw CloudAuthError.badConfig("supabaseUrl") }
        components.queryItems = [URLQueryItem(name: "grant_type", value: grant)]
        guard let url = components.url else { throw CloudAuthError.badConfig("supabaseUrl") }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.httpBody = body
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue(publishableKey, forHTTPHeaderField: "apikey")
        let data: Data
        let response: HTTPURLResponse
        do {
            (data, response) = try await transport.send(request)
        } catch {
            throw CloudAuthError.transport(error.localizedDescription)
        }
        let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        guard (200..<300).contains(response.statusCode) else {
            let message = (object?["error_description"] as? String) ?? (object?["msg"] as? String)
                ?? (object?["message"] as? String) ?? (object?["error"] as? String)
            let code = (object?["error_code"] as? String) ?? (object?["error"] as? String)
            throw CloudAuthError.http(status: response.statusCode, code: code, message: message)
        }
        guard let object else { throw CloudAuthError.badResponse("not JSON") }
        return object
    }

    /// A Supabase token answer as a session. Pure.
    static func session(from answer: [String: Any], supabaseUrl: String, publishableKey: String,
                        website: String, now: Date) throws -> CloudAuthSession {
        guard let access = answer["access_token"] as? String, !access.isEmpty,
              let refresh = answer["refresh_token"] as? String, !refresh.isEmpty else {
            throw CloudAuthError.badResponse("no tokens")
        }
        let expiresAt: Date
        if let epoch = JSONValue.double(answer["expires_at"]), epoch > 0 {
            expiresAt = Date(timeIntervalSince1970: epoch)
        } else if let seconds = JSONValue.double(answer["expires_in"]), seconds > 0 {
            expiresAt = now.addingTimeInterval(seconds)
        } else {
            expiresAt = now.addingTimeInterval(3600)
        }
        let user = answer["user"] as? [String: Any]
        return CloudAuthSession(
            accessToken: access,
            refreshToken: refresh,
            expiresAt: expiresAt,
            userId: user?["id"] as? String,
            email: user?["email"] as? String,
            supabaseUrl: supabaseUrl,
            publishableKey: publishableKey,
            websiteURL: website
        )
    }
}
