import Darwin
import Foundation
import Testing
@testable import ClaudeControl

/// Which config folder a process runs with, from the kernel, and nothing
/// else of its environment.
struct PP_ProcessConfigDirTests {
    /// A `KERN_PROCARGS2` buffer: argc, the executable, padding, arguments,
    /// then the environment.
    private func buffer(arguments: [String], environment: [String]) -> [UInt8] {
        var bytes: [UInt8] = []
        withUnsafeBytes(of: Int32(arguments.count).littleEndian) { bytes += $0 }
        bytes += Array("/usr/local/bin/claude".utf8) + [0, 0, 0, 0]
        for argument in arguments { bytes += Array(argument.utf8) + [0] }
        for entry in environment { bytes += Array(entry.utf8) + [0] }
        bytes += [0, 0]
        return bytes
    }

    private func parse(_ bytes: [UInt8]) -> ProcessConfigDir.Value {
        bytes.withUnsafeBytes { ProcessConfigDir.parse($0) }
    }

    @Test func findsOnlyTheOneVariable() {
        let dir = "/Users/me/.claude-windows/801f9dd51396"
        #expect(parse(buffer(arguments: ["claude", "--resume", "CLAUDE_CONFIG_DIR=/not/this"],
                             environment: ["CLAUDE_CODE_MESSAGING_TOKEN=secret", "CLAUDE_CONFIG_DIRX=/nope",
                                           "CLAUDE_CONFIG_DIR=\(dir)", "HOME=/Users/me"])) == .set(dir))
        #expect(parse(buffer(arguments: ["claude"], environment: ["PATH=/usr/bin"])) == .unset)
        #expect(parse(buffer(arguments: ["claude"], environment: ["CLAUDE_CONFIG_DIR="])) == .unset)
        // A value that merely mentions the name inside an argument doesn't count.
        #expect(parse(buffer(arguments: ["sh", "-c", "CLAUDE_CONFIG_DIR=/x claude"], environment: ["HOME=/h"])) == .unset)
        // No environment at all (macOS hides it for platform binaries): can't tell.
        #expect(parse(buffer(arguments: ["sleep", "30"], environment: [])) == .unreadable)
        #expect(parse([1, 0]) == .unreadable)
        // More arguments claimed than present.
        var broken = buffer(arguments: ["a"], environment: [])
        broken.replaceSubrange(0..<4, with: withUnsafeBytes(of: Int32(9).littleEndian) { Array($0) })
        #expect(parse(broken) == .unreadable)
    }

    /// A real child process: its folder, never its other variables. (A copy
    /// of `sleep`: macOS hides a platform binary's environment.)
    @Test func readsAChildProcessOfTheSameUser() async throws {
        let dir = "/tmp/agentnotch-pp-\(UUID().uuidString.prefix(6))/.claude-windows/1bf3e8f92b11"
        let bin = TestPaths.temporaryRoot("procargs")
        defer { try? FileManager.default.removeItem(atPath: bin) }
        try FileManager.default.copyItem(atPath: "/bin/sleep", toPath: bin + "/claude")
        let child = try TestChild(executable: bin + "/claude", environment: [
            "CLAUDE_CONFIG_DIR": dir, "CLAUDE_CODE_MESSAGING_TOKEN": "do-not-read", "PATH": "/usr/bin:/bin"])
        defer { child.terminate() }
        let plain = try TestChild(executable: bin + "/claude")
        defer { plain.terminate() }
        // A fresh copy's first launch can take a moment to get going.
        for _ in 0..<100 where ProcessConfigDir.read(pid: child.pid) != .set(dir)
            || ProcessConfigDir.read(pid: plain.pid) != .unset {
            try await Task.sleep(for: .milliseconds(50))
        }

        #expect(ProcessConfigDir.read(pid: child.pid) == .set(dir))
        #expect(ProcessConfigDir.read(pid: plain.pid) == .unset)
        // launchd is root's: not read.
        #expect(ProcessConfigDir.read(pid: 1) == .unreadable)
        #expect(ProcessConfigDir.read(pid: -4) == .unreadable)

        // Cached per process start: read once.
        nonisolated final class Counter: @unchecked Sendable { var reads = 0 }
        let counter = Counter()
        let cache = ProcessConfigDirCache(reader: { pid in counter.reads += 1; return ProcessConfigDir.read(pid: pid) })
        #expect(cache.value(pid: child.pid) == .set(dir))
        #expect(cache.value(pid: child.pid) == .set(dir))
        #expect(counter.reads == 1)
    }
}

