import AppKit
import ClaudeControl
import Combine
import Foundation

/// The only type AppDelegate talks to (seams U3, U4, U5). Everything the fork
/// adds to the notch hangs off `attach`: it wraps the fleet's callbacks
/// instead of editing them, so upstream's files stay as they are.
///
/// What it wires, all against ClaudeControl's public API:
/// - one usage ring per Claude account (`ClaudeUsageProvider`), added and
///   removed at runtime (`ClaudeProviderSync`);
/// - each ring's sessions in its arc and hover card (`ClaudeSessionFeed`),
///   and the store polling harder while any of them works (`isBusy`);
/// - a click on a Claude ring, a hover-card session row or a peek, routed to
///   the sessions panel or the session's terminal;
/// - the chime, peek, Dock badge and hold-open (`ClaudeAttentionReactions`),
///   and notification clicks opening the panel;
/// - Codenotch's nicknames and connected rings mirrored into the hub.
///
/// Fork-only file (design §2, §12). Owned by WP-C.
@MainActor
final class ClaudeBridge {
    static let shared = ClaudeBridge()

    private(set) var hub: ClaudeControlHub?
    /// The rings made at launch, before the store existed (U3).
    private var launched: [ClaudeUsageProvider] = []
    private var attached = false
    private var providerSync: ClaudeProviderSync?
    private var sessionFeed: ClaudeSessionFeed?
    private(set) var reactions: ClaudeAttentionReactions?
    private var cancellables = Set<AnyCancellable>()
    private var trace: (String) -> Void = { _ in }

    private init() {}

    /// Whether a provider id is one of ours: every Claude ring. Upstream's
    /// Claude providers are never created (U1), so every Claude id in the
    /// store is ours. Upstream's completion announcements skip these (U5);
    /// we announce them ourselves.
    nonisolated static func ownsProvider(_ id: String) -> Bool {
        ClaudeProfile.isClaude(providerID: id)
    }

    /// The menu bar summary's label for an account ring when several rings
    /// share Claude's mark (MBL): its nickname, else its short name
    /// ("Rivant" for "Claude Rivant"). Nil for any other ring, which keeps
    /// upstream's rule (the profile's folder slug); an account ring's id is
    /// a hash, which reads as nothing.
    static func menuBarLabel(providerID: String) -> String? {
        guard providerID.hasPrefix("claude-acct-") else { return nil }
        let name = shared.hub?.accounts.first { $0.ringID == providerID }?.label
            ?? ClaudeRingNames.shared.name(for: providerID)
        return shortMenuBarLabel(name)
    }

