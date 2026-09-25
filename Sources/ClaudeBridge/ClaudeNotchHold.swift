import AppKit
import ClaudeControl
import Combine

/// When the policy is chosen: which notches hold open while a session needs you.
enum ClaudeHoldPolicy { case auto, always, never }

extension ClaudeHoldPolicy {
    /// The same choice, as the package's settings and policy name it.
    var holdOpenPolicy: HoldOpenPolicy {
        switch self {
        case .auto: return .auto
        case .always: return .always
        case .never: return .never
        }
    }

    init(_ policy: HoldOpenPolicy) {
        switch policy {
        case .auto: self = .auto
        case .always: self = .always
        case .never: self = .never
        }
    }
}

/// The two ways the fork keeps a notch open without the pointer on it
/// (design §6, §7), in one place because both go through state upstream's
/// visibility setting also writes:
///
/// - **While a session needs you**, per `ClaudePanelPolicy.holdsNotchOpen`:
///   the notch is shown as if "Always show" were on (`apply(.alwaysShow)`),
///   so "fold for full screen" still folds it. Releasing re-applies the
///   user's own visibility.
/// - **While the sessions panel is open**, its anchor notch is pinned
///   (`togglePinned()`) so the ring the tail points at stays out. A notch the
///   user had pinned already is left alone, and stays pinned after.
///
/// Applying a visibility clears a notch's pin (upstream: "any pin made by
/// hand is subsumed by the setting"), so every apply here re-pins the panel's
/// anchor, and a visibility change made in Settings (which reaches every
/// notch) is followed by re-applying the holds and the pin.
///
/// Fork-only file. Owned by WP-D. WP-C calls `setNeedsYou`; the panel
/// controller calls `setPanel`.
@MainActor
final class ClaudeNotchHold {
    static let shared = ClaudeNotchHold()

    private(set) var needsYou = false
    private(set) var policy: ClaudeHoldPolicy = .auto

    private weak var fleet: NotchFleet?
    private weak var preferences: Preferences?
    private var cancellables = Set<AnyCancellable>()
    /// The notches this holds open. Weak: a notch the fleet retires drops
    /// out by itself, and one it builds in its place (often at the same
    /// address, which an `ObjectIdentifier` would mistake for the old one)
    /// is not taken for held.
    private let held = NSHashTable<NotchWindowController>.weakObjects()
    /// The panel's anchor notch, and whether the pin on it is the panel's.
    private weak var anchor: NotchWindowController?
    private(set) var pinnedForPanel = false

    private init() {}

    /// Called once from `ClaudePanelController.configure`.
    func configure(fleet: NotchFleet, preferences: Preferences) {
        self.fleet = fleet
        self.preferences = preferences
        cancellables.removeAll()

        // Upstream applies a new visibility to every notch, which drops the
        // holds and the pin; apply them again once it has (its sink is on the
        // main run loop too and was subscribed first, and the extra hop puts
        // this after it whatever the order).
        preferences.$notchVisibility
            .dropFirst()
            .removeDuplicates()
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in
                DispatchQueue.main.async { self?.visibilityWasReset() }
            }
            .store(in: &cancellables)

        // A display plugged in or out, or the edge moving between the camera
        // and a side, changes which notches exist and which are flush with
        // the hardware. After the fleet and the notches have settled.
        NotificationCenter.default.publisher(for: NSApplication.didChangeScreenParametersNotification)
            .merge(with: preferences.$notchEdge.dropFirst().removeDuplicates().map { _ in
                Notification(name: NSApplication.didChangeScreenParametersNotification)
            })
            .debounce(for: .seconds(0.4), scheduler: RunLoop.main)
            .sink { [weak self] _ in self?.reconcile() }
            .store(in: &cancellables)
    }

    /// Whether any session needs you, and the hold-open setting (WP-C).
    func setNeedsYou(_ on: Bool, policy: ClaudeHoldPolicy) {
        guard on != needsYou || policy != self.policy else { return }
        needsYou = on
        self.policy = policy
        reconcile()
    }

    /// The panel opened on `anchor`, moved to another notch, or closed (nil).
    func setPanel(anchor controller: NotchWindowController?) {
        guard controller !== anchor else { return }
        releasePin()
        guard let controller else { return }
        anchor = controller
        pinnedForPanel = !controller.model.isPinned
        if pinnedForPanel { controller.togglePinned() }
    }

    /// Hold or release each notch to match the policy, and keep the panel's pin.
    func reconcile() {
        guard let fleet else { return }
        let visibility = userVisibility
        for controller in fleet.claudeControllers {
            let wanted = ClaudePanelPolicy.holdsNotchOpen(
                policy: policy.holdOpenPolicy,
                needsYou: needsYou,
                isFlushWithHardware: controller.model.isFlushWithHardware,
                userHidesNotch: visibility == .hidden,
                userAlwaysShowsNotch: visibility == .alwaysShow
            )
            if wanted, !held.contains(controller) {
                held.add(controller)
                controller.apply(.alwaysShow)
                Log.sessions.info("claude hold: holding a notch open (\(String(describing: self.policy), privacy: .public))")
            } else if !wanted, held.contains(controller) {
                held.remove(controller)
                controller.apply(visibility)
                controller.cursorMoved()
                Log.sessions.info("claude hold: released a notch to \(visibility.rawValue, privacy: .public)")
            }
        }
        repinAnchor()
    }

    // MARK: - Private

    private var userVisibility: NotchVisibility {
        preferences?.notchVisibility ?? .onHover
    }

    private func visibilityWasReset() {
        // Every notch now shows the user's choice; none is held any more.
        held.removeAllObjects()
        reconcile()
    }

    private func repinAnchor() {
        // A notch the user just hid stays hidden: pinning would unfold a
        // window that is ordered out. The panel floats off it next.
        guard let anchor, pinnedForPanel, !anchor.model.isPinned, userVisibility != .hidden else { return }
        anchor.togglePinned()
    }

    private func releasePin() {
        guard let controller = anchor else { return }
        if pinnedForPanel, controller.model.isPinned { controller.togglePinned() }
        anchor = nil
        pinnedForPanel = false
        // The ordinary hover fold takes over from here.
        controller.cursorMoved()
    }
}
