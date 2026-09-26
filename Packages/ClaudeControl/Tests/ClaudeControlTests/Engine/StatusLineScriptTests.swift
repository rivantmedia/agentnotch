import Darwin
import Foundation
import Testing
@testable import ClaudeControl

/// A one-shot Unix socket server: accepts one connection and reads it to EOF.
nonisolated private final class OneShotSocketServer: @unchecked Sendable {
    let path: String
    private let fd: Int32
    private let done = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var received = Data()

    init(path: String) throws {
        self.path = path
        unlink(path)
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            for (index, byte) in bytes.enumerated() where index < buffer.count - 1 {
                buffer[index] = byte
            }
        }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0, listen(fd, 1) == 0 else {
            close(fd)
            throw POSIXError(.EADDRINUSE)
        }
        // A thread of its own, not a GCD worker: this suite's tests block
        // on child processes, and under a busy parallel run (CI) the global
        // pool can be starved long enough for the message to miss its wait.
        let thread = Thread { [self] in
            let client = accept(fd, nil, nil)
            if client >= 0 {
                var buffer = [UInt8](repeating: 0, count: 4096)
                while true {
                    let count = read(client, &buffer, buffer.count)
                    if count <= 0 { break }
                    lock.lock()
                    received.append(contentsOf: buffer[0..<count])
                    lock.unlock()
                }
                close(client)
            }
            done.signal()
        }
        thread.start()
    }

    /// What the one client sent, or nil if nobody connected in time.
    func message(timeout: TimeInterval) -> Data? {
        guard done.wait(timeout: .now() + timeout) == .success else { return nil }
        lock.lock()
        defer { lock.unlock() }
        return received
    }

    deinit {
        close(fd)
        unlink(path)
    }
}

