import Foundation
@testable import ClaudeControl

/// A throwaway home laid out the way Claude Parallel Profiles lays out the
/// user's Mac (design-parallel-profiles.md, "The real layout"):
///
/// | folder                          | what                         | identity         |
/// |---------------------------------|------------------------------|------------------|
/// | ~/.claude (+ ~/.claude.json)    | default, mirrored last-used  | paras@rivant.in  |
/// | ~/.claude-paras                 | store (marker, manifest)     | paras@rivant.in  |
/// | ~/.claude-paras-rivant-in       | store                        | paras@rivant.in  |
/// | ~/.claude-claude                | store                        | claude@biios.in  |
/// | ~/.claude-windows/1bf3e8f92b11  | VS Code window               | claude@biios.in  |
/// | ~/.claude-windows/801f9dd51396  | VS Code window               | paras@rivant.in  |
/// | ~/.claude-windows/b9fbb9ecd7cb  | VS Code window               | paras@rivant.in  |
/// | ~/.claude-windows/.manifest.json| the extension's manifest     | —                |
/// | ~/.claude-shared                | shared history, linked in    | —                |
///
/// Superpowered Vibe Notch's hooks and status line are in `~/.claude`, the
/// three stores and `~/.claude-shared` (window folders have none). Nothing
/// outside the temporary folder is read or written.
@MainActor
final class ParallelProfilesHome {
    static let parasUUID = "29638aea-5c1e-4d2a-9b7f-1e0d3c4b5a69"
    static let biiosUUID = "3d93ede5-8a2b-4c6d-9e1f-7a5b3c2d1e08"
    static let paras = "paras@rivant.in"
    static let biios = "claude@biios.in"
    static let sharedEntries = ["projects", "sessions", "session-env", "shell-snapshots", "file-history", "plans", "todos"]

    let home: String
    var support: String { home + "-support" }

    init(_ label: String = "pp") {
        home = TestPaths.temporaryRoot(label)
    }

    func cleanUp() {
        try? FileManager.default.removeItem(atPath: home)
        try? FileManager.default.removeItem(atPath: support)
    }

    func path(_ relative: String) -> String {
        relative.isEmpty ? home : home + "/" + relative
    }

    // MARK: Building blocks

    func mkdir(_ relative: String) throws {
        try FileManager.default.createDirectory(atPath: path(relative), withIntermediateDirectories: true)
    }

    func write(_ relative: String, _ text: String) throws {
        try mkdir((relative as NSString).deletingLastPathComponent)
        try Data(text.utf8).write(to: URL(fileURLWithPath: path(relative)))
    }

