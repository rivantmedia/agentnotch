import Foundation
import Testing
@testable import ClaudeControl

/// Codenotch ring ids for config folders (design §4.2).
struct A3_RingIdentityTests {
    let home = "/Users/me"

    @Test func defaultFolderIsTheClaudeRing() {
        #expect(ClaudeRingIdentity.ringID(configDir: "/Users/me/.claude", home: home) == "claude")
        #expect(ClaudeRingIdentity.ringID(configDir: "~/.claude", home: home) == "claude")
        #expect(ClaudeRingIdentity.ringID(configDir: "/Users/me/.claude/", home: home) == "claude")
        #expect(ClaudeRingIdentity.ringID(configDir: "/Users/me/./.claude", home: home) == "claude")
    }

    @Test func dashFoldersKeepUpstreamsIds() {
        #expect(ClaudeRingIdentity.ringID(configDir: "/Users/me/.claude-work", home: home) == "claude-work")
        #expect(ClaudeRingIdentity.ringID(configDir: "~/.claude-side", home: home) == "claude-side")
        #expect(ClaudeRingIdentity.ringID(configDir: "/Users/me/.claude-a.b", home: home) == "claude-a.b")
    }

    @Test func anythingElseIsHashedAndStable() {
        let elsewhere = ClaudeRingIdentity.ringID(configDir: "/Volumes/work/claude", home: home)
        #expect(elsewhere.hasPrefix("claude-dir-"))
        #expect(elsewhere.count == "claude-dir-".count + 8)
        let isHex = elsewhere.dropFirst("claude-dir-".count).allSatisfy { $0.isHexDigit }
        #expect(isHex)
        // Stable across calls and path spellings.
        #expect(ClaudeRingIdentity.ringID(configDir: "/Volumes/work/claude/", home: home) == elsewhere)
        #expect(ClaudeRingIdentity.ringID(configDir: "/Volumes/work/./claude", home: home) == elsewhere)
        // Different folders, different rings.
        #expect(ClaudeRingIdentity.ringID(configDir: "/Volumes/other/claude", home: home) != elsewhere)
        // `~/.claude_x`, `~/.config/claude` and an empty slug are not upstream ids.
        for dir in ["/Users/me/.claude_personal", "/Users/me/.config/claude", "/Users/me/.claude-", "/Users/other/.claude-work"] {
            #expect(ClaudeRingIdentity.ringID(configDir: dir, home: home).hasPrefix("claude-dir-"), "\(dir)")
        }
    }

    @Test func everyIdIsAClaudeRing() {
        let dirs = ["/Users/me/.claude", "/Users/me/.claude-work", "/Volumes/x/claude", "/Users/me/.config/claude"]
        for dir in dirs {
            let id = ClaudeRingIdentity.ringID(configDir: dir, home: home)
            #expect(ClaudeRingIdentity.isClaudeRing(id), "\(id)")
            // Upstream's `ClaudeProfile.isClaude(providerID:)`: "claude" or a "claude-" prefix.
            #expect(id == "claude" || id.hasPrefix("claude-"))
        }
        #expect(!ClaudeRingIdentity.isClaudeRing("openai"))
        #expect(!ClaudeRingIdentity.isClaudeRing("claudette"))
    }
}
