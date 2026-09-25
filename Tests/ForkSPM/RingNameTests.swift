import ClaudeControl
import Foundation
import Testing
@testable import Codenotch

/// GUX-4 / CS-4: a ring is called what the account is called in the panel
/// and settings (the engine's own name: a custom name, else its default
/// rule); Codenotch's rule only for an account the engine didn't name.
@Suite struct RingNameTests {
    private func account(_ ring: String, email: String?, dir: String, own: String?) -> ClaudeAccountSummary {
        ClaudeAccountSummary(id: dir, ringID: ring, configDir: dir, label: own ?? ring, email: email,
                             isDefault: ring == "claude", launchCommand: "claude", ownLabel: own)
    }

    @Test func aCustomNameIsTheRingsName() {
        let names = ClaudeRingNames.displayNames(for: [
            account("claude", email: "me@gmail.com", dir: "/h/.claude", own: "Claude Gmail"),
            account("claude-side", email: nil, dir: "/h/.claude-side", own: "Side project"),
            account("claude-dir-1a2b3c4d", email: nil, dir: "/h/.claude_work", own: "Claude (work)"),
        ])
        #expect(names["claude"] == "Claude Gmail")
        #expect(names["claude-side"] == "Side project")
        // The engine's folder word, not "Claude (claude_work)".
        #expect(names["claude-dir-1a2b3c4d"] == "Claude (work)")
    }

    @Test func theSameAddressInTwoOrganizationsKeepsTheEnginesNames() {
        let names = ClaudeRingNames.displayNames(for: [
            account("claude", email: "me@x.com", dir: "/h/.claude", own: "Claude me@x.com · OrgA"),
            account("claude-b", email: "me@x.com", dir: "/h/.claude-b", own: "Claude me@x.com · OrgB"),
        ])
        #expect(names["claude"] != names["claude-b"])
    }

    @Test func anAccountWithoutAnEngineNameFollowsCodenotchsRule() {
        let names = ClaudeRingNames.displayNames(for: [
            account("claude-side", email: nil, dir: "/h/.claude-side", own: nil),
            account("claude", email: "me@gmail.com", dir: "/h/.claude", own: "  "),
        ])
        #expect(names["claude-side"] == "Claude (side)")
        #expect(names["claude"] == "Claude \(ClaudeProfile.accountLabel(forAddress: "me@gmail.com") ?? "")")
    }
}

/// GUX-6: VoiceOver hears what a Claude ring's badges show.
@Suite struct RingAccessibilityTests {
    @Test func theValueSaysWhatTheBadgesShow() {
        #expect(ClaudeRingAccessibility.text(.zero) == "")
        #expect(ClaudeRingAccessibility.text(ClaudeAttentionCounts(needsYou: 2, review: 1, working: 3))
                == "2 need you, 1 to review, 3 working")
        #expect(ClaudeRingAccessibility.text(ClaudeAttentionCounts(needsYou: 1, failed: 1)) == "1 needs you, 1 failed")
    }
}

/// GUX-5: while "Turn on Claude Code control" is unanswered, Settings opens
/// on the Claude Code pane (the consent card), not Codenotch's Accounts.
@Suite struct SettingsFirstPaneTests {
    @Test func theConsentCardIsWhatSettingsOpensOn() {
        #expect(ClaudeSettingsNavigation.opensOnClaudeCode(hookConsent: nil, sealed: false))
        #expect(!ClaudeSettingsNavigation.opensOnClaudeCode(hookConsent: true, sealed: false))
        #expect(!ClaudeSettingsNavigation.opensOnClaudeCode(hookConsent: false, sealed: false))
        #expect(!ClaudeSettingsNavigation.opensOnClaudeCode(hookConsent: nil, sealed: true))
    }
}

/// BHV-6: a panel opened from a banner (which activated this app) gives the
/// front back to the app the user was in when it closes; nothing else does.
@MainActor
@Suite struct PanelFrontHandBackTests {
    @Test func onlyABannerOpenHandsTheFrontBack() {
        typealias C = ClaudePanelController
        #expect(C.returnsFrontOnClose(reason: .notification, frontmostIsSelf: true, hasOtherApp: true))
        #expect(!C.returnsFrontOnClose(reason: .notification, frontmostIsSelf: false, hasOtherApp: true))
        #expect(!C.returnsFrontOnClose(reason: .notification, frontmostIsSelf: true, hasOtherApp: false))
        for reason in [C.OpenReason.ringClick, .hoverRow, .peekClick, .hotKey, .auto, .settings] {
            #expect(!C.returnsFrontOnClose(reason: reason, frontmostIsSelf: true, hasOtherApp: true), "\(reason)")
        }
    }
}