    func writeJSON(_ relative: String, _ object: Any) throws {
        try mkdir((relative as NSString).deletingLastPathComponent)
        try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]).write(to: URL(fileURLWithPath: path(relative)))
    }

    /// A `.claude.json` with project state (skipped by the scanner), the
    /// login, and Claude Code's cached usage.
    func login(uuid: String, email: String, cachedFetchedAtMs: Double? = nil, cachedUuid: String? = nil,
               session: Double = 12) -> [String: Any] {
        var json: [String: Any] = [
            "numStartups": 7,
            "projects": ["/Users/x/repo": ["allowedTools": ["Bash(npm test)"], "history": [["display": "hi \"there\" {"]]]],
            "oauthAccount": [
                "accountUuid": uuid, "emailAddress": email, "organizationUuid": "org-" + uuid.prefix(8),
                "organizationType": "claude_max", "organizationRateLimitTier": "default_claude_max_20x",
            ],
        ]
        if let cachedFetchedAtMs {
            json["cachedUsageUtilization"] = [
                "accountUuid": cachedUuid ?? uuid,
                "fetchedAtMs": cachedFetchedAtMs,
                "utilization": ["five_hour": ["utilization": session, "resets_at": "2099-01-01T00:00:00Z"],
                                "seven_day": ["utilization": 20, "resets_at": "2099-01-05T00:00:00Z"]],
            ]
        }
        return json
    }

    /// Link the shared history into a folder, as the extension does.
    func linkShared(_ folder: String) throws {
        try mkdir(folder)
        for entry in Self.sharedEntries {
            try mkdir(".claude-shared/" + entry)
            let link = path(folder + "/" + entry)
            if (try? FileManager.default.destinationOfSymbolicLink(atPath: link)) == nil {
                try FileManager.default.createSymbolicLink(atPath: link, withDestinationPath: path(".claude-shared/" + entry))
            }
        }
    }

    func addStore(_ name: String, uuid: String, email: String, cachedFetchedAtMs: Double? = nil, marker: Bool = true) throws {
        let folder = ".claude-" + name
        try linkShared(folder)
        if marker { try write(folder + "/" + ParallelProfiles.storeMarkerName, "") }
        try writeJSON(folder + "/.claude.json", login(uuid: uuid, email: email, cachedFetchedAtMs: cachedFetchedAtMs))
    }

    func addWindow(_ id: String, uuid: String, email: String, cachedFetchedAtMs: Double? = nil) throws {
        let folder = ".claude-windows/" + id
        try linkShared(folder)
        try writeJSON(folder + "/.claude.json", login(uuid: uuid, email: email, cachedFetchedAtMs: cachedFetchedAtMs))
    }

    func writeManifest(stores: [String]) throws {
        try writeJSON(".claude-windows/.manifest.json", [
            "stores": stores.map { path($0) }, "created": stores.map { path($0) },
            "customOAuth": false, "defaultConfigDir": NSNull(),
        ])
    }

    /// Superpowered Vibe Notch's entries (absolute script paths, as it writes
    /// them), its scripts and, when given, the status line it saved.
    func addVibeNotch(_ folder: String, userContent: Bool, previous: OrderedJSON? = nil) throws {
        let dir = path(folder)
        if userContent {
            try write(folder + "/settings.json", TakeoverFixture.settings(configDir: dir))
        } else {
            try write(folder + "/settings.json", Self.vibeNotchOnlySettings(configDir: dir))
        }
        try TakeoverFixture.writeVibeNotchFiles(configDir: dir, previous: previous)
    }

    /// What Superpowered Vibe Notch writes into a folder it created settings for.
    static func vibeNotchOnlySettings(configDir: String) -> String {
        let hook = "python3 '\(configDir)/hooks/superpowered-notch-hook.py'"
        let wrapper = "python3 '\(configDir)/hooks/superpowered-notch-statusline.py'"
        return """
        {
          "hooks" : {
            "PermissionRequest" : [ { "hooks" : [ { "command" : "\(hook)", "timeout" : 86400, "type" : "command" } ], "matcher" : "*" } ],
            "Stop" : [ { "hooks" : [ { "command" : "\(hook)", "type" : "command" } ] } ]
          },
          "statusLine" : { "command" : "\(wrapper)", "padding" : 0, "type" : "command" }
        }
        """
    }

    // MARK: The user's layout

    /// Builds the table above. `vibeNotch` adds Superpowered Vibe Notch's
    /// leftovers where the user has them.
    func buildUserLayout(vibeNotch: Bool = true, manifest: Bool = true, markers: Bool = true) throws {
        try linkShared(".claude")
        try writeJSON(".claude.json", login(uuid: Self.parasUUID, email: Self.paras, cachedFetchedAtMs: 1_790_000_000_000))
        try addStore("paras", uuid: Self.parasUUID, email: Self.paras, cachedFetchedAtMs: 1_790_000_100_000, marker: markers)
        try addStore("paras-rivant-in", uuid: Self.parasUUID, email: Self.paras, marker: markers)
        try addStore("claude", uuid: Self.biiosUUID, email: Self.biios, cachedFetchedAtMs: 1_790_000_050_000, marker: markers)
        try addWindow("1bf3e8f92b11", uuid: Self.biiosUUID, email: Self.biios, cachedFetchedAtMs: 1_790_000_300_000)
        try addWindow("801f9dd51396", uuid: Self.parasUUID, email: Self.paras, cachedFetchedAtMs: 1_790_000_200_000)
        try addWindow("b9fbb9ecd7cb", uuid: Self.parasUUID, email: Self.paras)
        if manifest { try writeManifest(stores: [".claude-claude", ".claude-paras", ".claude-paras-rivant-in"]) }
        if vibeNotch {
            try addVibeNotch(".claude", userContent: true, previous: TakeoverFixture.originalStatusLine)
            for store in [".claude-paras", ".claude-paras-rivant-in", ".claude-claude", ".claude-shared"] {
                try addVibeNotch(store, userContent: false)
            }
        }
    }

    // MARK: The engine over it

    func registry(extraDirs: [String] = []) -> AccountRegistry {
        AccountRegistry(home: home, storeURL: URL(fileURLWithPath: support + "/accounts.json"),
                        configReader: ClaudeGlobalConfigReader(), extraConfigDirs: extraDirs)
    }

    /// Every file under the home (links not followed), with its bytes.
    func files() -> [String: Data] {
        var files: [String: Data] = [:]
        let fm = FileManager.default
        func walk(_ relative: String) {
            let full = path(relative)
            for name in (try? fm.contentsOfDirectory(atPath: full)) ?? [] {
                let child = relative.isEmpty ? name : relative + "/" + name
                let childPath = path(child)
                if let target = try? fm.destinationOfSymbolicLink(atPath: childPath) {
                    files[child + " ->"] = Data(target.utf8)
                    continue
                }
                var isDirectory: ObjCBool = false
                if fm.fileExists(atPath: childPath, isDirectory: &isDirectory), isDirectory.boolValue {
                    files[child + "/"] = Data()
                    walk(child)
                } else {
                    files[child] = fm.contents(atPath: childPath)
                }
            }
        }
        walk("")
        return files
    }
}

/// A child process for tests that holds none of this process's descriptors
/// (`POSIX_SPAWN_CLOEXEC_DEFAULT`, stdio on /dev/null): a parallel test
/// reading a pipe to EOF is never kept waiting by it.
nonisolated final class TestChild {
    let pid: pid_t

    init(executable: String, arguments: [String] = ["30"], environment: [String: String] = ["PATH": "/usr/bin:/bin"]) throws {
        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_CLOEXEC_DEFAULT))
        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        for fd in Int32(0)...2 {
            posix_spawn_file_actions_addopen(&actions, fd, "/dev/null", fd == 0 ? O_RDONLY : O_WRONLY, 0)
        }
        let argv = ([executable] + arguments).map { strdup($0) } + [nil]
        let envp = environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer {
            argv.forEach { free($0) }
            envp.forEach { free($0) }
        }
        var child: pid_t = 0
        let status = posix_spawn(&child, executable, &actions, &attributes, argv, envp)
        guard status == 0 else { throw POSIXError(POSIXErrorCode(rawValue: status) ?? .EIO) }
        pid = child
    }

    func terminate() {
        kill(pid, SIGKILL)
        var status: Int32 = 0
        waitpid(pid, &status, 0)
    }
}
