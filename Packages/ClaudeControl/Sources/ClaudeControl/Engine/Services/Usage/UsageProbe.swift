//
//  UsageProbe.swift
//  ClaudeControl
//
//  Asks Claude Code itself for an account's plan usage, so the app never
//  touches OAuth tokens: Claude Code reads its own keychain item, refreshes
//  the token under its own lock, and answers from its own 60-second cache.
//
//      CLAUDE_CONFIG_DIR=<raw, or unset for the default account> \
//      claude -p --input-format stream-json --output-format stream-json --verbose \
//        --no-session-persistence --strict-mcp-config --settings '{"disableAllHooks":true}'
//
//  run from an empty working directory, with two control requests on stdin:
//  `initialize`, then (once initialized) `get_usage` with `skip_behaviors`.
//  The `get_usage` control response carries the usage body. No model request
//  is made and no transcript is written; hooks are off, so the probe never
//  shows up as a session.
//
//  The child is bounded by a timeout and always terminated and reaped. It
//  never inherits the variables Claude Code sets for its own subprocesses
//  (a GUI app normally has none, but a dev run from a terminal might).
//

import Foundation
import os.log

nonisolated enum UsageProbe {
    private static var logger: Logger { EngineLog.logger("UsageProbe") }

    /// Upper bound for one probe, launch to answer.
    static let defaultTimeout: TimeInterval = 20

    enum Outcome: Equatable, Sendable {
        case usage(ParsedUsage)
        /// Usage can't be had for this login; don't retry soon.
        case unavailable(String)
        /// Claude Code's usage request was rate limited; back off.
        case rateLimited
        /// Anything else: launch failure, timeout, error response, bad output.
        case failed(String)
    }

    // MARK: - Command Line

    static let initializeRequestId = "agentnotch-init"
    static let usageRequestId = "agentnotch-usage"

    static let arguments: [String] = [
        "-p",
        "--input-format", "stream-json",
        "--output-format", "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--strict-mcp-config",
        "--settings", #"{"disableAllHooks":true}"#,
    ]

    static var initializeRequest: Data {
        controlRequest(id: initializeRequestId, request: ["subtype": "initialize"])
    }

    static var usageRequest: Data {
        controlRequest(id: usageRequestId, request: ["subtype": "get_usage", "skip_behaviors": true])
    }

    private static func controlRequest(id: String, request: [String: Any]) -> Data {
        let message: [String: Any] = ["type": "control_request", "request_id": id, "request": request]
        var data = (try? JSONSerialization.data(withJSONObject: message, options: [.sortedKeys])) ?? Data()
        data.append(0x0A)
        return data
    }

    /// The child's environment: ours minus everything Claude Code sets for
    /// its own subprocesses (session IDs, the messaging token, SDK/agent
    /// markers), with CLAUDE_CONFIG_DIR set to the account's raw value, or
    /// removed for the default account.
    static func environment(base: [String: String], configDirEnv: String?) -> [String: String] {
        var env = base.filter { key, _ in
            !(key == "CLAUDECODE"
                || key == "CLAUDE_PID"
                || key == "CLAUDE_EFFORT"
                || key == "AI_AGENT"
                || key == "CLAUDE_CONFIG_DIR"
                || key.hasPrefix("CLAUDE_CODE_")
                || key.hasPrefix("CLAUDE_AGENT_SDK_"))
        }
        if let configDirEnv, !configDirEnv.isEmpty {
            env["CLAUDE_CONFIG_DIR"] = configDirEnv
        }
        return env
    }

    // MARK: - Output Parsing

    /// What one stdout line means to the probe.
    enum LineEvent: Equatable, Sendable {
        /// The `initialize` request was answered (successfully or not).
        case initialized(error: String?)
        /// The `get_usage` request was answered.
        case usage(Outcome)
        /// Anything else Claude Code prints (system/init messages, ...).
        case other
    }

    /// Classify one line of stream-json output. Pure.
    static func parseLine(_ line: Data) -> LineEvent {
        guard let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
              (object["type"] as? String) == "control_response",
              let response = object["response"] as? [String: Any],
              let requestId = response["request_id"] as? String else {
            return .other
        }
        let subtype = response["subtype"] as? String
        let error = subtype == "success" ? nil : (response["error"] as? String ?? "Request failed")

        switch requestId {
        case initializeRequestId:
            return .initialized(error: error)

        case usageRequestId:
            if let error {
                return .usage(outcome(forErrorMessage: error))
            }
            guard let body = response["response"] as? [String: Any] else {
                return .usage(.failed("Empty usage response"))
            }
            switch UsageParser.parseGetUsageResponse(body) {
            case .usage(let usage): return .usage(.usage(usage))
            case .unavailable(let reason): return .usage(.unavailable(reason))
            case .rateLimited: return .usage(.rateLimited)
            case .malformed(let reason): return .usage(.failed(reason))
            }

        default:
            return .other
        }
    }

    /// Map a control-response error string to an outcome. Not-logged-in and
    /// API-key setups are permanent for this login, not worth retrying soon.
    static func outcome(forErrorMessage message: String) -> Outcome {
        let lower = message.lowercased()
        if lower.contains("rate limit") || lower.contains("429") {
            return .rateLimited
        }
        if lower.contains("not logged in") || lower.contains("please run /login") || lower.contains("claude.ai") {
            return .unavailable("Not signed in to Claude")
        }
        return .failed(String(message.prefix(200)))
    }

    // MARK: - Running

    /// Run one probe. Never throws; every failure becomes an outcome.
    /// `@concurrent`: launching the child (posix_spawn, pipe setup) must not
    /// run on the caller's actor, which for UsageStore is the main actor.
    @concurrent
    static func run(
        claudePath: String,
        configDirEnv: String?,
        workingDirectory: URL,
        timeout: TimeInterval = defaultTimeout
    ) async -> Outcome {
        let session = ProbeSession(
            claudePath: claudePath,
            configDirEnv: configDirEnv,
            workingDirectory: workingDirectory,
            timeout: timeout
        )
        let outcome = await session.run()
        switch outcome {
        case .usage:
            logger.info("Usage probe succeeded")
        case .unavailable(let reason):
            logger.info("Usage unavailable: \(reason, privacy: .public)")
        case .rateLimited:
            logger.notice("Usage probe rate limited")
        case .failed(let reason):
            logger.error("Usage probe failed: \(reason, privacy: .public)")
        }
        return outcome
    }

    /// One `claude -p` child and its plumbing. Callbacks arrive on arbitrary
    /// queues; state is guarded by `lock`, and the result is delivered once.
    private final class ProbeSession: @unchecked Sendable {
        private let claudePath: String
        private let configDirEnv: String?
        private let workingDirectory: URL
        private let timeout: TimeInterval

        private let lock = NSLock()
        private let process = Process()
        private let stdinPipe = Pipe()
        private let stdoutPipe = Pipe()
        private let stderrPipe = Pipe()
        private var buffer = Data()
        private var stderrTail = Data()
        private var sentUsageRequest = false
        private var finished = false
        private var continuation: CheckedContinuation<Outcome, Never>?
        /// Keeps the session (and its Process) alive until the child exits,
        /// even after the caller has its answer, so the child is always reaped.
        private var keepAlive: ProbeSession?

        /// Bytes of stdout we're willing to buffer without seeing a newline.
        private static let maxLineBytes = 8 * 1024 * 1024

        init(claudePath: String, configDirEnv: String?, workingDirectory: URL, timeout: TimeInterval) {
            self.claudePath = claudePath
            self.configDirEnv = configDirEnv
            self.workingDirectory = workingDirectory
            self.timeout = timeout
        }

        func run() async -> Outcome {
            await withCheckedContinuation { continuation in
                lock.lock()
                self.continuation = continuation
                lock.unlock()
                start()
            }
        }

        private func start() {
            try? FileManager.default.createDirectory(at: workingDirectory, withIntermediateDirectories: true)

            process.executableURL = URL(fileURLWithPath: claudePath)
            process.arguments = UsageProbe.arguments
            process.environment = UsageProbe.environment(
                base: ClaudeBinaryLocator.environment(forBinaryAt: claudePath),
                configDirEnv: configDirEnv
            )
            process.currentDirectoryURL = workingDirectory
            process.standardInput = stdinPipe
            process.standardOutput = stdoutPipe
            process.standardError = stderrPipe

            // Writing to a child that already exited must fail, not SIGPIPE the app.
            _ = fcntl(stdinPipe.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1)

            stdoutPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
                let data = handle.availableData
                if data.isEmpty {
                    handle.readabilityHandler = nil
                    return
                }
                self?.consumeStdout(data)
            }
            stderrPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
                let data = handle.availableData
                if data.isEmpty {
                    handle.readabilityHandler = nil
                    return
                }
                self?.consumeStderr(data)
            }
            process.terminationHandler = { [weak self] process in
                self?.childExited(status: process.terminationStatus)
            }

            do {
                try process.run()
            } catch {
                cleanUpHandlers()
                finish(.failed("Couldn't start Claude Code: \(error.localizedDescription)"))
                return
            }
            lock.lock()
            keepAlive = self
            lock.unlock()

            write(UsageProbe.initializeRequest)

            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + timeout) { [weak self] in
                self?.finish(.failed("Claude Code didn't answer within \(Int(self?.timeout ?? 0))s"))
            }
        }

        private func consumeStdout(_ data: Data) {
            lock.lock()
            buffer.append(data)
            var lines: [Data] = []
            while let newline = buffer.firstIndex(of: 0x0A) {
                lines.append(buffer.subdata(in: buffer.startIndex..<newline))
                buffer.removeSubrange(buffer.startIndex...newline)
            }
            let overflow = buffer.count > Self.maxLineBytes
            if overflow { buffer.removeAll() }
            lock.unlock()

            if overflow {
                finish(.failed("Unexpected output from Claude Code"))
                return
            }
            for line in lines where !line.isEmpty {
                handle(UsageProbe.parseLine(line))
            }
        }

        private func consumeStderr(_ data: Data) {
            lock.lock()
            stderrTail.append(data)
            if stderrTail.count > 4096 {
                stderrTail = stderrTail.suffix(4096)
            }
            lock.unlock()
        }

        private func handle(_ event: LineEvent) {
            switch event {
            case .initialized(let error):
                if let error {
                    finish(UsageProbe.outcome(forErrorMessage: error))
                    return
                }
                lock.lock()
                let alreadySent = sentUsageRequest
                sentUsageRequest = true
                lock.unlock()
                if !alreadySent {
                    write(UsageProbe.usageRequest)
                }
            case .usage(let outcome):
                finish(outcome)
            case .other:
                break
            }
        }

        private func write(_ data: Data) {
            do {
                try stdinPipe.fileHandleForWriting.write(contentsOf: data)
            } catch {
                // The child is gone or closed its input; its exit (or the
                // timeout) reports what happened.
                UsageProbe.logger.debug("Couldn't write to Claude Code: \(error.localizedDescription, privacy: .public)")
            }
        }

        private func childExited(status: Int32) {
            // Give the stdout handler a moment to deliver the last lines: an
            // answer written just before exiting must still count.
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 0.3) { [self] in
                lock.lock()
                let tail = String(data: stderrTail, encoding: .utf8) ?? ""
                lock.unlock()
                finish(.failed(Self.exitDescription(status: status, stderr: tail)))
                cleanUpHandlers()
                lock.lock()
                keepAlive = nil
                lock.unlock()
            }
        }

        /// A short reason for an early exit: the last non-empty stderr line
        /// (Claude Code prints errors such as "Invalid API key" there), capped.
        private static func exitDescription(status: Int32, stderr: String) -> String {
            let lastLine = stderr
                .split(whereSeparator: \.isNewline)
                .map { $0.trimmingCharacters(in: .whitespaces) }
                .last { !$0.isEmpty }
            if let lastLine {
                return "Claude Code exited (\(status)): \(lastLine.prefix(160))"
            }
            return "Claude Code exited with status \(status)"
        }

        /// Deliver the outcome once, then shut the child down: close stdin so
        /// it exits on its own, terminate after a grace period, kill if it
        /// still lingers. Foundation reaps it when it exits.
        private func finish(_ outcome: Outcome) {
            lock.lock()
            guard !finished else {
                lock.unlock()
                return
            }
            finished = true
            let continuation = self.continuation
            self.continuation = nil
            lock.unlock()

            continuation?.resume(returning: outcome)

            try? stdinPipe.fileHandleForWriting.close()
            guard process.isRunning else {
                cleanUpHandlers()
                return
            }
            let process = self.process
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 2) { [weak self] in
                if process.isRunning {
                    process.terminate()
                }
                DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 3) {
                    if process.isRunning {
                        kill(process.processIdentifier, SIGKILL)
                    }
                    self?.cleanUpHandlers()
                }
            }
        }

        private func cleanUpHandlers() {
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
        }
    }
}
