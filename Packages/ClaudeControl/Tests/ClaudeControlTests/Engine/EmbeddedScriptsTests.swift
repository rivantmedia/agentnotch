import Foundation
import Testing
@testable import ClaudeControl

/// The embedded scripts are the .py files byte for byte, and installing fills
/// in exactly one socket placeholder.
struct EmbeddedScriptsTests {
    private static func bytes(_ name: String) throws -> [UInt8] {
        Array(try Data(contentsOf: ScriptPaths.scripts.appendingPathComponent(name)))
    }

    /// Bytes, not `String ==`: Swift compares strings by canonical
    /// equivalence, which would let a differently normalized copy pass.
    @Test func embeddedCopiesMatchTheScripts() throws {
        #expect(Array(EmbeddedScriptSources.hook.utf8) == (try Self.bytes(ClaudeControlConfiguration.defaultHookScriptName)))
        #expect(Array(EmbeddedScriptSources.statusLine.utf8) == (try Self.bytes(ClaudeControlConfiguration.defaultStatusLineScriptName)))
    }

    @Test func eachScriptHasOnePlaceholder() {
        for source in [EmbeddedScriptSources.hook, EmbeddedScriptSources.statusLine] {
            #expect(source.components(separatedBy: EmbeddedScripts.socketPlaceholder).count == 2)
            #expect(source.contains(EmbeddedScripts.quotedPlaceholder))
        }
    }

    @Test func installFillsInTheSocketPath() {
        let path = "/Users/me/Library/Application Support/Superpowered Codenotch/Claude/hook.sock"
        for script in [EmbeddedScripts.hook(socketPath: path), EmbeddedScripts.statusLine(socketPath: path)] {
            #expect(!script.contains(EmbeddedScripts.socketPlaceholder))
            #expect(script.contains(#"SOCKET_PATH = (os.environ.get("SPCN_SOCKET") if os.environ.get("SPCN_DEV") == "1" else None) or "\#(path)""#))
        }
    }

    @Test func pathsAreEscapedForPython() {
        #expect(EmbeddedScripts.pythonStringLiteral(#"/a "b"\c"#) == #""/a \"b\"\\c""#)
        #expect(EmbeddedScripts.pythonStringLiteral("x\ny") == #""x\ny""#)
    }
}

/// Both scripts run the way the installed commands run them: `python3 -S`
/// (no `site`), stdlib only, exit 0 and silent when the app isn't there.
nonisolated struct ScriptRuntimeTests {
    private func python(_ arguments: [String], stdin: String) throws -> (status: Int32, stdout: String) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["python3", "-S"] + arguments
        process.environment = ["PATH": "/usr/bin:/bin", "SPCN_DEV": "1", "SPCN_SOCKET": "/tmp/spcn-tests-missing-\(UUID().uuidString.prefix(6)).sock"]
        let input = Pipe()
        let output = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try process.run()
        try? input.fileHandleForWriting.write(contentsOf: Data(stdin.utf8))
        try? input.fileHandleForWriting.close()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return (process.terminationStatus, String(decoding: data, as: UTF8.self))
    }

    @Test(arguments: [ClaudeControlConfiguration.defaultHookScriptName, ClaudeControlConfiguration.defaultStatusLineScriptName])
    func runsWithoutSiteAndWithoutTheApp(name: String) throws {
        let script = ScriptPaths.scripts.appendingPathComponent(name).path
        for input in ["{}", #"{"hook_event_name":"UserPromptSubmit","session_id":"s"}"#, "not json", ""] {
            let result = try python([script], stdin: input)
            #expect(result.status == 0, "\(name) with \(input)")
            #expect(result.stdout.isEmpty)
        }
    }

    /// The hook no longer looks up the terminal (the app reads it from the
    /// kernel), so it spawns nothing; and both scripts talk only to a socket
    /// owned by this user (the /tmp fallback folder is shared).
    @Test(arguments: [ClaudeControlConfiguration.defaultHookScriptName, ClaudeControlConfiguration.defaultStatusLineScriptName])
    func talksOnlyToThisUsersSocketAndSpawnsNothing(name: String) throws {
        let script = ScriptPaths.scripts.appendingPathComponent(name).path
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("spcn-owner-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let plainFile = directory.appendingPathComponent("hook.sock").path
        FileManager.default.createFile(atPath: plainFile, contents: Data())
        let probe = """
        import sys, importlib.util
        spec = importlib.util.spec_from_file_location("script", sys.argv[1])
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        print("subprocess" in sys.modules, hasattr(module, "get_tty"),
              module.is_own_socket(sys.argv[2]), module.is_own_socket(sys.argv[2] + ".missing"))
        """
        let result = try python(["-c", probe, script, plainFile], stdin: "")
        #expect(result.status == 0)
        // A plain file where the socket should be is not a socket to talk to.
        #expect(result.stdout == "False False False False\n")
    }
}