/// The status line wrapper, run as Claude Code runs it. Not main-actor
/// bound: these block on child processes, and must not queue unrelated
/// tests behind them (or be queued behind them).
@Suite(.serialized)
nonisolated struct StatusLineScriptTests {
    static let source = ScriptPaths.scripts.appendingPathComponent(AppIdentity.statusLineScriptName)

    static let statusLineJSON = """
    {"session_id":"sess-1","transcript_path":"/Users/u/.claude-work/projects/-Users-u-proj/sess-1.jsonl","cwd":"/Users/u/proj",
     "session_name":"my-session","model":{"id":"claude-opus-5-5","display_name":"Opus"},
     "workspace":{"current_dir":"/Users/u/proj","project_dir":"/Users/u/proj"},"version":"2.1.280",
     "cost":{"total_cost_usd":0.0123,"total_duration_ms":45000},
     "context_window":{"total_input_tokens":15500,"total_output_tokens":1200,"context_window_size":200000,"used_percentage":8,"remaining_percentage":92},
     "rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},"seven_day":{"used_percentage":41.2,"resets_at":1738857600}}}
    """

    let dir: URL

    init() throws {
        dir = URL(fileURLWithPath: TestPaths.temporaryRoot("statusline"))
        try FileManager.default.copyItem(at: Self.source, to: dir.appendingPathComponent(AppIdentity.statusLineScriptName))
    }

    private var script: URL { dir.appendingPathComponent(AppIdentity.statusLineScriptName) }

    private func setPrevious(_ command: String) throws {
        let data = Data(OrderedJSON.object(["type": .string("command"), "command": .string(command)]).serialized().utf8)
        try data.write(to: dir.appendingPathComponent(HookInstaller.previousStatusLineFileName))
    }

    /// The copy's cap on the previous command, so a test needn't wait 30 s.
    private func setPreviousTimeout(_ seconds: Int) throws {
        var text = try String(contentsOf: script, encoding: .utf8)
        text = text.replacingOccurrences(of: "PREVIOUS_TIMEOUT_SECONDS = 30", with: "PREVIOUS_TIMEOUT_SECONDS = \(seconds)")
        try text.write(to: script, atomically: true, encoding: .utf8)
    }

    private func launch(socket: String) throws -> (Process, Pipe, Pipe) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["python3", "-S", script.path]
        process.environment = [
            "PATH": "/usr/bin:/bin",
            "AGENTNOTCH_SOCKET": socket,
            "AGENTNOTCH_DEV": "1",
            "CLAUDE_CONFIG_DIR": "/Users/u/.claude-work",
        ]
        let input = Pipe()
        let output = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try process.run()
        return (process, input, output)
    }

    private func run(stdin: String, socket: String) throws -> (stdout: String, status: Int32) {
        let (process, input, output) = try launch(socket: socket)
        try input.fileHandleForWriting.write(contentsOf: Data(stdin.utf8))
        try input.fileHandleForWriting.close()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return (String(decoding: data, as: UTF8.self), process.terminationStatus)
    }

    @Test func forwardsToTheAppAndChainsThePreviousCommand() throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        let server = try OneShotSocketServer(path: dir.path + "/app.sock")
        try setPrevious("cat; echo TAIL; exit 3")

        let result = try run(stdin: Self.statusLineJSON, socket: server.path)

        // The previous command saw the same stdin; its output and status pass through.
        #expect(result.stdout == Self.statusLineJSON + "TAIL\n")
        #expect(result.status == 3)

        let data = try #require(server.message(timeout: 5))
        let message = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(message["event"] as? String == "StatusLine")
        #expect(message["session_id"] as? String == "sess-1")
        #expect(message["transcript_path"] as? String == "/Users/u/.claude-work/projects/-Users-u-proj/sess-1.jsonl")
        #expect(message["cwd"] as? String == "/Users/u/proj")
        #expect(message["config_dir_env"] as? String == "/Users/u/.claude-work")

        let statusLine = try #require(message["status_line"] as? [String: Any])
        #expect(Set(statusLine.keys) == ["rate_limits", "context_window", "model", "cost", "session_name", "version"])
        let contextWindow = statusLine["context_window"] as? [String: Any]
        #expect(contextWindow?["used_percentage"] as? Int == 8)
        #expect(contextWindow?["context_window_size"] as? Int == 200000)
        #expect((statusLine["cost"] as? [String: Any])?["total_cost_usd"] as? Double == 0.0123)
        #expect(statusLine["session_name"] as? String == "my-session")
        #expect(statusLine["version"] as? String == "2.1.280")

        // rate_limits reach the app exactly as Claude Code sent them.
        let limits = try #require(statusLine["rate_limits"] as? [String: Any])
        #expect((limits["five_hour"] as? [String: Any])?["used_percentage"] as? Double == 23.5)
        #expect((limits["seven_day"] as? [String: Any])?["resets_at"] as? Int == 1_738_857_600)
    }

    @Test func withoutAPreviousCommandPrintsNothing() throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        let server = try OneShotSocketServer(path: dir.path + "/app.sock")
        let result = try run(stdin: Self.statusLineJSON, socket: server.path)
        #expect(result.stdout.isEmpty)
        #expect(result.status == 0)
        #expect(server.message(timeout: 5)?.isEmpty == false)
    }

    @Test func appNotRunningStillChains() throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        try setPrevious("echo from-previous")
        let result = try run(stdin: Self.statusLineJSON, socket: dir.path + "/missing.sock")
        #expect(result.stdout == "from-previous\n")
        #expect(result.status == 0)
    }

    @Test func garbageInputIsNotForwardedButStillChained() throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        let server = try OneShotSocketServer(path: dir.path + "/app.sock")
        try setPrevious("wc -c | tr -d ' '")
        let result = try run(stdin: "not json", socket: server.path)
        #expect(result.stdout == "8\n")
        #expect(server.message(timeout: 1) == nil)
    }

    /// Nor to ours under the former name, or Superpowered Vibe Notch's
    /// wrapper, which may chain back to ours.
    @Test(arguments: ["agentnotch-statusline.py", "superpowered-codenotch-statusline.py", "superpowered-notch-statusline.py"])
    func neverChainsToAWrapper(name: String) throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        try setPrevious("python3 '/x/hooks/\(name)'")
        let result = try run(stdin: Self.statusLineJSON, socket: dir.path + "/missing.sock")
        #expect(result.stdout.isEmpty)
        #expect(result.status == 0)
    }

    /// A slow status line (a first-run `npx`) prints normally: the cap is
    /// for commands that hang, not for slow ones.
    @Test(.timeLimit(.minutes(1)))
    func aSlowPreviousCommandStillPrints() throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        try setPreviousTimeout(10)
        try setPrevious("sleep 1; echo slow")
        #expect(try run(stdin: Self.statusLineJSON, socket: dir.path + "/missing.sock").stdout == "slow\n")
    }

    /// Past the cap the command's whole group is stopped and nothing is
    /// printed. (The test's copy uses a 1 s cap; the shipped one is 30 s.)
    @Test(.timeLimit(.minutes(1)))
    func aHangingPreviousCommandIsCutOff() throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        try setPreviousTimeout(1)
        let marker = dir.appendingPathComponent("finished")
        try setPrevious("sleep 20; touch '\(marker.path)'")
        let result = try run(stdin: Self.statusLineJSON, socket: dir.path + "/missing.sock")
        #expect(result.stdout.isEmpty)
        #expect(result.status == 0)
        #expect(!FileManager.default.fileExists(atPath: marker.path))
    }

    /// Claude Code cancelling the run (SIGTERM to the wrapper) stops the
    /// previous command too, as it would have stopped it unwrapped.
    @Test(.timeLimit(.minutes(1)))
    func cancellingStopsThePreviousCommand() throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        let started = dir.appendingPathComponent("started")
        let finished = dir.appendingPathComponent("finished")
        try setPrevious("touch '\(started.path)'; sleep 3; touch '\(finished.path)'")
        let (process, input, _) = try launch(socket: dir.path + "/missing.sock")
        try input.fileHandleForWriting.write(contentsOf: Data(Self.statusLineJSON.utf8))
        try input.fileHandleForWriting.close()

        // Wait until the previous command is running, then cancel.
        let deadline = Date().addingTimeInterval(20)
        while !FileManager.default.fileExists(atPath: started.path), Date() < deadline {
            Thread.sleep(forTimeInterval: 0.02)
        }
        #expect(FileManager.default.fileExists(atPath: started.path))
        process.terminate()
        process.waitUntilExit()
        #expect(process.terminationStatus == 128 + SIGTERM)

        // Well past when it would have finished: it never did.
        Thread.sleep(forTimeInterval: 4)
        #expect(!FileManager.default.fileExists(atPath: finished.path))
    }

    /// A cancel that lands while Popen is still returning (the command's
    /// shell already running, its pid not yet known to the handler) stops
    /// the command too. Under load that window is real; here the SIGTERM is
    /// sent from inside Popen so the test hits it every time.
    @Test(.timeLimit(.minutes(1)))
    func aCancelWhileTheCommandStartsStillStopsIt() throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        let started = dir.appendingPathComponent("started")
        let finished = dir.appendingPathComponent("finished")
        try setPrevious("touch '\(started.path)'; sleep 3; touch '\(finished.path)'")
        let driver = """
        import importlib.util, os, signal, subprocess, sys, time
        spec = importlib.util.spec_from_file_location("wrapper", sys.argv[1])
        wrapper = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(wrapper)
        real_popen = subprocess.Popen
        def popen(*args, **kwargs):
            process = real_popen(*args, **kwargs)
            deadline = time.time() + 20
            while not os.path.exists(sys.argv[2]) and time.time() < deadline:
                time.sleep(0.01)
            os.kill(os.getpid(), signal.SIGTERM)
            return process
        subprocess.Popen = popen
        wrapper.start_previous()
        sys.exit(0)
        """
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["python3", "-S", "-c", driver, script.path, started.path]
        process.environment = ["PATH": "/usr/bin:/bin"]
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()

        #expect(FileManager.default.fileExists(atPath: started.path))
        #expect(process.terminationStatus == 128 + SIGTERM)
        Thread.sleep(forTimeInterval: 4)
        #expect(!FileManager.default.fileExists(atPath: finished.path))
    }

    /// The whole chain: install into a config dir that already has a status
    /// line, then run `statusLine.command` from the written settings.json the
    /// way Claude Code does (through the shell, JSON on stdin).
    @Test(.enabled(if: !DevFlags.installsDisabled))
    func installedStatusLineRunsEndToEnd() throws {
        defer { try? FileManager.default.removeItem(at: dir) }
        let configDir = dir.appendingPathComponent(".claude-test").path
        try FileManager.default.createDirectory(atPath: configDir + "/projects", withIntermediateDirectories: true)
        let settings = #"{"statusLine":{"type":"command","command":"printf 'previous:'; wc -c | tr -d ' '","padding":1}}"#
        try Data(settings.utf8).write(to: URL(fileURLWithPath: configDir + "/settings.json"))

        let outcome = HookInstaller.install(configDir: configDir, configuration: HookInstaller.Configuration(
            python: HookInstaller.detectPython(),
            version: nil,
            statusLineIntegration: true,
            hookScript: EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket),
            statusLineScript: EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket)
        ))
        #expect(outcome == .installed)

        let written = try OrderedJSON.parse(Data(contentsOf: URL(fileURLWithPath: configDir + "/settings.json")))
        let statusLine = try #require(written["statusLine"])
        let command = try #require(statusLine["command"]?.stringValue)
        #expect(OrderedJSON.equivalent(statusLine["padding"], .int(1)))

        let server = try OneShotSocketServer(path: dir.path + "/app.sock")
        let result = try TestShell.run(command, stdin: Self.statusLineJSON,
                                       environment: ["PATH": "/usr/bin:/bin", "AGENTNOTCH_DEV": "1", "AGENTNOTCH_SOCKET": server.path])
        #expect(result.stdout == "previous:\(Self.statusLineJSON.utf8.count)\n")
        #expect(result.status == 0)
        let message = try #require(server.message(timeout: 5))
        let object = try JSONSerialization.jsonObject(with: message) as? [String: Any]
        #expect(object?["event"] as? String == "StatusLine")
        #expect(object?["session_id"] as? String == "sess-1")
    }
}
