import Foundation
import Testing
@testable import ClaudeControl

/// Signing in to the website: PKCE, the Supabase exchange and refresh, the
/// 401 retry, signing out, and where the session is kept. A stand-in
/// network only.
struct CloudAuthTests {
    nonisolated static let supabase = "https://abcdefghijklmnop.supabase.co"
    nonisolated static let config = CloudConfig(supabaseUrl: supabase, supabasePublishableKey: "sb_publishable_test",
                                    redirectUrl: CloudContract.redirectURL,
                                    dashboardUrl: "https://agentnotch.example.com/dashboard")

    nonisolated static func session(expiresIn seconds: TimeInterval, access: String = "access-1", refresh: String = "refresh-1",
                        now: Date = Date()) -> CloudAuthSession {
        CloudAuthSession(accessToken: access, refreshToken: refresh, expiresAt: now.addingTimeInterval(seconds),
                         userId: "user-1", email: "me@example.com", supabaseUrl: supabase,
                         publishableKey: "sb_publishable_test", websiteURL: "https://agentnotch.example.com")
    }

    nonisolated static func tokenAnswer(access: String, refresh: String, expiresIn: Int = 3600) -> FakeTransport.Answer {
        .json(200, ["access_token": access, "refresh_token": refresh, "expires_in": expiresIn, "token_type": "bearer",
                    "user": ["id": "user-1", "email": "me@example.com"]])
    }

    // MARK: - PKCE

    @Test func pkceMatchesRFC7636AppendixB() {
        let octets: [UInt8] = [116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37, 77,
                               105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121]
        let verifier = PKCE.verifier(fromBytes: octets)
        #expect(verifier == "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")
        #expect(PKCE.challenge(for: verifier) == "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
    }

    @Test func verifiersAreRandomAndWellFormed() {
        let allowed = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")
        let first = PKCE.makeVerifier()
        let second = PKCE.makeVerifier()
        #expect(first != second)
        for verifier in [first, second] {
            #expect((43...128).contains(verifier.count))
            #expect(verifier.allSatisfy(allowed.contains))
        }
        #expect(!PKCE.challenge(for: first).contains("="))
    }

