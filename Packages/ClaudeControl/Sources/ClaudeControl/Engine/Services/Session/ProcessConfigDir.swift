//
//  ProcessConfigDir.swift
//  ClaudeControl
//
//  Which config folder a running Claude Code process uses: its
//  `CLAUDE_CONFIG_DIR`, read from the kernel (`sysctl KERN_PROCARGS2`, the
//  process's launch arguments and environment) for processes of the same
//  user only. Needed because Claude Parallel Profiles links every folder's
//  `sessions/` to one shared folder: the registry file a session writes says
//  nothing about which account runs it, but its process's environment does.
//
//  The environment can hold secrets (`CLAUDE_CODE_MESSAGING_TOKEN`, API
//  keys, …). Only the one variable is extracted: every other entry is
//  compared byte by byte against its name and skipped without ever becoming
//  a string, and the buffer is zeroed before it is freed. Results are cached
//  per pid and process start time, so a pid is read once per process.
//

import Darwin
import Foundation

nonisolated enum ProcessConfigDir {
    /// What a process says about `CLAUDE_CONFIG_DIR`.
    enum Value: Equatable, Sendable {
        /// Not set: the process uses `~/.claude`.
        case unset
        /// Set, verbatim.
        case set(String)
        /// Another user's process, gone, or not readable.
        case unreadable
    }

    static let variable = "CLAUDE_CONFIG_DIR"

    /// Extract `variable` from a `KERN_PROCARGS2` buffer: `argc` (a 32-bit
    /// integer), the executable path, NUL padding, `argc` arguments, then the
    /// environment, each NUL-terminated. Pure.
    static func parse(_ buffer: UnsafeRawBufferPointer, variable: String = ProcessConfigDir.variable) -> Value {
        let bytes = buffer.bindMemory(to: UInt8.self)
        guard bytes.count >= MemoryLayout<Int32>.size else { return .unreadable }
        let argc = Int(buffer.loadUnaligned(as: Int32.self))
        guard argc >= 0 else { return .unreadable }
        var index = MemoryLayout<Int32>.size
        // The executable path, then its padding.
        while index < bytes.count, bytes[index] != 0 { index += 1 }
        while index < bytes.count, bytes[index] == 0 { index += 1 }
        // The arguments.
        var skipped = 0
        while skipped < argc, index < bytes.count {
            while index < bytes.count, bytes[index] != 0 { index += 1 }
            index += 1
            skipped += 1
        }
        guard skipped == argc else { return .unreadable }
        // The environment, up to the first empty entry. macOS leaves it out
        // for some processes (platform binaries): none at all means "can't
        // tell", never "unset" (that would send the session to ~/.claude).
        let prefix = Array((variable + "=").utf8)
        var sawEnvironment = false
        while index < bytes.count, bytes[index] != 0 {
            sawEnvironment = true
            let start = index
            while index < bytes.count, bytes[index] != 0 { index += 1 }
            let length = index - start
            if length >= prefix.count {
                var matches = true
                for offset in 0..<prefix.count where bytes[start + offset] != prefix[offset] {
                    matches = false
                    break
                }
                if matches {
                    let value = UnsafeBufferPointer(rebasing: bytes[(start + prefix.count)..<index])
                    let text = String(decoding: value, as: UTF8.self)
                    return text.isEmpty ? .unset : .set(text)
                }
            }
            index += 1
        }
        return sawEnvironment ? .unset : .unreadable
    }

    /// The process's `CLAUDE_CONFIG_DIR`, from the kernel. Same user only.
    static func read(pid: Int32) -> Value {
        guard pid > 0 else { return .unreadable }
        var bsd = proc_bsdinfo()
        let infoSize = Int32(MemoryLayout<proc_bsdinfo>.stride)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &bsd, infoSize) == infoSize,
              bsd.pbi_uid == getuid() else { return .unreadable }

        var argMax: Int32 = 0
        var argMaxSize = MemoryLayout<Int32>.size
        var argMaxMib: [Int32] = [CTL_KERN, KERN_ARGMAX]
        guard sysctl(&argMaxMib, 2, &argMax, &argMaxSize, nil, 0) == 0, argMax > 0 else { return .unreadable }

        var size = Int(argMax)
        let buffer = UnsafeMutableRawPointer.allocate(byteCount: size, alignment: MemoryLayout<Int32>.alignment)
        defer {
            // The environment may hold secrets: nothing of it outlives this call.
            memset_s(buffer, Int(argMax), 0, Int(argMax))
            buffer.deallocate()
        }
        var mib: [Int32] = [CTL_KERN, KERN_PROCARGS2, pid]
        guard sysctl(&mib, 3, buffer, &size, nil, 0) == 0, size > 0 else { return .unreadable }
        return parse(UnsafeRawBufferPointer(start: buffer, count: size))
    }
}

/// `ProcessConfigDir.read`, cached per pid and process start time.
nonisolated final class ProcessConfigDirCache: @unchecked Sendable {
    static let shared = ProcessConfigDirCache()

    private let lock = NSLock()
    private var entries: [Int32: (startedAt: Date?, value: ProcessConfigDir.Value)] = [:]
    private let reader: @Sendable (Int32) -> ProcessConfigDir.Value
    private let startDate: @Sendable (Int32) -> Date?

    init(reader: @escaping @Sendable (Int32) -> ProcessConfigDir.Value = ProcessConfigDir.read,
         startDate: @escaping @Sendable (Int32) -> Date? = { ProcessInspector.startDate(pid: Int($0)) }) {
        self.reader = reader
        self.startDate = startDate
    }

    func value(pid: Int32) -> ProcessConfigDir.Value {
        let started = startDate(pid)
        lock.lock()
        if let cached = entries[pid], cached.startedAt == started, started != nil {
            lock.unlock()
            return cached.value
        }
        lock.unlock()
        let value = reader(pid)
        lock.lock()
        if started != nil, value != .unreadable {
            entries[pid] = (started, value)
        } else {
            entries.removeValue(forKey: pid)
        }
        if entries.count > 1024 { entries.removeAll() }
        lock.unlock()
        return value
    }

    /// Forget processes that are gone.
    func retain(pids: Set<Int32>) {
        lock.lock()
        entries = entries.filter { pids.contains($0.key) }
        lock.unlock()
    }
}
