import Foundation
import Testing
@testable import ClaudeControl

/// Per-session token totals from transcripts, counted once.
struct SessionTokenScannerTests {
    typealias L = CloudTranscriptLines
    let a = CloudFixture.sessionA
    let b = CloudFixture.sessionB

    /// A temporary `projects/<slug>` folder.
    final class Projects {
        let root: String
        let projects: String
        let slug: String

        init(_ label: String) throws {
            root = TestPaths.temporaryRoot(label)
            projects = (root as NSString).appendingPathComponent(".claude/projects")
            slug = (projects as NSString).appendingPathComponent("-Users-me-code-app")
            try FileManager.default.createDirectory(atPath: slug, withIntermediateDirectories: true)
        }

        func path(_ name: String) -> String { (slug as NSString).appendingPathComponent(name) }
        func transcript(_ id: String) -> String { path("\(id).jsonl") }

        deinit { try? FileManager.default.removeItem(atPath: root) }
    }

    @Test func countsEachResponseOnceAndSkipsSynthetic() throws {
        let dir = try Projects("scan-basic")
        try L.write([
            L.user("please fix the bug", session: a, at: CloudFixture.stamp(0), entrypoint: "claude-vscode"),
            L.aiTitle("Fix the parser bug", session: a),
            // One response written as two content-block lines: counted once, the latest usage.
            L.assistant(id: "msg_1", request: "req_1", session: a, input: 10, output: 1, cacheCreation: 100, cacheRead: 1000,
                        at: CloudFixture.stamp(5)),
            L.assistant(id: "msg_1", request: "req_1", session: a, input: 10, output: 40, cacheCreation: 100, cacheRead: 1000,
                        at: CloudFixture.stamp(6), toolUse: true),
            L.assistant(id: "msg_2", request: "req_2", session: a, model: "claude-haiku-4-5", input: 5, output: 7,
                        at: CloudFixture.stamp(20)),
            L.assistant(id: "msg_3", request: "req_3", session: a, input: 1, output: 2, at: CloudFixture.stamp(30)),
            L.assistant(id: "msg_4", request: "req_4", session: a, model: "<synthetic>", input: 999, output: 999,
                        at: CloudFixture.stamp(40)),
        ], to: dir.transcript(a))
        let scanner = SessionTokenScanner(fileURL: nil, persists: false)
        let summary = try #require(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a)))
        #expect(summary.tokens == CloudTokenTotals(input: 16, output: 49, cacheCreation: 100, cacheRead: 1000))
        #expect(summary.messageCount == 3)
        #expect(summary.models == ["claude-opus-4-5", "claude-haiku-4-5"])
        #expect(summary.firstTimestamp == CloudFixture.base)
        #expect(summary.lastTimestamp == CloudFixture.base.addingTimeInterval(40))
        #expect(summary.cwd == "/Users/me/code/app" && summary.entrypoint == "claude-vscode")
        #expect(summary.title == "Fix the parser bug")
        #expect(scanner.cachedSummary(of: a) == summary)
    }

