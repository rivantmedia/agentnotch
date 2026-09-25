import Foundation
import Testing
@testable import ClaudeControl

struct AccountPathsTests {
    @Test func configDirFromMainTranscript() {
        let home = NSHomeDirectory()
        let path = "\(home)/.claude-work/projects/-Users-me-proj/0b7c.jsonl"
        #expect(AccountPaths.configDir(fromTranscriptPath: path) == "\(home)/.claude-work")
    }

    @Test func configDirFromSubagentTranscript() {
        let path = "/Users/me/.claude/projects/-Users-me-proj/abc/subagents/agent-1.jsonl"
        #expect(AccountPaths.configDir(fromTranscriptPath: path) == "/Users/me/.claude")
    }

    @Test func configDirRejectsOtherShapes() {
        #expect(AccountPaths.configDir(fromTranscriptPath: "/tmp/file.jsonl") == nil)
    }

    @Test func normalizeDropsTrailingSlashAndExpandsTilde() {
        #expect(AccountPaths.normalize("~/.claude-work/") == NSHomeDirectory() + "/.claude-work")
    }

    @Test func globalConfigFileFollowsClaudeCode() {
        let home = NSHomeDirectory()
        #expect(AccountPaths.globalConfigFile(configDir: "~/.claude", configDirEnv: nil) == "\(home)/.claude.json")
        #expect(AccountPaths.globalConfigFile(configDir: "~/.claude-work", configDirEnv: "\(home)/.claude-work")
            == "\(home)/.claude-work/.claude.json")
    }

    @Test func usageWindowPace() {
        let now = Date()
        let window = UsageWindow(utilization: 60, resetsAt: now.addingTimeInterval(2.5 * 3600), duration: UsageWindow.sessionDuration)
        #expect(abs((window.elapsedFraction(now: now) ?? 0) - 0.5) < 0.0001)
        let expired = UsageWindow(utilization: 80, resetsAt: now.addingTimeInterval(-1), duration: UsageWindow.sessionDuration)
        #expect(expired.effectiveUtilization(now: now) == 0)
    }
}
