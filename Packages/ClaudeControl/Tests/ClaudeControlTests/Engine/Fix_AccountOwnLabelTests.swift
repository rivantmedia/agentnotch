import Foundation
import Testing
@testable import ClaudeControl

/// GUX-4 / CS-4: the summary carries the account's own name (custom, else
/// the engine's default) separately from `label`, which a nickname replaces.
struct Fix_AccountOwnLabelTests {
    @Test func theOwnLabelIsTheCustomNameElseTheDefault() {
        var created = ClaudeAccount(configDir: "/Users/x/.claude-research", customLabel: "research")
        #expect(ClaudeHostProjections.account(created, hookStatus: nil, home: "/Users/x").ownLabel == "research")
        created.customLabel = nil
        created.email = "me@gmail.com"
        #expect(ClaudeHostProjections.account(created, hookStatus: nil, home: "/Users/x").ownLabel == "Claude Gmail")
    }

    @Test func pathsAreAbbreviatedOneWay() {
        #expect(AccountPathDisplay.abbreviated("/Users/me/.claude", home: "/Users/me") == "~/.claude")
        #expect(AccountPathDisplay.abbreviated("/Users/me/", home: "/Users/me") == "~")
        #expect(AccountPathDisplayName.abbreviated("/Users/meow/.claude", home: "/Users/me") == "/Users/meow/.claude")
    }
}
