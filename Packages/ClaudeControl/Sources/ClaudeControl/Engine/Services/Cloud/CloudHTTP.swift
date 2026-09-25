//
//  CloudHTTP.swift
//  ClaudeControl
//
//  The website's API (`web/contract/README.md`) over one small transport:
//  `config` (no sign-in), `me` and `sync` (a Supabase access token as
//  Bearer, only ever the token of a session made through this same
//  website). A 401 is retried once after a refresh, with a token of the
//  sign-in the request started with (one made since, or through another
//  website, is never sent); a second one is the caller's to back off from
//  (the session is kept: only Supabase refusing the refresh token ends
//  it). The transport is a protocol so tests answer
//  with fixtures; the real one is an ephemeral URLSession that refuses to
//  run sealed or before bootstrap.
//
//  The website's address is the user's to type: only https, or plain http
//  to this Mac (`localhost`, `127.0.0.1`, `[::1]`) for development. There is
//  no built-in default.
//

import Foundation
import os.log

// MARK: - Transport

/// Sends one request. Implementations don't interpret status codes.
nonisolated protocol CloudTransport: Sendable {
    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse)
}

/// The real network, for a bootstrapped live run only.
nonisolated struct URLSessionCloudTransport: CloudTransport {
    static let shared = URLSessionCloudTransport()

    private let session: URLSession

    init() {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 30
        configuration.timeoutIntervalForResource = 60
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.urlCache = nil
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        session = URLSession(configuration: configuration)
    }

    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        // Sealed runs and tests (nothing bootstrapped) never reach the network.
        guard AppIdentity.isFrozen, !AppIdentity.isSealed else { throw CloudAPIError.networkNotAllowed }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw CloudAPIError.badResponse("Not an HTTP response") }
        return (data, http)
    }
}

// MARK: - Errors

nonisolated enum CloudAPIError: Error, Equatable, Sendable, LocalizedError {
    /// Sealed, or the engine isn't bootstrapped.
    case networkNotAllowed
    /// No usable website address.
    case invalidWebsite
    /// Not signed in (or the session ended).
    case notSignedIn
    /// The request didn't get an answer.
    case transport(String)
    /// The website answered with an error (the contract's error shape when it sent one).
    case server(status: Int, code: String?, message: String?, retryAfter: TimeInterval?)
    /// An answer that doesn't follow the contract.
    case badResponse(String)

    var isUnauthorized: Bool {
        if case .server(let status, let code, _, _) = self {
            return status == 401 || code == CloudErrorCode.unauthorized.rawValue
        }
        return self == .notSignedIn
    }

    var isRateLimited: Bool {
        if case .server(let status, let code, _, _) = self {
            return status == 429 || code == CloudErrorCode.rateLimited.rawValue
        }
        return false
    }

    var retryAfter: TimeInterval? {
        if case .server(_, _, _, let retryAfter) = self { return retryAfter }
        return nil
    }

    var errorDescription: String? {
        switch self {
        case .networkNotAllowed: return "The website can't be reached from this run."
        case .invalidWebsite: return "Enter the website's https:// address."
        case .notSignedIn: return "Sign in to the website first."
        case .transport(let message): return "Couldn't reach the website: \(message)"
        case .server(let status, _, let message, _):
            if let message, !message.isEmpty { return message }
            return "The website answered \(status)."
        case .badResponse(let message): return "Unexpected answer from the website: \(message)"
        }
    }

    /// The contract's error shape from a failed response (the status alone
    /// when the body isn't one).
    static func from(status: Int, data: Data, retryAfterHeader: String?) -> CloudAPIError {
        let body = try? CloudJSON.makeDecoder().decode(CloudErrorBody.self, from: data)
        let retryAfter = retryAfterHeader.flatMap { TimeInterval($0.trimmingCharacters(in: .whitespaces)) }
        return .server(status: status, code: body?.error.code, message: body?.error.message, retryAfter: retryAfter)
    }
}

// MARK: - Website address

