import Foundation
import Testing
@testable import ClaudeControl

struct TranscriptTests {
    private func summary(_ lines: [[String: Any]]) -> ConversationInfo {
        var summary = TranscriptSummary()
        for line in lines {
            summary.consume(line, dateParser: { _ in nil })
        }
        return summary.info
    }

    private func assistant(id: String?, requestId: String? = "req_1", model: String = "claude-opus-4-5", sidechain: Bool = false, input: Int, output: Int, cacheRead: Int = 0, cacheCreation: Int = 0, text: String = "hi") -> [String: Any] {
        var message: [String: Any] = [
            "model": model,
            "content": [["type": "text", "text": text]],
            "usage": [
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_input_tokens": cacheRead,
                "cache_creation_input_tokens": cacheCreation,
            ],
        ]
        if let id { message["id"] = id }
        var line: [String: Any] = ["type": "assistant", "isSidechain": sidechain, "uuid": UUID().uuidString, "message": message]
        if let requestId { line["requestId"] = requestId }
        return line
    }

    @Test func usageIsCountedOncePerResponse() {
        let info = summary([
            // One API response streamed as three content-block lines.
            assistant(id: "msg_1", input: 10, output: 5, cacheRead: 1000, cacheCreation: 200),
            assistant(id: "msg_1", input: 10, output: 5, cacheRead: 1000, cacheCreation: 200),
            assistant(id: "msg_1", input: 10, output: 5, cacheRead: 1000, cacheCreation: 200),
            assistant(id: "msg_2", requestId: "req_2", input: 3, output: 7, cacheRead: 1200),
        ])
        #expect(info.usage.inputTokens == 13)
        #expect(info.usage.outputTokens == 12)
        #expect(info.usage.cacheReadTokens == 2200)
        #expect(info.usage.cacheCreationTokens == 200)
        #expect(info.usage.totalTokens == 25)
        #expect(info.usage.totalTokensIncludingCache == 2425)
        #expect(info.usage.formattedTotal == "25")
        #expect(info.lastContextTokens == 3 + 1200)
        #expect(info.lastModel == "claude-opus-4-5")
    }

    @Test func repeatedResponseUsesItsLatestUsage() {
        let info = summary([
            assistant(id: "msg_1", input: 10, output: 1),
            assistant(id: "msg_1", input: 10, output: 40),
        ])
        #expect(info.usage.outputTokens == 40)
    }

    @Test func syntheticAndSidechainMessagesAreSkipped() {
        let info = summary([
            assistant(id: "msg_1", input: 10, output: 5),
            assistant(id: "msg_err", model: "<synthetic>", input: 0, output: 0, text: "API Error"),
            assistant(id: "msg_side", sidechain: true, input: 999, output: 999),
        ])
        #expect(info.usage.inputTokens == 10)
        #expect(info.usage.outputTokens == 5)
        #expect(info.lastModel == "claude-opus-4-5")
    }

    @Test func titlesPreferCustomOverAI() {
        #expect(summary([["type": "ai-title", "aiTitle": "Fix login bug"]]).title == "Fix login bug")
        let both = summary([
            ["type": "custom-title", "customTitle": "My rename"],
            ["type": "ai-title", "aiTitle": "Later AI title"],
        ])
        #expect(both.title == "My rename")
    }

    @Test func tracksMessagesAndSkipsCommands() {
        let info = summary([
            ["type": "user", "message": ["content": "<command-name>/model</command-name>"]],
            ["type": "user", "message": ["content": "Please fix the login flow"]],
            ["type": "assistant", "message": ["content": [["type": "tool_use", "id": "t1", "name": "Bash", "input": ["command": "npm test"]]]]],
        ])
        #expect(info.firstUserMessage == "Please fix the login flow")
        #expect(info.lastMessage == "npm test")
        #expect(info.lastMessageRole == "tool")
        #expect(info.lastToolName == "Bash")
    }

    @Test func contextEstimateWindow() {
        #expect(ContextUsageEstimator.windowSize(contextTokens: 50_000, statusLineWindowSize: nil, modelIds: ["claude-opus-4-5"]) == 200_000)
        #expect(ContextUsageEstimator.windowSize(contextTokens: 50_000, statusLineWindowSize: nil, modelIds: ["claude-opus-4-5[1m]"]) == 1_000_000)
        #expect(ContextUsageEstimator.windowSize(contextTokens: 250_000, statusLineWindowSize: nil, modelIds: [nil]) == 1_000_000)
        #expect(ContextUsageEstimator.windowSize(contextTokens: 50_000, statusLineWindowSize: 400_000, modelIds: []) == 400_000)
        #expect(ContextUsageEstimator.percent(contextTokens: 50_000, statusLineWindowSize: nil, modelIds: []) == 25)
    }