/// A sessions folder shared by several config folders is read once, and
/// every entry goes to the folder its process runs with.
@MainActor
@Suite(.serialized)
struct PP_RegistryScanTests {
    private func entry(_ pid: Int, _ session: String) -> SessionRegistryEntry {
        SessionRegistryEntry(pid: pid, sessionId: session, status: "idle")
    }

    @Test func attributionFollowsEachProcess() {
        let home = "/Users/me"
        let aliases: Set<String> = ["/Users/me/.claude", "/Users/me/.claude-windows/801f9dd51396", "/Users/me/.claude-windows/1bf3e8f92b11"]
        let environments: [Int32: ProcessConfigDir.Value] = [
            11: .set("/Users/me/.claude-windows/801f9dd51396/"),
            12: .unset,
            13: .set("/Users/me/.claude-windows/1bf3e8f92b11"),
            14: .unreadable,
            15: .set("/Users/me/.claude-windows/0a1b2c3d4e5f"),   // a window not known yet
        ]
        let result = SessionRegistryScanner.attribute(
            [11, 12, 13, 14, 15].map { entry($0, "s\($0)") }, aliases: aliases, isShared: true,
            defaultDir: home + "/.claude", configDirOfProcess: { environments[$0] ?? .unreadable })
        #expect(result["/Users/me/.claude-windows/801f9dd51396"]?.map(\.pid) == [11])
        #expect(result["/Users/me/.claude"]?.map(\.pid) == [12, 14])
        #expect(result["/Users/me/.claude-windows/1bf3e8f92b11"]?.map(\.pid) == [13])
        #expect(result["/Users/me/.claude-windows/0a1b2c3d4e5f"]?.map(\.pid) == [15])

        // A folder only one config folder reaches directly: no process is asked.
        let single = SessionRegistryScanner.attribute([entry(11, "a")], aliases: ["/Users/me/.claude-work"], isShared: false,
                                                      defaultDir: home + "/.claude", configDirOfProcess: { _ in
                                                          Issue.record("asked")
                                                          return .unset
                                                      })
        #expect(single["/Users/me/.claude-work"]?.count == 1)
    }