nonisolated enum CloudWebsite {
    private static let localHosts: Set<String> = ["localhost", "127.0.0.1", "::1", "[::1]"]

    /// The address as the app keeps it: `https://host[:port][/path]` with no
    /// trailing slash, query, fragment or credentials; or `http://` to this
    /// Mac. Nil for anything else. A bare host gets `https://`.
    static func validated(_ text: String?) -> URL? {
        guard var text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else { return nil }
        if !text.contains("://") { text = "https://" + text }
        guard var components = URLComponents(string: text),
              let scheme = components.scheme?.lowercased(),
              let host = components.host?.lowercased(), !host.isEmpty,
              components.user == nil, components.password == nil,
              components.query == nil, components.fragment == nil else { return nil }
        switch scheme {
        case "https": break
        case "http": guard localHosts.contains(host) else { return nil }
        default: return nil
        }
        components.scheme = scheme
        // An IPv6 literal is left as written (URLComponents would escape its colons).
        if !host.contains(":"), components.host != host { components.host = host }
        var path = components.path
        while path.hasSuffix("/") { path.removeLast() }
        components.path = path
        return components.url
    }

    /// A URL the website hands back (its dashboard, Supabase's address): the
    /// same rule, but a path is kept as given.
    static func validatedLink(_ text: String?) -> URL? {
        guard let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty,
              let components = URLComponents(string: text),
              let scheme = components.scheme?.lowercased(),
              let host = components.host?.lowercased(), !host.isEmpty,
              components.user == nil, components.password == nil else { return nil }
        switch scheme {
        case "https": return components.url
        case "http": return localHosts.contains(host) ? components.url : nil
        default: return nil
        }
    }

    /// `<website>/<path>`.
    static func endpoint(_ website: URL, _ path: String) -> URL {
        website.appendingPathComponent(path)
    }
}

// MARK: - API

/// The website's three endpoints.
nonisolated struct CloudAPI: Sendable {
    let website: URL
    let transport: any CloudTransport
    let auth: CloudAuth?
    let userAgent: String

    private static var logger: Logger { EngineLog.logger("CloudAPI") }

    init(website: URL, transport: any CloudTransport, auth: CloudAuth?, appVersion: String = "0") {
        self.website = website
        self.transport = transport
        self.auth = auth
        userAgent = "AgentNotch/\(appVersion)"
    }

    /// `GET /api/app/v1/config`.
    func config() async throws -> CloudConfig {
        let request = makeRequest(CloudContract.Path.config, method: "GET", body: nil, token: nil)
        let (data, response) = try await send(request)
        guard (200..<300).contains(response.statusCode) else {
            throw CloudAPIError.from(status: response.statusCode, data: data,
                                     retryAfterHeader: response.value(forHTTPHeaderField: "Retry-After"))
        }
        return try decode(CloudConfig.self, data)
    }

    /// `GET /api/app/v1/me`.
    func me() async throws -> CloudMe {
        try await authorized(CloudContract.Path.me, method: "GET", body: nil)
    }

    /// `POST /api/app/v1/sync` with the request clamped to the contract's
    /// limits. `@concurrent`: a full batch is a few hundred KB of JSON to
    /// encode, never on the caller's (main) actor.
    @concurrent
    func sync(_ request: CloudSyncRequest) async throws -> CloudSyncResponse {
        let body = try CloudJSON.makeEncoder().encode(request.clamped())
        return try await authorized(CloudContract.Path.sync, method: "POST", body: body)
    }

    /// An authenticated call: the current access token (refreshed when it
    /// has under a minute left) of a session made through this website, and
    /// on a 401 one retry after a refresh, with a token of the same sign-in
    /// for the same website (the user may have signed in elsewhere since).
    private func authorized<T: Decodable>(_ path: String, method: String, body: Data?) async throws -> T {
        guard let auth else { throw CloudAPIError.notSignedIn }
        let grant = try await auth.accessGrant(for: website.absoluteString)
        var (data, response) = try await send(makeRequest(path, method: method, body: body, token: grant.token))
        if response.statusCode == 401 {
            Self.logger.info("\(path, privacy: .public) answered 401; refreshing the session once")
            let fresh = try await auth.refreshAfterUnauthorized(failedToken: grant.token, website: grant.website,
                                                                signIn: grant.signIn)
            (data, response) = try await send(makeRequest(path, method: method, body: body, token: fresh))
        }
        guard (200..<300).contains(response.statusCode) else {
            throw CloudAPIError.from(status: response.statusCode, data: data,
                                     retryAfterHeader: response.value(forHTTPHeaderField: "Retry-After"))
        }
        return try decode(T.self, data)
    }

    private func makeRequest(_ path: String, method: String, body: Data?, token: String?) -> URLRequest {
        var request = URLRequest(url: CloudWebsite.endpoint(website, path))
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue(userAgent, forHTTPHeaderField: "User-Agent")
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        return request
    }

    private func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        do {
            return try await transport.send(request)
        } catch let error as CloudAPIError {
            throw error
        } catch {
            throw CloudAPIError.transport(error.localizedDescription)
        }
    }

    private func decode<T: Decodable>(_ type: T.Type, _ data: Data) throws -> T {
        do {
            return try CloudJSON.makeDecoder().decode(type, from: data)
        } catch {
            throw CloudAPIError.badResponse("\(T.self): \(error.localizedDescription)")
        }
    }
}
