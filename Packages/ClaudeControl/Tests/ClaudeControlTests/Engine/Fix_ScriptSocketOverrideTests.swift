import Foundation
import Testing
@testable import ClaudeControl

/// S10: an installed script follows AGENTNOTCH_SOCKET only with AGENTNOTCH_DEV=1 too, so
/// a leftover `export AGENTNOTCH_SOCKET` never sends a real session elsewhere.
///
/// Not main-actor bound: it blocks on child processes (see StatusLineScriptTests).
nonisolated struct Fix_ScriptSocketOverrideTests {
    private func socketPath(script: String, environment: [String: String]) throws -> String {
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("agentnotch-override-\(UUID().uuidString).py")
        try script.write(to: file, atomically: true, encoding: .utf8)
        defer { try? FileManager.default.removeItem(at: file) }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        // Loaded as a module (not __main__), so only its top level runs.
        process.arguments = ["python3", "-S", "-c",
                             "import runpy,sys; print(runpy.run_path(sys.argv[1], run_name='probe')['SOCKET_PATH'])",
                             file.path]
        process.environment = ["PATH": "/usr/bin:/bin"].merging(environment) { $1 }
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try process.run()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    @Test func aLeftoverExportIsIgnored() throws {
        let baked = "/Users/me/Library/Application Support/Agent Notch/Claude/hook.sock"
        for script in [EmbeddedScripts.hook(socketPath: baked), EmbeddedScripts.statusLine(socketPath: baked)] {
            #expect(try socketPath(script: script, environment: ["AGENTNOTCH_SOCKET": "/tmp/elsewhere.sock"]) == baked)
            #expect(try socketPath(script: script, environment: ["AGENTNOTCH_SOCKET": "/tmp/dev.sock", "AGENTNOTCH_DEV": "1"]) == "/tmp/dev.sock")
            #expect(try socketPath(script: script, environment: [:]) == baked)
        }
    }
}