    /// Two live processes in the shared folder, four config folders leading
    /// there: one read, each session under its own folder, and the folder
    /// whose last session ended gets its empty snapshot.
    @Test func theSharedFolderIsScannedOnceAndSplit() async throws {
        let fake = ParallelProfilesHome("scan")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let children = try (0..<2).map { _ in try TestChild(executable: "/bin/sleep") }
        defer { children.forEach { $0.terminate() } }
        let (paras, biios) = (children[0].pid, children[1].pid)
        for (pid, session) in [(paras, "s-paras"), (biios, "s-biios")] {
            try fake.writeJSON(".claude-shared/sessions/\(pid).json",
                               ["pid": Int(pid), "sessionId": session, "kind": "interactive", "entrypoint": "claude-vscode",
                                "status": "idle", "updatedAt": Date().timeIntervalSince1970 * 1000])
        }
        let windowParas = fake.path(".claude-windows/801f9dd51396")
        let windowBiios = fake.path(".claude-windows/1bf3e8f92b11")
        nonisolated final class Reads: @unchecked Sendable {
            let lock = NSLock()
            var asked: [Int32] = []
            var snapshots: [String: [String]] = [:]
        }
        let reads = Reads()
        let scanner = SessionRegistryScanner(defaultDir: fake.path(".claude"), configDirOfProcess: { pid in
            reads.lock.withLock { reads.asked.append(pid) }
            return pid == paras ? .set(windowParas) : pid == biios ? .set(windowBiios) : .unreadable
        })
        let dirs = Set([".claude", ".claude-windows/801f9dd51396", ".claude-windows/b9fbb9ecd7cb", ".claude-windows/1bf3e8f92b11"].map(fake.path))
        #expect(SessionRegistryScanner.groupedBySessionsFolder(dirs).count == 1)
        scanner.start(onSnapshot: { dir, entries in
            reads.lock.withLock { reads.snapshots[dir] = entries.map(\.sessionId) }
        })
        scanner.setConfigDirs(dirs)
        try await Task.sleep(for: .milliseconds(400))
        var snapshots = reads.lock.withLock { reads.snapshots }
        #expect(snapshots[windowParas] == ["s-paras"])
        #expect(snapshots[windowBiios] == ["s-biios"])
        #expect(snapshots[fake.path(".claude")] == nil)
        // Each process was asked once per read of the shared folder, not once
        // per config folder leading there (4 a read). How many reads there
        // were depends on timing (the first pass, the new folders' pass, a
        // timer's or a rescan's on a slow machine), so count them.
        let shared = SessionRegistryScanner.sessionsFolder(of: windowParas)
        let (folderReads, asked) = scanner.withFolderReads { folders in
            (folders[shared] ?? 0, reads.lock.withLock { reads.asked })
        }
        #expect(folderReads >= 1)
        #expect(asked.filter { $0 == paras }.count == folderReads)
        #expect(asked.filter { $0 == biios }.count == folderReads)

        // Biios's session ends: its window gets `[]`.
        try FileManager.default.removeItem(atPath: fake.path(".claude-shared/sessions/\(biios).json"))
        scanner.scanSoon(configDir: windowBiios)
        try await Task.sleep(for: .milliseconds(1_600))
        snapshots = reads.lock.withLock { reads.snapshots }
        #expect(snapshots[windowBiios] == [])
        #expect(snapshots[windowParas] == ["s-paras"])
        scanner.stop()
    }
}

/// One transcript named through several config folders is one file.
@MainActor
@Suite(.serialized)
struct PP_TranscriptTests {
    @Test func theSharedHistoryIsSearchedOnceAndSpeltThroughTheSessionsFolder() throws {
        let fake = ParallelProfilesHome("transcript")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let cwd = "/Users/me/@paraswtf/app"
        let slug = TranscriptLocator.projectSlug(for: cwd)
        try fake.write(".claude-shared/projects/\(slug)/s-1.jsonl", "{}\n")
        let window = fake.path(".claude-windows/801f9dd51396")
        let found = TranscriptLocator.transcriptPath(sessionId: "s-1", cwd: cwd,
                                                     configDirs: [window, fake.path(".claude")])
        #expect(found == window + "/projects/\(slug)/s-1.jsonl")
        // Spelt through the window, its folder reads back as the window, never the shared history.
        #expect(found.flatMap(AccountPaths.configDir(fromTranscriptPath:)) == window)
        #expect(TranscriptLocator.isSameFile(found ?? "", fake.path(".claude/projects/\(slug)/s-1.jsonl")))
        #expect(TranscriptLocator.isSameFile(found ?? "", fake.path(".claude-shared/projects/\(slug)/s-1.jsonl")))
        #expect(!TranscriptLocator.isSameFile(found ?? "", fake.path(".claude-shared/projects/\(slug)/s-2.jsonl")))
        #expect(TranscriptLocator.transcriptPath(sessionId: "s-9", cwd: cwd, configDirs: [window, fake.path(".claude")]) == nil)
    }

    /// A transcript path through the shared history's own folder names no
    /// account: the environment decides.
    @Test func aResolvedSharedPathFallsBackToTheEnvironment() {
        let shared = ParallelProfiles.sharedStore(home: AccountPaths.homeDirectory)
        let dir = SessionFilter.configDir(transcriptPath: shared + "/projects/-x/s.jsonl",
                                          configDirEnv: "/Users/me/.claude-windows/1bf3e8f92b11")
        #expect(dir == "/Users/me/.claude-windows/1bf3e8f92b11")
        #expect(SessionFilter.configDir(transcriptPath: shared + "/projects/-x/s.jsonl", configDirEnv: nil)
                == AccountPaths.defaultConfigDir)
    }
}
