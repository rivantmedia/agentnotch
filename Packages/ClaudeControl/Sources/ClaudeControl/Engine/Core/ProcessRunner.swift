//
//  ProcessRunner.swift
//  ClaudeControl
//
//  Runs a short helper process (a login shell asked where `claude` is,
//  `claude --version`, `xcode-select -p`) with a hard time limit, and never
//  leaves anything behind:
//  - the child gets its own process group, so a timeout signals everything it
//    started (an rc file's background jobs, a pipeline), not just the child;
//  - it inherits no descriptor but stdin (/dev/null), stdout (a pipe) and
//    stderr (/dev/null), so it can't hold the app's sockets open;
//  - stdout is drained while the child runs, so a chatty child never blocks
//    on a full pipe, and only the last `outputLimit` bytes are kept.
//
//  Blocking; call it off the main actor.
//

import Darwin
import Foundation

nonisolated enum ProcessRunner {
    struct Result: Equatable, Sendable {
        /// The exit status, or 128 + the signal number for a child killed by a signal.
        let exitCode: Int32
        let stdout: String
    }

    /// Most stdout kept per run: the tail, which is where every caller's
    /// answer is (`command -v` prints last; banners come first).
    static let outputLimit = 1 << 20

    /// Run `executable` and return its exit code and stdout, or nil when it
    /// could not be started or outlived `timeout`. On timeout the child's
    /// process group gets SIGTERM, then SIGKILL after `killGrace`, and the
    /// child is reaped before this returns.
    static func run(
        executable: String,
        arguments: [String],
        environment: [String: String]? = nil,
        timeout: TimeInterval,
        killGrace: TimeInterval = 1
    ) -> Result? {
        var pipeEnds: [Int32] = [-1, -1]
        guard pipe(&pipeEnds) == 0 else { return nil }
        let readEnd = pipeEnds[0]
        let writeEnd = pipeEnds[1]
        // Never let another child spawned concurrently (by Foundation's
        // Process, say) inherit the write end: our read would not see EOF
        // until that unrelated child exits.
        _ = fcntl(readEnd, F_SETFD, FD_CLOEXEC)
        _ = fcntl(writeEnd, F_SETFD, FD_CLOEXEC)

        guard let pid = spawn(executable: executable, arguments: arguments, environment: environment, stdout: writeEnd) else {
            close(readEnd)
            close(writeEnd)
            return nil
        }
        close(writeEnd)

        let output = OutputBuffer(limit: outputLimit)
        let finishedReading = DispatchSemaphore(value: 0)
        DispatchQueue.global(qos: .userInitiated).async {
            var chunk = [UInt8](repeating: 0, count: 65_536)
            while true {
                let count = read(readEnd, &chunk, chunk.count)
                if count > 0 {
                    output.append(chunk[0..<count])
                } else if count < 0 && errno == EINTR {
                    continue
                } else {
                    break
                }
            }
            close(readEnd)
            finishedReading.signal()
        }

        let exit = ExitBox()
        let exited = DispatchSemaphore(value: 0)
        DispatchQueue.global(qos: .userInitiated).async {
            var status: Int32 = 0
            while waitpid(pid, &status, 0) < 0 && errno == EINTR {}
            exit.set(status)
            exited.signal()
        }

        if exited.wait(timeout: .now() + timeout) == .timedOut {
            killpg(pid, SIGTERM)
            if exited.wait(timeout: .now() + killGrace) == .timedOut {
                killpg(pid, SIGKILL)
                kill(pid, SIGKILL)
                _ = exited.wait(timeout: .now() + 5)
            }
            // The leader is gone, but the group may not be: a child it was
            // forking when the SIGTERM went out never got it (bash blocks
            // SIGTERM around fork, and a signal sent before a process exists
            // never reaches it). Sweep the group until it is empty.
            sweep(group: pid)
            // Something that left the group may still hold the pipe; the
            // reader finishes on its own once it lets go.
            _ = finishedReading.wait(timeout: .now() + 1)
            return nil
        }
        // The group may outlive its leader (a backgrounded grandchild):
        // stop it too rather than wait on its output.
        if finishedReading.wait(timeout: .now() + 2) == .timedOut {
            sweep(group: pid)
            _ = finishedReading.wait(timeout: .now() + 1)
        }
        return Result(exitCode: exit.code, stdout: String(decoding: output.data, as: UTF8.self))
    }

    /// SIGKILL what is left of the process group `group` (its leader has
    /// exited), again while anything is: a member caught forking can leave
    /// a new child behind that the first signal never reached. Gives up
    /// after about half a second.
    private static func sweep(group: pid_t) {
        for _ in 0..<50 {
            guard killpg(group, 0) == 0 else { return }
            killpg(group, SIGKILL)
            usleep(10_000)
        }
    }

    // MARK: - Spawning

    private static func spawn(executable: String, arguments: [String], environment: [String: String]?, stdout: Int32) -> pid_t? {
        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_adddup2(&actions, stdout, 1)
        posix_spawn_file_actions_addopen(&actions, 2, "/dev/null", O_WRONLY, 0)

        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        let flags = POSIX_SPAWN_SETPGROUP | POSIX_SPAWN_CLOEXEC_DEFAULT | POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF
        posix_spawnattr_setflags(&attributes, Int16(flags))
        posix_spawnattr_setpgroup(&attributes, 0)
        var noSignals = sigset_t()
        sigemptyset(&noSignals)
        posix_spawnattr_setsigmask(&attributes, &noSignals)
        var defaults = sigset_t()
        sigemptyset(&defaults)
        for signal in [SIGPIPE, SIGINT, SIGTERM, SIGHUP, SIGQUIT, SIGCHLD, SIGALRM, SIGUSR1, SIGUSR2] {
            sigaddset(&defaults, signal)
        }
        posix_spawnattr_setsigdefault(&attributes, &defaults)

        let argv = ([executable] + arguments).map { strdup($0) }
        defer { argv.forEach { free($0) } }
        let env = (environment ?? Foundation.ProcessInfo.processInfo.environment).map { strdup("\($0.key)=\($0.value)") }
        defer { env.forEach { free($0) } }

        var pid: pid_t = 0
        let status: Int32 = (argv + [nil]).withUnsafeBufferPointer { argvBuffer in
            (env + [nil]).withUnsafeBufferPointer { envBuffer in
                posix_spawn(&pid, executable, &actions, &attributes,
                            UnsafeMutablePointer(mutating: argvBuffer.baseAddress!),
                            UnsafeMutablePointer(mutating: envBuffer.baseAddress!))
            }
        }
        return status == 0 ? pid : nil
    }

    // MARK: - Boxes

    /// stdout collected on the reader thread; keeps the newest `limit` bytes.
    private final class OutputBuffer: @unchecked Sendable {
        private let lock = NSLock()
        private let limit: Int
        private var bytes = Data()

        init(limit: Int) { self.limit = limit }

        func append(_ chunk: ArraySlice<UInt8>) {
            lock.lock()
            defer { lock.unlock() }
            bytes.append(contentsOf: chunk)
            if bytes.count > limit * 2 {
                bytes = bytes.suffix(limit)
            }
        }

        var data: Data {
            lock.lock()
            defer { lock.unlock() }
            return bytes.count > limit ? bytes.suffix(limit) : bytes
        }
    }

    private final class ExitBox: @unchecked Sendable {
        private let lock = NSLock()
        private var status: Int32 = 0

        func set(_ status: Int32) {
            lock.lock()
            self.status = status
            lock.unlock()
        }

        /// WEXITSTATUS, or 128 + the signal for a signalled child.
        var code: Int32 {
            lock.lock()
            defer { lock.unlock() }
            let signal = status & 0x7F
            if signal == 0 { return (status >> 8) & 0xFF }
            return 128 + signal
        }
    }
}
