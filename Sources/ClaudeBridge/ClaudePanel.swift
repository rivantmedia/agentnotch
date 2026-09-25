import AppKit
import ClaudeControl

/// The sessions panel's window: borderless, non-activating, one level above
/// Codenotch's notch and its hover cards, on every space and over full-screen
/// apps (design §7).
///
/// Unlike Codenotch's `NotchPanel` it can become key, because the panel has
/// text to type (chat, "Other", "Answer…"). Being non-activating, it becomes
/// key without activating the app: the terminal stays the frontmost app, and
/// putting the panel away hands key back to it. `becomesKeyOnlyIfNeeded`
/// keeps clicks on rows and buttons from taking key; a text field asks for it.
///
/// Fork-only file. Owned by WP-D.
final class ClaudePanel: NSPanel {
    /// Esc reached the window: nothing in the content handled it.
    var onCancel: (() -> Void)?
    /// An editing shortcut went through the fallback below, and whether a
    /// responder took it.
    var onEditCommand: ((ClaudePanelPolicy.EditCommand, Bool) -> Void)?

    init(contentRect: NSRect) {
        super.init(contentRect: contentRect,
                   styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered,
                   defer: false)
        // Before the level: `isFloatingPanel` resets the level to `.floating`.
        isFloatingPanel = true
        // Above the notch (`.statusBar`) and its hover cards.
        level = NSWindow.Level(rawValue: NSWindow.Level.statusBar.rawValue + 1)
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = true
        isOpaque = false
        backgroundColor = .clear
        // The chrome draws the silhouette; a window shadow would outline the
        // rectangle around it, card and tail's empty corners included.
        hasShadow = false
        isMovable = false
        isMovableByWindowBackground = false
        isReleasedWhenClosed = false
        animationBehavior = .utilityWindow
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    /// Esc with nothing in the content taking it: back from the chat, then close.
    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }

    /// The editing shortcuts, answered here as well as by the main menu.
    ///
    /// A non-activating panel is key while another app is active, and then the
    /// main menu's key equivalents never fire: ⌘V would do nothing in the chat
    /// composer. Whatever the content handles itself (its own shortcuts) goes
    /// first; the rest of ⌘A ⌘C ⌘V ⌘X ⌘Z ⇧⌘Z is sent down the responder chain
    /// the way the Edit menu would send it.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if super.performKeyEquivalent(with: event) { return true }
        guard event.type == .keyDown, isKeyWindow,
              let command = ClaudePanelPolicy.editCommand(
                charactersIgnoringModifiers: event.charactersIgnoringModifiers ?? "",
                modifiers: Self.modifiers(event.modifierFlags))
        else { return false }
        let handled = NSApp.sendAction(Self.action(for: command), to: nil, from: self)
        onEditCommand?(command, handled)
        return handled
    }

    static func modifiers(_ flags: NSEvent.ModifierFlags) -> ClaudePanelPolicy.KeyModifiers {
        var modifiers: ClaudePanelPolicy.KeyModifiers = []
        if flags.contains(.command) { modifiers.insert(.command) }
        if flags.contains(.shift) { modifiers.insert(.shift) }
        if flags.contains(.option) { modifiers.insert(.option) }
        if flags.contains(.control) { modifiers.insert(.control) }
        return modifiers
    }

    /// The Edit menu's action for each command.
    static func action(for command: ClaudePanelPolicy.EditCommand) -> Selector {
        switch command {
        case .selectAll: return #selector(NSResponder.selectAll(_:))
        case .copy: return #selector(NSText.copy(_:))
        case .paste: return #selector(NSText.paste(_:))
        case .cut: return #selector(NSText.cut(_:))
        case .undo: return Selector(("undo:"))
        case .redo: return Selector(("redo:"))
        }
    }
}

/// The panel's content view: a plain container around the hosting view, so
/// SwiftUI never gets a say in the window's frame (the same arrangement as
/// `NotchContainerView`; the frame comes from `ClaudePanelGeometry` only).
/// It also reports the pointer arriving: an auto-opened panel is then the
/// user's, and stays until it is closed the ordinary ways.
final class ClaudePanelContainerView: NSView {
    var onPointerEntered: (() -> Void)?
    private var tracking: NSTrackingArea?

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero,
                                  options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area)
        tracking = area
    }

    override func mouseEntered(with event: NSEvent) {
        onPointerEntered?()
    }
}
