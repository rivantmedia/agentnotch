import Carbon.HIToolbox
import Combine
import CoreGraphics
import Foundation
import Testing
@testable import ClaudeControl

/// The panel window's decisions (design §6, §7): ring clicks, anchoring,
/// closing, auto-close, holding notches open, edit keys, the hot key's chord,
/// the sealed launch hook, and the settings request that must survive the
/// settings window being built.
struct ClaudePanelPolicyTests {
    typealias Policy = ClaudePanelPolicy

    // MARK: - Ring clicks

    @Test func ringClickOpensTogglesAndSwitches() {
        #expect(Policy.ringClick(isOpen: false, anchorRingID: nil, clicked: "claude") == .open)
        #expect(Policy.ringClick(isOpen: false, anchorRingID: "claude", clicked: "claude") == .open)
        #expect(Policy.ringClick(isOpen: true, anchorRingID: "claude", clicked: "claude") == .close)
        #expect(Policy.ringClick(isOpen: true, anchorRingID: "claude", clicked: "claude-work") == .switchRing)
        // Floating (no ring): any ring click moves it onto that ring.
        #expect(Policy.ringClick(isOpen: true, anchorRingID: nil, clicked: "claude") == .switchRing)
    }

    @Test func theSameRingOnAnotherDisplaysNotchMovesThePanelThere() {
        // Every display's notch shows the same rings; the one clicked is the
        // one meant, so the panel follows it rather than closing.
        #expect(Policy.ringClick(isOpen: true, anchorRingID: "claude", clicked: "claude",
                                 onAnchorNotch: false) == .switchRing)
        #expect(Policy.ringClick(isOpen: true, anchorRingID: "claude", clicked: "claude",
                                 onAnchorNotch: true) == .close)
        #expect(Policy.ringClick(isOpen: false, anchorRingID: nil, clicked: "claude",
                                 onAnchorNotch: false) == .open)
    }

    // MARK: - Anchoring

    @Test func anchorRingFollowsTheRoute() {
        let rings = ["claude", "claude-work"]
        #expect(Policy.anchorRing(for: .sessions(ringID: "claude-work"), sessionRingID: nil, ringsInNotch: rings) == "claude-work")
        #expect(Policy.anchorRing(for: .sessions(ringID: nil), sessionRingID: nil, ringsInNotch: rings) == "claude")
        #expect(Policy.anchorRing(for: .setup, sessionRingID: nil, ringsInNotch: rings) == "claude")
        #expect(Policy.anchorRing(for: .session(id: "s"), sessionRingID: "claude-work", ringsInNotch: rings) == "claude-work")
        #expect(Policy.anchorRing(for: .session(id: "s"), sessionRingID: nil, ringsInNotch: rings) == "claude")
    }

    @Test func aRingThatIsSwitchedOffFloatsThePanel() {
        #expect(Policy.anchorRing(for: .sessions(ringID: "claude-off"), sessionRingID: nil, ringsInNotch: ["claude"]) == nil)
        #expect(Policy.anchorRing(for: .session(id: "s"), sessionRingID: "claude-off", ringsInNotch: ["claude"]) == nil)
        #expect(Policy.anchorRing(for: .sessions(ringID: nil), sessionRingID: nil, ringsInNotch: []) == nil)
    }

    @Test func anchorNotchIsUnderThePointerElseOnItsScreenElseOnTheMainScreen() {
        let laptop = CGRect(x: 0, y: 0, width: 1512, height: 982)
        let external = CGRect(x: 1512, y: 0, width: 1920, height: 1080)
        let notches = [CGRect(x: 3100, y: 100, width: 334, height: 800), CGRect(x: 1178, y: 100, width: 334, height: 782)]
        let screens = [external, laptop]
        #expect(Policy.anchorIndex(notchFrames: notches, screenFrames: screens,
                                   pointer: CGPoint(x: 1300, y: 500), mainScreen: laptop) == 1)
        #expect(Policy.anchorIndex(notchFrames: notches, screenFrames: screens,
                                   pointer: CGPoint(x: 2000, y: 500), mainScreen: laptop) == 0)
        #expect(Policy.anchorIndex(notchFrames: notches, screenFrames: screens,
                                   pointer: CGPoint(x: -500, y: 500), mainScreen: laptop) == 1)
        #expect(Policy.anchorIndex(notchFrames: notches, screenFrames: screens,
                                   pointer: nil, mainScreen: nil) == 0)
        #expect(Policy.anchorIndex(notchFrames: [], screenFrames: [], pointer: nil, mainScreen: laptop) == nil)
    }

    // MARK: - Closing

    @Test func outsideClicksCloseUnlessPinned() {
        #expect(!Policy.closesOnMouseDown(.panel, isPinned: false))
        #expect(!Policy.closesOnMouseDown(.notch, isPinned: false))
        #expect(Policy.closesOnMouseDown(.otherOwnWindow, isPinned: false))
        #expect(Policy.closesOnMouseDown(.elsewhere, isPinned: false))
        #expect(!Policy.closesOnMouseDown(.elsewhere, isPinned: true))
        #expect(!Policy.closesOnMouseDown(.otherOwnWindow, isPinned: true))
    }

    @Test func untouchedAutoOpenClosesAfterThePeekOrEightSeconds() {
        let opened = Date(timeIntervalSince1970: 1000)
        let state = Policy.AutoOpen(openedAt: opened)
        #expect(Policy.autoCloseDeadline(state, peekDuration: 5) == opened.addingTimeInterval(8))
        #expect(Policy.autoCloseDeadline(state, peekDuration: 10) == opened.addingTimeInterval(10))
    }

    @Test func autoOpenClosesOneSecondAfterItsSessionIsResolved() {
        let opened = Date(timeIntervalSince1970: 1000)
        let state = Policy.AutoOpen(openedAt: opened, resolvedAt: opened.addingTimeInterval(2))
        #expect(Policy.autoCloseDeadline(state, peekDuration: 5) == opened.addingTimeInterval(3))
        // Resolved late: the timeout still wins.
        let late = Policy.AutoOpen(openedAt: opened, resolvedAt: opened.addingTimeInterval(7.5))
        #expect(Policy.autoCloseDeadline(late, peekDuration: 5) == opened.addingTimeInterval(8))
    }

    @Test func aBannerOrAnAutoOpenLandsOnTheListWithTheRowHighlighted() {
        // Opening the chat would mark a finished session reviewed at once,
        // resolving (and closing) the panel that auto-opened for it.
        #expect(Policy.presentation(for: .session(id: "s1"), landsOnList: true)
            == Policy.Presentation(route: .sessions(ringID: nil), highlightedSessionID: "s1"))
        // A hover row or peek click asks for the chat, and gets it.
        #expect(Policy.presentation(for: .session(id: "s1"), landsOnList: false)
            == Policy.Presentation(route: .session(id: "s1")))
        // Anything that names no session shows what it asks for.
        #expect(Policy.presentation(for: .sessions(ringID: "claude"), landsOnList: true)
            == Policy.Presentation(route: .sessions(ringID: "claude")))
        #expect(Policy.presentation(for: .setup, landsOnList: true) == Policy.Presentation(route: .setup))
    }

    @Test func anUntouchedAutoOpenNeverTakesKeyForItsContent() {
        let opened = Date(timeIntervalSince1970: 1000)
        // The chat's composer asks for key as it appears: not granted.
        #expect(!Policy.grantsKeyRequest(autoOpen: Policy.AutoOpen(openedAt: opened)))
        // Once the user has clicked in it (or the pointer went in), it is theirs.
        #expect(Policy.grantsKeyRequest(autoOpen: Policy.AutoOpen(openedAt: opened, isEngaged: true)))
        // A panel opened by a click, the hot key, a banner or Settings.
        #expect(Policy.grantsKeyRequest(autoOpen: nil))
    }

    @Test func anEngagedAutoOpenStays() {
        let opened = Date(timeIntervalSince1970: 1000)
        let state = Policy.AutoOpen(openedAt: opened, resolvedAt: opened, isEngaged: true)
        #expect(Policy.autoCloseDeadline(state, peekDuration: 5) == nil)
    }

    @Test func autoOpenCausesAreNeedsInputAndReview() {
        let permission = ClaudeAttention.needsInput(ClaudeNeedsInput(kind: .permission, summary: "Allow Bash"))
        #expect(Policy.autoOpenCause(permission) == .needsInput)
        #expect(Policy.autoOpenCause(.readyForReview) == .readyForReview)
        #expect(Policy.autoOpenCause(.working) == nil)
        #expect(Policy.autoOpenCause(.idle) == nil)
    }

    @Test func anAutoOpenIsResolvedWhenItsSessionLeavesThatAttentionOrGoesAway() {
        let permission = ClaudeAttention.needsInput(ClaudeNeedsInput(kind: .permission, summary: "Allow Bash"))
        let question = ClaudeAttention.needsInput(ClaudeNeedsInput(kind: .question, summary: "Question · Scope"))
        // Still waiting, even on a different question: the user is still needed.
        #expect(!Policy.isResolved(.needsInput, attention: permission))
        #expect(!Policy.isResolved(.needsInput, attention: question))
        // Answered: back to work, or done.
        #expect(Policy.isResolved(.needsInput, attention: .working))
        #expect(Policy.isResolved(.needsInput, attention: .readyForReview))
        // Reviewed, or the session ended.
        #expect(Policy.isResolved(.readyForReview, attention: .idle))
        #expect(!Policy.isResolved(.readyForReview, attention: .readyForReview))
        #expect(Policy.isResolved(.needsInput, attention: nil))
    }

    // MARK: - Holding notches open

    @Test func holdMatrix() {
        func hold(_ policy: HoldOpenPolicy, needsYou: Bool = true, flush: Bool, hidden: Bool = false,
                  always: Bool = false) -> Bool {
            Policy.holdsNotchOpen(policy: policy, needsYou: needsYou, isFlushWithHardware: flush,
                                  userHidesNotch: hidden, userAlwaysShowsNotch: always)
        }
        // Auto: only beside the camera, where folded marks cannot show.
        #expect(hold(.auto, flush: true))
        #expect(!hold(.auto, flush: false))
        #expect(hold(.always, flush: false))
        #expect(!hold(.never, flush: true))
        // Nothing needs you: nothing held.
        #expect(!hold(.always, needsYou: false, flush: true))
        // Hidden stays hidden; already always shown needs no hold.
        #expect(!hold(.always, flush: true, hidden: true))
        #expect(!hold(.always, flush: true, always: true))
    }

    // MARK: - Keys

    @Test func editKeysTheNonActivatingPanelAnswersItself() {
        let cmd: Policy.KeyModifiers = .command
        #expect(Policy.editCommand(charactersIgnoringModifiers: "a", modifiers: cmd) == .selectAll)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "c", modifiers: cmd) == .copy)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "v", modifiers: cmd) == .paste)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "x", modifiers: cmd) == .cut)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "z", modifiers: cmd) == .undo)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "Z", modifiers: [.command, .shift]) == .redo)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "V", modifiers: cmd) == .paste)
    }

    @Test func otherShortcutsAreLeftAlone() {
        #expect(Policy.editCommand(charactersIgnoringModifiers: "v", modifiers: []) == nil)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "v", modifiers: [.command, .option]) == nil)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "c", modifiers: [.command, .control]) == nil)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "v", modifiers: [.command, .shift]) == nil)
        // ⌘J, ⌘R, ⌘⏎ belong to the panel's own key router.
        #expect(Policy.editCommand(charactersIgnoringModifiers: "j", modifiers: .command) == nil)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "r", modifiers: .command) == nil)
        #expect(Policy.editCommand(charactersIgnoringModifiers: "\r", modifiers: .command) == nil)
    }

    // MARK: - Hot key

    @Test func hotKeyChordsAreCarbonsValues() {
        #expect(Policy.hotKeyChord(.off) == nil)
        let space = Policy.hotKeyChord(.controlOptionSpace)
        #expect(space?.keyCode == UInt32(kVK_Space))
        #expect(space?.modifiers == UInt32(controlKey | optionKey))
        let j = Policy.hotKeyChord(.optionCommandJ)
        #expect(j?.keyCode == UInt32(kVK_ANSI_J))
        #expect(j?.modifiers == UInt32(optionKey | cmdKey))
    }

    // MARK: - Sealed launch hook

    @Test func launchRequestsParse() {
        #expect(Policy.launchRequest("sessions") == .init(route: .sessions(ringID: nil), isAuto: false))
        #expect(Policy.launchRequest(" sessions:claude-work ") == .init(route: .sessions(ringID: "claude-work"), isAuto: false))
        #expect(Policy.launchRequest("session:abc-123") == .init(route: .session(id: "abc-123"), isAuto: false))
        #expect(Policy.launchRequest("setup") == .init(route: .setup, isAuto: false))
        #expect(Policy.launchRequest("auto:session:abc") == .init(route: .session(id: "abc"), isAuto: true))
        #expect(Policy.launchRequest("auto:sessions") == .init(route: .sessions(ringID: nil), isAuto: true))
    }

    @Test func malformedLaunchRequestsAreIgnored() {
        for value in ["", "panel", "session", "session:", "sessions:", "setup:x", "auto:", "auto:auto:sessions", "1"] {
            #expect(Policy.launchRequest(value) == nil, "\(value)")
        }
    }

    // MARK: - Settings requests

    @Test func aRequestWithNoListenerWaitsForTheFirstOne() {
        var latch = ClaudeRequestLatch<String>(lifetime: 5)
        let now = Date(timeIntervalSince1970: 1000)
        // The settings window is being built: nobody listens yet.
        #expect(latch.offer("claudeCode", at: now, hasListeners: false) == nil)
        // Its view subscribes a moment later and gets the request once.
        #expect(latch.take(at: now.addingTimeInterval(0.2)) == "claudeCode")
        #expect(latch.take(at: now.addingTimeInterval(0.3)) == nil)
    }

    @Test func aListenerGetsTheRequestAtOnce() {
        var latch = ClaudeRequestLatch<String>()
        let now = Date(timeIntervalSince1970: 1000)
        #expect(latch.offer("notifications", at: now, hasListeners: true) == "notifications")
        #expect(latch.take(at: now) == nil)
    }

    @Test func aStaleRequestIsDropped() {
        var latch = ClaudeRequestLatch<String>(lifetime: 5)
        let now = Date(timeIntervalSince1970: 1000)
        _ = latch.offer("claudeCode", at: now, hasListeners: false)
        // Settings opened much later, by hand: it must not jump to the old pane.
        #expect(latch.take(at: now.addingTimeInterval(6)) == nil)
    }

    @Test func theNewestRequestWins() {
        var latch = ClaudeRequestLatch<String>()
        let now = Date(timeIntervalSince1970: 1000)
        _ = latch.offer("claudeCode", at: now, hasListeners: false)
        _ = latch.offer("notifications", at: now.addingTimeInterval(0.1), hasListeners: false)
        #expect(latch.take(at: now.addingTimeInterval(0.2)) == "notifications")
    }

    // MARK: - Held requests (the settings pane selection)

    final class Clock {
        var now = Date(timeIntervalSince1970: 1000)
    }

    @Test func aRequestSentBeforeTheSettingsViewSubscribesReachesItOnce() {
        let clock = Clock()
        let requests = ClaudeHeldRequests<String>(lifetime: 5, clock: { clock.now })
        requests.send("claudeCode")
        clock.now += 0.3
        var first: [String] = []
        let a = requests.publisher.sink { first.append($0) }
        #expect(first == ["claudeCode"])
        #expect(requests.listeners == 1)
        // A second listener (another window) does not replay it.
        var second: [String] = []
        let b = requests.publisher.sink { second.append($0) }
        #expect(second.isEmpty)
        // Live requests reach both, at once.
        requests.send("notifications")
        #expect(first == ["claudeCode", "notifications"])
        #expect(second == ["notifications"])
        a.cancel()
        b.cancel()
        #expect(requests.listeners == 0)
    }

    @Test func aHeldRequestGoesStale() {
        let clock = Clock()
        let requests = ClaudeHeldRequests<String>(lifetime: 5, clock: { clock.now })
        requests.send("claudeCode")
        clock.now += 6
        var got: [String] = []
        let a = requests.publisher.sink { got.append($0) }
        #expect(got.isEmpty)
        a.cancel()
    }

    @Test func afterTheLastListenerLeavesRequestsAreHeldAgain() {
        let clock = Clock()
        let requests = ClaudeHeldRequests<String>(clock: { clock.now })
        var got: [String] = []
        let a = requests.publisher.sink { got.append($0) }
        a.cancel()
        requests.send("notifications")
        #expect(got.isEmpty)
        let b = requests.publisher.sink { got.append($0) }
        #expect(got == ["notifications"])
        b.cancel()
    }
}
