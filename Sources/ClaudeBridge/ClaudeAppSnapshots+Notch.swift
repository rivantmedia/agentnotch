import AppKit
@_spi(Sealed) import ClaudeControl
import SwiftUI

/// The notch half of `--snapshot-claude <dir>` (design §11), and the sealed
/// run's `SPCN_SEALED_CAPTURE` captures: Codenotch's own `NotchRootView`
/// drawn offscreen with the Claude rings on it.
///
/// `--snapshot-claude` renders the sealed fixtures with the demo's first two
/// steps applied, so the three Claude rings show the three arcs (amber
/// needs-you, white working, green review):
/// - `notch-right-two-accounts.png`, `notch-top-two-accounts.png`: the
///   fixtures before any demo step: two accounts spread over `~/.claude`,
///   three VS Code windows and three Claude Parallel Profiles stores, so two
///   rings;
/// - `notch-<edge>-open.png` and `notch-<edge>-card.png` (hovering the work
///   ring) for right, left, top and bottom, with the badges;
/// - `notch-top-camera-open.png` / `-card.png`: the rings either side of a
///   MacBook camera, where the badges are dots;
/// - `notch-<edge>-folded.png`: the folded pill with its dots, and
///   `notch-top-camera-folded.png`, where the fold is the cutout itself and
///   draws nothing;
/// - `ring-badges.png`: the badges up close, every edge and the camera
///   strip, with counts past nine;
/// - `ring-settle.png`: the green arc pulsing (just finished) and steady.
///
/// Solid surface, because the system glass does not draw offscreen. Pulses
/// are caught wherever they are when the picture is taken.
///
/// After the notch sheets it renders the panel half, WP-D's
/// `ClaudeAppSnapshots+Panel` (`panel-chrome-*.png`).
///
/// Fork-only file. Owned by WP-C.
@MainActor
enum ClaudeNotchSnapshots {
    static let flag = "--snapshot-claude"
    /// The same, for a launch that passes no arguments through
    /// (`SPCN_SNAPSHOT_CLAUDE=<dir> Scripts/spm-run-sealed.sh …`).
    static let environmentKey = "SPCN_SNAPSHOT_CLAUDE"

    /// The folder after `--snapshot-claude` (or in `SPCN_SNAPSHOT_CLAUDE`),
    /// or nil when snapshots were not asked for.
    ///
    /// The environment is read only in a sealed run. A live app started from
    /// a shell that still exports the variable from an earlier snapshot run
    /// must start normally, not quit; the flag, which is typed on purpose, is
    /// answered either way (`renderAndExit` says it needs a sealed run).
    static func requestedDirectory(
        arguments: [String],
        environment: [String: String] = ProcessInfo.processInfo.environment,
        sealed: Bool = Fork.isSealed
    ) -> URL? {
        for (index, argument) in arguments.enumerated() {
            if argument == flag, arguments.indices.contains(index + 1) {
                return URL(fileURLWithPath: arguments[index + 1], isDirectory: true)
            }
            if argument.hasPrefix(flag + "=") {
                return URL(fileURLWithPath: String(argument.dropFirst(flag.count + 1)), isDirectory: true)
            }
        }
        if sealed, let path = environment[environmentKey], !path.isEmpty {
            return URL(fileURLWithPath: path, isDirectory: true)
        }
        return nil
    }

    /// Render everything into `directory` and quit. Sealed runs only: it
    /// needs the fixtures, and must never draw anyone's real sessions.
    static func renderAndExit(into directory: URL, hub: ClaudeControlHub, preferences: Preferences) -> Never {
        guard Fork.isSealed else {
            FileHandle.standardError.write(Data("\(flag) needs a sealed run (SPCN_SAFE_MODE=1)\n".utf8))
            exit(2)
        }
        do {
            let written = try render(into: directory, hub: hub, preferences: preferences)
            for url in written { print(url.path) }
            exit(0)
        } catch {
            FileHandle.standardError.write(Data("\(flag) failed: \(error.localizedDescription)\n".utf8))
            exit(1)
        }
    }

