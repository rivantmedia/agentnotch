//
//  ProcessTreeBuilder.swift
//  ClaudeControl
//
//  The process table, read straight from the kernel: `sysctl(KERN_PROC_ALL)`
//  for pids, parents, terminals and process groups, `proc_pidpath` for the
//  executable. No `ps` or `lsof` child, so a snapshot costs a few
//  milliseconds instead of a process spawn (30–120 ms with ~700 processes),
//  and asking about one process (its terminal, whether it runs under tmux,
//  whether it is in the foreground) costs a handful of syscalls.
//

import Darwin
import Foundation

/// One process in a snapshot of the table.
nonisolated struct ProcessEntry: Sendable, Equatable {
    let pid: Int
    let ppid: Int
    /// The executable's path (`proc_pidpath`), or the kernel's short name
    /// (16 characters) when the path can't be read (another user's process).
    let command: String
    /// The controlling terminal, e.g. `ttys003`; nil when it has none.
    let tty: String?

    init(pid: Int, ppid: Int, command: String, tty: String?) {
        self.pid = pid
        self.ppid = ppid
        self.command = command
        self.tty = tty
    }
}

/// What the kernel says about one live process, for the "is Claude really
/// the thing reading this terminal" checks.
nonisolated struct ProcessStatus: Sendable, Equatable {
    let pid: Int
    let ppid: Int
    let tty: String?
    /// The process group owns its terminal's foreground (`ps` shows `+`).
    let isForeground: Bool
    /// Suspended, e.g. with Ctrl+Z (`ps` shows `T`).
    let isStopped: Bool
}

nonisolated struct ProcessTreeBuilder: Sendable {
    static let shared = ProcessTreeBuilder()

    private init() {}

    // MARK: - Snapshot

    /// Every process, keyed by pid.
    func buildTree() -> [Int: ProcessEntry] {
        var tree: [Int: ProcessEntry] = [:]
        for info in Self.allProcesses() {
            let pid = Int(info.kp_proc.p_pid)
            guard pid > 0 else { continue }
            tree[pid] = Self.entry(info)
        }
        return tree
    }

    // MARK: - One Process

    /// Kernel status of `pid`; nil when it isn't running.
    func status(ofPid pid: Int) -> ProcessStatus? {
        guard let info = Self.kinfo(pid: pid) else { return nil }
        let stat = Int32(info.kp_proc.p_stat)
        let pgid = info.kp_eproc.e_pgid
        let tpgid = info.kp_eproc.e_tpgid
        return ProcessStatus(
            pid: pid,
            ppid: Int(info.kp_eproc.e_ppid),
            tty: Self.ttyName(info.kp_eproc.e_tdev),
            isForeground: tpgid > 0 && pgid == tpgid,
            isStopped: stat == SSTOP
        )
    }

    /// Whether tmux is among `pid`'s ancestors (or `pid` itself). Walks the
    /// parent chain with one syscall per hop instead of reading the table:
    /// what SessionStore needs for a new pid (it still reads the table).
    func isInTmux(pid: Int) -> Bool {
        var current = pid
        var depth = 0
        while current > 1 && depth < 40 {
            guard let info = Self.kinfo(pid: current) else { return false }
            if Self.entry(info).command.lowercased().contains("tmux") { return true }
            current = Int(info.kp_eproc.e_ppid)
            depth += 1
        }
        return false
    }

    /// Whether tmux is in `pid`'s parent chain, from a snapshot.
    func isInTmux(pid: Int, tree: [Int: ProcessEntry]) -> Bool {
        var current = pid
        var depth = 0
        while current > 1 && depth < 40 {
            guard let info = tree[current] else { break }
            if info.command.lowercased().contains("tmux") {
                return true
            }
            current = info.ppid
            depth += 1
        }
        return false
    }

    /// Whether `targetPid` is `ancestorPid` or one of its descendants.
    func isDescendant(targetPid: Int, ofAncestor ancestorPid: Int, tree: [Int: ProcessEntry]) -> Bool {
        var current = targetPid
        var depth = 0
        while current > 1 && depth < 50 {
            if current == ancestorPid {
                return true
            }
            guard let info = tree[current] else { break }
            current = info.ppid
            depth += 1
        }
        return false
    }

    // MARK: - Kernel

    private static func allProcesses() -> [kinfo_proc] {
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_ALL, 0]
        // The table can grow between the size query and the read; retry
        // with headroom a couple of times.
        for _ in 0..<3 {
            var size = 0
            guard sysctl(&mib, UInt32(mib.count), nil, &size, nil, 0) == 0, size > 0 else { return [] }
            size += size / 8
            let capacity = size / MemoryLayout<kinfo_proc>.stride
            var processes = [kinfo_proc](repeating: kinfo_proc(), count: capacity)
            let result = processes.withUnsafeMutableBytes { buffer in
                sysctl(&mib, UInt32(mib.count), buffer.baseAddress, &size, nil, 0)
            }
            if result == 0 {
                return Array(processes.prefix(size / MemoryLayout<kinfo_proc>.stride))
            }
            if errno != ENOMEM { return [] }
        }
        return []
    }

    private static func kinfo(pid: Int) -> kinfo_proc? {
        guard pid > 0, let pid32 = Int32(exactly: pid) else { return nil }
        var info = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid32]
        guard sysctl(&mib, UInt32(mib.count), &info, &size, nil, 0) == 0,
              size > 0, info.kp_proc.p_pid == pid32 else { return nil }
        return info
    }

    private static func entry(_ info: kinfo_proc) -> ProcessEntry {
        let pid = Int(info.kp_proc.p_pid)
        return ProcessEntry(
            pid: pid,
            ppid: Int(info.kp_eproc.e_ppid),
            command: executablePath(pid: pid) ?? shortName(info),
            tty: ttyName(info.kp_eproc.e_tdev)
        )
    }

    /// `PROC_PIDPATHINFO_MAXSIZE` (4 × MAXPATHLEN) is a macro Swift can't import.
    private static let maxPathSize = 4 * Int(MAXPATHLEN)

    private static func executablePath(pid: Int) -> String? {
        var buffer = [CChar](repeating: 0, count: maxPathSize)
        let length = proc_pidpath(Int32(pid), &buffer, UInt32(buffer.count))
        guard length > 0 else { return nil }
        return String(decoding: buffer.prefix(Int(length)).map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }

    private static func shortName(_ info: kinfo_proc) -> String {
        var comm = info.kp_proc.p_comm
        return withUnsafeBytes(of: &comm) { raw in
            String(decoding: raw.prefix { $0 != 0 }, as: UTF8.self)
        }
    }

    /// `ttys003` for a terminal device; nil for "no controlling terminal".
    private static func ttyName(_ device: dev_t) -> String? {
        // NODEV (-1) means none; devname would read it as a device number.
        guard device != -1, let name = devname(device, S_IFCHR) else { return nil }
        let tty = String(cString: name)
        return tty.isEmpty || tty == "??" ? nil : tty
    }
}
