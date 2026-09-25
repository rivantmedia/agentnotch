import Foundation
import Testing
@testable import ClaudeControl

/// Defects found reviewing WP-B, each pinned by a test.
struct B_ReviewFixesTests {
    private let t0 = Date(timeIntervalSince1970: 1_000)

    // MARK: Answers

    @Test func anAnsweredRequestStaysAnsweredAfterLeavingTheScreen() {
        var gate = AnswerGate()
        gate.noteShown(["a"], now: t0)
        let first = gate.claim("a", now: t0.addingTimeInterval(1))
        #expect(first)
        // The chat opens on another session (only its request is on screen),
        // then the list comes back while the engine still shows "a".
        gate.noteShown(["b"], now: t0.addingTimeInterval(1.2))
        gate.noteShown(["a", "b"], now: t0.addingTimeInterval(1.4))
        #expect(!gate.isArmed("a", now: t0.addingTimeInterval(3)))
        let again = gate.claim("a", now: t0.addingTimeInterval(3))
        #expect(!again)
        // Only b's wait is still ahead: the answered one plans no redraw.
        #expect(gate.nextArming(after: t0.addingTimeInterval(1.4)) == t0.addingTimeInterval(1.2).addingTimeInterval(AnswerGate.armDelay))
        #expect(gate.nextArming(after: t0.addingTimeInterval(2)) == nil)
        // Long after, the memory is let go.
        gate.noteShown(["b"], now: t0.addingTimeInterval(AnswerGate.answeredMemory + 2))
        #expect(gate.answered.isEmpty)
    }

    @Test func theStateRedrawsWhenARequestArms() async throws {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        state.noteShownRequests(["t"])
        #expect(!state.isAnswerArmed("t"))
        let before = state.armingTick
        // Nothing else happens meanwhile: only the state's own timer can
        // redraw the request armed. (Polled: other suites share the main actor.)
        for _ in 0..<50 where state.armingTick == before {
            try await Task.sleep(for: .milliseconds(100))
        }
        #expect(state.armingTick > before, "the list and the chat observe this to draw the buttons live")
        #expect(state.isAnswerArmed("t"))
    }

    /// Snapshots draw requests answerable at once.
    @Test func requestsShownArmedNeedNoRedraw() async throws {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        state.noteShownRequests(["t"], armed: true)
        try await Task.sleep(for: .seconds(AnswerGate.armDelay + 0.2))
        #expect(state.armingTick == 0)
        #expect(state.isAnswerArmed("t"))
    }

    // MARK: Keys

    private func target(_ actions: SessionPrimaryActions) -> ClaudeKeyRouter.Target {
        ClaudeKeyRouter.Target(sessionId: "s", actions: actions, canMarkReviewed: false, canJump: true)
    }

    @Test func alwaysAllowNeverSkipsTheReviewOfALongRequest() {
        let narrow = AlwaysAllowOffer(description: "Don't ask again for Bash(git push:*)", isInline: true)
        let long = SessionPrimaryActions.permission(toolUseId: "t", always: narrow, needsReview: true)
        #expect(ClaudeKeyRouter.command(for: .returnKey, modifiers: [.command, .option], in: .list(selection: target(long))) == nil)
        // Read whole in the chat, it can be.
        #expect(ClaudeKeyRouter.command(for: .returnKey, modifiers: [.command, .option], in: .chat(target(long), isTyping: false))
            == .alwaysAllow(sessionId: "s", toolUseId: "t"))
        // And the row offers nothing VoiceOver could use to skip it either.
        let row = rowModel(actions: long)
        #expect(!RowAnswerChoices.all(for: row).contains { $0.command == .alwaysAllow(sessionId: "s", toolUseId: "t") })
    }

    @Test func commandShortcutsWithNothingToDoLeaveTheKeyToTheField() throws {
        let index = (0..<PanelKeys.shortcutCount).first { i in
            PanelKeys.shortcutCommand(i, in: .chat(target(.permission(toolUseId: "t", always: nil, needsReview: false)), isTyping: false))
                == .deny(sessionId: "s", toolUseId: "t")
        }
        let deleteIndex = try #require(index, "⌘⌫ is one of the panel's shortcuts")
        // Typing a reply (no request pending): ⌘⌫ is the field's "delete to
        // the start of the line", not swallowed by the panel.
        #expect(PanelKeys.shortcutCommand(deleteIndex, in: .chat(target(.none), isTyping: true)) == nil)
        #expect(PanelKeys.shortcutCommand(deleteIndex, in: .list(selection: nil)) == nil)
        // Nothing needs every shortcut at once in the composer.
        let live = (0..<PanelKeys.shortcutCount).compactMap { PanelKeys.shortcutCommand($0, in: .chat(target(.none), isTyping: true)) }
        #expect(live == [.jump(sessionId: "s"), .markAllReviewed])
    }

    // MARK: Row

    private func rowModel(actions: SessionPrimaryActions) -> SessionRowModel {
        SessionRowModel(id: "s", title: "T", glyph: .needsInput, bucket: .needsInput, elapsed: nil,
                        detail: .text("", tone: .primary, lineLimit: 1), compactDetail: "", actions: actions,
                        tasks: SessionTaskList(), contextPercent: nil, projectName: "T", backgroundTasks: 0,
                        account: nil, canFocus: true, focusLabel: "Show terminal", accessibilityLabel: "T")
    }

    @Test func aRequestAllowedFromTheRowIsShownWhole() {
        #expect(SessionPrimaryActions.permission(toolUseId: "t", always: nil, needsReview: false).allowsFromRow)
        #expect(!SessionPrimaryActions.permission(toolUseId: "t", always: nil, needsReview: true).allowsFromRow)
        #expect(!SessionPrimaryActions.plan(toolUseId: "p").allowsFromRow)
        #expect(!SessionPrimaryActions.none.allowsFromRow)
    }

    // MARK: Routes

    @Test func settingAListRouteAgainResetsTheFilter() {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        state.ringFilter = "claude-work"
        // A banner opens "every session" over the narrowed list.
        state.route = .sessions(ringID: nil)
        #expect(state.ringFilter == nil)
        state.route = .sessions(ringID: "claude-side")
        #expect(state.ringFilter == "claude-side")
        state.route = .session(id: "x")
        #expect(state.ringFilter == "claude-side")
    }

    // MARK: Copy and settings

    @Test func theHookBannerAgreesInNumber() {
        #expect(HookHealth(accountsWithoutHooks: ["Work"]).consequence.hasPrefix("Its sessions"))
        #expect(HookHealth(accountsWithoutHooks: ["Work", "Side"]).consequence.hasPrefix("Their sessions"))
    }

    @Test func legacyHooksAreRemovedWheneverTheAppMayWrite() {
        typealias Legacy = AccountSettingsItem.LegacyHooks
        #expect(Legacy.vibeNotch.canRemove(canWrite: true, canInstall: false))
        #expect(!Legacy.vibeNotch.canRemove(canWrite: false, canInstall: false))
        // Integration: Superpowered Vibe Notch's are a takeover (its hooks
        // out, its wrapped status line back), which needs no hooks of ours.
        #expect(Legacy.superpoweredVibeNotch.canRemove(canWrite: true, canInstall: false))
        #expect(!Legacy.superpoweredVibeNotch.canRemove(canWrite: false, canInstall: true))
        #expect(Legacy.superpoweredVibeNotch.removalHelp(canInstall: false).contains("puts back the status line"))
        #expect(!Legacy.superpoweredVibeNotch.removalHelp(canInstall: true).contains("Every other hook stays"))
    }
}
