import AppKit
import Foundation
import SwiftUI
import Testing
@testable import ClaudeControl

/// Defects found in the second review pass of WP-B, each pinned by a test.
struct B_ReviewPass2Tests {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    // MARK: Long chats

    private func history(_ count: Int, prefix: String = "m") -> [ChatHistoryItem] {
        (0..<count).map { ChatHistoryItem(id: "\(prefix)-\($0)", type: .assistant("Message \($0)"), timestamp: now) }
    }

    @Test func aLongChatOpensOnItsNewestPage() {
        let items = history(2_000)
        let start = ChatTranscript.startIndex(in: items, firstShownId: nil)
        #expect(start == 2_000 - ChatTranscript.pageSize)
        #expect(items.count - start == ChatTranscript.pageSize, "only a page is drawn, not the whole session")
        // A short chat is drawn whole.
        #expect(ChatTranscript.startIndex(in: history(40), firstShownId: nil) == 0)
    }

    @Test func itemsArrivingKeepTheOldestShownInPlace() {
        var items = history(400)
        let first = items[ChatTranscript.startIndex(in: items, firstShownId: nil)].id
        items += history(30, prefix: "new")
        // The window grows downwards instead of sliding under the reader.
        #expect(items[ChatTranscript.startIndex(in: items, firstShownId: first)].id == first)
        // A history that no longer has that item (cleared, reloaded) starts over.
        #expect(ChatTranscript.startIndex(in: history(10, prefix: "x"), firstShownId: first) == 0)
    }

    @Test func showEarlierAddsAPageUntilTheStart() {
        #expect(ChatTranscript.earlierStart(from: 1_850) == 1_850 - ChatTranscript.pageSize)
        #expect(ChatTranscript.earlierStart(from: 20) == 0)
    }

    @Test func theTranscriptRedrawsOnlyForWhatItShows() {
        let items = history(3)
        let a = ChatTranscript(history: items, isLoading: false, workingLabel: nil, agentDescriptions: [:], onHeight: { _ in })
        // Another height callback (every redraw of the chat makes a new one)
        // is not a change: typing in the composer leaves the transcript alone.
        let b = ChatTranscript(history: items, isLoading: false, workingLabel: nil, agentDescriptions: [:], onHeight: { _ in print("x") })
        #expect(a == b)
        let grown = ChatTranscript(history: history(4), isLoading: false, workingLabel: nil, agentDescriptions: [:], onHeight: { _ in })
        #expect(a != grown)
        let working = ChatTranscript(history: items, isLoading: false, workingLabel: "Working…", agentDescriptions: [:], onHeight: { _ in })
        #expect(a != working)
    }