    static func render(into directory: URL, hub: ClaudeControlHub, preferences: Preferences) throws -> [URL] {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let started = Date()
        func note(_ what: String) {
            FileHandle.standardError.write(Data(String(format: "[spcn-snapshot] %@ at +%.2fs\n", what,
                                                       Date().timeIntervalSince(started)).utf8))
        }
        hub.start()
        // The fixtures arrive a few turns of the run loop after `start`.
        spin(until: { !hub.sessions.isEmpty && hub.ringReadings.count >= 2 })
        note("fixtures (\(hub.sessions.count) sessions)")
        var written: [URL] = []
        func write(_ image: CGImage?, _ name: String) throws {
            let url = directory.appendingPathComponent(name)
            try writePNG(image, to: url)
            written.append(url)
        }
        // The fixtures as they are: two accounts laid out the way Claude
        // Parallel Profiles lays them out (~/.claude, three VS Code windows,
        // three stores, the shared history), so two rings.
        let accounts = Fixture(hub: hub, preferences: preferences)
        note("fixture rings \(ClaudeProviderSync.ringIDs(hub.accounts))")
        ClaudeNotchState.shared.show(ringCounts: hub.ringCounts, freshSuccessUntil: hub.freshSuccessUntil)
        for edge in [NotchEdge.right, .top] {
            try write(image(of: accounts.model(edge: edge, expanded: true, now: Date())), "notch-\(edge.rawValue)-two-accounts.png")
        }
        for step in [ClaudeControlHub.SealedDemoStep.answerWorkPrompts, .addThirdAccount] {
            let done = Flag()
            Task { @MainActor in
                await hub.runSealedDemoStep(step)
                done.isSet = true
            }
            spin(until: { done.isSet })
        }
        note("demo steps applied")
        // The steps have landed once the hub shows the third ring's sessions
        // and nothing on the work ring still asks.
        spin(until: {
            Set(hub.sessions.map(\.ringID)).count >= 3 && hub.ringReadings.count >= 3
                && !hub.sessions.contains { session in
                    guard session.ringID == ClaudeFixtureRings.work, case .needsInput = session.attention else { return false }
                    return true
                }
        })
        note("rings \(Set(hub.sessions.map(\.ringID)).sorted()), readings \(hub.ringReadings.keys.sorted()), "
             + "work asks: \(hub.sessions.filter { $0.ringID == ClaudeFixtureRings.work && { if case .needsInput = $0 { return true }; return false }($0.attention) }.map(\.id))")

        let fixture = Fixture(hub: hub, preferences: preferences)
        let now = Date()
        // Pulsing where something just finished, as the fixtures stand.
        ClaudeNotchState.shared.show(ringCounts: hub.ringCounts, freshSuccessUntil: hub.freshSuccessUntil)

        let workIndex = fixture.snapshots.firstIndex { $0.id == ClaudeFixtureRings.work }

        for edge in [NotchEdge.right, .left, .top, .bottom] {
            let open = fixture.model(edge: edge, expanded: true, now: now)
            try write(image(of: open), "notch-\(edge.rawValue)-open.png")
            let card = fixture.model(edge: edge, expanded: true, hovered: workIndex, now: now)
            try write(image(of: card), "notch-\(edge.rawValue)-card.png")
            let folded = fixture.model(edge: edge, expanded: false, now: now)
            try write(image(of: folded), "notch-\(edge.rawValue)-folded.png")
            note("\(edge.rawValue) edge")
        }
        let camera = HardwareNotch(width: 220, height: 38)
        let cameraOpen = fixture.model(edge: .top, expanded: true, hardware: camera, now: now)
        try write(image(of: cameraOpen), "notch-top-camera-open.png")
        let cameraCard = fixture.model(edge: .top, expanded: true, hovered: workIndex, hardware: camera, now: now)
        try write(image(of: cameraCard), "notch-top-camera-card.png")
        let cameraFolded = fixture.model(edge: .top, expanded: false, hardware: camera, now: now)
        try write(image(of: cameraFolded), "notch-top-camera-folded.png")

        RingBadgeSheet.showCounts()
        try write(renderImage(RingBadgeSheet(snapshot: fixture.claudeSnapshot)), "ring-badges.png")
        RingSettleSheet.showDeadlines()
        try write(renderImage(RingSettleSheet(snapshot: fixture.claudeSnapshot)), "ring-settle.png")
        note("notch sheets written")
        // The panel half (WP-D): the chrome and its tail on every edge.
        written += try ClaudeAppSnapshots.render(ClaudeAppSnapshots.panelSheets(), into: directory)
        note("panel sheets written")
        return written
    }