    /// "Claude Rivant" → "Rivant"; a name of its own stays as it is. Pure.
    nonisolated static func shortMenuBarLabel(_ name: String) -> String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("Claude "), trimmed.count > 7 else { return trimmed.isEmpty ? "Claude" : trimmed }
        return String(trimmed.dropFirst(7))
    }

    /// The Claude rings to create at launch, before the store exists (U3):
    /// one per account ClaudeControl knows from its own files and a folder
    /// listing. Nothing is probed or read from a Claude config here.
    func launchUsageProviders() -> [UsageProvider] {
        let hub = bootstrap()
        // Per-folder rings of earlier versions become account rings: their
        // archived readings move before the store reads the archive.
        if !Fork.isSealed { ClaudeRingMigrator.migrateArchive(hub: hub) }
        let accounts = hub.launchAccounts()
        ClaudeRingNames.shared.update(accounts)
        launched = ClaudeProviderSync.ringIDs(accounts).map { ClaudeUsageProvider(ringID: $0) }
        return launched
    }

    /// Wire the engine to the notch (U4). Runs once, after every fleet
    /// callback is set and before the first controller exists.
    func attach(fleet: NotchFleet, store: UsageStore?, preferences: Preferences) {
        guard !attached else { return }
        attached = true
        let hub = bootstrap()

        // `--snapshot-claude <dir>` (sealed only): render the notch from the
        // fixtures and quit, before any controller exists.
        if let directory = ClaudeNotchSnapshots.requestedDirectory(arguments: CommandLine.arguments) {
            ClaudeNotchSnapshots.renderAndExit(into: directory, hub: hub, preferences: preferences)
        }

        if Fork.isSealed { trace = ClaudeSealedDemo.trace }
        // Services (live) or fixtures (sealed). Live, this also makes the
        // engine the notification centre's delegate: banners it posts open
        // the panel through `panelRequests`, and Codenotch's own alerts pass
        // through untouched.
        hub.start()
        ClaudeNotchState.shared.follow(hub)
        ClaudePanelController.shared.configure(fleet: fleet, preferences: preferences, hub: hub)

        // Sealed runs have no store of their own (the app builds none); they
        // get one over the fixtures, so the rings go through the same path.
        if Fork.isSealed, launched.isEmpty {
            launched = ClaudeProviderSync.ringIDs(hub.launchAccounts()).map { ClaudeUsageProvider(ringID: $0) }
        }
        let usageStore = store ?? (Fork.isSealed
            ? ClaudeSealedDemo.makeStore(claude: launched, fleet: fleet, preferences: preferences)
            : nil)
        if !Fork.isSealed {
            // Nicknames, on/off and order of the per-folder rings of earlier
            // versions move to the account rings (once per old ring).
            let sources = hub.ringMigrationSources()
            ClaudeRingMigrator.migratePreferences(accounts: sources.accounts, retired: sources.retired,
                                                  preferences: preferences, store: usageStore,
                                                  log: { Log.usage.info("\($0, privacy: .public)") })
        }
        if let usageStore {
            let sync = ClaudeProviderSync(
                hub: hub, store: usageStore, preferences: preferences,
                writesPreferences: !Fork.isSealed || ClaudeSealedDemo.ownsPreferencesDomain,
                log: trace)
            sync.adopt(launched)
            sync.start()
            providerSync = sync
            // Poll harder while a Claude session the notch shows works, as
            // upstream did for its own Claude monitor. The notch's own totals
            // (`ClaudeNotchState`), not the hub's: a ring switched off doesn't
            // speed up polling (CS-9).
            let upstreamBusy = usageStore.isBusy
            usageStore.isBusy = { upstreamBusy() || ClaudeNotchState.shared.totalCounts.working > 0 }
        }

        // Codenotch's names for the rings, for the panel and the banners.
        preferences.$accountNicknames
            .map { $0.filter { ClaudeBridge.ownsProvider($0.key) } }
            .removeDuplicates()
            .sink { [weak hub] in hub?.setNicknames($0) }
            .store(in: &cancellables)

        let feed = ClaudeSessionFeed(hub: hub, fleet: fleet, state: .shared) { [weak preferences] ring in
            preferences?.isConnected(ring) ?? true
        }
        feed.start(alsoOn: preferences.$connectedProviders.map { _ in () }.eraseToAnyPublisher())
        sessionFeed = feed

        wrapSessionClicks(fleet)
        wrapRingClicks(fleet)

        let reactions = ClaudeAttentionReactions(
            hub: hub, fleet: fleet, preferences: preferences, state: .shared, settings: .shared,
            playsSounds: !Fork.isSealed, log: trace)
        reactions.start()
        self.reactions = reactions

        // A banner clicked, or anything else asking for the panel.
        hub.panelRequests
            .receive(on: DispatchQueue.main)
            .sink { route in ClaudePanelController.shared.open(route, reason: .notification) }
            .store(in: &cancellables)

        NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)
            .sink { [weak self] _ in
                MainActor.assumeIsolated { self?.stop() }
            }
            .store(in: &cancellables)

        if Fork.isSealed {
            ClaudeSealedDemo.start(hub: hub, fleet: fleet, preferences: preferences, store: usageStore)
        }
    }

    private func stop() {
        reactions?.stop()
        sessionFeed?.stop()
        providerSync?.stop()
        // The system-wide hot key goes with the app (CS-8).
        ClaudeHotKey.shared.stop()
        hub?.stop()
    }

    // MARK: - Engine

    private func bootstrap() -> ClaudeControlHub {
        if let hub { return hub }
        let bundleID = Bundle.main.bundleIdentifier ?? Fork.bundleID
        let configuration: ClaudeControlConfiguration
        if Fork.isSealed {
            configuration = .sealed(appDisplayName: Fork.displayName, bundleIdentifier: bundleID)
        } else {
            var live = ClaudeControlConfiguration.live(
                appDisplayName: Fork.displayName,
                bundleIdentifier: bundleID,
                supportFolderName: Fork.applicationSupportFolder,
                environment: ProcessInfo.processInfo.environment,
                arguments: CommandLine.arguments)
            // What Codenotch can do that ClaudeControl cannot: select a tab in
            // the terminals it scripts itself, and read Claude Desktop's
            // cached usage. Neither reads a token.
            live.externalTabFocus = CodenotchTabFocus.select
            live.externalUsageSource = DesktopUsageSource.shared
            configuration = live
        }
        let hub = ClaudeControlHub.bootstrap(configuration)
        self.hub = hub
        return hub
    }

    // MARK: - Clicks

    /// A session row in a hover card, or a click on a peek (U10b), for a
    /// Claude session: `.smart` sends one that needs you to the panel, where
    /// it can be answered, and anything else to its terminal (which marks it
    /// reviewed). Other agents' sessions go wherever they went before.
    private func wrapSessionClicks(_ fleet: NotchFleet) {
        let upstream = fleet.onFocusSession
        fleet.onFocusSession = { [weak self] pid in
            MainActor.assumeIsolated { self?.focusSession(pid: pid, upstream: upstream) }
        }
    }

    private func focusSession(pid: pid_t, upstream: ((pid_t) -> Void)?) {
        guard let hub, let session = hub.session(pid: Int32(pid)) else {
            if let upstream {
                upstream(pid)
            } else if !Fork.isSealed {
                Task { _ = await SessionFocus.focus(pid: pid) }
            }
            return
        }
        let reason: ClaudePanelController.OpenReason = reactions?.answersPeek(pid: pid) == true ? .peekClick : .hoverRow
        let target = ClaudeAttentionPolicy.sessionClick(session.attention, setting: ClaudeControlSettings.sessionClick)
        trace("session click \(session.id) (\(reason)): \(target)")
        switch target {
        case .panel:
            ClaudePanelController.shared.open(.session(id: session.id), reason: reason)
        case .terminal:
            Task { @MainActor in
                guard !(await hub.focus(sessionId: session.id)) else { return }
                // No terminal to jump to (it closed, or can't be found): the
                // panel still has the session.
                Log.sessions.info("claude session \(session.id, privacy: .public): no terminal to focus; opening the panel")
                ClaudePanelController.shared.open(.session(id: session.id), reason: reason)
            }
        }
    }

    /// A click on a Claude ring opens the sessions panel for that ring (or
    /// closes it, or switches to it), or — set to refresh — asks for fresh
    /// usage and then refetches the ring the way any other ring's click does.
    /// Other rings refetch as before.
    private func wrapRingClicks(_ fleet: NotchFleet) {
        let upstream = fleet.onRefreshProvider
        fleet.onRefreshProvider = { [weak self] id in
            guard ClaudeBridge.ownsProvider(id), let self else {
                await upstream?(id)
                return
            }
            await self.ringClicked(id, upstream: upstream)
        }
    }

    private func ringClicked(_ ringID: String, upstream: ((String) async -> Void)?) async {
        switch ClaudeControlSettings.ringClick {
        case .openPanel:
            trace("ring click \(ringID): panel")
            ClaudePanelController.shared.toggle(ringID: ringID, at: NSEvent.mouseLocation)
        case .refresh:
            trace("ring click \(ringID): refresh")
            guard let hub else { await upstream?(ringID); return }
            // The probe (only when the reading is over two minutes old) and
            // the store's refetch together: the ring presses at once, and a
            // newer reading lands through the sync when the probe answers.
            async let probe: Void = hub.refreshUsage(ringID: ringID, reason: .ringClick)
            await upstream?(ringID)
            await probe
        }
    }
}
