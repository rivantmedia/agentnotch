//
//  ProcessInspector.swift
//  ClaudeControl
//
//  What the session store needs to know about a Claude Code process, read
//  straight from the kernel with `proc_pidinfo`: when it started (so a reused
//  pid is not taken for the same process), its controlling terminal, and
//  whether tmux is among its ancestors. Each call is a few system calls and
//  never spawns `ps`, so it is cheap enough to run inside the store actor
//  for every new session (a `ps` spawn there stalled every other session's
//  events by ~40 ms per new session).
//

import Darwin
import Foundation

nonisolated enum ProcessInspector {
    /// Kernel facts about one process.
    struct Info: Equatable, Sendable {
        let pid: Int
        let parentPid: Int
        /// The executable's short name (`p_comm`, at most 16 bytes).
        let command: String
        let startedAt: Date
        /// Controlling terminal as `ttys003`, nil when there is none.
        let tty: String?
    }

    /// Deepest ancestor chain walked when looking for tmux.
    static let maxAncestorDepth = 24

    /// Kernel info for a running process, nil when it isn't running (or
    /// belongs to another user and can't be inspected).
    static func info(pid: Int) -> Info? {
        guard let pid32 = Int32(exactly: pid), pid32 > 0 else { return nil }
        var bsd = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.stride)
        guard proc_pidinfo(pid32, PROC_PIDTBSDINFO, 0, &bsd, size) == size else { return nil }
        let started = Date(timeIntervalSince1970: TimeInterval(bsd.pbi_start_tvsec) + TimeInterval(bsd.pbi_start_tvusec) / 1_000_000)
        let command = withUnsafeBytes(of: bsd.pbi_comm) { raw in
            String(decoding: raw.prefix { $0 != 0 }, as: UTF8.self)
        }
        return Info(
            pid: pid,
            parentPid: Int(bsd.pbi_ppid),
            command: command,
            startedAt: started,
            tty: terminalName(device: bsd.e_tdev)
        )
    }

    /// When the process started; nil when it isn't running.
    static func startDate(pid: Int) -> Date? {
        info(pid: pid)?.startedAt
    }

    /// Whether a tmux server or client is among the process's ancestors
    /// (the session runs in a tmux pane).
    static func isInTmux(pid: Int) -> Bool {
        var current = pid
        for _ in 0..<maxAncestorDepth {
            guard current > 1, let info = info(pid: current) else { return false }
            if info.command.lowercased().contains("tmux") { return true }
            guard info.parentPid != current else { return false }
            current = info.parentPid
        }
        return false
    }

    /// `ttys003` for a controlling terminal device, nil for none.
    static func terminalName(device: UInt32) -> String? {
        // NODEV (-1) and 0 both mean "no controlling terminal".
        guard device != UInt32.max, device != 0 else { return nil }
        guard let name = devname(dev_t(bitPattern: device), mode_t(S_IFCHR)) else { return nil }
        let text = String(cString: name)
        return text.isEmpty || text == "??" ? nil : text
    }
}
