import AppKit
import Carbon.HIToolbox
import ClaudeControl

/// The sessions panel's global shortcut (`claudeControl.hotKey`: off, ⌃⌥Space
/// or ⌥⌘J; off by default).
///
/// Carbon's `RegisterEventHotKey`, which needs no Accessibility or Input
/// Monitoring permission and consumes only its own chord, rather than a global
/// key monitor, which needs both and sees every key typed anywhere.
///
/// Fork-only file (design §7). Owned by WP-D.
@MainActor
final class ClaudeHotKey {
    static let shared = ClaudeHotKey()

    /// 'AGENTNOTCH': tells our hot key apart from anyone else's in the handler.
    private static let signature: OSType = 0x5350_434E

    private var registered: PanelHotKey = .off
    private var hotKeyRef: EventHotKeyRef?
    private var handlerRef: EventHandlerRef?
    private var onPress: (() -> Void)?
    private var defaultsObserver: NSObjectProtocol?

    private init() {}

    /// Register the chosen shortcut now and whenever the setting changes.
    /// `onPress` runs on the main actor.
    func start(onPress: @escaping () -> Void) {
        self.onPress = onPress
        installHandlerIfNeeded()
        apply(ClaudeControlSettings.hotKey)
        if defaultsObserver == nil {
            // The settings pane writes the choice straight to the defaults.
            defaultsObserver = NotificationCenter.default.addObserver(
                forName: UserDefaults.didChangeNotification, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.apply(ClaudeControlSettings.hotKey) }
            }
        }
    }

    func stop() {
        if let defaultsObserver { NotificationCenter.default.removeObserver(defaultsObserver) }
        defaultsObserver = nil
        apply(.off)
        if let handlerRef { RemoveEventHandler(handlerRef) }
        handlerRef = nil
        onPress = nil
    }

    // MARK: - Registration

    private func apply(_ choice: PanelHotKey) {
        guard choice != registered else { return }
        if let hotKeyRef { UnregisterEventHotKey(hotKeyRef) }
        hotKeyRef = nil
        registered = .off
        guard let chord = ClaudePanelPolicy.hotKeyChord(choice) else { return }
        var ref: EventHotKeyRef?
        let status = RegisterEventHotKey(chord.keyCode, chord.modifiers,
                                         EventHotKeyID(signature: Self.signature, id: 1),
                                         GetApplicationEventTarget(), 0, &ref)
        guard status == noErr, let ref else {
            // Another app holds the chord; the panel stays reachable from the rings.
            Log.sessions.error("claude hot key: \(choice.displayName, privacy: .public) is taken (\(status, privacy: .public))")
            return
        }
        hotKeyRef = ref
        registered = choice
        Log.sessions.info("claude hot key: \(choice.displayName, privacy: .public)")
    }

    private func installHandlerIfNeeded() {
        guard handlerRef == nil else { return }
        var pressed = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, event, _ in
            var id = EventHotKeyID()
            let status = GetEventParameter(event, EventParamName(kEventParamDirectObject),
                                           EventParamType(typeEventHotKeyID), nil,
                                           MemoryLayout<EventHotKeyID>.size, nil, &id)
            guard status == noErr, id.signature == ClaudeHotKey.signature else {
                return OSStatus(eventNotHandledErr)
            }
            // Carbon calls the application target's handlers on the main thread.
            MainActor.assumeIsolated { ClaudeHotKey.shared.onPress?() }
            return noErr
        }, 1, &pressed, nil, &handlerRef)
    }
}
