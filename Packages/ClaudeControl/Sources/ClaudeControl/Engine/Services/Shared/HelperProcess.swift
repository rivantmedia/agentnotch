//
//  HelperProcess.swift
//  ClaudeControl
//
//  How the focus, message and visibility paths run their short-lived helpers
//  (tmux, yabai, osascript): both pipes drained while the child runs, so an
//  answer bigger than a pipe buffer (64 KB, e.g. `yabai -m query --windows`
//  with many windows) can never wedge it; a timeout that sends SIGTERM and,
//  if the child ignores it, SIGKILL; and completion through the termination
//  handler, so no thread or actor is ever blocked waiting for a child.
//
//  Never a shell: the executable and its arguments are passed as they are.
//

import Foundation

nonisolated enum HelperProcess {
    /// What a finished child left behind.
    struct Output: Sendable, Equatable {
        var status: Int32
        /// False when the child died of a signal (including our timeout).
        var exitedNormally: Bool
        var timedOut: Bool
        var stdout: Data
        var stderr: Data

        var succeeded: Bool { exitedNormally && status == 0 && !timedOut }
        var stdoutText: String { String(decoding: stdout, as: UTF8.self) }
        var stderrText: String { String(decoding: stderr, as: UTF8.self) }
    }

    enum LaunchError: Error, Equatable, Sendable {
        case notFound(String)
        case failed(String)
    }

    /// Default bound for helpers that answer at once (tmux, yabai).
    static let defaultTimeout: TimeInterval = 10
    /// How long a child gets between SIGTERM and SIGKILL.
    static let killGrace: TimeInterval = 2
    /// How long to wait for the pipes to close after the child exited. A
    /// grandchild that inherited them (a daemon the helper forked) could keep
    /// them open forever; what was read by then is the answer.
    static let drainGrace: TimeInterval = 1

    /// Runs `executable` with `arguments` and returns once it has exited and
    /// its output is read (or `timeout` passed and it was killed).
    @concurrent
    static func run(
        _ executable: String,
        arguments: [String],
        environment: [String: String]? = nil,
        currentDirectory: URL? = nil,
        timeout: TimeInterval = defaultTimeout
    ) async -> Result<Output, LaunchError> {
        let job = Job(timeout: timeout)
        return await withCheckedContinuation { continuation in
            job.start(
                executable: executable,
                arguments: arguments,
                environment: environment,
                currentDirectory: currentDirectory,
                continuation: continuation
            )
        }
    }

    /// One child and its plumbing. Callbacks arrive on arbitrary queues;
    /// state is guarded by `lock` and the result is delivered once.
    private final class Job: @unchecked Sendable {
        private let timeout: TimeInterval
        private let lock = NSLock()
        private let process = Process()
        private let stdoutPipe = Pipe()
        private let stderrPipe = Pipe()
        private var stdout = Data()
        private var stderr = Data()
        private var stdoutClosed = false
        private var stderrClosed = false
        private var exited = false
        private var timedOut = false
        private var delivered = false
        private var continuation: CheckedContinuation<Result<Output, LaunchError>, Never>?

        init(timeout: TimeInterval) {
            self.timeout = timeout
        }

        func start(
            executable: String,
            arguments: [String],
            environment: [String: String]?,
            currentDirectory: URL?,
            continuation: CheckedContinuation<Result<Output, LaunchError>, Never>
        ) {
            self.continuation = continuation
            guard FileManager.default.isExecutableFile(atPath: executable) else {
                deliver(.failure(.notFound(executable)))
                return
            }
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            if let environment { process.environment = environment }
            if let currentDirectory { process.currentDirectoryURL = currentDirectory }
            process.standardInput = FileHandle.nullDevice
            process.standardOutput = stdoutPipe
            process.standardError = stderrPipe

            stdoutPipe.fileHandleForReading.readabilityHandler = { [self] handle in
                let data = handle.availableData
                lock.lock()
                if data.isEmpty {
                    handle.readabilityHandler = nil
                    stdoutClosed = true
                } else {
                    stdout.append(data)
                }
                lock.unlock()
                if data.isEmpty { finishIfDone() }
            }
            stderrPipe.fileHandleForReading.readabilityHandler = { [self] handle in
                let data = handle.availableData
                lock.lock()
                if data.isEmpty {
                    handle.readabilityHandler = nil
                    stderrClosed = true
                } else if stderr.count < 64 * 1024 {
                    stderr.append(data)
                }
                lock.unlock()
                if data.isEmpty { finishIfDone() }
            }
            process.terminationHandler = { [self] _ in
                lock.lock()
                exited = true
                lock.unlock()
                finishIfDone()
                // Output still arriving after the child exited belongs to a
                // grandchild; stop waiting for it after a moment.
                DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + HelperProcess.drainGrace) { [self] in
                    lock.lock()
                    stdoutClosed = true
                    stderrClosed = true
                    lock.unlock()
                    finishIfDone()
                }
            }

            do {
                try process.run()
            } catch {
                closeReaders()
                deliver(.failure(.failed(error.localizedDescription)))
                return
            }

            let process = self.process
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + timeout) { [self] in
                guard process.isRunning else { return }
                lock.lock()
                timedOut = true
                lock.unlock()
                process.terminate()
                DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + HelperProcess.killGrace) {
                    if process.isRunning {
                        kill(process.processIdentifier, SIGKILL)
                    }
                }
            }
        }

        private func finishIfDone() {
            lock.lock()
            let done = exited && stdoutClosed && stderrClosed
            let output = Output(
                status: process.isRunning ? -1 : process.terminationStatus,
                exitedNormally: !process.isRunning && process.terminationReason == .exit,
                timedOut: timedOut,
                stdout: stdout,
                stderr: stderr
            )
            lock.unlock()
            guard done else { return }
            closeReaders()
            deliver(.success(output))
        }

        private func closeReaders() {
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
        }

        private func deliver(_ result: Result<Output, LaunchError>) {
            lock.lock()
            guard !delivered, let continuation else {
                lock.unlock()
                return
            }
            delivered = true
            self.continuation = nil
            lock.unlock()
            // Break the Process ↔ handler cycle once the answer is out.
            process.terminationHandler = nil
            continuation.resume(returning: result)
        }
    }
}