    // MARK: - Sealed captures

    /// Render every notch at points along the sealed timeline: folded (its
    /// dots) and open, with the card of a ring that has sessions of every
    /// kind. From copies of each notch's own model, so nothing on screen
    /// moves.
    static func scheduleCaptures(into directory: URL, fleet: NotchFleet, trace: @escaping (String) -> Void) {
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        } catch {
            trace("capture: \(error.localizedDescription)")
            return
        }
        // Before the third ring, with it, after the burst, after the prompt,
        // and after the third ring's arc has settled.
        let points: [(seconds: TimeInterval, hover: String?)] = [
            (2.3, ClaudeFixtureRings.work), (3.8, ClaudeFixtureRings.side), (5.3, ClaudeFixtureRings.work), (7.0, ClaudeFixtureRings.work), (8.7, ClaudeFixtureRings.side),
        ]
        for point in points {
            DispatchQueue.main.asyncAfter(deadline: .now() + point.seconds) { [weak fleet] in
                MainActor.assumeIsolated {
                    guard let fleet else { return }
                    for (index, controller) in fleet.controllersForTesting.enumerated() {
                        let live = controller.model
                        let stamp = String(format: "t%04.1f-%@%@", point.seconds, live.edge.rawValue,
                                           fleet.controllersForTesting.count > 1 ? "-\(index)" : "")
                        let folded = copy(of: live, expanded: false, hovered: nil)
                        let hovered = point.hover.flatMap { id in live.snapshots.firstIndex { $0.id == id } }
                        let open = copy(of: live, expanded: true, hovered: hovered)
                        for (image, name) in [(image(of: folded), "\(stamp)-folded.png"), (image(of: open), "\(stamp)-open.png")] {
                            let url = directory.appendingPathComponent(name)
                            do {
                                try writePNG(image, to: url)
                                trace("captured \(url.lastPathComponent)")
                            } catch {
                                trace("capture \(name) failed: \(error.localizedDescription)")
                            }
                        }
                    }
                }
            }
        }
    }

    // MARK: - Models

    /// A notch model like `live`, for drawing offscreen: the same edge, size,
    /// screen, rings and sessions, open or folded as asked.
    static func copy(of live: NotchViewModel, expanded: Bool, hovered: Int?) -> NotchViewModel {
        let model = NotchViewModel()
        model.edge = live.edge
        model.sizeScale = live.sizeScale
        model.hardwareNotch = live.hardwareNotch
        model.screenSize = live.screenSize
        model.visibleAlongRange = live.visibleAlongRange
        model.resetTimeFormat = live.resetTimeFormat
        model.accentColor = live.accentColor
        model.watchLimit = live.watchLimit
        model.criticalLimit = live.criticalLimit
        model.colorTransitionStyle = live.colorTransitionStyle
        model.weeklyRing = live.weeklyRing
        model.weeklyRingDashed = live.weeklyRingDashed
        model.showsNotchReadings = live.showsNotchReadings
        model.showsMoveHandle = live.showsMoveHandle
        model.surfaceStyle = .solid
        model.deepSeekPricingEnabled = live.deepSeekPricingEnabled
        model.deepSeekPricingSchedule = live.deepSeekPricingSchedule
        model.snapshots = live.snapshots
        model.thinkingModels = live.thinkingModels
        model.localActivities = live.localActivities
        model.refreshing = live.refreshing
        model.sessions = live.sessions
        model.now = live.now
        model.isExpanded = expanded
        model.hoveredIndex = expanded ? hovered : nil
        return model
    }

    private final class Flag {
        var isSet = false
    }

    /// The sealed fixtures as the notch would be given them.
    @MainActor
    private struct Fixture {
        let snapshots: [ProviderSnapshot]
        let sessions: [String: [AgentSession]]
        /// What the fleet applies to every notch from Preferences, so the
        /// pictures draw what the app does (the percent under each ring above
        /// all: the badges have to clear it).
        let preferences: Preferences

        init(hub: ClaudeControlHub, preferences: Preferences) {
            let now = Date()
            let rings = ClaudeProviderSync.ringIDs(hub.accounts)
            ClaudeRingNames.shared.update(hub.accounts)
            // Named as the store names a live ring: the nickname, else the
            // ring's own name (not the account's ClaudeControl label, which
            // the ring never shows).
            let claude = rings.compactMap { ring in
                try? ClaudeUsageProvider.snapshot(
                    ringID: ring,
                    displayName: preferences.accountNicknames[ring] ?? ClaudeRingNames.shared.name(for: ring),
                    reading: hub.ringReadings[ring] ?? ClaudeRingReading(status: .waitingForFirstReading),
                    now: now)
            }
            let others = Fixtures.snapshots(now: now).filter { !ClaudeBridge.ownsProvider($0.id) }
            snapshots = AppDelegate.drawn(claude + others, weekly: preferences.weeklyHeadline,
                                          paced: preferences.claudeDailyPaceRing)
            sessions = ClaudeSessionFeed.rows(
                rings: rings,
                sessionRings: hub.sessions.map(\.ringID),
                previous: [],
                isShown: { _ in true },
                rows: { hub.activityRows(ringID: $0, now: now) })
            self.preferences = preferences
        }

        var claudeSnapshot: ProviderSnapshot {
            snapshots.first { ClaudeBridge.ownsProvider($0.id) } ?? snapshots[0]
        }

        func model(edge: NotchEdge, expanded: Bool, hovered: Int? = nil,
                   hardware: HardwareNotch? = nil, now: Date) -> NotchViewModel {
            let model = NotchViewModel()
            model.edge = edge
            model.hardwareNotch = hardware
            // A 27" display, so every card has room for its sessions (on a
            // laptop a side edge's card lists fewer and counts the rest), and
            // the same every time.
            model.screenSize = CGSize(width: 2560, height: 1440)
            model.surfaceStyle = .solid
            model.resetTimeFormat = preferences.resetTimeFormat
            model.accentColor = preferences.accentColor
            model.watchLimit = preferences.watchLimit
            model.criticalLimit = preferences.criticalLimit
            model.colorTransitionStyle = preferences.colorTransitionStyle
            model.weeklyRing = preferences.weeklyRing
            model.weeklyRingDashed = preferences.weeklyRingDashed
            model.showsNotchReadings = preferences.showsNotchReadings
            model.showsMoveHandle = preferences.showsMoveHandle
            model.snapshots = snapshots
            model.sessions = sessions
            model.now = now
            model.isExpanded = expanded
            model.hoveredIndex = expanded ? hovered : nil
            return model
        }
    }

    // MARK: - Drawing

    /// The whole panel, as the window would show it, over a neutral desktop.
    static func image(of model: NotchViewModel) -> CGImage? {
        let size = model.panelSize
        return renderImage(
            NotchRootView(model: model)
                .frame(width: size.width, height: size.height)
                .background(Color(white: 0.42))
                .environment(\.codenotchHeadlessGlass, true),
            size: size)
    }

    /// `view` as a picture at the screen's scale. Drawn by AppKit in a window
    /// that is never shown, rather than by `ImageRenderer`, which cannot draw
    /// an AppKit view inside SwiftUI — and the working arc is one (a Core
    /// Animation layer). `ImageRenderer` only if that draws nothing. With no
    /// `size`, the view's own.
    static func renderImage<Content: View>(_ view: Content, size: CGSize? = nil) -> CGImage? {
        let content = view
            .environment(\.colorScheme, .dark)
            .environment(\.claudeStillFrame, true)
        let host = NSHostingView(rootView: content)
        host.frame = CGRect(origin: .zero, size: size ?? host.fittingSize)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        // A turn of the run loop for SwiftUI to commit what it laid out.
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        if let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) {
            host.cacheDisplay(in: host.bounds, to: rep)
            if let image = rep.cgImage { return image }
        }
        let renderer = ImageRenderer(content: content)
        renderer.scale = 2
        return renderer.cgImage
    }

    struct SnapshotError: LocalizedError {
        let errorDescription: String?
    }

    static func writePNG(_ image: CGImage?, to url: URL) throws {
        guard let image else { throw SnapshotError(errorDescription: "\(url.lastPathComponent): nothing rendered") }
        guard let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else {
            throw SnapshotError(errorDescription: "\(url.lastPathComponent): PNG encoding failed")
        }
        try data.write(to: url, options: .atomic)
    }

    /// Spin the run loop until `done`, for at most `timeout` seconds. For the
    /// snapshot path only, which runs inside `applicationDidFinishLaunching`.
    private static func spin(until done: () -> Bool, timeout: TimeInterval = 5) {
        let deadline = Date().addingTimeInterval(timeout)
        while !done(), Date() < deadline {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02))
        }
    }
}

