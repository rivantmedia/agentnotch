import Testing
@testable import ClaudeControl

/// The sessions panel's whole keyboard map (design §7).
struct B_KeyRouterTests {
    typealias Router = ClaudeKeyRouter

    private let always = AlwaysAllowOffer(description: "Don't ask again for Bash(ls)", isInline: true)
    private let chatOnlyAlways = AlwaysAllowOffer(description: "Switch to accept-edits mode", isInline: false)

    private func target(_ actions: SessionPrimaryActions, reviewable: Bool = false, jump: Bool = true) -> Router.Target {
        Router.Target(sessionId: "s", actions: actions, canMarkReviewed: reviewable, canJump: jump)
    }

    private func list(_ actions: SessionPrimaryActions, reviewable: Bool = false, jump: Bool = true) -> Router.Context {
        .list(selection: target(actions, reviewable: reviewable, jump: jump))
    }

    private func command(_ key: Router.Key, _ modifiers: Router.Modifiers = [], _ context: Router.Context) -> Router.Command? {
        Router.command(for: key, modifiers: modifiers, in: context)
    }

    private var question: ChatQuestion {
        ChatQuestion(index: 0, question: "Which?", answerKey: "Which?", header: nil, multiSelect: false,
                     options: [.init(label: "A", description: nil), .init(label: "B", description: nil)])
    }

    @Test func arrowsMoveTheSelectionAndReturnOpensTheChat() {
        #expect(command(.up, [], .list(selection: nil)) == .moveSelection(by: -1))
        #expect(command(.down, [], .list(selection: nil)) == .moveSelection(by: 1))
        #expect(command(.returnKey, [], list(.none)) == .openChat(sessionId: "s"))
        #expect(command(.returnKey, [], .list(selection: nil)) == nil)
    }

    @Test func commandReturnIsTheRowsPrimaryAction() {
        let permission = SessionPrimaryActions.permission(toolUseId: "t", always: always, needsReview: false)
        #expect(command(.returnKey, .command, list(permission)) == .allow(sessionId: "s", toolUseId: "t"))
        #expect(command(.returnKey, .command, list(.plan(toolUseId: "p"))) == .approvePlan(sessionId: "s", toolUseId: "p"))
        #expect(command(.returnKey, .command, list(.none, reviewable: true)) == .markReviewed(sessionId: "s"))
        #expect(command(.returnKey, .command, list(.none)) == nil)
        #expect(command(.returnKey, .command, list(.answerInChat(toolUseId: "q"))) == .openChat(sessionId: "s"))
        // Too long to judge from the row: open it whole instead of allowing.
        let long = SessionPrimaryActions.permission(toolUseId: "t", always: nil, needsReview: true)
        #expect(command(.returnKey, .command, list(long)) == .openChat(sessionId: "s"))
        #expect(command(.returnKey, .command, .chat(target(long), isTyping: false)) == .allow(sessionId: "s", toolUseId: "t"))
    }

    @Test func approvalsNeverFireOnABareKey() {
        let permission = SessionPrimaryActions.permission(toolUseId: "t", always: always, needsReview: false)
        #expect(command(.returnKey, [], list(permission)) == .openChat(sessionId: "s"))
        #expect(command(.delete, [], list(permission)) == nil)
        #expect(command(.character("y"), [], list(permission)) == nil)
    }

    @Test func alwaysAllowAndDeny() {
        let permission = SessionPrimaryActions.permission(toolUseId: "t", always: always, needsReview: false)
        #expect(command(.returnKey, [.command, .option], list(permission)) == .alwaysAllow(sessionId: "s", toolUseId: "t"))
        #expect(command(.delete, .command, list(permission)) == .deny(sessionId: "s", toolUseId: "t"))
        #expect(command(.delete, .command, list(.plan(toolUseId: "p"))) == .keepPlanning(sessionId: "s", toolUseId: "p"))
        // A broad rule is offered only where its description is shown: the chat.
        let broad = SessionPrimaryActions.permission(toolUseId: "t", always: chatOnlyAlways, needsReview: false)
        #expect(command(.returnKey, [.command, .option], list(broad)) == nil)
        #expect(command(.returnKey, [.command, .option], .chat(target(broad), isTyping: false)) == .alwaysAllow(sessionId: "s", toolUseId: "t"))
        let noRule = SessionPrimaryActions.permission(toolUseId: "t", always: nil, needsReview: false)
        #expect(command(.returnKey, [.command, .option], .chat(target(noRule), isTyping: false)) == nil)
    }

    @Test func digitsPickAnAnswerChip() {
        let chips = SessionPrimaryActions.questionChips(toolUseId: "q", question: question)
        #expect(command(.character("1"), [], list(chips)) == .chooseOption(sessionId: "s", toolUseId: "q", index: 0))
        #expect(command(.character("2"), [], list(chips)) == .chooseOption(sessionId: "s", toolUseId: "q", index: 1))
        #expect(command(.character("3"), [], list(chips)) == nil)
        #expect(command(.character("0"), [], list(chips)) == nil)
        #expect(command(.character("1"), [], list(.none)) == nil)
        #expect(command(.character("1"), .command, list(chips)) == nil)
    }

    @Test func jumpAndReviewShortcuts() {
        #expect(command(.character("j"), .command, list(.none)) == .jump(sessionId: "s"))
        #expect(command(.character("j"), .command, list(.none, jump: false)) == nil)
        #expect(command(.character("r"), .command, list(.none, reviewable: true)) == .markReviewed(sessionId: "s"))
        #expect(command(.character("r"), .command, list(.none)) == nil)
        #expect(command(.character("r"), [.command, .shift], .list(selection: nil)) == .markAllReviewed)
        #expect(command(.character("R"), [.command, .shift], .list(selection: nil)) == .markAllReviewed)
    }

    @Test func escapeGoesBackThenCloses() {
        #expect(command(.escape, [], .chat(target(.none), isTyping: true)) == .back)
        #expect(command(.escape, [], .list(selection: nil)) == .close)
        #expect(command(.escape, [], .setup) == .close)
    }

    @Test func theComposerOwnsPlainKeysButNotCommandKeys() {
        let permission = SessionPrimaryActions.permission(toolUseId: "t", always: always, needsReview: false)
        let typing = Router.Context.chat(target(permission), isTyping: true)
        #expect(command(.returnKey, [], typing) == nil)
        #expect(command(.character("1"), [], typing) == nil)
        #expect(command(.up, [], typing) == nil)
        #expect(command(.returnKey, .command, typing) == .allow(sessionId: "s", toolUseId: "t"))
        // Arrows don't move a list that isn't shown.
        #expect(command(.down, [], .chat(target(.none), isTyping: false)) == nil)
        // Control is ignored, like the rest of the Mac.
        #expect(command(.returnKey, [.command, .control], list(.plan(toolUseId: "p"))) == .approvePlan(sessionId: "s", toolUseId: "p"))
    }

    @Test func setupTakesNoSessionKeys() {
        #expect(command(.down, [], .setup) == nil)
        #expect(command(.returnKey, .command, .setup) == nil)
    }

    @Test func movingStopsAtTheEnds() {
        let order = ["a", "b", "c"]
        #expect(Router.moved(nil, by: 1, in: order) == "a")
        #expect(Router.moved(nil, by: -1, in: order) == "c")
        #expect(Router.moved("a", by: 1, in: order) == "b")
        #expect(Router.moved("c", by: 1, in: order) == "c")
        #expect(Router.moved("a", by: -1, in: order) == "a")
        #expect(Router.moved("gone", by: 1, in: order) == "a")
        #expect(Router.moved("a", by: 1, in: []) == nil)
    }
}
