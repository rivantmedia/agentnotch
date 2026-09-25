import Foundation
import Testing
@testable import ClaudeControl

/// Covers locating the `claude` binary. Getting this wrong is silent: version
/// detection returns nil, only the baseline hook set is registered, and the
/// usage probe can't run. Not main-actor bound: the process tests block, and
/// must neither queue other tests behind them nor be timed while queued.
nonisolated struct ClaudeBinaryLocatorTests {
    /// Stands in for the filesystem so path parsing can be checked without one.
    let anyClaudeBinary: (String) -> Bool = { $0.hasSuffix("/claude") }

    @Test func plainAnswer() {
        let out = "/Users/x/.nvm/versions/node/v22.3.0/bin/claude\n"
        #expect(ClaudeBinaryLocator.parseShellResolvedPath(from: out, isExecutable: anyClaudeBinary)
            == "/Users/x/.nvm/versions/node/v22.3.0/bin/claude")
    }

    @Test func ignoresBanners() {
        let out = """
        Last login: Mon Sep  1 09:14:22 on ttys003
        You have new mail.
        /Users/x/.volta/bin/claude
        """
        #expect(ClaudeBinaryLocator.parseShellResolvedPath(from: out, isExecutable: anyClaudeBinary) == "/Users/x/.volta/bin/claude")
    }

    @Test(arguments: [
        "claude\n",                                  // a shell function or builtin
        "claude: aliased to claude --verbose\n",     // an alias
        "/opt/nope/claude-notes\n",                  // a path, but not our binary
        "",
        "   \n\n  \n",
    ])
    func rejectsNonPaths(output: String) {
        #expect(ClaudeBinaryLocator.parseShellResolvedPath(from: output, isExecutable: anyClaudeBinary) == nil)
    }

    @Test func lastPathWins() {
        #expect(ClaudeBinaryLocator.parseShellResolvedPath(from: "/old/claude\n/new/claude\n", isExecutable: anyClaudeBinary) == "/new/claude")
    }

    @Test(arguments: [
        ("2.1.88", ClaudeCodeVersion(major: 2, minor: 1, patch: 88)),
        ("v2.1.88", ClaudeCodeVersion(major: 2, minor: 1, patch: 88)),
        ("2.1.280 (Claude Code)", ClaudeCodeVersion(major: 2, minor: 1, patch: 280)),
        ("claude 10.0.3 (something)", ClaudeCodeVersion(major: 10, minor: 0, patch: 3)),
    ])
    func parsesVersion(text: String, expected: ClaudeCodeVersion) {
        #expect(ClaudeCodeVersion.parse(text) == expected)
    }

    @Test(arguments: ["not a version", "", "2.1", "vNext"])
    func rejectsNonVersions(text: String) {
        #expect(ClaudeCodeVersion.parse(text) == nil)
    }

    @Test func versionOrdering() {
        #expect(ClaudeCodeVersion(major: 2, minor: 1, patch: 84) > ClaudeCodeVersion(major: 2, minor: 1, patch: 33))
        #expect(ClaudeCodeVersion(major: 2, minor: 1, patch: 100) > ClaudeCodeVersion(major: 2, minor: 1, patch: 99))
        #expect(ClaudeCodeVersion(major: 3, minor: 0, patch: 0) > ClaudeCodeVersion(major: 2, minor: 9, patch: 999))
    }

    @Test func fixedCandidatesIncludeAccountLocalInstalls() {
        let candidates = ClaudeBinaryLocator.fixedCandidates(home: "/Users/x", configDirs: ["/Users/x/.claude-work", "/Users/x/.claude"])
        #expect(candidates.first == "/Users/x/.local/bin/claude")
        #expect(candidates.contains("/opt/homebrew/bin/claude"))
        #expect(candidates.contains("/usr/local/bin/claude"))
        #expect(candidates.contains("/Users/x/.claude-work/local/claude"))
        #expect(candidates.filter { $0 == "/Users/x/.claude/local/claude" }.count == 1)
    }

    @Test func environmentPutsTheBinaryDirectoryOnPath() {
        let env = ClaudeBinaryLocator.environment(
            forBinaryAt: "/Users/x/.nvm/versions/node/v22.3.0/bin/claude",
            base: ["PATH": "/usr/bin:/bin", "HOME": "/Users/x"]
        )
        let path = env["PATH"]?.split(separator: ":").map(String.init) ?? []
        #expect(path.first == "/Users/x/.nvm/versions/node/v22.3.0/bin")
        #expect(path.contains("/opt/homebrew/bin"))
        #expect(path.filter { $0 == "/usr/bin" }.count == 1)
        #expect(env["HOME"] == "/Users/x")
    }

    // MARK: - Driving real (stub) processes

    private func tempDir() throws -> URL {
        URL(fileURLWithPath: TestPaths.temporaryRoot("locator"))
    }

    private func executable(_ body: String, named name: String, in dir: URL) throws -> String {
        let script = dir.appendingPathComponent(name)
        try "#!/bin/sh\n\(body)\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        return script.path
    }

    @Test func resolvesViaShell() throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let binary = try executable("echo 9.9.9", named: "claude", in: dir)
        let shell = try executable("echo 'Last login: whenever'\necho \(binary)", named: "shell", in: dir)
        #expect(ClaudeBinaryLocator.resolveViaLoginShell(shell: shell, timeout: 30) == binary)
    }

    /// A plain login shell is asked first; the interactive one (which runs
    /// .zshrc and whatever it starts) only when that finds nothing.
    @Test func triesANonInteractiveShellFirst() throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let binary = try executable("echo 9.9.9", named: "claude", in: dir)
        let calls = dir.appendingPathComponent("calls").path
        let onlyInteractive = try executable("""
        echo "$1" >> '\(calls)'
        [ "$1" = "-i" ] && echo \(binary)
        exit 0
        """, named: "shell-i", in: dir)
        #expect(ClaudeBinaryLocator.resolveViaLoginShell(shell: onlyInteractive, timeout: 30) == binary)
        #expect(try String(contentsOfFile: calls, encoding: .utf8) == "-l\n-i\n")

        try FileManager.default.removeItem(atPath: calls)
        let plain = try executable("""
        echo "$1" >> '\(calls)'
        echo \(binary)
        """, named: "shell-l", in: dir)
        #expect(ClaudeBinaryLocator.resolveViaLoginShell(shell: plain, timeout: 30) == binary)
        #expect(try String(contentsOfFile: calls, encoding: .utf8) == "-l\n")
    }

    @Test func shellWithoutClaude() throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        #expect(ClaudeBinaryLocator.resolveViaLoginShell(shell: try executable("exit 1", named: "shell", in: dir), timeout: 30) == nil)
    }

    @Test func survivesChattyShell() throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let binary = try executable("echo 9.9.9", named: "claude", in: dir)
        // Enough output to fill the pipe buffer many times over. The timeout
        // is generous: this is about draining the pipe, not about speed.
        let shell = try executable("yes 'noise noise noise noise' | head -200000\necho \(binary)", named: "shell", in: dir)
        #expect(ClaudeBinaryLocator.resolveViaLoginShell(shell: shell, timeout: 60) == binary)
    }

    @Test(.timeLimit(.minutes(1)))
    func abandonsAHangingShell() throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let shell = try executable("sleep 120", named: "shell", in: dir)
        #expect(ClaudeBinaryLocator.resolveViaLoginShell(shell: shell, timeout: 0.3) == nil)
    }

    /// On timeout the shell's whole process group goes, so nothing an rc
    /// file started in the background outlives it.
    @Test(.timeLimit(.minutes(1)))
    func aTimeoutStopsEverythingTheShellStarted() throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let marker = dir.appendingPathComponent("survived").path
        let shell = try executable("(sleep 2; touch '\(marker)') &\nsleep 120", named: "shell", in: dir)
        #expect(ClaudeBinaryLocator.resolveViaLoginShell(shell: shell, timeout: 0.3) == nil)
        Thread.sleep(forTimeInterval: 3)
        #expect(!FileManager.default.fileExists(atPath: marker))
    }

    @Test func rejectsBadShell() {
        #expect(ClaudeBinaryLocator.resolveViaLoginShell(shell: "/nope/not/a/shell") == nil)
    }

    @Test func readsTheVersionOfABinary() throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let binary = try executable("echo '2.1.280 (Claude Code)'", named: "claude", in: dir)
        #expect(ClaudeBinaryLocator.version(ofBinaryAt: binary, timeout: 30) == ClaudeCodeVersion(major: 2, minor: 1, patch: 280))
        let broken = try executable("exit 3", named: "claude-broken", in: dir)
        #expect(ClaudeBinaryLocator.version(ofBinaryAt: broken, timeout: 30) == nil)
    }

    /// Versions of live sessions in an account's registry; dead ones and
    /// `.key` files are ignored.
    @Test func sessionVersionsOfLiveSessionsOnly() throws {
        let dir = try tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let sessions = dir.appendingPathComponent("sessions")
        try FileManager.default.createDirectory(at: sessions, withIntermediateDirectories: true)
        try Data(#"{"pid":101,"version":"2.1.90"}"#.utf8).write(to: sessions.appendingPathComponent("101.json"))
        try Data(#"{"pid":102,"version":"2.0.10"}"#.utf8).write(to: sessions.appendingPathComponent("102.json"))
        try Data("not json".utf8).write(to: sessions.appendingPathComponent("103.json"))
        try Data("secret".utf8).write(to: sessions.appendingPathComponent("101.key"))
        let versions = AccountHookManager.sessionVersions(configDir: dir.path, isAlive: { $0 == 101 || $0 == 103 })
        #expect(versions == [ClaudeCodeVersion(major: 2, minor: 1, patch: 90)])
    }
}

/// The runner every helper process goes through.
nonisolated struct ProcessRunnerTests {
    @Test func returnsStatusAndOutput() {
        let result = ProcessRunner.run(executable: "/bin/sh", arguments: ["-c", "echo hi; exit 7"], timeout: 30)
        #expect(result == ProcessRunner.Result(exitCode: 7, stdout: "hi\n"))
    }

    @Test func passesTheEnvironmentAndNoStdin() {
        let result = ProcessRunner.run(executable: "/bin/sh", arguments: ["-c", "echo \"$X\"; cat"],
                                       environment: ["X": "value", "PATH": "/usr/bin:/bin"], timeout: 30)
        #expect(result?.stdout == "value\n")
    }

    @Test func aSignalledChildReports128PlusTheSignal() {
        let result = ProcessRunner.run(executable: "/bin/sh", arguments: ["-c", "kill -TERM $$"], timeout: 30)
        #expect(result?.exitCode == 128 + SIGTERM)
    }

    @Test func missingExecutable() {
        #expect(ProcessRunner.run(executable: "/nope/nothing", arguments: [], timeout: 1) == nil)
    }

    @Test func keepsTheTailOfHugeOutput() {
        let result = ProcessRunner.run(executable: "/bin/sh", arguments: ["-c", "yes x | head -2000000; echo END"], timeout: 60)
        #expect(result?.stdout.hasSuffix("x\nEND\n") == true)
        #expect((result?.stdout.utf8.count ?? 0) <= ProcessRunner.outputLimit)
    }
}
