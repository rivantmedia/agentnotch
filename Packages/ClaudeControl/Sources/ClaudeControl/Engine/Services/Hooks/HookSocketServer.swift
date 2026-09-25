//
//  HookSocketServer.swift
//  ClaudeControl
//
//  Unix domain socket server for real-time hook and status line messages.
//  Supports request/response for permission decisions.
//
//  Protocol (v2): every client writes one JSON object and half-closes its
//  side (SHUT_WR); the server reads until EOF. A PermissionRequest client then
//  waits for the server's response (a `PermissionResponse` JSON object)
//  followed by EOF, or just EOF when the app has no decision.
//
//  Socket file lifecycle:
//  - The path is the configuration's (`SPCN_SOCKET`, else
//    `<support>/hook.sock`, else `/tmp/spcn-<uid>/hook.sock` when that would
//    not fit in `sun_path`). A missing parent folder is created 0700; the
//    `/tmp` fallback folder must be a real folder owned by this user and is
//    made 0700 if it is not.
//  - A stale socket at the path (a crashed run's) is replaced; anything
//    that isn't a socket is left alone and reported.
//  - The server remembers the inode it bound. `stop()` unlinks the path only
//    if it still is that socket, so an instance being retired never deletes
//    the socket of the instance that replaced it.
//  - Every 5 s the server checks the path. If it is gone (deleted by hand or
//    by an older build's stop), it binds again. If another process's socket
//    sits there, it leaves it alone.
//
//  Threading: all mutable state is confined to `queue`. Public methods may be
//  called from any thread; they hop onto the queue.
//

import Darwin
import Foundation
import os.log

/// Logger for hook socket server
nonisolated private var logger: Logger { EngineLog.logger("Socket") }

/// Pending permission request waiting for user decision
nonisolated struct PendingPermission: Sendable {
    let sessionId: String
    let toolUseId: String
    /// The subagent that asked; nil for the main session.
    let agentId: String?
    let clientSocket: Int32
    let event: HookEvent
    let receivedAt: Date
}