// MARK: - Close-ups

/// Badges on every edge and in the camera strip, with small and large counts.
private struct RingBadgeSheet: View {
    let snapshot: ProviderSnapshot

    static let cases: [(ringID: String, counts: ClaudeAttentionCounts, state: AgentSession.State, label: String)] = [
        (ClaudeFixtureRings.personal, ClaudeAttentionCounts(needsYou: 2, review: 1, working: 1), .waiting, "2 need you, 1 to review"),
        (ClaudeFixtureRings.work, ClaudeAttentionCounts(needsYou: 12, working: 3), .waiting, "12 need you"),
        (ClaudeFixtureRings.side, ClaudeAttentionCounts(review: 3, idle: 1), .success, "3 to review"),
    ]
    private let rows: [(edge: NotchEdge, compact: Bool, title: String)] = [
        (.right, false, "right"), (.left, false, "left"), (.top, false, "top"), (.bottom, false, "bottom"),
        (.top, true, "camera strip"),
    ]

    /// What the badges read from, for this sheet.
    static func showCounts() {
        ClaudeNotchState.shared.show(
            ringCounts: Dictionary(uniqueKeysWithValues: cases.map { ($0.ringID, $0.counts) }),
            freshSuccessUntil: [:])
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                HStack(spacing: 22) {
                    Text(row.title)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.white)
                        .frame(width: 84, alignment: .leading)
                    ForEach(Array(Self.cases.enumerated()), id: \.offset) { _, item in
                        VStack(spacing: 4) {
                            closeUpCell(snapshot, ringID: item.ringID, state: item.state)
                                .environment(\.claudeCellContext, ClaudeCellContext(edge: row.edge, compact: row.compact))
                                .scaleEffect(row.compact ? 0.68 : 1)
                                .frame(width: NotchLayout.ringDiameter + 24, height: NotchLayout.cellExtent + 8)
                            Text(item.label)
                                .font(.system(size: 8))
                                .foregroundStyle(.white.opacity(0.7))
                        }
                    }
                }
            }
        }
        .padding(18)
        .background(Palette.notch)
    }
}

