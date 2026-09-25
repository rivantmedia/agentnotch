import Foundation
import Testing
@testable import ClaudeControl

/// A config folder as Superpowered Vibe Notch leaves it: its hooks on the
/// lifecycle events beside the user's own, its wrapper as the status line,
/// and the wrapped status line saved beside its scripts.
nonisolated enum TakeoverFixture {
    /// The status line the user had before Superpowered Vibe Notch wrapped it.
    static let originalStatusLine = OrderedJSON.object([
        "type": .string("command"),
        "command": .string("~/.claude/statusline.sh"),
        "padding": .int(2),
        "hideVimModeIndicator": .bool(true),
    ])

    static func settings(configDir: String) -> String {
        let hook = "python3 '\(configDir)/hooks/superpowered-notch-hook.py'"
        let wrapper = "python3 '\(configDir)/hooks/superpowered-notch-statusline.py'"
        return """
        {
          "hooks" : {
            "PermissionRequest" : [
              {
                "hooks" : [
                  {
                    "command" : "\(hook)",
                    "timeout" : 86400,
                    "type" : "command"
                  }
                ],
                "matcher" : "*"
              }
            ],
            "PreToolUse" : [
              {
                "hooks" : [
                  {
                    "command" : "~/bin/guard-bash.sh",
                    "type" : "command"
                  }
                ],
                "matcher" : "Bash"
              },
              {
                "hooks" : [
                  {
                    "command" : "\(hook)",
                    "type" : "command"
                  }
                ],
                "matcher" : "*"
              }
            ],
            "Stop" : [
              {
                "hooks" : [
                  {
                    "command" : "\(hook)",
                    "type" : "command"
                  }
                ]
              }
            ]
          },
          "model" : "opus",
          "statusLine" : {
            "command" : "\(wrapper)",
            "padding" : 2,
            "type" : "command"
          }
        }
        """
    }

    static func writeVibeNotchFiles(configDir: String, previous: OrderedJSON? = originalStatusLine) throws {
        let hooks = configDir + "/hooks"
        try FileManager.default.createDirectory(atPath: hooks, withIntermediateDirectories: true)
        try Data("# SPVN hook\n".utf8).write(to: URL(fileURLWithPath: hooks + "/superpowered-notch-hook.py"))
        try Data("# SPVN wrapper\n".utf8).write(to: URL(fileURLWithPath: hooks + "/superpowered-notch-statusline.py"))
        if let previous {
            try Data(previous.serialized().utf8).write(to: URL(fileURLWithPath: hooks + "/" + AppIdentity.vibeNotchPreviousStatusLineFileName))
        }
    }
}

struct TakeoverPlanTests {
    let home = "/Users/me"
    let configDir = "/Users/me/.claude"

    private func plan(previous: OrderedJSON?, backup: OrderedJSON? = nil) -> HookInstaller.InstallPlan {
        HookInstaller.planLegacyRemoval(existingData: Data(TakeoverFixture.settings(configDir: configDir).utf8),
                                        kinds: [.superpoweredVibeNotch],
                                        vibeNotchPreviousStatusLine: previous, vibeNotchBackupStatusLine: backup, home: home)
    }

    @Test func removesItsHooksAndRestoresItsStatusLineExactly() throws {
        let json = try JSONTest.written(plan(previous: TakeoverFixture.originalStatusLine))
        // Its entries are gone, every event it alone had with them; the user's stay.
        #expect(JSONTest.commands(in: json, event: "PreToolUse") == ["~/bin/guard-bash.sh"])
        #expect(json["hooks"]?["PermissionRequest"] == nil)
        #expect(json["hooks"]?["Stop"] == nil)
        // The status line is exactly what it wrapped: every key, in order.
        #expect(json["statusLine"]?.serialized() == TakeoverFixture.originalStatusLine.serialized())
        #expect(json["model"]?.stringValue == "opus")
    }

    @Test func withoutItsSavedCopyTheNewestBackupRestores() throws {
        let backup = OrderedJSON.object(["type": .string("command"), "command": .string("starship statusline"), "padding": .int(2)])
        let json = try JSONTest.written(plan(previous: nil, backup: backup))
        #expect(json["statusLine"]?.serialized() == backup.serialized())
    }