/// Unix domain socket server that receives events from Claude Code hooks
/// Uses GCD dispatch sources for non-blocking I/O
nonisolated final class HookSocketServer: @unchecked Sendable {
    static let shared = HookSocketServer()

    /// The configuration's socket path (see `ClaudeControlConfiguration`).
    static var socketPath: String { AppIdentity.socketPath }

    /// Callback for every decoded message, in arrival order, on the socket queue.
    typealias MessageHandler = @Sendable (HookSocketMessage) -> Void

    /// Callback when a pending permission can no longer be answered (the hook
    /// process went away, or writing the decision failed).
    typealias PermissionFailureHandler = @Sendable (_ sessionId: String, _ toolUseId: String) -> Void

    /// Called on the socket queue whenever the listening state changes: nil
    /// once listening, else a user-facing reason it isn't.
    typealias StatusHandler = @Sendable (_ error: String?) -> Void

    /// Largest message accepted from a client. Tool inputs are truncated by the
    /// hook script, so real messages stay far below this.
    static let maxMessageBytes = 8 * 1024 * 1024
    /// A client that hasn't finished writing by then is dropped.
    static let clientReadTimeout: TimeInterval = 5
    /// How often pending permission sockets are checked for a vanished hook.
    static let livenessCheckInterval: TimeInterval = 2
    /// How often the socket file is checked (and bound again if it is gone).
    static let bindingCheckInterval: TimeInterval = 5
    /// Connections the kernel queues while the app is busy. macOS caps it at
    /// kern.ipc.somaxconn (128); a hook whose connect is refused loses its
    /// event, so take all of it.
    static let listenBacklog = SOMAXCONN

    private let queue = DispatchQueue(label: EngineLog.queueLabel("socket"), qos: .userInitiated)
    private let socketPathOverride: String?
    private let checkInterval: TimeInterval

    // MARK: State (queue-confined)

    private var serverSocket: Int32 = -1
    private var boundPath: String?
    /// Device and inode of the socket file this server created.
    private var boundFile: FileIdentity?
    private var acceptSource: DispatchSourceRead?
    private var bindingTimer: DispatchSourceTimer?
    private var isRunning = false
    private var messageHandler: MessageHandler?
    private var permissionFailureHandler: PermissionFailureHandler?
    private var statusHandler: StatusHandler?
    private var currentError: String?
    /// The path holds another process's socket; logged once.
    private var reportedForeignSocket = false

    /// Connections still being read, keyed by file descriptor
    private var clients: [Int32: ClientConnection] = [:]

    /// Pending permission requests indexed by toolUseId
    private var pendingPermissions: [String: PendingPermission] = [:]
    private var livenessTimer: DispatchSourceTimer?

    /// tool_use_ids from PreToolUse, to correlate PermissionRequests (which carry none)
    private var toolUseIdCache = ToolUseIdCache()

    private init() {
        socketPathOverride = nil
        checkInterval = Self.bindingCheckInterval
    }

    /// A private server on its own socket path (tests).
    init(socketPath: String, bindingCheckInterval: TimeInterval = HookSocketServer.bindingCheckInterval) {
        socketPathOverride = socketPath
        checkInterval = bindingCheckInterval
    }

    private var effectiveSocketPath: String {
        socketPathOverride ?? Self.socketPath
    }

    // MARK: - Lifecycle

    /// Start the socket server. Messages are delivered on the socket queue, in order.
    func start(
        onMessage: @escaping MessageHandler,
        onPermissionFailure: PermissionFailureHandler? = nil,
        onStatus: StatusHandler? = nil
    ) {
        queue.async { [self] in
            guard !isRunning else { return }
            isRunning = true
            messageHandler = onMessage
            permissionFailureHandler = onPermissionFailure
            statusHandler = onStatus
            bindSocket()
            startBindingTimer()
        }
    }

    /// Stop the server, drop pending permissions (their hooks fall back to
    /// Claude Code's own prompt) and unlink the socket file if it is still
    /// ours. Synchronous so it can run from applicationWillTerminate.
    func stop() {
        queue.sync { [self] in
            isRunning = false
            bindingTimer?.cancel()
            bindingTimer = nil
            closeListener()
            for client in clients.values {
                client.cancel(closeSocket: true)
            }
            clients.removeAll()
            for pending in pendingPermissions.values {
                close(pending.clientSocket)
            }
            pendingPermissions.removeAll()
            livenessTimer?.cancel()
            livenessTimer = nil
            if let boundPath, let boundFile {
                Self.unlinkIfSame(path: boundPath, identity: boundFile)
            }
            boundPath = nil
            boundFile = nil
            messageHandler = nil
            permissionFailureHandler = nil
            statusHandler = nil
        }
    }

    /// Why the server isn't listening, or nil when it is (or wasn't started).
    var listenError: String? {
        queue.sync { currentError }
    }

    /// The path the server is bound to, nil when it isn't.
    var listeningPath: String? {
        queue.sync { serverSocket >= 0 ? boundPath : nil }
    }

    // MARK: - Binding

    /// Creates, binds and listens on the socket. Queue-confined.
    private func bindSocket() {
        guard isRunning, serverSocket < 0 else { return }
        let socketPath = effectiveSocketPath

        // sockaddr_un.sun_path holds 104 bytes including the terminator. The
        // configuration already falls back to /tmp for long support paths;
        // an overlong SPCN_SOCKET is refused rather than truncated.
        guard socketPath.utf8.count <= ClaudeControlConfiguration.maxSocketPathBytes else {
            setError("The socket path is longer than \(ClaudeControlConfiguration.maxSocketPathBytes) bytes: \(socketPath)")
            return
        }
        if let problem = HookSocketDirectory.prepare(forSocketAt: socketPath, userID: getuid()) {
            setError(problem)
            return
        }

        // A socket at the path is stale: a crashed run's, or a retired
        // instance's (which won't unlink ours, see stop()). Anything else
        // there (a file SPCN_SOCKET was pointed at by mistake) isn't ours
        // to delete.
        if let problem = Self.removeStaleSocket(at: socketPath) {
            setError(problem)
            return
        }

        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            setError("Couldn't create the hook socket (errno \(errno))")
            return
        }
        let flags = fcntl(fd, F_GETFL)
        _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let capacity = MemoryLayout.size(ofValue: addr.sun_path)
        withUnsafeMutableBytes(of: &addr.sun_path) { buffer in
            let bytes = Array(socketPath.utf8.prefix(capacity - 1))
            buffer.copyBytes(from: bytes)
            buffer[bytes.count] = 0
        }
        let bindResult = withUnsafePointer(to: &addr) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bindResult == 0 else {
            let code = errno
            close(fd)
            setError("Couldn't bind the hook socket at \(socketPath) (errno \(code))")
            return
        }
        chmod(socketPath, 0o600)

        guard listen(fd, Self.listenBacklog) == 0 else {
            let code = errno
            close(fd)
            unlink(socketPath)
            setError("Couldn't listen on the hook socket (errno \(code))")
            return
        }

        serverSocket = fd
        boundPath = socketPath
        boundFile = FileIdentity(path: socketPath)
        reportedForeignSocket = false
        logger.info("Listening on \(socketPath, privacy: .public)")

        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: queue)
        source.setEventHandler { [weak self] in
            self?.acceptConnections()
        }
        source.setCancelHandler {
            close(fd)
        }
        acceptSource = source
        source.resume()
        setError(nil)
    }

    /// Stops accepting; the listening descriptor closes once the source is cancelled.
    private func closeListener() {
        acceptSource?.cancel()
        acceptSource = nil
        serverSocket = -1
    }

    private func startBindingTimer() {
        guard bindingTimer == nil else { return }
        let timer = DispatchSource.makeTimerSource(queue: queue)
        let interval = checkInterval
        timer.schedule(deadline: .now() + interval, repeating: interval, leeway: .milliseconds(100))
        timer.setEventHandler { [weak self] in
            self?.checkBinding()
        }
        bindingTimer = timer
        timer.resume()
    }

    /// The periodic check: bind again when the socket file vanished (or the
    /// first bind failed); never fight another process's socket.
    private func checkBinding() {
        guard isRunning else { return }
        guard serverSocket >= 0, let boundPath else {
            bindSocket()
            return
        }
        switch Self.bindingState(path: boundPath, identity: boundFile) {
        case .ours:
            return
        case .missing:
            logger.warning("Socket file \(boundPath, privacy: .public) disappeared; binding again")
            closeListener()
            self.boundPath = nil
            boundFile = nil
            bindSocket()
        case .replaced:
            guard !reportedForeignSocket else { return }
            reportedForeignSocket = true
            logger.warning("Another process owns \(boundPath, privacy: .public) now; leaving it alone")
        }
    }

    /// What is at a bound socket's path now.
    enum BindingState: Equatable, Sendable {
        /// Our socket file is still there.
        case ours
        /// Nothing is there.
        case missing
        /// Something else is there (another instance's socket).
        case replaced
    }

    static func bindingState(path: String, identity: FileIdentity?) -> BindingState {
        guard let current = FileIdentity(path: path) else { return .missing }
        return current == identity ? .ours : .replaced
    }

    /// Removes a socket file left at `path` before binding. Nil when the path
    /// is free now, else why it can't be used (something other than a socket
    /// is there, or it couldn't be removed).
    static func removeStaleSocket(at path: String) -> String? {
        var info = stat()
        guard lstat(path, &info) == 0 else { return nil }
        guard (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFSOCK) else {
            return "\(path) exists and is not a socket; not replacing it"
        }
        guard unlink(path) == 0 || errno == ENOENT else {
            return "Couldn't remove the old hook socket at \(path) (errno \(errno))"
        }
        return nil
    }

    /// Unlinks `path` only if it is still the file `identity` names. Returns
    /// whether it did.
    @discardableResult
    static func unlinkIfSame(path: String, identity: FileIdentity) -> Bool {
        guard FileIdentity(path: path) == identity else {
            logger.info("Not unlinking \(path, privacy: .public): it is no longer our socket")
            return false
        }
        return unlink(path) == 0
    }

    private func setError(_ error: String?) {
        if let error, error != currentError {
            logger.error("\(error, privacy: .public)")
        }
        guard currentError != error else { return }
        currentError = error
        statusHandler?(error)
    }

    // MARK: - Permission Responses

    /// Answers a pending PermissionRequest. Returns false if there is no such
    /// request or its hook is gone (the failure handler is NOT called for the
    /// caller's own request; the caller handles the result).
    func respondToPermission(
        toolUseId: String,
        decision: PermissionDecision,
        reason: String? = nil,
        updatedInput: [String: AnyCodable]? = nil,
        updatedPermissions: [AnyCodable]? = nil,
        interrupt: Bool? = nil
    ) async -> Bool {
        let response = PermissionResponse(
            decision: decision,
            reason: reason,
            updatedInput: updatedInput,
            updatedPermissions: updatedPermissions,
            interrupt: interrupt
        )
        return await withCheckedContinuation { continuation in
            queue.async { [self] in
                continuation.resume(returning: sendPermissionResponse(toolUseId: toolUseId, response: response))
            }
        }
    }

    /// Cancel all pending permissions for a session (when Claude stops waiting)
    func cancelPendingPermissions(sessionId: String) {
        queue.async { [self] in
            cleanupPendingPermissions(sessionId: sessionId, mainSessionOnly: false)
        }
    }

    /// Close the sockets of these requests without an answer, so Claude Code
    /// falls back to its own dialog (the app stopped showing them).
    func cancelPendingPermissions(toolUseIds: [String]) {
        guard !toolUseIds.isEmpty else { return }
        queue.async { [self] in
            for toolUseId in toolUseIds {
                cleanupSpecificPermission(toolUseId: toolUseId)
            }
        }
    }

    /// Whether a PermissionRequest with this id is still waiting for an answer.
    func hasPendingPermission(toolUseId: String) -> Bool {
        queue.sync { pendingPermissions[toolUseId] != nil }
    }

    /// Ids of the PermissionRequests still waiting, optionally for one session.
    func pendingPermissionIds(sessionId: String? = nil) -> [String] {
        queue.sync {
            pendingPermissions.values
                .filter { sessionId == nil || $0.sessionId == sessionId }
                .sorted { $0.receivedAt < $1.receivedAt }
                .map(\.toolUseId)
        }
    }

    private func sendPermissionResponse(toolUseId: String, response: PermissionResponse) -> Bool {
        guard let pending = pendingPermissions.removeValue(forKey: toolUseId) else {
            logger.debug("No pending permission for toolUseId: \(toolUseId.prefix(12), privacy: .public)")
            return false
        }
        updateLivenessTimer()
        defer { close(pending.clientSocket) }

        guard Self.isPeerAlive(pending.clientSocket) else {
            logger.warning("Hook for \(pending.sessionId.prefix(8), privacy: .public) tool:\(toolUseId.prefix(12), privacy: .public) is gone; decision not delivered")
            return false
        }
        guard let data = try? JSONEncoder().encode(response) else {
            logger.error("Failed to encode permission response")
            return false
        }

        let age = Date().timeIntervalSince(pending.receivedAt)
        logger.info("Sending response: \(response.decision.rawValue, privacy: .public) for \(pending.sessionId.prefix(8), privacy: .public) tool:\(toolUseId.prefix(12), privacy: .public) (age: \(String(format: "%.1f", age), privacy: .public)s)")

        guard Self.writeAll(data, to: pending.clientSocket) else {
            logger.error("Write failed with errno: \(errno)")
            return false
        }
        return true
    }

    private func cleanupSpecificPermission(toolUseId: String) {
        guard let pending = pendingPermissions.removeValue(forKey: toolUseId) else { return }
        updateLivenessTimer()
        logger.debug("Tool resolved elsewhere, closing socket for \(pending.sessionId.prefix(8), privacy: .public) tool:\(toolUseId.prefix(12), privacy: .public)")
        close(pending.clientSocket)
    }

    /// Closes a session's pending requests. With `mainSessionOnly`, requests
    /// of background subagents stay: they outlive the main turn's Stop (the
    /// agent keeps running and still waits for its answer).
    private func cleanupPendingPermissions(sessionId: String, mainSessionOnly: Bool) {
        let matching = pendingPermissions.filter {
            $0.value.sessionId == sessionId && (!mainSessionOnly || $0.value.agentId == nil)
        }
        for (toolUseId, pending) in matching {
            logger.debug("Cleaning up stale permission for \(sessionId.prefix(8), privacy: .public) tool:\(toolUseId.prefix(12), privacy: .public)")
            close(pending.clientSocket)
            pendingPermissions.removeValue(forKey: toolUseId)
        }
        updateLivenessTimer()
    }

    // MARK: - Dead Hook Detection

    /// Runs while permissions are pending: a hook that exited (timeout, killed,
    /// or Claude Code aborted it because the terminal dialog answered first)
    /// leaves a socket nobody reads, so the UI must stop offering Approve.
    private func updateLivenessTimer() {
        if pendingPermissions.isEmpty {
            livenessTimer?.cancel()
            livenessTimer = nil
            return
        }
        guard livenessTimer == nil else { return }
        let timer = DispatchSource.makeTimerSource(queue: queue)
        let interval = Self.livenessCheckInterval
        timer.schedule(deadline: .now() + interval, repeating: interval, leeway: .milliseconds(250))
        timer.setEventHandler { [weak self] in
            self?.checkPendingPermissionsAlive()
        }
        livenessTimer = timer
        timer.resume()
    }

    private func checkPendingPermissionsAlive() {
        for (toolUseId, pending) in pendingPermissions where !Self.isPeerAlive(pending.clientSocket) {
            logger.info("Hook for \(pending.sessionId.prefix(8), privacy: .public) tool:\(toolUseId.prefix(12), privacy: .public) went away; permission no longer answerable here")
            pendingPermissions.removeValue(forKey: toolUseId)
            close(pending.clientSocket)
            permissionFailureHandler?(pending.sessionId, toolUseId)
        }
        updateLivenessTimer()
    }

    /// The client half-closed its write side long ago, so readability says
    /// nothing. Poll for writability instead: once the peer has closed the
    /// socket completely, the kernel reports POLLHUP for our write side.
    static func isPeerAlive(_ fd: Int32) -> Bool {
        var pfd = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
        let result = poll(&pfd, 1, 0)
        if result < 0 { return false }
        let deadMask = Int16(POLLHUP) | Int16(POLLERR) | Int16(POLLNVAL)
        return pfd.revents & deadMask == 0
    }

    /// Blocking write of the whole buffer with a send timeout.
    private static func writeAll(_ data: Data, to fd: Int32) -> Bool {
        let flags = fcntl(fd, F_GETFL)
        _ = fcntl(fd, F_SETFL, flags & ~O_NONBLOCK)
        var timeout = timeval(tv_sec: 2, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))

        return data.withUnsafeBytes { buffer -> Bool in
            guard var pointer = buffer.baseAddress else { return false }
            var remaining = buffer.count
            while remaining > 0 {
                let written = write(fd, pointer, remaining)
                if written < 0 {
                    if errno == EINTR { continue }
                    return false
                }
                remaining -= written
                pointer = pointer.advanced(by: written)
            }
            return true
        }
    }

    // MARK: - Reading Clients

    private func acceptConnections() {
        while serverSocket >= 0 {
            let clientSocket = accept(serverSocket, nil, nil)
            guard clientSocket >= 0 else { return }  // EAGAIN: nothing left to accept

            var nosigpipe: Int32 = 1
            setsockopt(clientSocket, SOL_SOCKET, SO_NOSIGPIPE, &nosigpipe, socklen_t(MemoryLayout<Int32>.size))
            let flags = fcntl(clientSocket, F_GETFL)
            _ = fcntl(clientSocket, F_SETFL, flags | O_NONBLOCK)

            let client = ClientConnection(fd: clientSocket, queue: queue)
            clients[clientSocket] = client
            client.start(
                onReadable: { [weak self] in self?.readAvailable(from: clientSocket) }
            )
            queue.asyncAfter(deadline: .now() + Self.clientReadTimeout) { [weak self, weak client] in
                guard let self, let client, self.clients[clientSocket] === client else { return }
                logger.warning("Client didn't finish its message within \(Self.clientReadTimeout, privacy: .public)s; dropping it")
                self.clients.removeValue(forKey: clientSocket)
                client.cancel(closeSocket: true)
            }
        }
    }

    /// Reads everything available; on EOF the message is complete.
    private func readAvailable(from fd: Int32) {
        guard let client = clients[fd] else { return }
        var buffer = [UInt8](repeating: 0, count: 65536)
        while true {
            let bytesRead = read(fd, &buffer, buffer.count)
            if bytesRead > 0 {
                client.data.append(contentsOf: buffer[0..<bytesRead])
                if client.data.count > Self.maxMessageBytes {
                    logger.warning("Message exceeds \(Self.maxMessageBytes) bytes; dropping client")
                    clients.removeValue(forKey: fd)
                    client.cancel(closeSocket: true)
                    return
                }
            } else if bytesRead == 0 {
                clients.removeValue(forKey: fd)
                finishMessage(client)
                return
            } else {
                if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }
                clients.removeValue(forKey: fd)
                client.cancel(closeSocket: true)
                return
            }
        }
    }

    /// Whether a permission request is answered at once, with no decision:
    /// its session belongs to an untracked or forgotten account.
    nonisolated static func passesThrough(_ event: HookEvent) -> Bool {
        FolderRings.isUntracked(folder: event.resolvedConfigDir, pid: event.pid)
    }

    private func finishMessage(_ client: ClientConnection) {
        guard !client.data.isEmpty, let message = HookSocketMessage.decode(client.data) else {
            if !client.data.isEmpty {
                logger.warning("Failed to parse message (\(client.data.count) bytes)")
            }
            client.cancel(closeSocket: true)
            return
        }

        switch message {
        case .statusLine:
            client.cancel(closeSocket: true)
            messageHandler?(message)

        case .hook(var event):
            if event.isFromIgnoredSession {
                // Background, daemon and SDK sessions: no session, no held socket.
                client.cancel(closeSocket: true)
                logger.debug("Ignoring \(event.event, privacy: .public) from unattended/SDK session \(event.sessionId.prefix(8), privacy: .public)")
                return
            }
            logger.debug("Received: \(event.event, privacy: .public) for \(event.sessionId.prefix(8), privacy: .public)")
            updateToolUseIdCache(for: event)

            guard event.expectsResponse else {
                client.cancel(closeSocket: true)
                messageHandler?(.hook(event))
                return
            }

            // A session of an untracked or forgotten account (its folder can
            // keep our hooks: `~/.claude` stays hooked while Claude Parallel
            // Profiles mirrors an untracked account into it). Nothing shows
            // it, so nothing may hold its request: closed at once, with no
            // decision, and the session asks in its own terminal.
            if Self.passesThrough(event) {
                client.cancel(closeSocket: true)
                logger.info("Permission request from an untracked account's session \(event.sessionId.prefix(8), privacy: .public): left to its terminal")
                return
            }

            // Keep the connection open until the user decides.
            client.cancel(closeSocket: false)
            if event.toolUseId == nil {
                if let exact = toolUseIdCache.pop(sessionId: event.sessionId, toolName: event.tool, toolInput: event.toolInput) {
                    event.toolUseId = exact
                } else if let only = toolUseIdCache.popOnlyInFlight(sessionId: event.sessionId, toolName: event.tool, agentId: event.agentId) {
                    // The input differs from its PreToolUse (another hook
                    // rewrote it), but exactly one call of this tool is in
                    // flight: it is that one, and its own PostToolUse will
                    // resolve the request.
                    event.toolUseId = only
                    logger.info("Permission request matched the only in-flight \(event.tool ?? "?", privacy: .public) call for \(event.sessionId.prefix(8), privacy: .public)")
                } else {
                    // None or several candidates: still answerable through this socket.
                    event.toolUseId = "permission-\(UUID().uuidString)"
                    event.hasSyntheticToolUseId = true
                    logger.warning("Permission request without matching PreToolUse for \(event.sessionId.prefix(8), privacy: .public); using a synthetic id")
                }
            }
            let toolUseId = event.toolUseId ?? ""
            if let stale = pendingPermissions.removeValue(forKey: toolUseId) {
                close(stale.clientSocket)
            }
            pendingPermissions[toolUseId] = PendingPermission(
                sessionId: event.sessionId,
                toolUseId: toolUseId,
                agentId: event.isSubagentEvent ? event.agentId : nil,
                clientSocket: client.fd,
                event: event,
                receivedAt: event.receivedAt
            )
            updateLivenessTimer()
            logger.debug("Permission request - keeping socket open for \(event.sessionId.prefix(8), privacy: .public) tool:\(toolUseId.prefix(12), privacy: .public)")
            messageHandler?(.hook(event))
        }
    }

    /// Cache hygiene: ids are recorded on PreToolUse and dropped as soon as the
    /// tool is resolved, so an auto-allowed call can't leave an id behind for a
    /// later identical call to pop.
    private func updateToolUseIdCache(for event: HookEvent) {
        switch event.event {
        case "PreToolUse":
            if let toolUseId = event.toolUseId {
                toolUseIdCache.record(
                    sessionId: event.sessionId,
                    toolName: event.tool,
                    toolInput: event.toolInput,
                    toolUseId: toolUseId,
                    agentId: event.isSubagentEvent ? event.agentId : nil
                )
            }
        case "PostToolUse", "PostToolUseFailure", "PermissionDenied":
            if let toolUseId = event.toolUseId {
                toolUseIdCache.remove(toolUseId: toolUseId)
                // Resolved in the terminal (or by a rule): nothing left to answer.
                cleanupSpecificPermission(toolUseId: toolUseId)
            }
        case "Stop", "StopFailure":
            // The main turn ended. Background subagents keep running (and
            // keep waiting for their answers), so only the main session's
            // calls and requests are over.
            guard !event.isSubagentEvent else { return }
            toolUseIdCache.removeAll(sessionId: event.sessionId, mainSessionOnly: true)
            cleanupPendingPermissions(sessionId: event.sessionId, mainSessionOnly: true)
        case "SessionEnd":
            toolUseIdCache.removeAll(sessionId: event.sessionId, mainSessionOnly: false)
            cleanupPendingPermissions(sessionId: event.sessionId, mainSessionOnly: false)
        default:
            break
        }
    }
}

