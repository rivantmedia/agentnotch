//
//  AppIdentity.swift
//  ClaudeControl
//
//  Names and paths that identify the host app, read by every engine service.
//  Superpowered Vibe Notch hard-coded them; here they come from the
//  `ClaudeControlConfiguration` the app passes to `ClaudeControlHub.bootstrap`,
//  which freezes them for the life of the process.
//
//  Before bootstrap (tests, the snapshots tool) the identity is
//  `ClaudeControlConfiguration.unbootstrapped`: the real home, but a support
//  folder under the temporary directory, so nothing reaches the app's own
//  Application Support folder by accident.
//

import Foundation

nonisolated enum AppIdentity {
    // MARK: - Frozen configuration

    private static let lock = NSLock()
    nonisolated(unsafe) private static var frozen: ClaudeControlConfiguration?
    nonisolated(unsafe) private static var fallback: ClaudeControlConfiguration?

    /// The configuration every service reads. Frozen by `freeze(_:)`.
    static var configuration: ClaudeControlConfiguration {
        lock.lock()
        defer { lock.unlock() }
        if let frozen { return frozen }
        if let fallback { return fallback }
        let made = ClaudeControlConfiguration.unbootstrapped()
        fallback = made
        return made
    }

    /// Whether `freeze(_:)` has run.
    static var isFrozen: Bool {
        lock.lock()
        defer { lock.unlock() }
        return frozen != nil
    }

    /// Fix the configuration for the rest of the process. Only the first call
    /// counts; later ones are ignored and return false.
    @discardableResult
    static func freeze(_ configuration: ClaudeControlConfiguration) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard frozen == nil else { return false }
        frozen = configuration
        return true
    }

    // MARK: - Names

    static var displayName: String { configuration.appDisplayName }
    static var bundleIdentifier: String { configuration.bundleIdentifier }
    static var isSealed: Bool { configuration.mode == .sealed }

    /// Hook script copied into every account's `<configDir>/hooks/`.
    static var hookScriptName: String { configuration.hookScriptName }
    /// Status line wrapper copied into every account's `<configDir>/hooks/`.
    static var statusLineScriptName: String { configuration.statusLineScriptName }

    /// Upstream Vibe Notch's hook script, used only to detect leftover entries.
    static let legacyHookScriptName = "claude-island-state.py"
    /// Superpowered Vibe Notch's scripts, detected so they can be taken over
    /// instead of stacked on top of ours.
    static let vibeNotchHookScriptName = "superpowered-notch-hook.py"
    static let vibeNotchStatusLineScriptName = "superpowered-notch-statusline.py"
    /// Where Superpowered Vibe Notch saved the status line its wrapper chains to.
    static let vibeNotchPreviousStatusLineFileName = "superpowered-notch-statusline.previous.json"
    /// Superpowered Vibe Notch's settings.json backups.
    static let vibeNotchBackupPrefix = "settings.json.superpowered-notch-"

    /// Superpowered Vibe Notch. While it runs nothing is installed: both apps
    /// would rewrite the same settings.json with their own hooks.
    static let vibeNotchBundleIdentifier = VibeNotchImport.bundleIdentifier
    /// Upstream Vibe Notch, whose app re-adds its hooks at every launch.
    static let upstreamVibeNotchBundleIdentifier = "com.celestial.ClaudeIsland"

    // MARK: - Paths

    /// The engine's own folder (accounts, review queue, usage state, socket),
    /// created with 0700 on first access. In a bootstrapped live run the
    /// first access also imports Superpowered Vibe Notch's accounts and
    /// review queue once (see `VibeNotchImport`), before anything reads them.
    static var supportDirectory: URL {
        let configuration = configuration
        let dir = configuration.supportDirectory
        prepareOnce(configuration)
        return dir
    }

    private static let prepareLock = NSLock()
    nonisolated(unsafe) private static var preparedSupport: URL?

    private static func prepareOnce(_ configuration: ClaudeControlConfiguration) {
        let dir = configuration.supportDirectory
        prepareLock.lock()
        defer { prepareLock.unlock() }
        guard preparedSupport != dir else { return }
        if !FileManager.default.fileExists(atPath: dir.path) {
            try? FileManager.default.createDirectory(
                at: dir,
                withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700]
            )
        }
        if isFrozen, configuration.mode == .live {
            VibeNotchImport.runOnce(
                from: VibeNotchImport.sourceDirectory(home: configuration.homeDirectory),
                to: dir,
                settings: ClaudeControlSettings.Store(defaults: configuration.defaults)
            )
        }
        preparedSupport = dir
    }

    /// Unix socket the hook and status line scripts write to. Installed
    /// scripts carry this path (templated at install time); `SPCN_SOCKET`
    /// overrides it on both sides.
    static var socketPath: String { configuration.socketPath }

    /// The user's home folder as the engine sees it (a temporary one when sealed).
    static var homeDirectory: String { configuration.homeDirectory }

    /// UserDefaults for `ClaudeControlSettings`.
    static var defaults: UserDefaults { configuration.defaults }
}
