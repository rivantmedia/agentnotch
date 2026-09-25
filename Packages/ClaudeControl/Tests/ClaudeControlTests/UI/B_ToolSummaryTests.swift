import Foundation
import Testing
@testable import ClaudeControl

/// What the chat says after a tool's name, and the Edit diff.
struct B_ToolSummaryTests {
    private func tool(_ name: String, _ input: [String: String] = [:], status: ToolStatus = .success,
                      result: ToolResultData? = nil) -> ToolCallItem {
        ToolCallItem(name: name, input: input, status: status, result: nil, structuredResult: result, subagentTools: [])
    }

    @Test func completedToolsSayWhatTheyWorkedOn() {
        #expect(ToolCallSummary.text(for: tool("Bash", ["command": "npm test\n--ci", "description": "Run the tests"])) == "Run the tests")
        #expect(ToolCallSummary.text(for: tool("Bash", ["command": "npm test\n--ci"])) == "npm test")
        #expect(ToolCallSummary.text(for: tool("Edit", ["file_path": "/a/b/app.ts"])) == "app.ts")
        #expect(ToolCallSummary.text(for: tool("Mystery")) == "Completed")
    }

    @Test func theToolsNameIsNotRepeated() {
        let read = tool("Read", result: .read(ReadResult(filePath: "/x/redirect.ts", content: "a", numLines: 5, startLine: 1, totalLines: 9)))
        #expect(ToolCallSummary.text(for: read) == "redirect.ts (5+ lines)")
        #expect(ToolCallSummary.text(for: tool("Grep", ["pattern": "TODO"], status: .running)) == "Searching: TODO")
    }

    @Test func subagentsCountTheirTools() {
        var task = tool("Task", ["description": "Audit the API"], status: .running)
        task.subagentTools = [SubagentToolCall(id: "1", name: "Read", input: [:], status: .success, timestamp: Date())]
        #expect(ToolCallSummary.text(for: task) == "Audit the API · 1 tool")
    }

    @Test func diffShowsRemovalsThenAdditionsAndStopsAtTheLimit() {
        let diff = LineDiff.changes(old: "a\nb\nc", new: "a\nB\nc", limit: 12)
        #expect(diff.lines == [
            LineDiff.Line(text: "b", kind: .removed, number: 2),
            LineDiff.Line(text: "B", kind: .added, number: 2),
        ])
        #expect(!diff.isTruncated)
        let long = LineDiff.changes(old: (1...20).map(String.init).joined(separator: "\n"), new: "", limit: 5)
        #expect(long.lines.count == 5)
        #expect(long.isTruncated)
        #expect(LineDiff.changes(old: "", new: "", limit: 5).lines.isEmpty)
    }
}