// MARK: - File identity

/// Device and inode of a file, from lstat (a symlink is its own file).
nonisolated struct FileIdentity: Equatable, Sendable {
    let device: Int64
    let inode: UInt64

    init?(path: String) {
        var info = stat()
        guard lstat(path, &info) == 0 else { return nil }
        device = Int64(info.st_dev)
        inode = UInt64(info.st_ino)
    }
}

// MARK: - Socket directory

/// Makes sure the socket's folder exists and, for the shared `/tmp`
/// fallback, that nobody else can reach into it.
nonisolated enum HookSocketDirectory {
    /// `/tmp/spcn-<uid>`, the folder the configuration falls back to for
    /// support paths too long for `sun_path`.
    static func fallbackDirectory(userID: uid_t) -> String {
        "/tmp/spcn-\(userID)"
    }

    /// Nil when the socket can be created at `socketPath`, else a
    /// user-facing reason it can't. `sharedDirectory` is the folder held to
    /// the stricter rules (the `/tmp` fallback; a parameter for tests).
    static func prepare(forSocketAt socketPath: String, userID: uid_t, sharedDirectory: String? = nil) -> String? {
        let directory = (socketPath as NSString).deletingLastPathComponent
        guard !directory.isEmpty else { return nil }
        let fallback = sharedDirectory ?? fallbackDirectory(userID: userID)
        let isSharedFallback = directory == fallback || directory == "/private" + fallback

        var info = stat()
        if lstat(directory, &info) != 0 {
            guard errno == ENOENT else {
                return "Can't read the socket folder \(directory) (errno \(errno))"
            }
            do {
                try FileManager.default.createDirectory(
                    atPath: directory,
                    withIntermediateDirectories: !isSharedFallback,
                    attributes: [.posixPermissions: 0o700]
                )
            } catch {
                return "Can't create the socket folder \(directory): \(error.localizedDescription)"
            }
            guard lstat(directory, &info) == 0 else {
                return "Can't read the socket folder \(directory) (errno \(errno))"
            }
        }
        guard isSharedFallback else { return nil }

        // /tmp is shared: the folder must be a real folder (not a symlink
        // someone planted), owned by us, and closed to everyone else.
        guard (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR) else {
            return "The socket folder \(directory) is not a folder"
        }
        guard info.st_uid == userID else {
            return "The socket folder \(directory) belongs to another user"
        }
        if (info.st_mode & 0o077) != 0, chmod(directory, 0o700) != 0 {
            return "Can't make the socket folder \(directory) private (errno \(errno))"
        }
        return nil
    }
}