    @Test func authorizeURLAsksGoogleWithTheChallenge() throws {
        let url = try #require(CloudAuth.authorizeURL(supabaseUrl: URL(string: Self.supabase)!, challenge: "abc"))
        let components = try #require(URLComponents(url: url, resolvingAgainstBaseURL: false))
        #expect(components.path == "/auth/v1/authorize")
        let items = Dictionary(uniqueKeysWithValues: (components.queryItems ?? []).map { ($0.name, $0.value ?? "") })
        #expect(items == ["provider": "google", "redirect_to": "agentnotch://auth-callback",
                          "code_challenge": "abc", "code_challenge_method": "s256"])
    }

    @Test func callbacksGiveTheCodeOrTheError() throws {
        #expect(try CloudAuth.authorizationCode(fromCallback: URL(string: "agentnotch://auth-callback?code=abc-123")!) == "abc-123")
        #expect(try CloudAuth.authorizationCode(fromCallback: URL(string: "agentnotch://auth-callback#code=frag")!) == "frag")
        #expect(throws: CloudAuthError.provider("Email link is invalid or has expired")) {
            try CloudAuth.authorizationCode(fromCallback: URL(string:
                "agentnotch://auth-callback#error=access_denied&error_description=Email+link+is+invalid+or+has+expired")!)
        }
        #expect(throws: CloudAuthError.provider("access_denied")) {
            try CloudAuth.authorizationCode(fromCallback: URL(string: "agentnotch://auth-callback?error=access_denied")!)
        }
        #expect(throws: CloudAuthError.invalidCallback) {
            try CloudAuth.authorizationCode(fromCallback: URL(string: "https://evil.example.com/auth-callback?code=x")!)
        }
        #expect(throws: CloudAuthError.invalidCallback) {
            try CloudAuth.authorizationCode(fromCallback: URL(string: "agentnotch://auth-callback")!)
        }
        // A malformed escape is someone else's text, never a crash.
        #expect(try CloudAuth.authorizationCode(fromCallback: URL(string: "agentnotch://auth-callback?x=%zz&code=ok")!) == "ok")
        #expect(CloudAuth.formPairs("a=%zz&b=two+words&c").map(\.value) == ["%zz", "two words", ""])
    }

    // MARK: - Exchange

    @Test func signInExchangesTheCodeAndSavesTheSession() async throws {
        let transport = FakeTransport { request in
            guard request.url?.path == "/auth/v1/token" else { return .json(404, [:]) }
            return Self.tokenAnswer(access: "access-new", refresh: "refresh-new")
        }
        let store = CloudSessionMemoryStore()
        let auth = CloudAuth(transport: transport, store: store)
        let opened = OpenedURLs()
        let session = try await auth.signIn(config: Self.config, website: "https://agentnotch.example.com") { url in
            opened.add(url)
            return URL(string: "agentnotch://auth-callback?code=the-code")!
        }
        #expect(session.accessToken == "access-new" && session.refreshToken == "refresh-new")
        #expect(session.email == "me@example.com" && session.userId == "user-1")
        // Not used until the caller knows it is still wanted (finding 20).
        let current = await auth.currentSession()
        #expect(store.load() == nil && current == nil)
        await auth.adopt(session)
        #expect(store.load() == session)
        #expect(try await auth.validAccessToken(for: "https://agentnotch.example.com") == "access-new")
        await #expect(throws: CloudAuthError.otherWebsite) {
            _ = try await auth.validAccessToken(for: "https://other.example.com")
        }

        // The browser got the challenge; the exchange carried its verifier.
        let authorize = try #require(opened.urls.first)
        let challenge = try #require(URLComponents(url: authorize, resolvingAgainstBaseURL: false)?
            .queryItems?.first { $0.name == "code_challenge" }?.value)
        let exchange = try #require(transport.requests(to: "/auth/v1/token").first)
        #expect(exchange.httpMethod == "POST")
        #expect(exchange.url?.query == "grant_type=pkce")
        #expect(exchange.value(forHTTPHeaderField: "apikey") == "sb_publishable_test")
        let body = try #require(FakeTransport.body(exchange))
        #expect(body["auth_code"] as? String == "the-code")
        let verifier = try #require(body["code_verifier"] as? String)
        #expect(PKCE.challenge(for: verifier) == challenge)
    }

    @Test func aClosedSignInWindowIsCancelled() async {
        let auth = CloudAuth(transport: FakeTransport { _ in .json(500, [:]) }, store: CloudSessionMemoryStore())
        await #expect(throws: CloudAuthError.cancelled) {
            _ = try await auth.signIn(config: Self.config, website: "https://agentnotch.example.com") { _ in
                throw CancellationError()
            }
        }
        await #expect(throws: CloudAuthError.badConfig("supabaseUrl")) {
            var config = Self.config
            config.supabaseUrl = "http://evil.example.com"
            _ = try await auth.signIn(config: config, website: "https://agentnotch.example.com") { $0 }
        }
    }

    /// Regression (M4): a host with no browser step (the protocol's default)
    /// fails the sign-in with its reason, not as a cancellation.
    @Test func aHostWithNoBrowserStepSaysSo() async {
        let auth = CloudAuth(transport: FakeTransport { _ in .json(500, [:]) }, store: CloudSessionMemoryStore())
        await #expect(throws: CloudAuthError.provider("This app can't open the website's sign-in.")) {
            _ = try await auth.signIn(config: Self.config, website: "https://agentnotch.example.com") { _ in
                throw ClaudeWebsiteSignInUnavailable()
            }
        }
    }

    // MARK: - Refresh

    @Test func anExpiringTokenIsRefreshedOnceAndSavedFirst() async throws {
        let transport = FakeTransport { request in
            guard request.url?.query == "grant_type=refresh_token" else { return .json(404, [:]) }
            Thread.sleep(forTimeInterval: 0.05)
            return Self.tokenAnswer(access: "access-2", refresh: "refresh-2")
        }
        let store = CloudSessionMemoryStore(Self.session(expiresIn: 30))
        let auth = CloudAuth(transport: transport, store: store)
        // Two callers at once: one request (the refresh token is single-use).
        async let first = auth.validAccessToken()
        async let second = auth.validAccessToken()
        let tokens = try await [first, second]
        #expect(tokens == ["access-2", "access-2"])
        #expect(transport.requests(to: "/auth/v1/token").count == 1)
        let body = try #require(FakeTransport.body(transport.recorded[0]))
        #expect(body["refresh_token"] as? String == "refresh-1")
        #expect(store.load()?.refreshToken == "refresh-2")
        #expect(store.saveCount == 1)
        // A fresh token needs no request.
        #expect(try await auth.validAccessToken() == "access-2")
        #expect(transport.recorded.count == 1)
    }

    @Test func aRefusedRefreshSignsOut() async {
        let transport = FakeTransport { _ in .json(400, ["error": "invalid_grant", "error_description": "Invalid Refresh Token: Already Used"]) }
        let store = CloudSessionMemoryStore(Self.session(expiresIn: -10))
        let auth = CloudAuth(transport: transport, store: store)
        await #expect(throws: CloudAuthError.signedOut) { _ = try await auth.validAccessToken() }
        #expect(store.load() == nil)
        #expect(await auth.currentSession() == nil)
    }

    /// Regression (review finding 9): only Supabase refusing the refresh
    /// token itself ends the session; its being down doesn't.
    @Test func onlyARefusedRefreshTokenEndsTheSession() async {
        for (status, body) in [(503, ["msg": "Service Unavailable"] as [String: Any]), (500, [:]), (429, ["msg": "slow down"]),
                               (400, ["error_code": "validation_failed", "msg": "Unsupported content type"]),
                               (403, ["error_code": "bad_jwt", "msg": "invalid JWT"])] {
            let store = CloudSessionMemoryStore(Self.session(expiresIn: -10))
            let auth = CloudAuth(transport: FakeTransport { _ in .json(status, body) }, store: store)
            await #expect(throws: (any Error).self) { _ = try await auth.validAccessToken() }
            #expect(store.load()?.refreshToken == "refresh-1", "\(status) \(body)")
        }
        for body in [["error": "invalid_grant", "error_description": "Invalid Refresh Token: Already Used"],
                     ["code": 400, "error_code": "refresh_token_not_found", "msg": "Invalid Refresh Token: Refresh Token Not Found"],
                     ["error_code": "session_not_found", "msg": "Session from session_id claim in JWT does not exist"]] as [[String: Any]] {
            let store = CloudSessionMemoryStore(Self.session(expiresIn: -10))
            let auth = CloudAuth(transport: FakeTransport { _ in .json(400, body) }, store: store)
            await #expect(throws: CloudAuthError.signedOut) { _ = try await auth.validAccessToken() }
            #expect(store.load() == nil, "\(body)")
        }
        #expect(CloudAuth.refusesRefreshToken(status: 401, code: "refresh_token_already_used", message: nil))
        #expect(!CloudAuth.refusesRefreshToken(status: 503, code: "invalid_grant", message: nil))
        #expect(!CloudAuth.refusesRefreshToken(status: 400, code: nil, message: "Bad request"))
    }

    @Test func aNetworkFailureKeepsTheSession() async {
        struct Offline: Error {}
        let store = CloudSessionMemoryStore(Self.session(expiresIn: -10))
        let auth = CloudAuth(transport: FakeTransport { _ in throw Offline() }, store: store)
        await #expect(throws: (any Error).self) { _ = try await auth.validAccessToken() }
        #expect(store.load()?.refreshToken == "refresh-1")
    }

    @Test func a401IsRetriedOnceAfterARefresh() async throws {
        let transport = FakeTransport { request in
            if request.url?.path == "/auth/v1/token" { return Self.tokenAnswer(access: "access-2", refresh: "refresh-2") }
            if request.url?.path == "/api/app/v1/me" {
                if request.value(forHTTPHeaderField: "Authorization") == "Bearer access-2" {
                    return FakeTransport.Answer(status: 200, body: try TestPaths.contractFixture("me.json"))
                }
                return FakeTransport.Answer(status: 401, body: try TestPaths.contractFixture("error.json"))
            }
            return .json(404, [:])
        }
        let store = CloudSessionMemoryStore(Self.session(expiresIn: 3600))
        let auth = CloudAuth(transport: transport, store: store)
        let api = CloudAPI(website: URL(string: "https://agentnotch.example.com")!, transport: transport, auth: auth)
        let me = try await api.me()
        #expect(me.user.email == "me@example.com")
        #expect(transport.recorded.map { $0.url!.path } == ["/api/app/v1/me", "/auth/v1/token", "/api/app/v1/me"])
        #expect(transport.recorded[0].value(forHTTPHeaderField: "Authorization") == "Bearer access-1")

        // Refused again after the refresh: not retried a second time.
        transport.answer { request in
            if request.url?.path == "/auth/v1/token" { return Self.tokenAnswer(access: "access-3", refresh: "refresh-3") }
            return FakeTransport.Answer(status: 401, body: try TestPaths.contractFixture("error.json"))
        }
        await #expect(throws: CloudAPIError.server(status: 401, code: "UNAUTHORIZED", message: "Sign in again.", retryAfter: nil)) {
            _ = try await api.me()
        }
        #expect(transport.requests(to: "/api/app/v1/me").count == 4)
    }

    @Test func configNeedsNoSignIn() async throws {
        let transport = FakeTransport { _ in FakeTransport.Answer(status: 200, body: try TestPaths.contractFixture("config.json")) }
        let api = CloudAPI(website: URL(string: "https://agentnotch.example.com")!, transport: transport, auth: nil)
        let config = try await api.config()
        #expect(config.redirectUrl == "agentnotch://auth-callback")
        #expect(transport.recorded.first?.value(forHTTPHeaderField: "Authorization") == nil)
        await #expect(throws: CloudAPIError.notSignedIn) { _ = try await api.me() }
    }

    // MARK: - Signing out

    @Test func signOutEndsThisSessionOnlyAndForgetsIt() async throws {
        let transport = FakeTransport { _ in .json(500, [:]) }
        let store = CloudSessionMemoryStore(Self.session(expiresIn: 3600))
        let auth = CloudAuth(transport: transport, store: store)
        await auth.signOut()
        let logout = try #require(transport.recorded.first)
        #expect(logout.url?.path == "/auth/v1/logout" && logout.url?.query == "scope=local")
        #expect(logout.value(forHTTPHeaderField: "Authorization") == "Bearer access-1")
        #expect(logout.value(forHTTPHeaderField: "apikey") == "sb_publishable_test")
        // Forgotten even though the website answered 500.
        #expect(store.load() == nil)
        await #expect(throws: CloudAuthError.signedOut) { _ = try await auth.validAccessToken() }
    }

    // MARK: - Where the session is kept

    @Test func theFileStoreWritesAtomicallyAndPrivately() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-session"))
        defer { try? FileManager.default.removeItem(at: root) }
        let store = CloudSessionFileStore(directory: { root }, isAllowed: { true })
        #expect(store.load() == nil)
        try store.save(Self.session(expiresIn: 3600))
        try store.save(Self.session(expiresIn: 3600, access: "access-2", refresh: "refresh-2"))
        let file = root.appendingPathComponent(CloudSessionFileStore.fileName)
        #expect(CloudFiles.permissions(of: file) == 0o600)
        #expect(store.load()?.refreshToken == "refresh-2")
        // Only the file itself: no temporary file left beside it.
        #expect(try FileManager.default.contentsOfDirectory(atPath: root.path) == [CloudSessionFileStore.fileName])
        store.clear()
        #expect(!FileManager.default.fileExists(atPath: file.path))
    }

    @Test func theFileStoreDoesNothingSealedOrBeforeBootstrap() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-session-off"))
        defer { try? FileManager.default.removeItem(at: root) }
        // The tests run unbootstrapped: the default gate is closed.
        #expect(!AppIdentity.isFrozen)
        #expect(!CloudSessionFileStore.defaultGate())
        let store = CloudSessionFileStore(directory: { root })
        #expect(throws: CloudSessionFileStore.StoreError.notAllowed) { try store.save(Self.session(expiresIn: 60)) }
        // A file put there by someone else isn't read either.
        let other = CloudSessionFileStore(directory: { root }, isAllowed: { true })
        try other.save(Self.session(expiresIn: 60))
        #expect(store.load() == nil)
        store.clear()
        #expect(FileManager.default.fileExists(atPath: root.appendingPathComponent(CloudSessionFileStore.fileName).path))
    }
}

/// URLs a stand-in browser was asked to open.
nonisolated final class OpenedURLs: @unchecked Sendable {
    private let lock = NSLock()
    private var list: [URL] = []
    func add(_ url: URL) { lock.withLock { list.append(url) } }
    var urls: [URL] { lock.withLock { list } }
}
