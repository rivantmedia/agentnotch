import Darwin
import Foundation
import Testing
@testable import ClaudeControl

/// The hook socket's file lifecycle (design §9): the /tmp fallback folder's
/// rules, an unlink that checks the inode, rebinding when the file vanishes,
/// and the listen backlog.
@Suite(.serialized)
struct A1_SocketLifecycleTests {
    /// Short paths: sun_path holds only 104 bytes.
    private static func socketPath() -> String {
        "/tmp/spcn-a1-\(getpid())-\(UInt32.random(in: 0...UInt32.max)).sock"
    }

    private static func waitUntil(timeout: TimeInterval = 5, _ condition: () -> Bool) async throws -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            guard Date() < deadline else { return false }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        return true
    }

    private static func isSocket(_ path: String) -> Bool {
        var info = stat()
        return lstat(path, &info) == 0 && (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFSOCK)
    }

    /// Writes one message the way the hook script does (write, half-close).
    private static func send(_ json: [String: Any], to path: String) throws -> Bool {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }
        defer { close(fd) }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &addr.sun_path) { buffer in
            let bytes = Array(path.utf8)
            buffer.copyBytes(from: bytes)
            buffer[bytes.count] = 0
        }
        let connected = withUnsafePointer(to: &addr) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else { return false }
        let data = try JSONSerialization.data(withJSONObject: json)
        _ = data.withUnsafeBytes { write(fd, $0.baseAddress, data.count) }
        shutdown(fd, SHUT_WR)
        return true
    }

    private nonisolated final class Received: @unchecked Sendable {
        private let lock = NSLock()
        private var events: [String] = []
        func add(_ message: HookSocketMessage) {
            guard case .hook(let event) = message else { return }
            lock.lock(); events.append(event.event); lock.unlock()
        }
        var count: Int { lock.lock(); defer { lock.unlock() }; return events.count }
    }

    private static let stopEvent: [String: Any] = [
        "event": "Stop", "session_id": "sock-1", "cwd": "/tmp", "status": "waiting_for_input",
        "attended": true, "entrypoint": "cli",
    ]

    @Test func stopLeavesAnotherProcesssSocketInPlace() async throws {
        let path = Self.socketPath()
        let server = HookSocketServer(socketPath: path)
        server.start(onMessage: { _ in })
        #expect(try await Self.waitUntil { Self.isSocket(path) })

        // A newer instance took the path over (it unlinked ours and bound its own).
        unlink(path)
        let replacement = HookSocketServer(socketPath: path)
        replacement.start(onMessage: { _ in })
        #expect(try await Self.waitUntil { Self.isSocket(path) })

        // The retiring instance must not delete the newer one's socket.
        server.stop()
        #expect(Self.isSocket(path))
        replacement.stop()
        #expect(!FileManager.default.fileExists(atPath: path))
    }

    @Test func unlinkChecksTheInode() throws {
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("spcn-a1-inode-\(UUID().uuidString)").path
        FileManager.default.createFile(atPath: path, contents: Data("a".utf8))
        defer { try? FileManager.default.removeItem(atPath: path) }
        let original = try #require(FileIdentity(path: path))

        try FileManager.default.removeItem(atPath: path)
        FileManager.default.createFile(atPath: path, contents: Data("b".utf8))
        #expect(HookSocketServer.bindingState(path: path, identity: original) == .replaced)
        #expect(HookSocketServer.unlinkIfSame(path: path, identity: original) == false)
        #expect(FileManager.default.fileExists(atPath: path))

        let current = try #require(FileIdentity(path: path))
        #expect(HookSocketServer.bindingState(path: path, identity: current) == .ours)
        #expect(HookSocketServer.unlinkIfSame(path: path, identity: current))
        #expect(HookSocketServer.bindingState(path: path, identity: current) == .missing)
    }

    @Test func bindsAgainWhenTheSocketFileDisappears() async throws {
        let path = Self.socketPath()
        let received = Received()
        let server = HookSocketServer(socketPath: path, bindingCheckInterval: 0.2)
        server.start(onMessage: { received.add($0) })
        defer { server.stop() }
        #expect(try await Self.waitUntil { Self.isSocket(path) })
        #expect(try Self.send(Self.stopEvent, to: path))
        #expect(try await Self.waitUntil { received.count == 1 })

        // Someone deleted the file (an old build's stop, a cleanup script).
        unlink(path)
        #expect(!Self.isSocket(path))
        #expect(try await Self.waitUntil(timeout: 3) { Self.isSocket(path) })
        #expect(server.listeningPath == path)
        #expect(try Self.send(Self.stopEvent, to: path))
        #expect(try await Self.waitUntil { received.count == 2 })
    }

    @Test func doesNotFightAForeignSocket() async throws {
        let path = Self.socketPath()
        let server = HookSocketServer(socketPath: path, bindingCheckInterval: 0.2)
        server.start(onMessage: { _ in })
        #expect(try await Self.waitUntil { Self.isSocket(path) })

        let other = HookSocketServer(socketPath: path, bindingCheckInterval: 60)
        other.start(onMessage: { _ in })
        try await Task.sleep(nanoseconds: 300_000_000)
        let otherIdentity = FileIdentity(path: path)
        // Several binding checks later the first server has not taken it back.
        try await Task.sleep(nanoseconds: 800_000_000)
        #expect(FileIdentity(path: path) == otherIdentity)
        server.stop()
        #expect(Self.isSocket(path))
        other.stop()
    }

    @Test func refusesAnOverlongPath() async throws {
        let long = "/tmp/" + String(repeating: "x", count: 120) + ".sock"
        let server = HookSocketServer(socketPath: long)
        server.start(onMessage: { _ in })
        defer { server.stop() }
        #expect(try await Self.waitUntil { server.listenError != nil })
        #expect(server.listenError?.contains("longer than") == true)
        #expect(server.listeningPath == nil)
    }

    @Test func neverDeletesAFileThatIsNotASocket() async throws {
        let path = Self.socketPath()
        FileManager.default.createFile(atPath: path, contents: Data("keep me".utf8))
        defer { unlink(path) }
        let server = HookSocketServer(socketPath: path)
        server.start(onMessage: { _ in })
        defer { server.stop() }
        #expect(try await Self.waitUntil { server.listenError != nil })
        #expect(server.listenError?.contains("not a socket") == true)
        #expect(FileManager.default.contents(atPath: path) == Data("keep me".utf8))

        // A stale socket (a crashed run's) is replaced.
        unlink(path)
        let stale = socket(AF_UNIX, SOCK_STREAM, 0)
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &addr.sun_path) { buffer in
            let bytes = Array(path.utf8)
            buffer.copyBytes(from: bytes)
            buffer[bytes.count] = 0
        }
        _ = withUnsafePointer(to: &addr) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(stale, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        close(stale)
        #expect(Self.isSocket(path))
        #expect(HookSocketServer.removeStaleSocket(at: path) == nil)
        #expect(!FileManager.default.fileExists(atPath: path))
    }

    @Test func sharedFallbackFolderIsPrivateAndOurs() throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent("spcn-a1-fallback-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let shared = base.appendingPathComponent("spcn-\(getuid())").path
        let socket = (shared as NSString).appendingPathComponent("hook.sock")

        // Missing: created 0700.
        #expect(HookSocketDirectory.prepare(forSocketAt: socket, userID: getuid(), sharedDirectory: shared) == nil)
        var info = stat()
        #expect(lstat(shared, &info) == 0)
        #expect(info.st_mode & 0o777 == 0o700)

        // Opened up by someone: closed again.
        chmod(shared, 0o755)
        #expect(HookSocketDirectory.prepare(forSocketAt: socket, userID: getuid(), sharedDirectory: shared) == nil)
        #expect(lstat(shared, &info) == 0)
        #expect(info.st_mode & 0o777 == 0o700)

        // Not ours: refused.
        let foreign = HookSocketDirectory.prepare(forSocketAt: socket, userID: getuid() + 1, sharedDirectory: shared)
        #expect(foreign?.contains("another user") == true)

        // A planted symlink: refused.
        try FileManager.default.removeItem(atPath: shared)
        try FileManager.default.createSymbolicLink(atPath: shared, withDestinationPath: base.path)
        #expect(HookSocketDirectory.prepare(forSocketAt: socket, userID: getuid(), sharedDirectory: shared)?.contains("not a folder") == true)
    }

    @Test func otherParentFoldersAreOnlyCreated() throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent("spcn-a1-support-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: base) }
        let socket = base.appendingPathComponent("Claude/hook.sock").path
        #expect(HookSocketDirectory.prepare(forSocketAt: socket, userID: getuid()) == nil)
        var info = stat()
        #expect(lstat(base.appendingPathComponent("Claude").path, &info) == 0)
        #expect(info.st_mode & 0o777 == 0o700)
        // /tmp itself (root's, world-writable) is fine as a plain parent.
        #expect(HookSocketDirectory.prepare(forSocketAt: "/tmp/x.sock", userID: getuid()) == nil)
    }

    @Test func configurationFallsBackToTheSharedFolder() {
        let longSupport = "/Users/" + String(repeating: "u", count: 80)
        let configuration = ClaudeControlConfiguration.live(
            appDisplayName: "Test",
            bundleIdentifier: "test.spcn",
            supportFolderName: "Superpowered Codenotch",
            environment: ["HOME": longSupport],
            arguments: []
        )
        #expect(configuration.socketPath == HookSocketDirectory.fallbackDirectory(userID: getuid()) + "/hook.sock")
        #expect(configuration.socketPath.utf8.count <= ClaudeControlConfiguration.maxSocketPathBytes)

        let overridden = ClaudeControlConfiguration.live(
            appDisplayName: "Test", bundleIdentifier: "test.spcn", supportFolderName: "X",
            environment: ["HOME": "/Users/u", "SPCN_SOCKET": "/tmp/custom.sock"], arguments: []
        )
        #expect(overridden.socketPath == "/tmp/custom.sock")
    }

    @Test func backlogIsTheSystemMaximum() {
        #expect(HookSocketServer.listenBacklog == SOMAXCONN)
        #expect(HookSocketServer.listenBacklog > 64)
    }
}