/// A finished session's green arc: pulsing for 90 s after it finished, then
/// steady.
private struct RingSettleSheet: View {
    let snapshot: ProviderSnapshot

    static let pulsing = ClaudeFixtureRings.work
    static let steady = ClaudeFixtureRings.side

    /// The first ring just finished; the second finished long ago.
    static func showDeadlines() {
        ClaudeNotchState.shared.show(ringCounts: [pulsing: ClaudeAttentionCounts(review: 1),
                                                  steady: ClaudeAttentionCounts(review: 1)],
                                     freshSuccessUntil: [pulsing: Date().addingTimeInterval(60)])
    }

    var body: some View {
        HStack(spacing: 28) {
            ForEach([Self.pulsing, Self.steady], id: \.self) { ring in
                VStack(spacing: 6) {
                    closeUpCell(snapshot, ringID: ring, state: .success)
                    Text(ring == Self.pulsing ? "just finished: pulsing" : "after 90 s: steady")
                        .font(.system(size: 9))
                        .foregroundStyle(.white.opacity(0.75))
                }
            }
        }
        .padding(18)
        .background(Palette.notch)
    }
}

/// One Claude cell, as `snapshot` reads, under another ring id, with one
/// session in `state`.
@MainActor
private func closeUpCell(_ snapshot: ProviderSnapshot, ringID: String, state: AgentSession.State) -> some View {
    let ring = ProviderSnapshot(id: ringID, displayName: snapshot.displayName, glyph: snapshot.glyph,
                                fidelity: snapshot.fidelity, status: snapshot.status, windows: snapshot.windows,
                                headlineID: snapshot.headlineID, weeklyID: snapshot.weeklyID, plan: snapshot.plan)
    let session = AgentSession(id: ringID, name: ringID, detail: "", state: state, waitingFor: nil, since: Date())
    return ProviderCell(snapshot: ring, activity: ActivitySummary(sessions: [session]), weeklyRing: .outside)
}
