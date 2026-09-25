//
//  ProcessExecutor.swift
//  ClaudeControl
//
//  Runs tmux and yabai for the focus and message paths, on top of
//  `HelperProcess` (pipes drained concurrently, a timeout that escalates to
//  SIGKILL, no blocked thread or actor). A plain value: every call is its own
//  child, so there is nothing to serialize.
//

import Foundation
import os.log

/// Why a helper produced no usable output.
nonisolated enum ProcessExecutorError: Error, LocalizedError, Equatable, Sendable {
    case executionFailed(command: String, exitCode: Int32, stderr: String?)
    case timedOut(command: String)
    case commandNotFound(String)
    case launchFailed(command: String, reason: String)

    var errorDescription: String? {
        switch self {
        case .executionFailed(let command, let exitCode, let stderr):
            let stderrInfo = stderr.map { ", stderr: \($0)" } ?? ""
            return "Command '\(command)' failed with exit code \(exitCode)\(stderrInfo)"
        case .timedOut(let command):
            return "Command '\(command)' did not finish in time"
        case .commandNotFound(let command):
            return "Command not found: \(command)"
        case .launchFailed(let command, let reason):
            return "Failed to launch '\(command)': \(reason)"
        }
    }
}

/// A finished helper's output.
nonisolated struct ProcessResult: Sendable, Equatable {
    let output: String
    let exitCode: Int32
    let stderr: String?

    var isSuccess: Bool { exitCode == 0 }
}

nonisolated struct ProcessExecutor: Sendable {
    static let shared = ProcessExecutor()

    private static var logger: Logger { EngineLog.logger("ProcessExecutor") }

    /// Bound for one helper run.
    let timeout: TimeInterval

    init(timeout: TimeInterval = HelperProcess.defaultTimeout) {
        self.timeout = timeout
    }

    /// The helper's stdout; throws when it can't be run, fails or times out.
    func run(_ executable: String, arguments: [String]) async throws -> String {
        switch await runWithResult(executable, arguments: arguments) {
        case .success(let result): return result.output
        case .failure(let error): throw error
        }
    }

    /// The helper's stdout, exit code and stderr.
    func runWithResult(_ executable: String, arguments: [String]) async -> Result<ProcessResult, ProcessExecutorError> {
        let outcome = await HelperProcess.run(executable, arguments: arguments, timeout: timeout)
        return Self.result(of: outcome, executable: executable, arguments: arguments)
    }

    /// Maps a runner outcome to this API's result. Pure.
    static func result(
        of outcome: Result<HelperProcess.Output, HelperProcess.LaunchError>,
        executable: String,
        arguments: [String]
    ) -> Result<ProcessResult, ProcessExecutorError> {
        switch outcome {
        case .failure(.notFound(let path)):
            logger.error("Command not found: \(path, privacy: .public)")
            return .failure(.commandNotFound(path))
        case .failure(.failed(let reason)):
            logger.error("Failed to launch \(executable, privacy: .public): \(reason, privacy: .public)")
            return .failure(.launchFailed(command: executable, reason: reason))
        case .success(let output):
            if output.timedOut {
                logger.error("\(executable, privacy: .public) \(arguments.first ?? "", privacy: .public) timed out")
                return .failure(.timedOut(command: executable))
            }
            let stderr = output.stderr.isEmpty ? nil : output.stderrText
            guard output.succeeded else {
                logger.warning("\(executable, privacy: .public) \(arguments.first ?? "", privacy: .public) exited \(output.status)")
                return .failure(.executionFailed(command: executable, exitCode: output.status, stderr: stderr))
            }
            return .success(ProcessResult(output: output.stdoutText, exitCode: output.status, stderr: stderr))
        }
    }
}