    @Test func pricesEachResponseOnceAtItsModelsPrices() throws {
        let dir = try Projects("scan-cost")
        try L.write([
            L.user("go", session: a, at: CloudFixture.stamp(0)),
            // Written twice as it streamed: priced once, at its last usage.
            L.assistant(id: "msg_1", request: "req_1", session: a, input: 10, output: 1, cacheCreation: 100, cacheRead: 1000,
                        at: CloudFixture.stamp(1)),
            L.assistant(id: "msg_1", request: "req_1", session: a, input: 10, output: 40, cacheCreation: 100, cacheRead: 1000,
                        at: CloudFixture.stamp(2), toolUse: true),
            L.assistant(id: "msg_2", request: "req_2", session: a, model: "claude-haiku-4-5", input: 5, output: 7,
                        at: CloudFixture.stamp(3)),
            L.assistant(id: "msg_3", request: "req_3", session: a, model: "<synthetic>", input: 999, output: 999,
                        at: CloudFixture.stamp(4)),
        ], to: dir.transcript(a))
        let file = URL(fileURLWithPath: dir.root).appendingPathComponent("scan-state.json")
        let scanner = SessionTokenScanner(fileURL: file, persists: true)
        let summary = try #require(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a)))
        // Per million: Opus 4.5 10×5 + 40×25 + 100×6.25 + 1000×0.5 = 2175, Haiku 4.5 5×1 + 7×5 = 40.
        #expect(summary.cost == 2_215_000)
        #expect(summary.part(for: "").cost == 2_215_000 && summary.part(for: "").estimatedCostUsd == 0.002215)
        // Kept with the watermarks.
        scanner.saveNow()
        #expect(SessionTokenScanner(fileURL: file, persists: true).cachedSummary(of: a)?.cost == 2_215_000)

        // A model with no known price: the cost is unknown, not a guess.
        try L.write([L.assistant(id: "msg_4", request: "req_4", session: a, model: "claude-opus-9", input: 1, output: 1,
                                 at: CloudFixture.stamp(5))], to: dir.transcript(a), append: true)
        let unknown = try #require(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a)))
        #expect(unknown.messageCount == 3 && unknown.cost == nil && unknown.part(for: "").estimatedCostUsd == nil)
    }

    @Test func subagentsCountOnceWhereverTheirLinesAre() throws {
        let dir = try Projects("scan-agents")
        try L.write([
            L.user("go", session: a, at: CloudFixture.stamp(0)),
            L.assistant(id: "msg_main", request: "req_main", session: a, input: 100, output: 10, at: CloudFixture.stamp(1)),
            // An older transcript repeats a subagent's response in the parent.
            L.assistant(id: "msg_sub", request: "req_sub", session: a, input: 50, output: 5, at: CloudFixture.stamp(2), sidechain: true),
        ], to: dir.transcript(a))
        // Current layout: <session>/subagents/…, workflows one level deeper.
        try L.write([
            L.assistant(id: "msg_sub", request: "req_sub", session: a, input: 50, output: 5, at: CloudFixture.stamp(2), sidechain: true),
            L.assistant(id: "msg_sub2", request: "req_sub2", session: a, model: "claude-haiku-4-5", input: 20, output: 2,
                        at: CloudFixture.stamp(3), sidechain: true),
        ], to: dir.path("\(a)/subagents/agent-one.jsonl"))
        try L.write([
            L.assistant(id: "msg_wf", request: "req_wf", session: a, input: 7, output: 1, at: CloudFixture.stamp(4), sidechain: true),
        ], to: dir.path("\(a)/subagents/workflows/wf1/agent-two.jsonl"))
        // Legacy flat agent files: this session's, and another session's.
        try L.write([
            L.assistant(id: "msg_flat", request: "req_flat", session: a, input: 3, output: 3, at: CloudFixture.stamp(5), sidechain: true),
        ], to: dir.path("agent-flat.jsonl"))
        try L.write([
            L.assistant(id: "msg_other", request: "req_other", session: b, input: 1000, output: 1000, at: CloudFixture.stamp(6), sidechain: true),
        ], to: dir.path("agent-other.jsonl"))

        let scanner = SessionTokenScanner(fileURL: nil, persists: false)
        let summary = try #require(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a)))
        #expect(summary.tokens.input == 100 + 50 + 20 + 7 + 3)
        #expect(summary.tokens.output == 10 + 5 + 2 + 1 + 3)
        #expect(summary.messageCount == 5)
        #expect(summary.models.first == "claude-opus-4-5" && summary.models.contains("claude-haiku-4-5"))
        // Scanning again changes nothing.
        #expect(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a)) == summary)
    }

