import Foundation
import Testing
@testable import ClaudeControl

/// S4: what leaves the panel (hover rows, the phone, banners) is only a
/// named field; an MCP or unknown tool's free text never does.
struct Fix_OffPanelPreviewTests {
    @Test func namedFieldsOnly() {
        #expect(ToolInput.offPanelPreview(toolName: "Bash", input: ["command": "npm test"]) == "npm test")
        #expect(ToolInput.offPanelPreview(toolName: "Edit", input: ["file_path": "/Users/x/p/auth.ts"]) == "auth.ts")
        #expect(ToolInput.offPanelPreview(toolName: "Grep", input: ["pattern": "TODO"]) == "TODO")
        #expect(ToolInput.offPanelPreview(toolName: "WebFetch", input: ["url": "https://example.com/a?token=x"]) == "example.com")
        #expect(ToolInput.offPanelPreview(toolName: "WebSearch", input: ["query": "swift testing"]) == "swift testing")
    }

    @Test func mcpAndUnknownToolsShowTheirNameAlone() {
        let email = ["to": "boss@x.com", "subject": "Quarterly", "body": "Here is what Claude wrote for you"]
        #expect(ToolInput.offPanelPreview(toolName: "mcp__gmail__send_email", input: email) == nil)
        #expect(ToolInput.offPanelPreview(toolName: "Task", input: ["description": "Explore", "prompt": "secret"]) == nil)
        // The panel's own preview still shows what it always did.
        #expect(ToolInput.preview(toolName: "mcp__gmail__send_email", input: email) != nil)
    }

    @Test func theHoverRowAndBannerUseIt() {
        var session = SessionState(sessionId: "s", cwd: "/tmp/p")
        session.phase = .waitingForApproval(PermissionContext(
            toolUseId: "t", toolName: "mcp__slack__post_message",
            toolInput: ["channel": AnyCodable("#general"), "text": AnyCodable("Claude's draft")], receivedAt: Date()))
        session.needsInputReason = .permission(tool: "mcp__slack__post_message")
        let summary = ClaudeHostProjections.needsInput(.permission(tool: "mcp__slack__post_message"),
                                                       session: session, limitHit: nil, now: Date())
        #expect(!summary.summary.contains("draft") && !summary.summary.contains("general"))
        let banner = SessionNotificationContent.needsInput(session: session, reason: .permission(tool: "mcp__slack__post_message"),
                                                           accountLabel: nil)
        #expect(!banner.body.contains("draft") && !banner.body.contains("general"))
    }
}
