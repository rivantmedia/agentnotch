//
//  PanelKeys.swift
//  ClaudeControl
//
//  Wires the keyboard to `ClaudeKeyRouter`. Plain keys (arrows, ⏎, digits,
//  Esc) arrive through `.onKeyPress` on the focused panel; ⌘ combinations
//  arrive as key equivalents, which the window offers every view before the
//  focused one, so they work while the composer has the focus too. Either
//  way the router decides, so the table in ClaudeKeyRouter is the whole
//  keyboard map.
//

import SwiftUI

struct PanelKeys: ViewModifier {
    let context: ClaudeKeyRouter.Context
    let perform: (ClaudeKeyRouter.Command) -> Void

    @FocusState private var isFocused: Bool

    /// The ⌘ combinations the router knows, as key equivalents.
    private static let commandShortcuts: [(KeyEquivalent, EventModifiers, ClaudeKeyRouter.Key)] = [
        (.return, [.command], .returnKey),
        (.return, [.command, .option], .returnKey),
        (.delete, [.command], .delete),
        ("j", [.command], .character("j")),
        ("r", [.command], .character("r")),
        ("r", [.command, .shift], .character("r")),
    ]

    func body(content: Content) -> some View {
        content
            .focusable()
            .focusEffectDisabled()
            .focused($isFocused)
            .onKeyPress(phases: .down) { press in
                guard let key = Self.key(for: press) else { return .ignored }
                guard let command = ClaudeKeyRouter.command(for: key, modifiers: Self.modifiers(press.modifiers), in: context) else {
                    return .ignored
                }
                perform(command)
                return .handled
            }
            .background {
                ForEach(Array(Self.commandShortcuts.enumerated()), id: \.offset) { index, shortcut in
                    let command = Self.shortcutCommand(index, in: context)
                    Button("") {
                        if let command { perform(command) }
                    }
                    .keyboardShortcut(shortcut.0, modifiers: shortcut.1)
                    // Nothing to do here: leave the key to the focused view,
                    // so ⌘⌫ still deletes to the start of the line in a field.
                    .disabled(command == nil)
                    .buttonStyle(.plain)
                    .frame(width: 0, height: 0)
                    .opacity(0)
                    .accessibilityHidden(true)
                }
            }
            .onAppear { isFocused = true }
    }

    /// What the `index`th ⌘ shortcut does in `context`; nil leaves the key
    /// to whatever has the focus.
    static func shortcutCommand(_ index: Int, in context: ClaudeKeyRouter.Context) -> ClaudeKeyRouter.Command? {
        let shortcut = commandShortcuts[index]
        return ClaudeKeyRouter.command(for: shortcut.2, modifiers: modifiers(shortcut.1), in: context)
    }

    /// How many ⌘ shortcuts the panel registers.
    static var shortcutCount: Int { commandShortcuts.count }

    static func key(for press: KeyPress) -> ClaudeKeyRouter.Key? {
        switch press.key {
        case .upArrow: return .up
        case .downArrow: return .down
        case .return: return .returnKey
        case .delete, .deleteForward: return .delete
        case .escape: return .escape
        default:
            guard let character = press.characters.first else { return nil }
            return .character(character)
        }
    }

    static func modifiers(_ modifiers: EventModifiers) -> ClaudeKeyRouter.Modifiers {
        var result: ClaudeKeyRouter.Modifiers = []
        if modifiers.contains(.command) { result.insert(.command) }
        if modifiers.contains(.option) { result.insert(.option) }
        if modifiers.contains(.shift) { result.insert(.shift) }
        if modifiers.contains(.control) { result.insert(.control) }
        return result
    }
}