    @Test func aSharedHistoryReachedTwoWaysCountsOnce() throws {
        let root = TestPaths.temporaryRoot("scan-shared")
        defer { try? FileManager.default.removeItem(atPath: root) }
        let fm = FileManager.default
        let shared = (root as NSString).appendingPathComponent(".claude-shared/projects")
        let slug = (shared as NSString).appendingPathComponent("-Users-me-code-app")
        try fm.createDirectory(atPath: slug, withIntermediateDirectories: true)
        for name in [".claude", ".claude-windows/0123456789ab"] {
            let folder = (root as NSString).appendingPathComponent(name)
            try fm.createDirectory(atPath: folder, withIntermediateDirectories: true)
            try fm.createSymbolicLink(atPath: (folder as NSString).appendingPathComponent("projects"), withDestinationPath: shared)
        }
        try L.write([
            L.user("go", session: a, at: CloudFixture.stamp(0)),
            L.assistant(id: "msg_1", request: "req_1", session: a, input: 10, output: 10, at: CloudFixture.stamp(1)),
        ], to: (slug as NSString).appendingPathComponent("\(a).jsonl"))

        let scanner = SessionTokenScanner(fileURL: nil, persists: false)
        let viaDefault = (root as NSString).appendingPathComponent(".claude/projects/-Users-me-code-app/\(a).jsonl")
        let viaWindow = (root as NSString).appendingPathComponent(".claude-windows/0123456789ab/projects/-Users-me-code-app/\(a).jsonl")
        let first = try #require(scanner.scan(sessionId: a, transcriptPath: viaDefault))
        let second = try #require(scanner.scan(sessionId: a, transcriptPath: viaWindow))
        #expect(first.messageCount == 1 && second == first)
        #expect(first.tokens.input == 10)
    }

    @Test func aResumedSessionsCopiedLinesStayWithTheOriginal() throws {
        let dir = try Projects("scan-resume")
        try L.write([
            L.user("start", session: a, at: CloudFixture.stamp(0)),
            L.assistant(id: "msg_1", request: "req_1", session: a, input: 10, output: 10, at: CloudFixture.stamp(1)),
            L.assistant(id: "msg_2", request: "req_2", session: a, input: 20, output: 20, at: CloudFixture.stamp(2)),
        ], to: dir.transcript(a))
        // B resumed A: A's lines copied verbatim (naming A), one copied as B,
        // then B's own work.
        try L.write([
            L.user("start", session: a, at: CloudFixture.stamp(0)),
            L.assistant(id: "msg_1", request: "req_1", session: a, input: 10, output: 10, at: CloudFixture.stamp(1)),
            L.assistant(id: "msg_2", request: "req_2", session: b, input: 20, output: 20, at: CloudFixture.stamp(2)),
            L.user("continue", session: b, at: CloudFixture.stamp(3600)),
            L.assistant(id: "msg_3", request: "req_3", session: b, input: 5, output: 5, at: CloudFixture.stamp(3601)),
        ], to: dir.transcript(b))

        let scanner = SessionTokenScanner(fileURL: nil, persists: false)
        // The newer one first: the copies still don't count for it.
        let resumed = try #require(scanner.scan(sessionId: b, transcriptPath: dir.transcript(b)))
        let original = try #require(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a)))
        #expect(original.tokens.input + resumed.tokens.input == 35)
        #expect(original.messageCount + resumed.messageCount == 3)
        #expect(original.messageCount >= 1 && resumed.messageCount >= 1)
        // Dated by its own lines, not the copies.
        #expect(resumed.firstTimestamp == CloudFixture.base.addingTimeInterval(2) || resumed.firstTimestamp == CloudFixture.base.addingTimeInterval(3600))
        #expect(original.tokens.input >= 10)
    }