// MARK: - Client Connection

/// One accepted client being read on the server queue.
private nonisolated final class ClientConnection: @unchecked Sendable {
    let fd: Int32
    var data = Data()
    private let queue: DispatchQueue
    private var source: DispatchSourceRead?
    private var closeOnCancel = true
    private var isCancelled = false

    init(fd: Int32, queue: DispatchQueue) {
        self.fd = fd
        self.queue = queue
    }

    func start(onReadable: @escaping () -> Void) {
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: queue)
        source.setEventHandler(handler: onReadable)
        // Strong capture on purpose: GCD releases the handlers once the source
        // is cancelled, and the socket may only be closed after that.
        source.setCancelHandler { [self] in
            if closeOnCancel {
                close(fd)
            }
        }
        self.source = source
        source.resume()
    }

    /// Stops reading; closes the socket unless it is kept for a response.
    func cancel(closeSocket: Bool) {
        guard !isCancelled else { return }
        isCancelled = true
        closeOnCancel = closeSocket
        if let source {
            source.cancel()
            self.source = nil
        } else if closeSocket {
            close(fd)
        }
    }
}

// MARK: - Tool Use ID Cache

/// FIFO of PreToolUse tool_use_ids per (session, tool, input), used to give a
/// PermissionRequest (which carries no id) the id of the call it is about.
nonisolated struct ToolUseIdCache: Sendable {
    private struct Entry: Sendable {
        let toolUseId: String
        let recordedAt: Date
        /// The subagent that made the call; nil for the main session.
        let agentId: String?
    }

    /// Entries older than this are dropped (their tool must have resolved long ago).
    static let maxAge: TimeInterval = 60 * 60

    private var queues: [String: [Entry]] = [:]
    /// Reverse index: tool_use_id → cache key.
    private var keyById: [String: String] = [:]

    init() {}

    var count: Int { keyById.count }

    mutating func record(
        sessionId: String,
        toolName: String?,
        toolInput: [String: AnyCodable]?,
        toolUseId: String,
        agentId: String? = nil,
        at date: Date = Date()
    ) {
        pruneExpired(now: date)
        if keyById[toolUseId] != nil { return }
        let key = Self.key(sessionId: sessionId, toolName: toolName, toolInput: toolInput)
        queues[key, default: []].append(Entry(toolUseId: toolUseId, recordedAt: date, agentId: agentId))
        keyById[toolUseId] = key
    }

    /// Oldest id recorded for this call, removed from the cache.
    mutating func pop(sessionId: String, toolName: String?, toolInput: [String: AnyCodable]?) -> String? {
        let key = Self.key(sessionId: sessionId, toolName: toolName, toolInput: toolInput)
        guard var entries = queues[key], !entries.isEmpty else { return nil }
        let entry = entries.removeFirst()
        queues[key] = entries.isEmpty ? nil : entries
        keyById.removeValue(forKey: entry.toolUseId)
        return entry.toolUseId
    }

    /// The id of the one call of `toolName` still in flight in the session
    /// (by the same agent), removed from the cache; nil when there are none
    /// or several (then which one the request is about can't be told).
    mutating func popOnlyInFlight(sessionId: String, toolName: String?, agentId: String?) -> String? {
        let prefix = Self.keyPrefix(sessionId: sessionId, toolName: toolName)
        let agent = agentId.flatMap { $0.isEmpty ? nil : $0 }
        var match: (key: String, id: String)?
        for (key, entries) in queues where key.hasPrefix(prefix) {
            for entry in entries where entry.agentId == agent {
                guard match == nil else { return nil }
                match = (key, entry.toolUseId)
            }
        }
        guard let match else { return nil }
        remove(toolUseId: match.id)
        return match.id
    }

    mutating func remove(toolUseId: String) {
        guard let key = keyById.removeValue(forKey: toolUseId) else { return }
        queues[key]?.removeAll { $0.toolUseId == toolUseId }
        if queues[key]?.isEmpty == true {
            queues.removeValue(forKey: key)
        }
    }

    /// Forgets a session's calls; with `mainSessionOnly`, background
    /// subagents' calls (still running after the main Stop) are kept.
    mutating func removeAll(sessionId: String, mainSessionOnly: Bool = false) {
        let prefix = "\(sessionId):"
        for key in queues.keys where key.hasPrefix(prefix) {
            let entries = queues[key] ?? []
            let kept = mainSessionOnly ? entries.filter { $0.agentId != nil } : []
            for entry in entries where !kept.contains(where: { $0.toolUseId == entry.toolUseId }) {
                keyById.removeValue(forKey: entry.toolUseId)
            }
            queues[key] = kept.isEmpty ? nil : kept
        }
    }

    private mutating func pruneExpired(now: Date) {
        let cutoff = now.addingTimeInterval(-Self.maxAge)
        for (key, entries) in queues {
            let kept = entries.filter { $0.recordedAt >= cutoff }
            guard kept.count != entries.count else { continue }
            for entry in entries where entry.recordedAt < cutoff {
                keyById.removeValue(forKey: entry.toolUseId)
            }
            queues[key] = kept.isEmpty ? nil : kept
        }
    }

    /// Encoder with sorted keys for deterministic cache keys
    private static let sortedEncoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        return encoder
    }()

    static func key(sessionId: String, toolName: String?, toolInput: [String: AnyCodable]?) -> String {
        let inputString: String
        if let toolInput,
           let data = try? sortedEncoder.encode(toolInput),
           let string = String(data: data, encoding: .utf8) {
            inputString = string
        } else {
            inputString = "{}"
        }
        return keyPrefix(sessionId: sessionId, toolName: toolName) + inputString
    }

    private static func keyPrefix(sessionId: String, toolName: String?) -> String {
        "\(sessionId):\(toolName ?? "unknown"):"
    }
}
