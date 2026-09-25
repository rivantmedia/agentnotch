//
//  ClaudeControlConfiguration.swift
//  ClaudeControl
//
//  Everything the engine needs to know about its host, passed once to
//  `ClaudeControlHub.bootstrap` and frozen there (see `AppIdentity`).
//  Capabilities the host has and the engine does not (tab focus for terminals
//  it can't script, Claude Desktop's usage cache) come in as closures and
//  protocols, so the package never links against the app.
//

import Foundation

public nonisolated struct ClaudeControlConfiguration {
    public enum Mode: Sendable { case live, sealed }

    public var mode: Mode
    public var appDisplayName: String
    public var bundleIdentifier: String
    /// The engine's folder: accounts.json, review-state.json, usage-state.json, hook.sock.
    public var supportDirectory: URL
    /// The socket the hook scripts write to (templated into them at install time).
    public var socketPath: String
    /// What `~` means to the engine. A temporary folder when sealed.
    public var homeDirectory: String
    public var hookScriptName: String
    public var statusLineScriptName: String
    /// Where `ClaudeControlSettings` lives (`claudeControl.*` keys).
    public var defaults: UserDefaults
    /// Hooks and status line may be written into Claude config folders (still
    /// only after the user's consent).
    public var installsAllowed: Bool
    public var notificationsAllowed: Bool
    /// Claude Code may be launched to ask for usage (`get_usage`).
    public var probesAllowed: Bool
    /// Extra config folders to treat as accounts (`AGENTNOTCH_EXTRA_CONFIG_DIRS`).
    public var extraConfigDirs: [String]
    /// Selects a session's terminal tab in apps the engine cannot script
    /// itself (Ghostty, cmux, …). Returns whether it did.
    public var externalTabFocus: (@Sendable (ClaudeExternalTabRequest) -> Bool)?
    /// Usage read from somewhere other than Claude Code (Claude Desktop's cache).
    public var externalUsageSource: (any ClaudeExternalUsageSource)?

    public init(
        mode: Mode,
        appDisplayName: String,
        bundleIdentifier: String,
        supportDirectory: URL,
        socketPath: String,
        homeDirectory: String,
        hookScriptName: String = ClaudeControlConfiguration.defaultHookScriptName,
        statusLineScriptName: String = ClaudeControlConfiguration.defaultStatusLineScriptName,
        defaults: UserDefaults = .standard,
        installsAllowed: Bool,
        notificationsAllowed: Bool,
        probesAllowed: Bool,
        extraConfigDirs: [String] = [],
        externalTabFocus: (@Sendable (ClaudeExternalTabRequest) -> Bool)? = nil,
        externalUsageSource: (any ClaudeExternalUsageSource)? = nil
    ) {
        self.mode = mode
        self.appDisplayName = appDisplayName
        self.bundleIdentifier = bundleIdentifier
        self.supportDirectory = supportDirectory
        self.socketPath = socketPath
        self.homeDirectory = homeDirectory
        self.hookScriptName = hookScriptName
        self.statusLineScriptName = statusLineScriptName
        self.defaults = defaults
        self.installsAllowed = installsAllowed
        self.notificationsAllowed = notificationsAllowed
        self.probesAllowed = probesAllowed
        self.extraConfigDirs = extraConfigDirs
        self.externalTabFocus = externalTabFocus
        self.externalUsageSource = externalUsageSource
    }

    // MARK: - Names

    public static let defaultHookScriptName = "agentnotch-hook.py"
    public static let defaultStatusLineScriptName = "agentnotch-statusline.py"
    /// Sub-folder of the host's Application Support folder the engine owns.
    public static let engineFolderName = "Claude"
    /// Longest socket path `sockaddr_un` takes (104 bytes with the terminator).
    public static let maxSocketPathBytes = 103

    // MARK: - Factories

    /// The real app. Environment (all optional; see `DevFlags` for the full list):
    /// - `AGENTNOTCH_SUPPORT_DIR`: the engine's folder, instead of
    ///   `~/Library/Application Support/<supportFolderName>/Claude`
    /// - `AGENTNOTCH_SOCKET`: the socket path, used as given
    /// - `AGENTNOTCH_NO_INSTALL` or `--no-install`: never write hooks
    /// - `AGENTNOTCH_NO_NOTIFICATIONS`: never post a notification
    /// - `AGENTNOTCH_EXTRA_CONFIG_DIRS`: more config folders, `:`-separated
    ///
    /// Boolean switches are on for `1`, `true` or `yes` (`DevFlags.truthy`).
    /// Without `AGENTNOTCH_SOCKET` the socket is `<support>/hook.sock`, or
    /// `/tmp/agentnotch-<uid>/hook.sock` when that path is too long for a Unix socket.
    public static func live(
        appDisplayName: String,
        bundleIdentifier: String,
        supportFolderName: String,
        environment: [String: String],
        arguments: [String]
    ) -> Self {
        let home = environment["HOME"].flatMap { $0.isEmpty ? nil : $0 } ?? NSHomeDirectory()
        let support: URL
        if let override = environment["AGENTNOTCH_SUPPORT_DIR"], !override.isEmpty {
            support = URL(fileURLWithPath: DevFlags.expandTilde(override, home: home), isDirectory: true)
        } else {
            support = URL(fileURLWithPath: home, isDirectory: true)
                .appendingPathComponent("Library/Application Support", isDirectory: true)
                .appendingPathComponent(supportFolderName, isDirectory: true)
                .appendingPathComponent(engineFolderName, isDirectory: true)
        }
        let socket: String
        if let override = environment["AGENTNOTCH_SOCKET"], !override.isEmpty {
            socket = DevFlags.expandTilde(override, home: home)
        } else {
            socket = socketPath(in: support, userID: getuid())
        }
        return Self(
            mode: .live,
            appDisplayName: appDisplayName,
            bundleIdentifier: bundleIdentifier,
            supportDirectory: support,
            socketPath: socket,
            homeDirectory: home,
            defaults: .standard,
            installsAllowed: !(arguments.contains("--no-install") || DevFlags.truthy(environment["AGENTNOTCH_NO_INSTALL"])),
            notificationsAllowed: !DevFlags.truthy(environment["AGENTNOTCH_NO_NOTIFICATIONS"]),
            probesAllowed: true,
            extraConfigDirs: DevFlags.pathList(environment["AGENTNOTCH_EXTRA_CONFIG_DIRS"], home: home)
        )
    }

    /// Fixtures only: no socket, scanner, probe, install, notification or read
    /// of `~/.claude*`. Its home and support folder are under the temporary
    /// directory and are never the user's.
    public static func sealed(appDisplayName: String, bundleIdentifier: String) -> Self {
        let root = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("agentnotch-sealed-\(getuid())", isDirectory: true)
        let support = root.appendingPathComponent(engineFolderName, isDirectory: true)
        return Self(
            mode: .sealed,
            appDisplayName: appDisplayName,
            bundleIdentifier: bundleIdentifier,
            supportDirectory: support,
            socketPath: support.appendingPathComponent("hook.sock").path,
            homeDirectory: root.appendingPathComponent("home", isDirectory: true).path,
            defaults: .standard,
            installsAllowed: false,
            notificationsAllowed: false,
            probesAllowed: false
        )
    }

    /// What the engine uses before `bootstrap` (tests, the snapshots tool):
    /// the real home, so pure path helpers behave as they will in the app, but
    /// a support folder under the temporary directory. `AGENTNOTCH_SUPPORT_DIR`,
    /// `AGENTNOTCH_SOCKET` and `AGENTNOTCH_NO_INSTALL` apply as in `live`.
    ///
    /// The real home is read, never changed, and `claude` never runs: each
    /// side effect that could reach it refuses while `AppIdentity` is not
    /// frozen, whatever this configuration allows — the installer and
    /// `createAccount` for `~`, `~/.claude*` and `~/.config/claude*`
    /// (`HookInstaller.isProtectedBeforeBootstrap`), the default usage probe
    /// runner, the login-shell lookup and `claude --version` (only a stub
    /// under the temporary directory runs). Injected runners and temporary
    /// homes work as usual, which is what the tests use.
    static func unbootstrapped(
        environment: [String: String] = Foundation.ProcessInfo.processInfo.environment
    ) -> Self {
        var environment = environment
        if (environment["AGENTNOTCH_SUPPORT_DIR"] ?? "").isEmpty {
            environment["AGENTNOTCH_SUPPORT_DIR"] = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
                .appendingPathComponent("ClaudeControl-\(getuid())", isDirectory: true).path
        }
        var configuration = live(
            appDisplayName: "ClaudeControl",
            bundleIdentifier: "com.rivantmedia.agentnotch",
            supportFolderName: "Agent Notch",
            environment: environment,
            arguments: CommandLine.arguments
        )
        configuration.notificationsAllowed = false
        return configuration
    }

    // MARK: - Helpers

    /// Where the socket falls back to when the preferred path is too long.
    static func fallbackSocketPath(userID: uid_t) -> String {
        "/tmp/agentnotch-\(userID)/hook.sock"
    }

    /// `<support>/hook.sock`, or `/tmp/agentnotch-<uid>/hook.sock` when that would
    /// be longer than a Unix socket path may be (`sockaddr_un` holds 104
    /// bytes with the terminator). The socket server prepares the folder
    /// before binding (`HookSocketDirectory`: the shared fallback must be a
    /// real folder, ours, closed to others).
    static func socketPath(in support: URL, userID: uid_t) -> String {
        let preferred = support.appendingPathComponent("hook.sock").path
        if preferred.utf8.count <= maxSocketPathBytes { return preferred }
        return fallbackSocketPath(userID: userID)
    }
}

/// A request to select the terminal tab a session runs in.
public nonisolated struct ClaudeExternalTabRequest: Sendable {
    public let bundleID: String?
    public let pid: Int32
    public let tty: String?
    public let cwd: String?

    public init(bundleID: String?, pid: Int32, tty: String?, cwd: String?) {
        self.bundleID = bundleID
        self.pid = pid
        self.tty = tty
        self.cwd = cwd
    }
}

/// Usage the host can read without a token (Claude Desktop's HTTP cache).
public protocol ClaudeExternalUsageSource: Sendable {
    func reading(organizationUuid: String, now: Date) async -> ClaudeExternalUsageReading?
}

public nonisolated struct ClaudeExternalUsageReading: Sendable {
    public let windows: [ClaudeRingReading.Window]
    public let observedAt: Date

    public init(windows: [ClaudeRingReading.Window], observedAt: Date) {
        self.windows = windows
        self.observedAt = observedAt
    }
}