    /// Regression (review finding 24): lines `/branch` copied into a fork
    /// name the fork but carry `forkedFrom`: they stay with the original
    /// whichever is scanned first, and the fork is dated by its own lines.
    @Test func aForksCopiedLinesStayWithTheOriginalWhateverTheOrder() throws {
        let dir = try Projects("scan-fork")
        let original = [
            L.user("start", session: b, at: CloudFixture.stamp(0)),
            L.assistant(id: "msg_1", request: "req_1", session: b, input: 10, output: 10, at: CloudFixture.stamp(1)),
            L.assistant(id: "msg_2", request: "req_2", session: b, input: 20, output: 20, at: CloudFixture.stamp(2)),
        ]
        try L.write(original, to: dir.transcript(b))
        // `a` sorts before `b`: the fork is the first file a listing gives.
        try L.write(original.map { L.forked($0, into: a, from: b) } + [
            L.user("try another way", session: a, at: CloudFixture.stamp(600)),
            L.assistant(id: "msg_3", request: "req_3", session: a, input: 5, output: 5, at: CloudFixture.stamp(601)),
        ], to: dir.transcript(a))

        let scanner = SessionTokenScanner(fileURL: nil, persists: false)
        let fork = try #require(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a)))
        let parent = try #require(scanner.scan(sessionId: b, transcriptPath: dir.transcript(b)))
        #expect(parent.messageCount == 2 && parent.tokens.input == 30)
        #expect(fork.messageCount == 1 && fork.tokens.input == 5)
        #expect(fork.firstTimestamp == CloudFixture.base.addingTimeInterval(600))
        #expect(scanner.firstTimestamp(ofTranscript: dir.transcript(a), sessionId: a) == CloudFixture.base.addingTimeInterval(600))
        #expect(SessionTokenScanner.readFirstTimestamp(path: dir.transcript(b), sessionId: b) == CloudFixture.base)
        // Read without scanning, the same answer.
        let fresh = SessionTokenScanner(fileURL: nil, persists: false)
        #expect(fresh.firstTimestamp(ofTranscript: dir.transcript(a), sessionId: a) == CloudFixture.base.addingTimeInterval(600))
    }

    /// Regression (review finding 7): a session two accounts ran is counted
    /// per account, by who ran it when each line was written.
    @Test func aSessionTwoAccountsRanIsCountedPerAccount() throws {
        let dir = try Projects("scan-owners")
        let first = "key-first", second = "key-second"
        try L.write([
            L.user("start", session: a, at: CloudFixture.stamp(0)),
            L.assistant(id: "m1", request: "r1", session: a, input: 10, output: 1, at: CloudFixture.stamp(10)),
            L.assistant(id: "m2", request: "r2", session: a, input: 20, output: 2, at: CloudFixture.stamp(20)),
        ], to: dir.transcript(a))
        let scanner = SessionTokenScanner(fileURL: nil, persists: false)
        let alone = [SessionOwner(from: nil, accountKey: first)]
        let before = try #require(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a), owners: alone))
        #expect(before.part(for: first).messageCount == 2 && before.part(for: second).messageCount == 0)

        // Resumed under the second account at 100 s: what came before stays
        // the first's, without reading the file again.
        let handedOver = alone + [SessionOwner(from: CloudFixture.base.addingTimeInterval(100), accountKey: second)]
        try L.write([
            L.user("continue", session: a, at: CloudFixture.stamp(120)),
            L.assistant(id: "m3", request: "r3", session: a, model: "claude-haiku-4-5", input: 300, output: 3,
                        at: CloudFixture.stamp(130)),
            // A subagent of the second account's turn.
        ], to: dir.transcript(a), append: true)
        try L.write([
            L.assistant(id: "m4", request: "r4", session: a, input: 4000, output: 4, at: CloudFixture.stamp(140), sidechain: true),
        ], to: dir.path("\(a)/subagents/agent-x.jsonl"))
        let after = try #require(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a), owners: handedOver))
        let mine = after.part(for: first), theirs = after.part(for: second)
        #expect(mine.messageCount == 2 && mine.tokens.input == 30)
        #expect(mine.firstTimestamp == CloudFixture.base && mine.lastTimestamp == CloudFixture.base.addingTimeInterval(20))
        #expect(theirs.messageCount == 2 && theirs.tokens.input == 4300)
        #expect(theirs.firstTimestamp == CloudFixture.base.addingTimeInterval(120))
        #expect(theirs.models.contains("claude-haiku-4-5") && !mine.models.contains("claude-haiku-4-5"))
        #expect(after.messageCount == 4 && after.tokens.input == 4330)
        // Each part priced by its own responses (per million: Opus 4.5 75 + 150; Haiku 4.5 315, Opus 4.5 20100).
        #expect(mine.cost == 225_000 && theirs.cost == 20_415_000 && after.cost == 20_640_000)
        #expect(scanner.cachedSummary(of: a) == after)

        // Owners that disagree with what was counted: counted again.
        let otherWay = [SessionOwner(from: nil, accountKey: second)]
        let recounted = try #require(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a), owners: otherWay))
        #expect(recounted.part(for: second).messageCount == 4 && recounted.part(for: first).messageCount == 0)
        #expect(recounted.tokens == after.tokens)
        #expect(recounted.part(for: second).cost == 20_640_000 && recounted.part(for: first).cost == nil)
    }

