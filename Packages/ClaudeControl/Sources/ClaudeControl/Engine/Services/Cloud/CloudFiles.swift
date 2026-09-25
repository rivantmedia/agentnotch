//
//  CloudFiles.swift
//  ClaudeControl
//
//  How the cloud services keep their state in the engine's folder
//  (`AppIdentity.supportDirectory`, 0700): each file is written whole and
//  atomically (a new file, created 0600 beside the old one, flushed, then
//  renamed over it), so a crash never leaves half a file and no other user
//  can read one even for a moment. A sealed run keeps everything in memory.
//

import Foundation
import os.log
import Security

nonisolated enum CloudFiles {
    private static var logger: Logger { EngineLog.logger("CloudFiles") }

    enum WriteError: Error, Equatable {
        case open(Int32)
        case write(Int32)
        case rename(Int32)
    }

    /// Write `data` to `url`: a temporary file in the same folder, created
    /// with `permissions` (0600 by default), flushed to disk and renamed over
    /// `url`. The folder is created 0700 when missing and `createsFolder`.
    static func writeAtomically(_ data: Data, to url: URL, permissions: mode_t = 0o600,
                                createsFolder: Bool = true) throws {
        let folder = url.deletingLastPathComponent()
        if createsFolder, !FileManager.default.fileExists(atPath: folder.path) {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
        }
        let temporary = folder.appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString).tmp")
        let fd = open(temporary.path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, permissions)
        guard fd >= 0 else { throw WriteError.open(errno) }
        var failure: WriteError?
        // The mode passed to open is filtered by the umask; set it outright.
        _ = fchmod(fd, permissions)
        data.withUnsafeBytes { buffer in
            guard var pointer = buffer.baseAddress else { return }
            var remaining = buffer.count
            while remaining > 0 {
                let written = Darwin.write(fd, pointer, remaining)
                if written < 0 {
                    if errno == EINTR { continue }
                    failure = .write(errno)
                    return
                }
                remaining -= written
                pointer = pointer.advanced(by: written)
            }
        }
        if failure == nil, fsync(fd) != 0 { failure = .write(errno) }
        close(fd)
        if let failure {
            unlink(temporary.path)
            throw failure
        }
        guard rename(temporary.path, url.path) == 0 else {
            let code = errno
            unlink(temporary.path)
            throw WriteError.rename(code)
        }
    }

    /// The POSIX permission bits of a file (tests).
    static func permissions(of url: URL) -> mode_t? {
        var info = stat()
        guard stat(url.path, &info) == 0 else { return nil }
        return info.st_mode & 0o777
    }

    /// Create `url` with `data` only if nothing is there yet: a temporary
    /// file (0600, flushed) hard-linked into place, which fails when `url`
    /// exists, so two runs racing both end up with the first one's file.
    /// Returns whether this call created it.
    static func createExclusively(_ data: Data, at url: URL) throws -> Bool {
        let folder = url.deletingLastPathComponent()
        if !FileManager.default.fileExists(atPath: folder.path) {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
        }
        let temporary = folder.appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString).tmp")
        try writeAtomically(data, to: temporary, createsFolder: false)
        defer { unlink(temporary.path) }
        if link(temporary.path, url.path) == 0 { return true }
        let code = errno
        if code == EEXIST { return false }
        throw WriteError.rename(code)
    }
}

