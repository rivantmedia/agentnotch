import Foundation
import Testing
@testable import ClaudeControl

/// The app was called Superpowered Codenotch before it became Agent Notch.
/// Entries it wrote under that name are the app's own: an install replaces
/// them rather than stacking the new ones beside them, and the status line
/// they wrapped keeps being chained (then given back on uninstall).
@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
nonisolated struct FormerNameTakeoverTests {
    private let mine = #"{"type":"command","command":"echo mine"}"#

    private func configuration(statusLine: Bool = true) -> HookInstaller.Configuration {
        HookInstaller.Configuration(
            python: "python3",
            version: ClaudeCodeVersion(major: 2, minor: 1, patch: 280),
            statusLineIntegration: statusLine,
            hookScript: EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket),
            statusLineScript: EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket)
        )
    }

    /// A folder as Superpowered Codenotch left it: its hooks beside another
    /// tool's, its wrapper as the status line chaining `echo mine`, and its
    /// scripts and saved status line in `hooks/`.
    private func makeFormerInstall() throws -> String {
        let dir = TestPaths.temporaryRoot("former") + "/.claude-test"
        let hooks = dir + "/hooks"
        try FileManager.default.createDirectory(atPath: hooks, withIntermediateDirectories: true)
        let hook = HookCommands.command(runningScript: hooks + "/" + AppIdentity.formerHookScriptName, python: "python3")
        let status = HookCommands.command(runningScript: hooks + "/" + AppIdentity.formerStatusLineScriptName, python: "python3")
        var settings = OrderedJSON.object([
            "model": .string("opus"),
            "hooks": .object([
                "PreToolUse": .array([
                    .object(["matcher": .string("*"), "hooks": .array([.object(["type": .string("command"), "command": .string("other-tool")])])]),
                    .object(["matcher": .string("*"), "hooks": .array([.object(["type": .string("command"), "command": .string(hook)])])]),
                ]),
                "Stop": .array([.object(["hooks": .array([.object(["type": .string("command"), "command": .string(hook)])])])]),
            ]),
        ])
        settings.set("statusLine", .object(["type": .string("command"), "command": .string(status), "padding": .int(2)]))
        try Data(settings.serialized().utf8).write(to: URL(fileURLWithPath: dir + "/settings.json"))
        try Data("# hook".utf8).write(to: URL(fileURLWithPath: hooks + "/" + AppIdentity.formerHookScriptName))
        try Data("# status".utf8).write(to: URL(fileURLWithPath: hooks + "/" + AppIdentity.formerStatusLineScriptName))
        try Data(mine.utf8).write(to: URL(fileURLWithPath: hooks + "/" + AppIdentity.formerPreviousStatusLineFileName))
        return dir
    }

    private func settings(_ dir: String) throws -> OrderedJSON {
        try JSONTest.object(Data(contentsOf: URL(fileURLWithPath: dir + "/settings.json")))
    }

    private func hookCommands(_ json: OrderedJSON) -> [String] {
        (json["hooks"]?.members ?? []).flatMap { member in
            (member.value.items ?? []).flatMap { group in
                (group["hooks"]?.items ?? []).compactMap { $0["command"]?.stringValue }
            }
        }
    }

    private func exists(_ dir: String, _ name: String) -> Bool {
        FileManager.default.fileExists(atPath: dir + "/hooks/" + name)
    }

    @Test func theFormerNamesEntriesCountAsOurs() throws {
        let command = HookCommands.command(runningScript: "/Users/me/.claude/hooks/" + AppIdentity.formerHookScriptName, python: "python3")
        #expect(HookInstaller.isOurHook(command, home: "/Users/me"))
        let status = OrderedJSON.object(["type": .string("command"),
                                         "command": .string("python3 '/x/hooks/\(AppIdentity.formerStatusLineScriptName)'")])
        #expect(HookInstaller.isOurStatusLine(status, home: "/Users/me"))
    }

    @Test func anInstallReplacesTheFormerNamesEntriesAndKeepsTheChain() throws {
        let dir = try makeFormerInstall()
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }

        #expect(HookInstaller.install(configDir: dir, configuration: configuration()) == .installed)
        let json = try settings(dir)
        let commands = hookCommands(json)
        #expect(!commands.contains { $0.contains(AppIdentity.formerHookScriptName) })
        #expect(commands.contains("other-tool"))
        // One entry of ours per event, not the former one plus a new one.
        let preToolUse = (json["hooks"]?["PreToolUse"]?.items ?? []).flatMap { $0["hooks"]?.items ?? [] }
        #expect(preToolUse.filter { HookInstaller.isOurHook($0["command"]?.stringValue ?? "") }.count == 1)

        // The wrapper runs under the new name, keeps the user's padding, and
        // chains what the former wrapper chained.
        let statusLine = json["statusLine"]
        #expect(statusLine?["command"]?.stringValue?.contains(AppIdentity.statusLineScriptName) == true)
        #expect(OrderedJSON.equivalent(statusLine?["padding"], .int(2)))
        let saved = try JSONTest.object(Data(contentsOf: HookInstaller.previousStatusLineURL(configDir: dir)))
        #expect(saved["command"]?.stringValue == "echo mine")

        // Nothing runs the former files any more.
        #expect(!exists(dir, AppIdentity.formerHookScriptName))
        #expect(!exists(dir, AppIdentity.formerStatusLineScriptName))
        #expect(!exists(dir, AppIdentity.formerPreviousStatusLineFileName))
        #expect(exists(dir, AppIdentity.hookScriptName))
    }

    @Test func anUninstallGivesBackWhatTheFormerWrapperChained() throws {
        let dir = try makeFormerInstall()
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }

        #expect(HookInstaller.uninstall(configDir: dir) == .removed)
        let json = try settings(dir)
        #expect(hookCommands(json) == ["other-tool"])
        #expect(json["statusLine"]?["command"]?.stringValue == "echo mine")
        #expect(OrderedJSON.equivalent(json["statusLine"]?["padding"], .int(2)))
        #expect(!exists(dir, AppIdentity.formerHookScriptName))
        #expect(!exists(dir, AppIdentity.formerPreviousStatusLineFileName))
    }

    /// The former files stay while settings.json still runs them (an install
    /// that couldn't write, say): a missing wrapper would blank the status line.
    @Test func theFormerFilesStayWhileSettingsStillRunsThem() throws {
        let dir = try makeFormerInstall()
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }

        HookInstaller.removeFormerNameFiles(configDir: dir)
        #expect(exists(dir, AppIdentity.formerHookScriptName))
        #expect(exists(dir, AppIdentity.formerStatusLineScriptName))
        #expect(exists(dir, AppIdentity.formerPreviousStatusLineFileName))
    }

    /// A lost saved copy is recovered from the backups: this name's first
    /// (whatever the former name's timestamps say), then the former name's,
    /// then the originals.
    @Test func backupsUnderThisNameAreTrustedBeforeTheFormerNames() throws {
        let dir = TestPaths.temporaryRoot("former-backups") + "/.claude-test"
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }
        let settingsURL = URL(fileURLWithPath: dir + "/settings.json")
        func write(_ name: String, _ command: String) throws {
            try Data(#"{"statusLine":{"type":"command","command":"\#(command)"}}"#.utf8).write(to: URL(fileURLWithPath: dir + "/" + name))
        }
        func scan() -> String? {
            HookInstaller.newestStatusLine(inBackupsBeside: settingsURL, prefixes: HookInstaller.ownBackupPrefixes, home: "/Users/me")?["command"]?.stringValue
        }

        try write(AppIdentity.formerOriginalBackupName, "former original")
        #expect(scan() == "former original")
        try write(HookInstaller.originalBackupName, "original")
        #expect(scan() == "original")
        try write(AppIdentity.formerBackupPrefix + "20260101-000000-000" + HookInstaller.backupSuffix, "former")
        #expect(scan() == "former")
        try write(HookInstaller.backupPrefix + "20250101-000000-000" + HookInstaller.backupSuffix, "new")
        #expect(scan() == "new")
    }
}