    @Test func readsOnlyWhatWasAddedAndRecountsARewrite() throws {
        let dir = try Projects("scan-incremental")
        let path = dir.transcript(a)
        try L.write([
            L.user("go", session: a, at: CloudFixture.stamp(0)),
            L.assistant(id: "msg_1", request: "req_1", session: a, input: 10, output: 1, at: CloudFixture.stamp(1)),
        ], to: path)
        let file = URL(fileURLWithPath: dir.root).appendingPathComponent("scan-state.json")
        let scanner = SessionTokenScanner(fileURL: file, persists: true)
        #expect(scanner.scan(sessionId: a, transcriptPath: path)?.tokens.input == 10)

        // Appended, with a half-written last line: only complete lines count.
        try L.write([L.assistant(id: "msg_2", request: "req_2", session: a, input: 20, output: 2, at: CloudFixture.stamp(2))],
                    to: path, append: true)
        let handle = try #require(FileHandle(forWritingAtPath: path))
        try handle.seekToEnd()
        try handle.write(contentsOf: Data(#"{"type":"assistant","message":{"id":"msg_3""#.utf8))
        try handle.close()
        #expect(scanner.scan(sessionId: a, transcriptPath: path)?.tokens.input == 30)

        // Saved, reloaded, finished: the new scanner continues from the watermark.
        scanner.saveNow()
        #expect(CloudFiles.permissions(of: file) == 0o600)
        let reloaded = SessionTokenScanner(fileURL: file, persists: true)
        #expect(reloaded.cachedSummary(of: a)?.tokens.input == 30)
        let rest = #","usage":{"input_tokens":5,"output_tokens":5},"model":"claude-opus-4-5"},"requestId":"req_3","sessionId":"\#(a)","timestamp":"\#(CloudFixture.stamp(3))"}"# + "\n"
        let more = try #require(FileHandle(forWritingAtPath: path))
        try more.seekToEnd()
        try more.write(contentsOf: Data(rest.utf8))
        try more.close()
        let grown = try #require(reloaded.scan(sessionId: a, transcriptPath: path))
        #expect(grown.tokens.input == 35 && grown.messageCount == 3)
        // Priced across the reload too (per million: 75 + 150 + 150).
        #expect(grown.cost == 375_000)

        // Rewritten shorter: counted again from the start, nothing doubled.
        try L.write([
            L.user("go", session: a, at: CloudFixture.stamp(0)),
            L.assistant(id: "msg_1", request: "req_1", session: a, input: 10, output: 1, at: CloudFixture.stamp(1)),
        ], to: path)
        let rewritten = try #require(reloaded.scan(sessionId: a, transcriptPath: path))
        #expect(rewritten.tokens.input == 10 && rewritten.messageCount == 1)
    }

    @Test func deletedTranscriptsAreForgotten() throws {
        let dir = try Projects("scan-prune")
        try L.write([L.assistant(id: "m1", request: "r1", session: a, input: 1, output: 1, at: CloudFixture.stamp(0))],
                    to: dir.transcript(a))
        try L.write([L.assistant(id: "m2", request: "r2", session: b, input: 1, output: 1, at: CloudFixture.stamp(0))],
                    to: dir.transcript(b))
        let scanner = SessionTokenScanner(fileURL: nil, persists: false)
        #expect(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a)) != nil)
        #expect(scanner.scan(sessionId: b, transcriptPath: dir.transcript(b)) != nil)
        try FileManager.default.removeItem(atPath: dir.transcript(a))
        #expect(scanner.pruneMissingFiles() == 1)
        #expect(scanner.cachedSummary(of: a) == nil)
        #expect(scanner.cachedSummary(of: b)?.messageCount == 1)
    }

    @Test func neverOpensAnythingOutsideProjects() {
        #expect(SessionTokenScanner.isSafeTranscriptPath("/Users/me/.claude/projects/-x/\(a).jsonl"))
        #expect(SessionTokenScanner.isSafeTranscriptPath("/Users/me/.claude/projects/-x/\(a)/subagents/agent-1.jsonl"))
        #expect(!SessionTokenScanner.isSafeTranscriptPath("/Users/me/.claude/sessions/123.json"))
        #expect(!SessionTokenScanner.isSafeTranscriptPath("/Users/me/.claude/projects/sessions/x.jsonl"))
        #expect(!SessionTokenScanner.isSafeTranscriptPath("/Users/me/.claude/projects/-x/../../settings.jsonl"))
        #expect(!SessionTokenScanner.isSafeTranscriptPath("/Users/me/.claude/history.jsonl"))
        let scanner = SessionTokenScanner(fileURL: nil, persists: false)
        #expect(scanner.scan(sessionId: a, transcriptPath: "/Users/me/.claude/sessions/\(a).jsonl") == nil)
        #expect(scanner.scan(sessionId: "not-a-session", transcriptPath: "/Users/me/.claude/projects/-x/y.jsonl") == nil)
    }

    /// Regression (fix check): a `<id>.jsonl` link in a projects folder
    /// that points out of it (at a key file in `sessions/`, say) is never opened
    /// by the backfill's first-line read, as `scan` never opens it; a link
    /// into another projects folder (a shared history) still is.
    @Test func firstTimestampNeverOpensALinkOutOfProjects() throws {
        let dir = try Projects("scan-head-link")
        let fm = FileManager.default
        let sessions = (dir.root as NSString).appendingPathComponent(".claude/sessions")
        try fm.createDirectory(atPath: sessions, withIntermediateDirectories: true)
        let key = (sessions as NSString).appendingPathComponent("123.key")
        // Looks like a transcript line, so a read would find a timestamp.
        try L.write([L.user("x", session: a, at: CloudFixture.stamp(0))], to: key)
        try fm.createSymbolicLink(atPath: dir.transcript(a), withDestinationPath: key)
        #expect(SessionTokenScanner.isSafeTranscriptPath(dir.transcript(a)))
        let scanner = SessionTokenScanner(fileURL: nil, persists: false)
        #expect(scanner.firstTimestamp(ofTranscript: dir.transcript(a), sessionId: a) == nil)
        #expect(scanner.scan(sessionId: a, transcriptPath: dir.transcript(a)) == nil)

        let shared = (dir.root as NSString).appendingPathComponent(".claude-shared/projects/-Users-me-code-app")
        let real = (shared as NSString).appendingPathComponent("\(b).jsonl")
        try L.write([L.user("x", session: b, at: CloudFixture.stamp(60))], to: real)
        try fm.createSymbolicLink(atPath: dir.transcript(b), withDestinationPath: real)
        #expect(scanner.firstTimestamp(ofTranscript: dir.transcript(b), sessionId: b) == CloudFixture.base.addingTimeInterval(60))
    }

    @Test func listsSessionTranscriptsOfAProjectsFolder() throws {
        let dir = try Projects("scan-list")
        try L.write([L.user("x", session: a, at: CloudFixture.stamp(0))], to: dir.transcript(a))
        try L.write([L.user("x", session: b, at: CloudFixture.stamp(0))], to: dir.transcript(b))
        try L.write([L.user("x", session: a, at: CloudFixture.stamp(0))], to: dir.path("agent-x.jsonl"))
        try L.write([L.user("x", session: a, at: CloudFixture.stamp(0))], to: dir.path("notes.jsonl"))
        let found = SessionTokenScanner.sessionFiles(inProjectsRoot: dir.projects)
        #expect(Set(found.map(\.sessionId)) == [a, b])
        #expect(SessionTokenScanner.sessionFiles(inProjectsRoot: dir.root).isEmpty)
    }
}
