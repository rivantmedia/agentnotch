import Foundation
import Testing
@testable import ClaudeControl

/// GUX-7, GUX-9, GUX-13: what the settings and panel say.
struct Fix_SettingsAndPanelCopyTests {
    /// GUX-7: an untracked account's usage isn't checked because it isn't
    /// tracked, whatever its ring switch says.
    @Test func anUntrackedAccountSaysWhyItsUsageIsNotChecked() {
        #expect(SettingsUsageLine.text(for: nil, isRingShown: true, isTracked: false, now: Date())
                == "Not checked while it isn't tracked")
        #expect(SettingsUsageLine.text(for: nil, isRingShown: false, isTracked: false, now: Date())
                == "Not checked while it isn't tracked")
        #expect(SettingsUsageLine.text(for: nil, isRingShown: false, now: Date()) == "Not checked while its ring is off")
    }

    /// GUX-13: the chip's total is never the needs-you count; VoiceOver hears both.
    @Test func aChipSaysHowManyNeedYouApartFromTheTotal() {
        let chip = RingChip(ringID: "claude", label: "Work", colorIndex: 1, count: 9, needsYouCount: 1)
        #expect(chip.needsYou)
        #expect(RingChip(ringID: "c", label: "P", colorIndex: 0, count: 3, needsYouCount: 0).needsYou == false)
    }

    /// GUX-9: control off is said once, and the per-account line no longer
    /// claims "done" needs hooks.
    @Test func controlOffIsSaidOnceAndHonestly() {
        var health = HookHealth()
        #expect(!health.controlOff)
        health.controlOff = true
        #expect(health.isHealthy)
        #expect(!HookHealth.controlOffMessage.contains("done"))
        #expect(!HookHealth(accountsWithoutHooks: ["Work"]).consequence.contains("done"))
    }
}

extension Fix_SettingsAndPanelCopyTests {
    @Test func theChipsAccessibilityLabelNamesBoth() {
        #expect(FilterChip.accessibilityLabel(label: "Work", count: 9, needsYouCount: 1) == "Work, 9 sessions, 1 needs you")
        #expect(FilterChip.accessibilityLabel(label: "Work", count: 1, needsYouCount: 0) == "Work, 1 session")
    }
}

extension Fix_SettingsAndPanelCopyTests {
    /// S5: the usage check says what it runs and what it may touch.
    @Test func theUsageCheckSaysWhatItRuns() {
        let text = UsageCheckCopy.explanation(minutes: 5)
        #expect(text.contains("Every 5 min"))
        #expect(text.contains("Claude Code's own usage check"))
        #expect(text.contains("update its own files"))
        #expect(text.contains("login shell"))
        #expect(text.contains("never reads your login token"))
    }
}