    @Test func withNothingToRestoreTheStatusLineGoes() throws {
        let json = try JSONTest.written(plan(previous: nil))
        #expect(json["statusLine"] == nil)
    }

    /// A saved copy that is itself a wrapper is never put back.
    @Test func neverRestoresAWrapper() throws {
        let wrapper = OrderedJSON.object(["type": .string("command"),
                                          "command": .string("python3 '/x/hooks/superpowered-notch-statusline.py'")])
        let json = try JSONTest.written(plan(previous: wrapper))
        #expect(json["statusLine"] == nil)
    }

    @Test func otherKindsLeaveItAlone() throws {
        let plan = HookInstaller.planLegacyRemoval(existingData: Data(TakeoverFixture.settings(configDir: configDir).utf8),
                                                   kinds: [.vibeNotch], home: home)
        #expect(plan.settings == .alreadyCurrent)
    }

    @Test func itsHooksAreRecognised() {
        let status = TakeoverFixture.settings(configDir: configDir)
        let json = try? OrderedJSON.parse(Data(status.utf8))
        #expect(HookInstaller.firstHookCommand(in: json ?? .null) {
            HookInstaller.isLegacyHook($0, kind: .superpoweredVibeNotch, home: home)
        } != nil)
        #expect(HookInstaller.isLegacyStatusLine(json?["statusLine"], kind: .superpoweredVibeNotch, home: home))
        #expect(!HookInstaller.isOurStatusLine(json?["statusLine"], home: home))
    }
}

@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
nonisolated struct TakeoverFileTests {
    private func makeConfigDir() throws -> String {
        let dir = TestPaths.temporaryRoot("takeover") + "/.claude"
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        try Data(TakeoverFixture.settings(configDir: dir).utf8).write(to: URL(fileURLWithPath: dir + "/settings.json"))
        return dir
    }

    private func settings(_ dir: String) throws -> OrderedJSON {
        try OrderedJSON.parse(Data(contentsOf: URL(fileURLWithPath: dir + "/settings.json")))
    }

    @Test func takeoverOnDiskReadsItsSavedCopyAndLeavesItsFiles() throws {
        let dir = try makeConfigDir()
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }
        try TakeoverFixture.writeVibeNotchFiles(configDir: dir)
        let savedPath = dir + "/hooks/" + AppIdentity.vibeNotchPreviousStatusLineFileName
        let savedBefore = try Data(contentsOf: URL(fileURLWithPath: savedPath))

        #expect(HookInstaller.readStatus(configDir: dir).superpoweredVibeNotchHooksPresent)
        #expect(HookInstaller.removeLegacyHooks(configDir: dir, kinds: [.superpoweredVibeNotch]) == .removed)

        let json = try settings(dir)
        #expect(json["statusLine"]?.serialized() == TakeoverFixture.originalStatusLine.serialized())
        #expect(!HookInstaller.readStatus(configDir: dir).superpoweredVibeNotchHooksPresent)
        // Its files are only read.
        #expect(try Data(contentsOf: URL(fileURLWithPath: savedPath)) == savedBefore)
        #expect(FileManager.default.fileExists(atPath: dir + "/hooks/superpowered-notch-hook.py"))
        #expect(HookInstaller.removeLegacyHooks(configDir: dir, kinds: [.superpoweredVibeNotch]) == .alreadyCurrent)
    }

    /// Its saved copy gone: the newest of its own backups with a real status line.
    @Test func takeoverFallsBackToItsBackups() throws {
        let dir = try makeConfigDir()
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }
        try TakeoverFixture.writeVibeNotchFiles(configDir: dir, previous: nil)
        let old = #"{"statusLine":{"type":"command","command":"old-status"}}"#
        let newer = #"{"statusLine":{"type":"command","command":"newer-status"}}"#
        try Data(old.utf8).write(to: URL(fileURLWithPath: dir + "/settings.json.superpowered-notch-20260101-000000-000.bak"))
        try Data(newer.utf8).write(to: URL(fileURLWithPath: dir + "/settings.json.superpowered-notch-20260102-000000-000.bak"))

        #expect(HookInstaller.removeLegacyHooks(configDir: dir, kinds: [.superpoweredVibeNotch]) == .removed)
        #expect(try settings(dir)["statusLine"]?["command"]?.stringValue == "newer-status")
    }
}
