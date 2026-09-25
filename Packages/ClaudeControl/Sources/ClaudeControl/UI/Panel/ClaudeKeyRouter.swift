//
//  ClaudeKeyRouter.swift
//  ClaudeControl
//
//  Every key the sessions panel answers, as one pure table: a key, its
//  modifiers and what is on screen in, a command out. The panel's views only
//  translate SwiftUI key presses into `Key`s and carry out the `Command`, so
//  the whole keyboard map is unit-tested here.
//
//  | Key   | Action                                             |
//  |-------|----------------------------------------------------|
//  | ↑ / ↓ | select a row                                       |
//  | ⏎     | open the chat (in the composer: send)              |
//  | ⌘⏎    | primary action: Allow / Approve plan / Mark reviewed |
//  | ⌥⌘⏎   | Always allow                                       |
//  | ⌘⌫    | Deny (a plan: keep planning)                        |
//  | 1–4   | answer with that option                            |
//  | ⌘J    | jump to the terminal                               |
//  | ⌘R    | mark reviewed                                      |
//  | ⌘⇧R   | mark all reviewed                                  |
//  | Esc   | back, then close                                   |
//
//  Approvals never fire on a bare key: an accidental ⏎ opens a chat, it
//  doesn't allow a command.
//

import Foundation

nonisolated enum ClaudeKeyRouter {
    enum Key: Hashable, Sendable {
        case up, down, returnKey, delete, escape
        case character(Character)
    }

    struct Modifiers: OptionSet, Hashable, Sendable {
        let rawValue: Int
        static let command = Modifiers(rawValue: 1 << 0)
        static let option = Modifiers(rawValue: 1 << 1)
        static let shift = Modifiers(rawValue: 1 << 2)
        static let control = Modifiers(rawValue: 1 << 3)
    }

    /// The session a key acts on: the selected row, or the open chat's.
    struct Target: Equatable, Sendable {
        let sessionId: String
        var actions: SessionPrimaryActions
        /// Ready for review, so "mark reviewed" means something.
        var canMarkReviewed: Bool
        /// Its terminal can be brought to the front.
        var canJump: Bool
        /// Its turn failed: ⌘R dismisses the failure (never ⌘Return).
        var canDismissFailure = false

        init(sessionId: String, actions: SessionPrimaryActions, canMarkReviewed: Bool, canJump: Bool) {
            self.sessionId = sessionId
            self.actions = actions
            self.canMarkReviewed = canMarkReviewed
            self.canJump = canJump
        }
    }

    enum Context: Equatable, Sendable {
        /// The session list, with the keyboard selection if there is one.
        case list(selection: Target?)
        /// One session's chat; `isTyping` while the composer has focus.
        case chat(Target, isTyping: Bool)
        /// The setup card.
        case setup
    }

    enum Command: Equatable, Sendable {
        case moveSelection(by: Int)
        case openChat(sessionId: String)
        case allow(sessionId: String, toolUseId: String)
        case alwaysAllow(sessionId: String, toolUseId: String)
        case deny(sessionId: String, toolUseId: String)
        case approvePlan(sessionId: String, toolUseId: String)
        case keepPlanning(sessionId: String, toolUseId: String)
        case chooseOption(sessionId: String, toolUseId: String, index: Int)
        case markReviewed(sessionId: String)
        case markAllReviewed
        case jump(sessionId: String)
        case back
        case close
    }

    /// The command for a key press, or nil to let it through (typing, keys
    /// that mean nothing here).
    static func command(for key: Key, modifiers: Modifiers, in context: Context) -> Command? {
        let mods = modifiers.subtracting(.control)
        if key == .escape {
            if case .chat = context { return .back }
            return .close
        }

        let target: Target?
        let inChat: Bool
        switch context {
        case .setup:
            return nil
        case .list(let selection):
            target = selection
            inChat = false
        case .chat(let chatTarget, let isTyping):
            // The composer owns plain keys, digits and ⏎ included.
            if isTyping, !mods.contains(.command) { return nil }
            target = chatTarget
            inChat = true
        }

        switch (key, mods) {
        case (.character("r"), [.command, .shift]), (.character("R"), [.command, .shift]):
            return .markAllReviewed
        case (.character("r"), .command):
            guard let target, target.canMarkReviewed || target.canDismissFailure else { return nil }
            return .markReviewed(sessionId: target.sessionId)
        case (.character("j"), .command):
            guard let target, target.canJump else { return nil }
            return .jump(sessionId: target.sessionId)
        case (.returnKey, .command):
            guard let target else { return nil }
            return primary(for: target, inChat: inChat)
        case (.returnKey, [.command, .option]):
            // From the list only what the row offers: a narrow rule, for a
            // request short enough to be read there whole.
            guard let target, case let .permission(toolUseId, always?, needsReview) = target.actions,
                  inChat || (always.isInline && !needsReview) else { return nil }
            return .alwaysAllow(sessionId: target.sessionId, toolUseId: toolUseId)
        case (.delete, .command):
            guard let target else { return nil }
            switch target.actions {
            case .permission(let toolUseId, _, _):
                return .deny(sessionId: target.sessionId, toolUseId: toolUseId)
            case .plan(let toolUseId):
                return .keepPlanning(sessionId: target.sessionId, toolUseId: toolUseId)
            default:
                return nil
            }
        default:
            break
        }

        guard !inChat, mods.isEmpty else { return nil }
        switch key {
        case .up: return .moveSelection(by: -1)
        case .down: return .moveSelection(by: 1)
        case .returnKey:
            return target.map { .openChat(sessionId: $0.sessionId) }
        case .character(let character):
            guard let digit = character.wholeNumberValue, (1...ChatQuestion.maxInlineOptions).contains(digit),
                  let target, case let .questionChips(toolUseId, question) = target.actions,
                  digit <= question.options.count else { return nil }
            return .chooseOption(sessionId: target.sessionId, toolUseId: toolUseId, index: digit - 1)
        default:
            return nil
        }
    }

    /// ⌘⏎: the action the row leads with.
    private static func primary(for target: Target, inChat: Bool) -> Command? {
        switch target.actions {
        case let .permission(toolUseId, _, needsReview):
            // Too long to judge from the row: open it whole first.
            if needsReview && !inChat { return .openChat(sessionId: target.sessionId) }
            return .allow(sessionId: target.sessionId, toolUseId: toolUseId)
        case .plan(let toolUseId):
            return .approvePlan(sessionId: target.sessionId, toolUseId: toolUseId)
        case .questionChips, .answerInChat:
            return inChat ? nil : .openChat(sessionId: target.sessionId)
        case .none, .answerInTerminal:
            return target.canMarkReviewed ? .markReviewed(sessionId: target.sessionId) : nil
        }
    }

    /// The selection after moving `delta` rows through `order` (the list as
    /// displayed): the first (or last) row when nothing is selected yet,
    /// stopping at the ends rather than wrapping.
    static func moved(_ selection: String?, by delta: Int, in order: [String]) -> String? {
        guard !order.isEmpty else { return nil }
        guard let selection, let index = order.firstIndex(of: selection) else {
            return delta >= 0 ? order.first : order.last
        }
        let next = min(max(index + delta, 0), order.count - 1)
        return order[next]
    }
}
