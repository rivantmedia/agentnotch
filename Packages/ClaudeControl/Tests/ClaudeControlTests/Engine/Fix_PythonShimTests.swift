import Foundation
import Testing
@testable import ClaudeControl

/// S8: the bare-`python3` command runs a real interpreter found on PATH, and
/// exits 0 (running nothing) when that is only xcode-select's shim.
struct Fix_PythonShimTests {
    private func run(_ command: String, path: String) throws -> (status: Int32, output: String) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", command]
        process.environment = ["PATH": path]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try process.run()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return (process.terminationStatus, String(decoding: data, as: UTF8.self))
    }

    @Test func anInterpreterOnPathRuns() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("agentnotch-shim-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let script = dir.appendingPathComponent("hook.py")
        try "".write(to: script, atomically: true, encoding: .utf8)
        // A stand-in `python3` that says it ran.
        let fake = dir.appendingPathComponent("python3")
        try "#!/bin/sh\necho ran \"$@\"\n".write(to: fake, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: fake.path)
        let command = HookCommands.command(runningScript: script.path, python: "python3")
        let result = try run(command, path: dir.path + ":/usr/bin:/bin")
        #expect(result.status == 0)
        #expect(result.output.hasPrefix("ran -S "))
    }

    @Test func nothingOnPathExitsQuietly() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("agentnotch-shim-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let script = dir.appendingPathComponent("hook.py")
        try "".write(to: script, atomically: true, encoding: .utf8)
        let command = HookCommands.command(runningScript: script.path, python: "python3")
        // No python3 anywhere on this PATH: exit 0 when there are no developer
        // tools, else 127 from exec (which Claude Code only logs); never 2.
        let result = try run(command, path: dir.path)
        #expect(result.status == 0 || result.status == 127)
    }
}