/// This install's secret for project keys (`CloudKeys.projectKey`): 32
/// random bytes made once and kept in `<support>/cloud-install-secret`
/// (0600, in the 0700 folder). It never leaves the Mac. Only a live run's
/// `CloudStores` reads or makes it, the first time a sync pass needs a
/// project key (signed in, with sync on), never at launch; a sealed run has
/// no stores, and a store that isn't persisted (tests) gets a random one in
/// memory.
nonisolated enum CloudInstallSecret {
    static let fileName = "cloud-install-secret"
    static let length = 32

    private static var logger: Logger { EngineLog.logger("CloudFiles") }

    /// 32 bytes from the system's random source.
    static func random() -> Data {
        var bytes = [UInt8](repeating: 0, count: length)
        if SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) != errSecSuccess {
            bytes = (0..<length).map { _ in UInt8.random(in: .min ... .max) }
        }
        return Data(bytes)
    }

    /// The secret in `directory`, made first when there is none. A file of
    /// the wrong size (damaged) is replaced: project keys made from now on
    /// differ from the old ones, which is better than none. Nil when the
    /// folder can't be written.
    static func load(directory: URL) -> Data? {
        let url = directory.appendingPathComponent(fileName)
        if let saved = try? Data(contentsOf: url), saved.count == length { return saved }
        let made = random()
        do {
            if FileManager.default.fileExists(atPath: url.path) {
                logger.error("\(fileName, privacy: .public) is damaged; making a new one")
                try CloudFiles.writeAtomically(made, to: url)
                return made
            }
            if try CloudFiles.createExclusively(made, at: url) { return made }
            // Another run made it a moment ago: use that one.
            if let saved = try? Data(contentsOf: url), saved.count == length { return saved }
            return nil
        } catch {
            logger.error("Couldn't keep \(fileName, privacy: .public): \(String(describing: error), privacy: .public)")
            return nil
        }
    }
}

/// One JSON file of cloud state: read once, written in the background or
/// right away. Background writes are throttled: at most one per
/// `writeDelay`, always of the newest value. `persists: false` (sealed
/// runs, some tests) never touches the disk.
nonisolated final class CloudStateFile<Value: Codable & Sendable>: @unchecked Sendable {
    let url: URL?
    private let persists: Bool
    private let writeDelay: TimeInterval
    private let queue: DispatchQueue
    private let lock = NSLock()
    /// The newest value not written yet, and whether a write is on its way.
    private var pending: Value?
    private var scheduled = false

    private static var logger: Logger { EngineLog.logger("CloudFiles") }

    init(url: URL?, persists: Bool, label: String, writeDelay: TimeInterval = 0) {
        self.url = url
        self.persists = persists && url != nil
        self.writeDelay = writeDelay
        queue = DispatchQueue(label: EngineLog.queueLabel(label), qos: .utility)
    }

    /// The saved value; nil when there is none, it can't be read, or the
    /// file isn't persisted.
    func load() -> Value? {
        guard persists, let url, let data = try? Data(contentsOf: url) else { return nil }
        do {
            return try CloudJSON.makeDecoder().decode(Value.self, from: data)
        } catch {
            Self.logger.error("\(url.lastPathComponent, privacy: .public) can't be read; starting fresh: \(error.localizedDescription, privacy: .public)")
            return nil
        }
    }

    /// Write `value` in the background, within `writeDelay`; the newest
    /// value saved by then is the one written.
    func save(_ value: Value) {
        guard persists, let url else { return }
        let schedules = lock.withLock { () -> Bool in
            pending = value
            if scheduled { return false }
            scheduled = true
            return true
        }
        guard schedules else { return }
        queue.asyncAfter(deadline: .now() + writeDelay) { [weak self] in
            guard let self else { return }
            let value = self.lock.withLock { () -> Value? in
                self.scheduled = false
                defer { self.pending = nil }
                return self.pending
            }
            if let value { Self.write(value, to: url) }
        }
    }

    /// Write `value` now, on the calling thread (quitting, tests).
    func saveNow(_ value: Value) {
        guard persists, let url else { return }
        lock.withLock { pending = nil }
        queue.sync { Self.write(value, to: url) }
    }

    /// Remove the file.
    func remove() {
        guard persists, let url else { return }
        lock.withLock { pending = nil }
        queue.sync { _ = try? FileManager.default.removeItem(at: url) }
    }

    private static func write(_ value: Value, to url: URL) {
        do {
            let data = try CloudJSON.makeEncoder().encode(value)
            try CloudFiles.writeAtomically(data, to: url)
        } catch {
            logger.error("Saving \(url.lastPathComponent, privacy: .public) failed: \(String(describing: error), privacy: .public)")
        }
    }
}