    // MARK: - Slug fallback

    @Test func slugReplacesEveryNonAlphanumeric() {
        #expect(TranscriptLocator.projectSlug(for: "/Users/paras/Documents/GitHub/@paraswtf/superpowered-vibe-notch")
            == "-Users-paras-Documents-GitHub--paraswtf-superpowered-vibe-notch")
        #expect(TranscriptLocator.projectSlug(for: "/Users/me/my_app.v2 (copy)") == "-Users-me-my-app-v2--copy-")
        // JavaScript replaces per UTF-16 unit: é is one unit, an emoji two.
        #expect(TranscriptLocator.projectSlug(for: "/tmp/café") == "-tmp-caf-")
        #expect(TranscriptLocator.projectSlug(for: "/tmp/a😀") == "-tmp-a--")
    }

    @Test func locatesTranscriptBySlugThenBySearch() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("agentnotch-transcripts-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let configDir = root.appendingPathComponent(".claude-work").path
        let cwd = "/Users/me/@org/app"

        // Expected slug location.
        let expected = TranscriptLocator.expectedTranscriptPath(sessionId: "s1", cwd: cwd, configDir: configDir)
        #expect(expected.hasSuffix("/projects/-Users-me--org-app/s1.jsonl"))
        try FileManager.default.createDirectory(atPath: (expected as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: expected, contents: Data("{}\n".utf8))
        #expect(TranscriptLocator.transcriptPath(sessionId: "s1", cwd: cwd, configDir: configDir) == expected)

        // A session whose folder name we can't reproduce is found by search.
        let odd = (configDir as NSString).appendingPathComponent("projects/-some-truncated-slug-abc123/s2.jsonl")
        try FileManager.default.createDirectory(atPath: (odd as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: odd, contents: Data("{}\n".utf8))
        #expect(TranscriptLocator.transcriptPath(sessionId: "s2", cwd: "/elsewhere", configDir: configDir) == odd)

        // The hook's path wins when it exists; a missing session is nil.
        #expect(TranscriptLocator.transcriptPath(sessionId: "s2", cwd: cwd, configDir: configDir, hint: expected) == expected)
        #expect(TranscriptLocator.transcriptPath(sessionId: "nope", cwd: cwd, configDir: configDir) == nil)
        #expect(TranscriptLocator.searchTranscript(sessionId: "../s1", configDir: configDir) == nil)
    }

    @Test func subagentTranscriptLookup() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("agentnotch-subagents-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let project = root.appendingPathComponent("projects/-p").path
        let main = (project as NSString).appendingPathComponent("s1.jsonl")
        let workflow = (project as NSString).appendingPathComponent("s1/subagents/workflows/wf_1/agent-a7.jsonl")
        try FileManager.default.createDirectory(atPath: (workflow as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: workflow, contents: Data())

        #expect(TranscriptLocator.subagentTranscriptPath(transcriptPath: main, agentId: "a7") == workflow)
        #expect(TranscriptLocator.subagentTranscriptPath(transcriptPath: main, agentId: "zz")
            == (project as NSString).appendingPathComponent("s1/subagents/agent-zz.jsonl"))
    }

    @Test func incrementalParseSkipsPartialLines() async throws {
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("agentnotch-partial-\(UUID().uuidString).jsonl").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        let first = try JSONSerialization.data(withJSONObject: assistant(id: "m1", input: 1, output: 1))
        let second = try JSONSerialization.data(withJSONObject: assistant(id: "m2", requestId: "r2", input: 2, output: 2))

        var content = first + Data("\n".utf8) + second.prefix(10)
        FileManager.default.createFile(atPath: path, contents: content)
        // What the parser's sync does: read complete lines from an offset.
        var offset: UInt64 = 0
        var summary = TranscriptSummary()
        func read() {
            _ = TranscriptLineReader.forEachLine(path: path, from: &offset) { line in
                guard let json = ConversationParser.decode(line) else { return }
                summary.consume(json, dateParser: ConversationParser.parseDate)
            }
        }
        read()
        #expect(summary.info.usage.inputTokens == 1)

        content = first + Data("\n".utf8) + second + Data("\n".utf8)
        try content.write(to: URL(fileURLWithPath: path))
        read()
        #expect(summary.info.usage.inputTokens == 3)
    }
}
