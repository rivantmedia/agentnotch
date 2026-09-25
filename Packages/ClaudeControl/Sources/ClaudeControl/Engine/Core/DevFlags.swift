//
//  DevFlags.swift
//  ClaudeControl
//
//  Every run-time switch the engine has, parsed in one place with one rule
//  for "on" (`1`, `true` or `yes`, any case). The ones that shape a run come
//  from the frozen `ClaudeControlConfiguration` (see its `live` factory);
//  the debugging aids read the process's own environment and arguments.
//
//  | Switch                                   | Effect                                                   |
//  |------------------------------------------|----------------------------------------------------------|
//  | `AGENTNOTCH_SAFE_MODE=1` / `CODENOTCH_DEMO=1`  | sealed: fixtures only (see `SealedMode`; fails closed:   |
//  |                                          | any value but empty, 0, false, no or off seals the run)  |
//  | `--no-install` / `AGENTNOTCH_NO_INSTALL`       | never write a settings.json or hooks folder              |
//  | `AGENTNOTCH_NO_NOTIFICATIONS`                  | never post a notification or ask for permission          |
//  | `AGENTNOTCH_SUPPORT_DIR=<path>`                | the engine's folder (accounts, review queue, socket)     |
//  | `AGENTNOTCH_SOCKET=<path>`                     | the hook socket (the scripts only with `AGENTNOTCH_DEV=1` too) |
//  | `AGENTNOTCH_EXTRA_CONFIG_DIRS=<a>:<b>`         | more Claude config folders to track                      |
//  | `AGENTNOTCH_USAGE_PROBE`                       | usage probes on schedule even with `--no-install`        |
//  | `--dump-state` / `AGENTNOTCH_DUMP_STATE`       | print a line per session on every change                 |
//  | `--dev-console` / `AGENTNOTCH_DEV_CONSOLE`     | drive sessions from stdin                                |
//
//  Safe to query from any actor or thread.
//

import Foundation
import os.log

nonisolated enum DevFlags {
    private static var logger: Logger {
        EngineLog.logger("DevFlags")
    }

    // MARK: - Parsing

    /// The one rule for a boolean switch: `1`, `true` or `yes`, any case,
    /// surrounding spaces ignored. Anything else (including unset) is off.
    static func truthy(_ value: String?) -> Bool {
        guard let value = value?.trimmingCharacters(in: .whitespaces).lowercased() else { return false }
        return value == "1" || value == "true" || value == "yes"
    }

    /// A switch that is either a command-line argument or an environment variable.
    static func flag(
        _ argument: String,
        env: String,
        environment: [String: String] = Foundation.ProcessInfo.processInfo.environment,
        arguments: [String] = CommandLine.arguments
    ) -> Bool {
        arguments.contains(argument) || truthy(environment[env])
    }

    /// `a:b:c` split into paths, `~` expanded against `home`, empties dropped.
    static func pathList(_ value: String?, home: String) -> [String] {
        (value ?? "")
            .split(separator: ":")
            .map { expandTilde(String($0).trimmingCharacters(in: .whitespaces), home: home) }
            .filter { !$0.isEmpty }
    }

    /// `~` and `~/…` against `home`; anything else unchanged.
    static func expandTilde(_ path: String, home: String) -> String {
        if path == "~" { return home }
        if path.hasPrefix("~/") { return (home as NSString).appendingPathComponent(String(path.dropFirst(2))) }
        return path
    }

    // MARK: - From the configuration

    /// When true the app never writes to any Claude settings.json or hooks
    /// directory: `HookInstaller` install and uninstall become no-ops.
    static var installsDisabled: Bool { !AppIdentity.configuration.installsAllowed }

    /// When true no macOS notification is posted and no permission is asked.
    static var notificationsDisabled: Bool { !AppIdentity.configuration.notificationsAllowed }

    /// When true Claude Code is never launched to ask for usage.
    static var probesDisabled: Bool { !AppIdentity.configuration.probesAllowed }

    /// Sealed development mode: fixtures only, nothing real is read or written.
    static var isSealed: Bool { AppIdentity.isSealed }

    /// `AGENTNOTCH_EXTRA_CONFIG_DIRS`, normalized: folders to track as accounts and
    /// to scan for sessions, beside the ones discovered in the home folder.
    static var extraConfigDirs: [String] {
        AppIdentity.configuration.extraConfigDirs.map(AccountPaths.normalize)
    }

    // MARK: - Debugging aids

    /// `AGENTNOTCH_USAGE_PROBE`: keep scheduled usage probes on in a `--no-install` run.
    static let usageProbeOnDevRun: Bool = truthy(Foundation.ProcessInfo.processInfo.environment["AGENTNOTCH_USAGE_PROBE"])

    /// `--dump-state` / `AGENTNOTCH_DUMP_STATE`: print a one-line summary per session on change.
    static let dumpState: Bool = flag("--dump-state", env: "AGENTNOTCH_DUMP_STATE")

    /// `--dev-console` / `AGENTNOTCH_DEV_CONSOLE`: read session commands from stdin.
    static let devConsole: Bool = flag("--dev-console", env: "AGENTNOTCH_DEV_CONSOLE")

    // MARK: - Logging

    /// Log the active flags once at launch so a dev run is easy to recognise.
    static func logActiveFlags() {
        if isSealed {
            logger.notice("Sealed: fixtures only; no socket, scanner, probe, install or notification")
            if let odd = SealedMode.unrecognised(environment: Foundation.ProcessInfo.processInfo.environment) {
                logger.warning("\(odd, privacy: .public) is not 1, true or yes: sealed anyway (only empty, 0, false, no or off leave a run live)")
                print("[\(AppIdentity.displayName)] \(odd) is not 1, true or yes: sealed anyway")
            }
            return
        }
        if installsDisabled {
            logger.notice("Hook installation disabled (--no-install / AGENTNOTCH_NO_INSTALL=1): settings.json and hooks dirs will not be touched")
            print("[\(AppIdentity.displayName)] Hook installation disabled (--no-install / AGENTNOTCH_NO_INSTALL=1)")
        }
        if notificationsDisabled {
            logger.notice("Notifications disabled (AGENTNOTCH_NO_NOTIFICATIONS=1)")
        }
        if !extraConfigDirs.isEmpty {
            logger.notice("Extra config folders (AGENTNOTCH_EXTRA_CONFIG_DIRS): \(extraConfigDirs.joined(separator: ", "), privacy: .public)")
        }
        if usageProbeOnDevRun { logger.notice("Scheduled usage probes forced on (AGENTNOTCH_USAGE_PROBE)") }
        if dumpState { logger.notice("State dump on (--dump-state)") }
        if devConsole { logger.notice("Dev console on (--dev-console)") }
    }
}