    /// A 2,000-item session opens in the panel and still asks for a height
    /// within the chat's bounds.
    @MainActor @Test func aTwoThousandItemChatBuildsWithinBounds() {
        _ = NSApplication.shared
        let state = ClaudePanelState(route: .session(id: "long"))
        state.contentWidth = 440
        let view = ChatContent(session: SampleSessions.working()[0], history: history(2_000), isLoading: false,
                               canFocus: true, messageRoute: .tmux, account: nil, agentDescriptions: [:],
                               sendFailure: nil, state: state, hooks: ChatHooks(), onSend: { _ in })
            .claudeControlTheme(.codenotchDark)
        let hosting = NSHostingView(rootView: view)
        hosting.sizingOptions = []
        let window = NSWindow(contentRect: NSRect(x: -12_000, y: -12_000, width: 440, height: 780),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = hosting
        window.orderFront(nil)
        defer { window.close() }
        for _ in 0..<4 {
            hosting.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        }
        #expect(state.idealContentHeight >= ClaudePanelState.minimumContentHeight)
        #expect(state.idealContentHeight <= ClaudePanelGeometry.heightCap(.chat))
    }

    // MARK: Diffs

    @Test func aHugeEditDiffsWithinItsBudget() {
        // 3,000 × 3,000 lines would be a 9-million-cell table in a view body.
        let old = (0..<3_000).map { "old line \($0)" }.joined(separator: "\n")
        let new = (0..<3_000).map { "new line \($0)" }.joined(separator: "\n")
        let diff = LineDiff.changes(old: old, new: new, limit: 12)
        #expect(diff.lines.count == 12)
        #expect(diff.isTruncated)
        #expect(diff.lines.allSatisfy { $0.kind == .removed })
    }

    @Test func sharedStartAndEndAreSetAsideWithTheirLineNumbersKept() {
        var lines = (1...2_000).map { "line \($0)" }
        let old = lines.joined(separator: "\n")
        lines[999] = "changed"
        let diff = LineDiff.changes(old: old, new: lines.joined(separator: "\n"), limit: 12)
        #expect(diff.lines == [
            LineDiff.Line(text: "line 1000", kind: .removed, number: 1_000),
            LineDiff.Line(text: "changed", kind: .added, number: 1_000),
        ])
        #expect(!diff.isTruncated)
        // An insertion keeps the lines after it.
        let inserted = LineDiff.changes(old: "a\nb\nc", new: "a\nx\nb\nc", limit: 12)
        #expect(inserted.lines == [LineDiff.Line(text: "x", kind: .added, number: 2)])
    }

    // MARK: Density

    private func session(_ id: String, phase: SessionPhase, lastActivity: TimeInterval = -60) -> SessionState {
        var session = SessionState(sessionId: id, cwd: "/tmp/\(id)", phase: phase, lastActivity: now.addingTimeInterval(lastActivity))
        if phase == .processing { session.turnStartedAt = now.addingTimeInterval(-30) }
        return session
    }

    private func layout(_ sessions: [SessionState], folds: [AttentionBucket: Bool] = [:]) -> SessionListLayout {
        SessionListLayout.make(
            sections: SessionSections.build(sessions),
            rows: { SessionRowModel.make($0, account: nil, rateLimit: nil, canFocus: false, now: now, home: "/Users/me") },
            folds: folds
        )
    }

    @Test func aFoldedIdleListDoesNotMakeTheRowsAboveItCompact() {
        let working = (0..<5).map { session("w\($0)", phase: .processing) }
        let idle = (0..<20).map { session("i\($0)", phase: .idle, lastActivity: -Double($0 + 1) * 60) }
        // 25 sessions, but only the 5 working rows are drawn.
        #expect(!layout(working + idle).isCompact)
        // Unfolding the idle list draws 25 rows: now they go to one line.
        #expect(layout(working + idle, folds: [.idle: false]).isCompact)
    }

    @Test func aHighlightedRowNamesTheFoldedSectionItIsIn() {
        let working = (0..<2).map { session("w\($0)", phase: .processing) }
        let idle = (0..<5).map { session("i\($0)", phase: .idle, lastActivity: -Double($0 + 1) * 60) }
        let list = layout(working + idle)
        // Idle starts folded: a banner pointing at i3 must unfold it.
        #expect(list.foldedBucket(containing: "i3") == .idle)
        #expect(list.foldedBucket(containing: "w1") == nil, "already on screen")
        #expect(list.foldedBucket(containing: "gone") == nil)
        #expect(list.foldedBucket(containing: nil) == nil)
    }

    // MARK: Adding a folder

    @Test func aLinkToTheHomeFolderIsTheHomeFolder() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("spcn-b-review-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let home = root.appendingPathComponent("home", isDirectory: true)
        try FileManager.default.createDirectory(at: home.appendingPathComponent("projects"), withIntermediateDirectories: true)
        let link = root.appendingPathComponent("claude-link")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: home)

        let check = AddFolderCheck.evaluate(path: link.path, home: home.path, entries: ["projects", ".claude.json"],
                                            knownConfigDirs: [], resolvedPath: link.resolvingSymlinksInPath().path)
        guard case .reject = check else {
            Issue.record("a link to home would write settings.json and hooks/ into home: \(check)")
            return
        }
        // A real config folder next to it is still fine.
        let config = home.appendingPathComponent(".claude-work", isDirectory: true)
        #expect(AddFolderCheck.evaluate(path: config.path, home: home.path, entries: ["projects"], knownConfigDirs: [],
                                        resolvedPath: config.resolvingSymlinksInPath().path) == .add)
    }

    // MARK: VoiceOver

    private func row(_ actions: SessionPrimaryActions, bucket: AttentionBucket = .needsInput, canFocus: Bool = true) -> SessionRowModel {
        SessionRowModel(
            id: "s", title: "Title", glyph: .needsInput, bucket: bucket, elapsed: nil,
            detail: .text("Detail", tone: .primary, lineLimit: 1), compactDetail: "", actions: actions,
            tasks: SessionTaskList(), contextPercent: nil, projectName: "p", backgroundTasks: 0, account: nil,
            canFocus: canFocus, focusLabel: "Show terminal", accessibilityLabel: "Title"
        )
    }

    @Test func aTerminalDialogOffersShowTerminalOnce() {
        let labels = RowAnswerChoices.accessibilityActions(for: row(.answerInTerminal)).map(\.label)
        #expect(labels == ["Show terminal"])
    }

    @Test func rowsOfferTheirAnswersThenReviewAndTerminal() {
        let review = RowAnswerChoices.accessibilityActions(for: row(.none, bucket: .readyForReview))
        #expect(review.map(\.label) == ["Mark reviewed", "Show terminal"])
        let permission = RowAnswerChoices.accessibilityActions(for: row(.permission(toolUseId: "t", always: nil, needsReview: false)))
        #expect(permission.map(\.label) == ["Deny", "Allow", "Show terminal"])
        let unfocusable = RowAnswerChoices.accessibilityActions(for: row(.answerInTerminal, canFocus: false))
        #expect(unfocusable.isEmpty)
    }
}
