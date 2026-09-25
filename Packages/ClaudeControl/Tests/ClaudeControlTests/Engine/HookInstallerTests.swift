import Foundation
import Testing
@testable import ClaudeControl

/// A settings.json like a real one: other tools' hooks on the same events
/// (including upstream Vibe Notch's and Vibe Island's), a status line,
/// permissions and env.
nonisolated let realisticSettings = """
{
  "$schema": "https://json.schemastore.org/claude-code-settings.json",
  "model": "opus",
  "env": {"BASH_DEFAULT_TIMEOUT_MS": "300000"},
  "permissions": {"allow": ["Bash(npm test:*)", "Read(~/notes/**)"], "deny": [], "defaultMode": "default"},
  "enabledPlugins": {"superpowers@claude-plugins-official": true},
  "hooks": {
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "~/bin/guard-bash.sh", "timeout": 30}]},
      {"matcher": "*", "hooks": [{"type": "command", "command": "python3 ~/.claude/hooks/claude-island-state.py"}]}
    ],
    "PermissionRequest": [
      {"matcher": "*", "hooks": [
        {"type": "command", "command": "/bin/sh -c '[ -x \\"$HOME/.vibe-island/bin/vibe-island-bridge\\" ] && \\"$HOME/.vibe-island/bin/vibe-island-bridge\\" --source claude; exit 0'", "timeout": 86400},
        {"type": "command", "command": "python3 ~/.claude/hooks/claude-island-state.py", "timeout": 86400}
      ]}
    ],
    "Stop": [{"hooks": [{"type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff"}]}],
    "TeammateIdle": [{"hooks": [{"type": "command", "command": "python3 '/old/path/hooks/superpowered-codenotch-hook.py'"}]}]
  },
  "statusLine": {"type": "command", "command": "~/.claude/statusline.sh", "padding": 2, "refreshInterval": 5}
}
"""

/// Test helpers over OrderedJSON.
nonisolated enum JSONTest {
    static func object(_ data: Data) throws -> OrderedJSON {
        let value = try OrderedJSON.parse(data)
        #expect(value.isObject)
        return value
    }

    static func object(_ text: String) throws -> OrderedJSON {
        try object(Data(text.utf8))
    }

    static func commands(in json: OrderedJSON, event: String) -> [String] {
        (json["hooks"]?[event]?.items ?? []).flatMap { ($0["hooks"]?.items ?? []).compactMap { $0["command"]?.stringValue } }
    }

    static func written(_ plan: HookInstaller.InstallPlan) throws -> OrderedJSON {
        guard case .write(let data) = plan.settings else {
            Issue.record("expected a write, got \(plan.settings)")
            return .object([])
        }
        return try object(data)
    }
}

struct HookInstallerPlanTests {
    let home = "/Users/me"
    var command: String { HookCommands.command(runningScript: "/Users/me/.claude-work/hooks/superpowered-codenotch-hook.py", python: "/usr/bin/python3") }
    var statusCommand: String { HookCommands.command(runningScript: "/Users/me/.claude-work/hooks/superpowered-codenotch-statusline.py", python: "/usr/bin/python3") }
    let latest = ClaudeCodeVersion(major: 2, minor: 1, patch: 280)

    private func install(_ data: Data?, statusLine: HookInstaller.StatusLineIntent = .leave, version: ClaudeCodeVersion? = nil,
                         saved: OrderedJSON? = nil, backup: OrderedJSON? = nil) -> HookInstaller.InstallPlan {
        HookInstaller.planInstall(existingData: data, hookCommand: command, version: version, statusLine: statusLine,
                                  savedPreviousStatusLine: saved, backupStatusLine: backup, home: home)
    }

    private func withoutOurHooks(_ json: OrderedJSON) -> OrderedJSON {
        var copy = json
        var cleaned: [OrderedJSON.Member] = []
        for member in json["hooks"]?.members ?? [] {
            let groups: [OrderedJSON] = (member.value.items ?? []).compactMap { group in
                let entries = (group["hooks"]?.items ?? []).filter { !HookInstaller.isOurHook($0["command"]?.stringValue ?? "", home: home) }
                if entries.isEmpty { return nil }
                var updated = group
                updated.set("hooks", .array(entries))
                return updated
            }
            if !groups.isEmpty { cleaned.append(OrderedJSON.Member(key: member.key, value: .array(groups))) }
        }
        copy.set("hooks", .object(cleaned))
        return copy
    }

    @Test func addsOurHooksAndLeavesEverythingElse() throws {
        let original = try JSONTest.object(realisticSettings)
        let plan = install(Data(realisticSettings.utf8), version: latest)
        let json = try JSONTest.written(plan)
        #expect(plan.previousStatusLine == nil)

        for event in ["UserPromptSubmit", "PreToolUse", "PostToolUse", "PermissionRequest", "Notification", "Stop",
                      "SubagentStop", "SessionStart", "SessionEnd", "PreCompact", "PostToolUseFailure", "SubagentStart",
                      "PostCompact", "StopFailure", "PermissionDenied", "TaskCreated", "TaskCompleted"] {
            #expect(JSONTest.commands(in: json, event: event).contains(command), "missing \(event)")
        }
        // Other tools' entries, including upstream Vibe Notch's and Vibe Island's, are untouched.
        #expect(JSONTest.commands(in: json, event: "PreToolUse").prefix(2) == ["~/bin/guard-bash.sh", "python3 ~/.claude/hooks/claude-island-state.py"])
        #expect(JSONTest.commands(in: json, event: "PermissionRequest").filter { $0.contains("claude-island-state.py") }.count == 1)
        #expect(JSONTest.commands(in: json, event: "PermissionRequest").first?.contains("vibe-island-bridge") == true)
        #expect(JSONTest.commands(in: json, event: "Stop").first == "afplay /System/Library/Sounds/Glass.aiff")

        // Our stale entry on an event we no longer register is gone.
        #expect(json["hooks"]?["TeammateIdle"] == nil)

        // Every non-hook key is exactly as it was, in the same order.
        #expect(json.members?.map(\.key) == original.members?.map(\.key))
        for member in original.members ?? [] where member.key != "hooks" {
            #expect(OrderedJSON.equivalent(json[member.key], member.value), "changed \(member.key)")
        }
        // Removing ours again gives back the original hooks (minus our stale one).
        var expectedHooks = original["hooks"] ?? .object([])
        expectedHooks.set("TeammateIdle", nil)
        #expect(OrderedJSON.equivalent(withoutOurHooks(json)["hooks"], expectedHooks))
    }

    @Test func permissionRequestWaitsForTheApp() throws {
        let json = try JSONTest.written(install(nil))
        let hook = json["hooks"]?["PermissionRequest"]?.items?.first?["hooks"]?.items?.first
        #expect(OrderedJSON.equivalent(hook?["timeout"], .int(86400)))
    }

    @Test func secondPlanIsANoOp() throws {
        let first = install(Data(realisticSettings.utf8), statusLine: .wrap(command: statusCommand), version: latest)
        guard case .write(let data) = first.settings, case .save(let saved) = first.previousStatusLine else {
            Issue.record("expected a write that saves the status line")
            return
        }
        let second = install(data, statusLine: .wrap(command: statusCommand), version: latest, saved: try JSONTest.object(saved))
        #expect(second == HookInstaller.InstallPlan(settings: .alreadyCurrent))
    }

    @Test func reformattedButEqualFileIsNotRewritten() throws {
        let first = try JSONTest.written(install(nil, version: latest))
        // Claude Code (or an editor) rewrote the file compactly, keys reordered, same content.
        let reordered = OrderedJSON.object((first.members ?? []).reversed())
        let compact = Data(reordered.serialized(unit: "").utf8)
        #expect(install(compact, version: latest).settings == .alreadyCurrent)
    }

    @Test(arguments: [
        "{\"hooks\": {",                     // truncated mid-save
        "[1, 2, 3]",                          // not an object
        "hello",                              // garbage
        "\"just a string\"",
    ])
    func refusesUnparseableSettings(_ text: String) {
        let data = Data(text.utf8)
        #expect(install(data, statusLine: .wrap(command: statusCommand), version: latest).settings == .refuse(.unreadable))
        #expect(HookInstaller.planUninstall(existingData: data, savedPreviousStatusLine: nil, home: home).settings == .refuse(.unreadable))
        #expect(HookInstaller.planLegacyRemoval(existingData: data, kinds: [.vibeNotch], home: home).settings == .refuse(.unreadable))
    }

    @Test func refusesBinaryGarbage() {
        #expect(install(Data([0xFF, 0xFE, 0x00, 0x7B])).settings == .refuse(.unreadable))
    }

    /// A `hooks` value that isn't an object is the user's, however odd.
    @Test(arguments: [#"{"hooks": []}"#, #"{"hooks": null}"#, #"{"hooks": "disabled"}"#, #"{"hooks": 3}"#])
    func refusesAHooksValueThatIsNotAnObject(_ text: String) {
        let data = Data(text.utf8)
        #expect(install(data).settings == .refuse(.hooksNotAnObject))
        #expect(HookInstaller.planUninstall(existingData: data, savedPreviousStatusLine: nil, home: home).settings == .refuse(.hooksNotAnObject))
        #expect(HookInstaller.planLegacyRemoval(existingData: data, kinds: Set(LegacyHookKind.allCases), home: home).settings
            == .refuse(.hooksNotAnObject))
    }

    @Test func malformedEventValuesAreLeftAlone() throws {
        let json = try JSONTest.written(install(Data(#"{"hooks":{"Stop":"not-a-list","PreToolUse":[]}}"#.utf8)))
        #expect(json["hooks"]?["Stop"]?.stringValue == "not-a-list")
        #expect(JSONTest.commands(in: json, event: "PreToolUse") == [command])
    }

    @Test func blankFileStartsFromEmpty() throws {
        let json = try JSONTest.written(install(Data("  \n".utf8)))
        #expect(JSONTest.commands(in: json, event: "Stop") == [command])
    }

    /// Commands written in an earlier form (`python3 '<script>'`) are
    /// replaced by the fail-open one, not kept beside it.
    @Test func olderCommandFormsAreReplaced() throws {
        let old = "python3 '/Users/me/.claude-work/hooks/superpowered-codenotch-hook.py'"
        let settings = #"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"\#(old)"}]}]}}"#
        let json = try JSONTest.written(install(Data(settings.utf8)))
        #expect(JSONTest.commands(in: json, event: "Stop") == [command])
    }

    @Test func versionGating() throws {
        func events(_ version: ClaudeCodeVersion?) -> Set<String> {
            Set(HookInstaller.hookEventConfigs(command: command, version: version).map(\.0))
        }
        let baseline: Set<String> = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "PermissionRequest", "Notification",
                                     "Stop", "SubagentStop", "SessionStart", "SessionEnd", "PreCompact"]
        #expect(events(nil) == baseline)
        #expect(!events(ClaudeCodeVersion(major: 2, minor: 1, patch: 32)).contains("TaskCompleted"))
        #expect(events(ClaudeCodeVersion(major: 2, minor: 1, patch: 33)).contains("TaskCompleted"))
        #expect(!events(ClaudeCodeVersion(major: 2, minor: 1, patch: 83)).contains("TaskCreated"))
        #expect(events(ClaudeCodeVersion(major: 2, minor: 1, patch: 84)).contains("TaskCreated"))
        // PermissionDenied shipped in 2.1.89 (the changelog has no 2.1.88).
        #expect(!events(ClaudeCodeVersion(major: 2, minor: 1, patch: 88)).contains("PermissionDenied"))
        #expect(events(ClaudeCodeVersion(major: 2, minor: 1, patch: 89)).contains("PermissionDenied"))
        #expect(events(latest) == baseline.union(["PostToolUseFailure", "SubagentStart", "TaskCompleted", "PostCompact",
                                                   "StopFailure", "TaskCreated", "PermissionDenied"]))
    }

    /// The version an account's hooks are written for is the lowest of every
    /// binary and every session seen, so an old client sharing the folder
    /// never meets an event name it doesn't know.
    @Test func effectiveVersionIsTheLowestKnown() {
        let new = ClaudeCodeVersion(major: 2, minor: 1, patch: 280)
        let old = ClaudeCodeVersion(major: 2, minor: 0, patch: 50)
        let middle = ClaudeCodeVersion(major: 2, minor: 1, patch: 90)
        #expect(AccountHookManager.effectiveVersion(binaries: [new], observed: nil, sessions: []) == new)
        #expect(AccountHookManager.effectiveVersion(binaries: [new, old], observed: nil, sessions: []) == old)
        #expect(AccountHookManager.effectiveVersion(binaries: [new], observed: middle, sessions: []) == middle)
        #expect(AccountHookManager.effectiveVersion(binaries: [new], observed: nil, sessions: [old]) == old)
        #expect(AccountHookManager.effectiveVersion(binaries: [], observed: nil, sessions: []) == nil)
    }

    // MARK: - Status Line

    @Test func wrapsTheExistingStatusLineKeepingEveryKey() throws {
        let settings = #"{"statusLine":{"type":"command","command":"~/.claude/statusline.sh","padding":2,"hideVimModeIndicator":true,"refreshInterval":5}}"#
        let original = try JSONTest.object(settings)
        let plan = install(Data(settings.utf8), statusLine: .wrap(command: statusCommand))
        let json = try JSONTest.written(plan)
        let statusLine = try #require(json["statusLine"])
        #expect(statusLine["command"]?.stringValue == statusCommand)
        #expect(statusLine.members?.map(\.key) == ["type", "command", "padding", "hideVimModeIndicator", "refreshInterval"])
        #expect(OrderedJSON.equivalent(statusLine["hideVimModeIndicator"], .bool(true)))

        guard case .save(let saved) = plan.previousStatusLine else {
            Issue.record("the previous status line must be saved")
            return
        }
        #expect(OrderedJSON.equivalent(try JSONTest.object(saved), original["statusLine"]))
    }

    @Test func uninstallRestoresTheStatusLineExactly() throws {
        let install = install(Data(realisticSettings.utf8), statusLine: .wrap(command: statusCommand), version: latest)
        guard case .write(let installed) = install.settings, case .save(let saved) = install.previousStatusLine else {
            Issue.record("expected install write")
            return
        }
        let uninstall = HookInstaller.planUninstall(existingData: installed, savedPreviousStatusLine: try JSONTest.object(saved), home: home)
        #expect(uninstall.previousStatusLine == .remove)
        guard case .write(let restored) = uninstall.settings else {
            Issue.record("expected uninstall write")
            return
        }
        // Byte for byte, minus our own stale entry that install cleaned up.
        var expected = try JSONTest.object(realisticSettings)
        var hooks = expected["hooks"] ?? .object([])
        hooks.set("TeammateIdle", nil)
        expected.set("hooks", hooks)
        #expect(try JSONTest.object(restored).isEquivalent(to: expected))
        #expect(try JSONTest.object(restored)["statusLine"]?.serialized() == expected["statusLine"]?.serialized())
    }

    /// Settings the user changed on the wrapper entry while wrapped stay
    /// through reinstalls and come along when it is unwrapped.
    @Test func userEditsOnTheWrapperAreKept() throws {
        let previous = try JSONTest.object(#"{"type":"command","command":"starship prompt","padding":1}"#)
        var wrapper = previous
        wrapper.set("command", .string(statusCommand))
        wrapper.set("padding", .int(5))
        wrapper.set("hideVimModeIndicator", .bool(true))
        let settings = Data(OrderedJSON.object(["statusLine": wrapper]).serialized().utf8)

        // Reinstall: the wrapper entry stays as the user left it.
        let reinstalled = try JSONTest.written(install(settings, statusLine: .wrap(command: statusCommand), saved: previous))
        #expect(reinstalled["statusLine"]?.serialized() == wrapper.serialized())

        // Unwrap: the saved command, with the user's padding and addition.
        let json = try JSONTest.written(HookInstaller.planUninstall(existingData: settings, savedPreviousStatusLine: previous, home: home))
        #expect(json["statusLine"]?["command"]?.stringValue == "starship prompt")
        #expect(OrderedJSON.equivalent(json["statusLine"]?["padding"], .int(5)))
        #expect(OrderedJSON.equivalent(json["statusLine"]?["hideVimModeIndicator"], .bool(true)))
    }

    /// An older installer added `padding: 0` to the wrapper; it isn't carried
    /// back onto a status line that never had one.
    @Test func olderWrapperPaddingIsNotCarriedBack() throws {
        let previous = try JSONTest.object(#"{"type":"command","command":"starship prompt"}"#)
        let wrapper = try JSONTest.object(#"{"type":"command","command":"\#(statusCommand.replacingOccurrences(of: "\"", with: "\\\""))","padding":0}"#)
        #expect(HookInstaller.restored(from: previous, wrapper: wrapper).serialized() == previous.serialized())
    }

    @Test func noPreviousStatusLineMeansRemoveOnUninstall() throws {
        let install = install(Data(#"{"model":"sonnet"}"#.utf8), statusLine: .wrap(command: statusCommand))
        // Saved as "nothing", so no older status line in a backup stands in for it.
        #expect(install.previousStatusLine == .chainNothing)
        guard case .write(let installed) = install.settings else { return }
        let restored = try JSONTest.written(HookInstaller.planUninstall(existingData: installed, savedPreviousStatusLine: nil, home: home))
        #expect(restored["statusLine"] == nil)
        #expect(restored["hooks"] == nil)
        #expect(restored["model"]?.stringValue == "sonnet")
    }

    /// previous.json lost: the status line comes back from a backup instead
    /// of disappearing.
    @Test func lostSavedStatusLineIsRestoredFromABackup() throws {
        let install = install(Data(realisticSettings.utf8), statusLine: .wrap(command: statusCommand), version: latest)
        guard case .write(let installed) = install.settings else { return }
        let backup = try JSONTest.object(realisticSettings)["statusLine"]
        let restored = try JSONTest.written(HookInstaller.planUninstall(
            existingData: installed, savedPreviousStatusLine: nil, backupStatusLine: backup, home: home))
        #expect(OrderedJSON.equivalent(restored["statusLine"], backup))
    }

    @Test func disablingTheIntegrationRestoresTheStatusLine() throws {
        let previous = try JSONTest.object(#"{"type": "command", "command": "~/.claude/statusline.sh", "padding": 2, "refreshInterval": 5}"#)
        let install = install(Data(realisticSettings.utf8), statusLine: .wrap(command: statusCommand), version: latest)
        guard case .write(let installed) = install.settings else { return }
        let disabled = HookInstaller.planInstall(existingData: installed, hookCommand: command, version: latest, statusLine: .unwrap,
                                                 savedPreviousStatusLine: previous, home: home)
        let json = try JSONTest.written(disabled)
        #expect(OrderedJSON.equivalent(json["statusLine"], previous))
        #expect(JSONTest.commands(in: json, event: "Stop").contains(command))
        #expect(disabled.previousStatusLine == .remove)
    }

    @Test func rewrapKeepsTheEntryAndUpdatesTheCommand() throws {
        let previous = try JSONTest.object(#"{"type":"command","command":"starship prompt","padding":3}"#)
        let settings = Data(#"{"statusLine":{"type":"command","command":"python '/x/hooks/superpowered-codenotch-statusline.py'","padding":3}}"#.utf8)
        let plan = install(settings, statusLine: .wrap(command: statusCommand), saved: previous)
        let json = try JSONTest.written(plan)
        #expect(json["statusLine"]?["command"]?.stringValue == statusCommand)
        #expect(OrderedJSON.equivalent(json["statusLine"]?["padding"], .int(3)))
        // Already ours: keep chaining to the saved status line, saved beside
        // this account's wrapper too (it may have come from another account's).
        guard case .save(let saved) = plan.previousStatusLine else {
            Issue.record("expected the chained status line to be saved")
            return
        }
        #expect(OrderedJSON.equivalent(try JSONTest.object(saved), previous))
    }

    @Test func rewrapNeverChainsToItself() throws {
        let ours = try JSONTest.object(#"{"type":"command","command":"python3 '/x/hooks/superpowered-codenotch-statusline.py'"}"#)
        let settings = Data(OrderedJSON.object(["statusLine": ours]).serialized().utf8)
        let plan = install(settings, statusLine: .wrap(command: statusCommand), saved: ours)
        #expect(plan.previousStatusLine == nil)
        let restored = try JSONTest.written(HookInstaller.planUninstall(existingData: settings, savedPreviousStatusLine: ours, home: home))
        #expect(restored["statusLine"] == nil)
    }

    @Test func leavesAStatusLineItDoesNotUnderstand() throws {
        let plan = install(Data(#"{"statusLine":"echo hi"}"#.utf8), statusLine: .wrap(command: statusCommand))
        let json = try JSONTest.written(plan)
        #expect(json["statusLine"]?.stringValue == "echo hi")
        #expect(plan.previousStatusLine == nil)
    }

    /// Superpowered Vibe Notch's wrapper is taken over, never wrapped.
    @Test func neverWrapsSuperpoweredVibeNotchsWrapper() throws {
        let settings = #"{"statusLine":{"type":"command","command":"python3 '/Users/me/.claude/hooks/superpowered-notch-statusline.py'","padding":0}}"#
        let plan = install(Data(settings.utf8), statusLine: .wrap(command: statusCommand))
        let json = try JSONTest.written(plan)
        #expect(json["statusLine"]?["command"]?.stringValue?.contains("superpowered-notch-statusline.py") == true)
        #expect(plan.previousStatusLine == nil)
    }

    // MARK: - Recognising commands

    @Test func scriptPathOfOurCommands() {
        let name = AppIdentity.hookScriptName
        #expect(HookCommands.scriptPath(in: "python3 '/Users/me/.claude-work/hooks/\(name)'", named: name, home: home)
            == "/Users/me/.claude-work/hooks/\(name)")
        #expect(HookCommands.scriptPath(in: HookInstaller.hookCommand(configDir: "/Users/me/My Claude's/.claude", python: "python3"),
                                        named: name, home: home) == "/Users/me/My Claude's/.claude/hooks/\(name)")
        #expect(HookCommands.scriptPath(in: command, named: name, home: home) == "/Users/me/.claude-work/hooks/\(name)")
        #expect(HookCommands.scriptPath(in: "python3 ~/.claude/hooks/\(name)", named: name, home: home) == "/Users/me/.claude/hooks/\(name)")
        #expect(HookCommands.scriptPath(in: "python3 \"$HOME/.claude/hooks/\(name)\"", named: name, home: home) == "/Users/me/.claude/hooks/\(name)")
        #expect(HookCommands.scriptPath(in: "/usr/bin/env python3 /x/hooks/\(name)", named: name, home: home) == "/x/hooks/\(name)")
        #expect(HookCommands.scriptPath(in: "python3 '/x/hooks/other.py'", named: name, home: home) == nil)
        #expect(HookCommands.scriptPath(in: "", named: name, home: home) == nil)
        #expect(HookCommands.scriptPath(in: "python3 relative/\(name)", named: name, home: home) == nil)
        // Merely mentioning the script is not running it.
        #expect(HookCommands.scriptPath(in: "~/bin/notify.sh --skip /x/hooks/\(name)", named: name, home: home) == nil)
        #expect(HookCommands.scriptPath(in: "echo /x/hooks/\(name)", named: name, home: home) == nil)
    }

    @Test func thirdPartyHooksMentioningAScriptAreNotRemoved() throws {
        let settings = #"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"~/bin/notify.sh --skip claude-island-state.py"},{"type":"command","command":"python3 ~/.claude/hooks/claude-island-state.py"}]}]}}"#
        let json = try JSONTest.written(HookInstaller.planLegacyRemoval(existingData: Data(settings.utf8), kinds: [.vibeNotch], home: home))
        #expect(JSONTest.commands(in: json, event: "Stop") == ["~/bin/notify.sh --skip claude-island-state.py"])
    }

    // MARK: - Legacy

    @Test func legacyRemovalOnlyRemovesClaudeIslandState() throws {
        let installed = try JSONTest.written(install(Data(realisticSettings.utf8), version: latest))
        let data = Data(installed.serialized().utf8)
        let plan = HookInstaller.planLegacyRemoval(existingData: data, kinds: [.vibeNotch], home: home)
        let cleaned = try JSONTest.written(plan)
        #expect(HookInstaller.firstHookCommand(in: cleaned) { HookInstaller.isLegacyHook($0, kind: .vibeNotch, home: home) } == nil)
        #expect(JSONTest.commands(in: cleaned, event: "PreToolUse") == ["~/bin/guard-bash.sh", command])
        #expect(JSONTest.commands(in: cleaned, event: "PermissionRequest").count == 2)  // vibe-island bridge + ours
        #expect(JSONTest.commands(in: cleaned, event: "PermissionRequest").first?.contains("vibe-island-bridge") == true)
        #expect(OrderedJSON.equivalent(cleaned["statusLine"], installed["statusLine"]))

        // Nothing legacy left: nothing to write.
        guard case .write(let cleanedData) = plan.settings else { return }
        #expect(HookInstaller.planLegacyRemoval(existingData: cleanedData, kinds: [.vibeNotch], home: home).settings == .alreadyCurrent)
    }

    @Test func vibeIslandRemovalTakesItsHooksAndStatusLine() throws {
        var settings = try JSONTest.object(realisticSettings)
        settings.set("statusLine", try JSONTest.object(#"{"type":"command","command":"~/.vibe-island/bin/vibe-island-statusline"}"#))
        let json = try JSONTest.written(HookInstaller.planLegacyRemoval(
            existingData: Data(settings.serialized().utf8), kinds: [.vibeIsland], home: home))
        #expect(!JSONTest.commands(in: json, event: "PermissionRequest").contains { $0.contains("vibe-island") })
        #expect(JSONTest.commands(in: json, event: "PermissionRequest") == ["python3 ~/.claude/hooks/claude-island-state.py"])
        #expect(json["statusLine"] == nil)
    }

    /// Ours wrapping Vibe Island's (silent) status line: ours stops chaining to it.
    @Test func vibeIslandRemovalDropsItFromOurChain() throws {
        let settings = Data(#"{"statusLine":{"type":"command","command":"python3 '/x/hooks/superpowered-codenotch-statusline.py'"}}"#.utf8)
        let saved = try JSONTest.object(#"{"type":"command","command":"$HOME/.vibe-island/bin/vibe-island-statusline"}"#)
        let plan = HookInstaller.planLegacyRemoval(existingData: settings, kinds: [.vibeIsland], savedPreviousStatusLine: saved, home: home)
        #expect(plan.settings == .alreadyCurrent)
        #expect(plan.previousStatusLine == .chainNothing)
    }
}

// MARK: - Settings stay byte for byte

/// The installer changes only the values it owns, in the file's own layout.
struct SettingsFidelityTests {
    let home = "/Users/me"
    var command: String { HookCommands.command(runningScript: "/Users/me/.claude/hooks/superpowered-codenotch-hook.py", python: "python3") }
    var statusCommand: String { HookCommands.command(runningScript: "/Users/me/.claude/hooks/superpowered-codenotch-statusline.py", python: "python3") }

    /// As Claude Code writes it: `JSON.stringify(value, null, 2)`, unsorted
    /// keys, numbers and escapes as the user typed them.
    let claudeStyle = """
    {
      "permissions": {
        "allow": [
          "Bash(npm test:*)"
        ],
        "deny": []
      },
      "feedbackSurveyRate": 0.1,
      "tiny": 1e-7,
      "one": 1.0,
      "path": "caf\\u00e9 \\/ slash",
      "env": {
        "ZED": "1",
        "ALPHA": "2"
      },
      "statusLine": {
        "type": "command",
        "command": "~/.claude/statusline.sh",
        "hideVimModeIndicator": true
      },
      "model": "opus"
    }
    """

    @Test func installThenUninstallGivesBackTheSameBytes() throws {
        let original = Data(claudeStyle.utf8)
        let install = HookInstaller.planInstall(existingData: original, hookCommand: command, version: nil,
                                                statusLine: .wrap(command: statusCommand), savedPreviousStatusLine: nil, home: home)
        guard case .write(let installed) = install.settings, case .save(let saved) = install.previousStatusLine else {
            Issue.record("expected an install write")
            return
        }
        let text = String(decoding: installed, as: UTF8.self)
        // Untouched members keep their exact spelling and order.
        for fragment in ["\"feedbackSurveyRate\": 0.1,", "\"tiny\": 1e-7,", "\"one\": 1.0,", #""path": "caf\u00e9 \/ slash","#,
                         "\"ZED\": \"1\",\n    \"ALPHA\": \"2\"", "\"deny\": []"] {
            #expect(text.contains(fragment), "lost \(fragment)")
        }
        #expect(text.hasSuffix("\n}"))

        let uninstall = HookInstaller.planUninstall(existingData: installed, savedPreviousStatusLine: try JSONTest.object(saved), home: home)
        guard case .write(let restored) = uninstall.settings else {
            Issue.record("expected an uninstall write")
            return
        }
        #expect(restored == original)
    }

    @Test func aCompactFileStaysCompact() throws {
        let original = #"{"model":"opus","env":{"B":"1","A":"2"}}"#
        let plan = HookInstaller.planInstall(existingData: Data(original.utf8), hookCommand: command, version: nil,
                                             statusLine: .leave, savedPreviousStatusLine: nil, home: home)
        guard case .write(let data) = plan.settings else { return }
        let text = String(decoding: data, as: UTF8.self)
        #expect(text.hasPrefix(#"{"model":"opus","env":{"B":"1","A":"2"},"hooks":{"#))
        #expect(!text.contains("\n"))
        let back = HookInstaller.planUninstall(existingData: data, savedPreviousStatusLine: nil, home: home)
        guard case .write(let restored) = back.settings else { return }
        #expect(String(decoding: restored, as: UTF8.self) == original)
    }
}

// MARK: - File IO against temporary config dirs

/// Runs the real installer against throwaway config dirs in the temporary folder.
@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
nonisolated struct HookInstallerFileTests {
    let configuration = HookInstaller.Configuration(
        python: "python3",
        version: ClaudeCodeVersion(major: 2, minor: 1, patch: 280),
        statusLineIntegration: true,
        hookScript: EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket),
        statusLineScript: EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket)
    )

    private func makeConfigDir(settings: String?) throws -> String {
        let root = TestPaths.temporaryRoot("installer")
        let dir = root + "/.claude-test"
        try FileManager.default.createDirectory(atPath: dir + "/projects", withIntermediateDirectories: true)
        if let settings {
            try Data(settings.utf8).write(to: URL(fileURLWithPath: dir + "/settings.json"))
        }
        return dir
    }

    private func cleanUp(_ dir: String) {
        try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent)
    }

    private func read(_ path: String) throws -> Data {
        try Data(contentsOf: URL(fileURLWithPath: path))
    }

    private func permissions(_ path: String) -> Int? {
        (try? FileManager.default.attributesOfItem(atPath: path)[.posixPermissions] as? NSNumber)?.intValue
    }

    private func backups(in dir: String) -> [String] {
        ((try? FileManager.default.contentsOfDirectory(atPath: dir)) ?? [])
            .filter { $0.hasPrefix(HookInstaller.backupPrefix) }
            .sorted()
    }

    @Test func installIsIdempotentAndBacksUp() throws {
        let dir = try makeConfigDir(settings: realisticSettings)
        defer { cleanUp(dir) }

        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)

        // Scripts in place and executable; previous status line saved, owner-only.
        #expect(FileManager.default.isExecutableFile(atPath: dir + "/hooks/" + AppIdentity.hookScriptName))
        #expect(FileManager.default.isExecutableFile(atPath: dir + "/hooks/" + AppIdentity.statusLineScriptName))
        let savedPath = dir + "/hooks/" + HookInstaller.previousStatusLineFileName
        #expect(HookInstaller.readSavedStatusLine(at: URL(fileURLWithPath: savedPath))?["command"]?.stringValue == "~/.claude/statusline.sh")
        #expect(permissions(savedPath) == 0o600)

        // Backups hold the original bytes, owner-only; the original is kept apart.
        let firstBackups = backups(in: dir)
        #expect(firstBackups.count == 1)
        #expect(try read(dir + "/" + firstBackups[0]) == Data(realisticSettings.utf8))
        #expect(permissions(dir + "/" + firstBackups[0]) == 0o600)
        #expect(try read(dir + "/" + HookInstaller.originalBackupName) == Data(realisticSettings.utf8))
        #expect(permissions(dir + "/" + HookInstaller.originalBackupName) == 0o600)

        let status = HookInstaller.readStatus(configDir: dir)
        #expect(status.hooksInstalled)
        #expect(status.statusLineInstalled)
        #expect(status.vibeNotchHooksPresent)
        #expect(status.vibeIslandHooksPresent)
        #expect(status.legacyHooksPresent)
        #expect(!status.superpoweredVibeNotchHooksPresent)
        #expect(status.settingsReadable)
        #expect(status.newestBackupPath == dir + "/" + firstBackups[0])

        // Second run: nothing written, no new backup.
        let before = try read(dir + "/settings.json")
        let modified = try FileManager.default.attributesOfItem(atPath: dir + "/settings.json")[.modificationDate] as? Date
        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .alreadyCurrent)
        #expect(try read(dir + "/settings.json") == before)
        #expect(try FileManager.default.attributesOfItem(atPath: dir + "/settings.json")[.modificationDate] as? Date == modified)
        #expect(backups(in: dir) == firstBackups)
    }

    @Test func keepsTheSettingsFilesPermissions() throws {
        let dir = try makeConfigDir(settings: #"{"env":{"API_KEY":"secret"}}"#)
        defer { cleanUp(dir) }
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: dir + "/settings.json")
        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)
        #expect(permissions(dir + "/settings.json") == 0o600)
        for backup in backups(in: dir) {
            #expect(permissions(dir + "/" + backup) == 0o600)
        }
    }

    @Test func refusesUnparseableSettingsWithoutTouchingAnything() throws {
        let garbage = "{\"permissions\": {\"allow\": [\"Bash(ls)\"]"
        let dir = try makeConfigDir(settings: garbage)
        defer { cleanUp(dir) }

        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .settingsUnreadable)
        #expect(try read(dir + "/settings.json") == Data(garbage.utf8))
        #expect(!FileManager.default.fileExists(atPath: dir + "/hooks"))
        #expect(backups(in: dir).isEmpty)
        #expect(HookInstaller.readStatus(configDir: dir).settingsReadable == false)
        #expect(HookInstaller.uninstall(configDir: dir) == .settingsUnreadable)
        #expect(try read(dir + "/settings.json") == Data(garbage.utf8))
    }

    @Test func refusesAHooksValueThatIsNotAnObject() throws {
        let settings = #"{"hooks": [], "model": "opus"}"#
        let dir = try makeConfigDir(settings: settings)
        defer { cleanUp(dir) }
        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .settingsHooksMalformed)
        #expect(try read(dir + "/settings.json") == Data(settings.utf8))
        #expect(!FileManager.default.fileExists(atPath: dir + "/hooks"))
        #expect(HookInstaller.readStatus(configDir: dir).settingsReadable == false)
    }

    /// A settings.json that links to a file that isn't there (a dotfiles repo
    /// not cloned yet) stays a link.
    @Test func refusesADanglingSymlink() throws {
        let dir = try makeConfigDir(settings: nil)
        defer { cleanUp(dir) }
        let target = (dir as NSString).deletingLastPathComponent + "/dotfiles/claude/settings.json"
        try FileManager.default.createSymbolicLink(atPath: dir + "/settings.json", withDestinationPath: target)

        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .settingsLinkBroken(target))
        #expect(HookInstaller.uninstall(configDir: dir) == .settingsLinkBroken(target))
        #expect((try? FileManager.default.destinationOfSymbolicLink(atPath: dir + "/settings.json")) == target)
        #expect(!FileManager.default.fileExists(atPath: target))
        #expect(!FileManager.default.fileExists(atPath: dir + "/hooks"))
        #expect(HookInstaller.readStatus(configDir: dir).settingsReadable == false)
    }

    @Test func uninstallRestoresTheOriginal() throws {
        let dir = try makeConfigDir(settings: realisticSettings)
        defer { cleanUp(dir) }

        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)
        #expect(HookInstaller.uninstall(configDir: dir) == .removed)

        let restored = try JSONTest.object(try read(dir + "/settings.json"))
        var expected = try JSONTest.object(realisticSettings)
        var hooks = expected["hooks"] ?? .object([])
        hooks.set("TeammateIdle", nil)  // our own stale entry
        expected.set("hooks", hooks)
        #expect(restored.isEquivalent(to: expected))

        for name in [AppIdentity.hookScriptName, AppIdentity.statusLineScriptName, HookInstaller.previousStatusLineFileName] {
            #expect(!FileManager.default.fileExists(atPath: dir + "/hooks/" + name))
        }
        #expect(HookInstaller.uninstall(configDir: dir) == .alreadyCurrent)
    }

    /// previous.json deleted by hand: uninstall brings the status line back
    /// from the newest backup rather than deleting it.
    @Test func uninstallWithoutTheSavedStatusLineUsesABackup() throws {
        let dir = try makeConfigDir(settings: realisticSettings)
        defer { cleanUp(dir) }
        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)
        try FileManager.default.removeItem(atPath: dir + "/hooks/" + HookInstaller.previousStatusLineFileName)

        #expect(HookInstaller.uninstall(configDir: dir) == .removed)
        let restored = try JSONTest.object(try read(dir + "/settings.json"))
        #expect(OrderedJSON.equivalent(restored["statusLine"], try JSONTest.object(realisticSettings)["statusLine"]))
    }

    @Test func disablingTheIntegrationUnwrapsAndRemovesTheWrapper() throws {
        let dir = try makeConfigDir(settings: realisticSettings)
        defer { cleanUp(dir) }

        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)
        var off = configuration
        off.statusLineIntegration = false
        #expect(HookInstaller.install(configDir: dir, configuration: off) == .installed)

        let json = try JSONTest.object(try read(dir + "/settings.json"))
        #expect(json["statusLine"]?["command"]?.stringValue == "~/.claude/statusline.sh")
        #expect(!FileManager.default.fileExists(atPath: dir + "/hooks/" + AppIdentity.statusLineScriptName))
        #expect(!FileManager.default.fileExists(atPath: dir + "/hooks/" + HookInstaller.previousStatusLineFileName))
        let status = HookInstaller.readStatus(configDir: dir)
        #expect(status.hooksInstalled && !status.statusLineInstalled)
    }

    @Test func legacyRemovalOnDisk() throws {
        let dir = try makeConfigDir(settings: realisticSettings)
        defer { cleanUp(dir) }

        #expect(HookInstaller.removeLegacyHooks(configDir: dir, kinds: [.vibeNotch]) == .removed)
        let status = HookInstaller.readStatus(configDir: dir)
        #expect(!status.vibeNotchHooksPresent)
        #expect(status.vibeIslandHooksPresent)
        let text = String(decoding: try read(dir + "/settings.json"), as: UTF8.self)
        #expect(text.contains("vibe-island-bridge"))
        #expect(text.contains("guard-bash.sh"))
        #expect(text.contains("~/.claude/statusline.sh"))
        #expect(HookInstaller.removeLegacyHooks(configDir: dir, kinds: [.vibeNotch]) == .alreadyCurrent)
        #expect(backups(in: dir).count == 1)
    }

    @Test func freshConfigDirWithoutSettings() throws {
        let dir = try makeConfigDir(settings: nil)
        defer { cleanUp(dir) }

        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)
        #expect(backups(in: dir).isEmpty)
        #expect(!FileManager.default.fileExists(atPath: dir + "/" + HookInstaller.originalBackupName))
        let json = try JSONTest.object(try read(dir + "/settings.json"))
        #expect(HookInstaller.isOurStatusLine(json["statusLine"]))
        // Nothing to chain, said explicitly.
        #expect(try read(dir + "/hooks/" + HookInstaller.previousStatusLineFileName) == HookInstaller.chainsNothing)
    }

    @Test func missingConfigDirIsReported() {
        let dir = TestPaths.temporaryRoot("installer") + "/.claude-nope"
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }
        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .configDirMissing)
        #expect(!FileManager.default.fileExists(atPath: dir))
    }

    @Test func writesThroughASymlinkedSettingsFile() throws {
        let dir = try makeConfigDir(settings: nil)
        let root = (dir as NSString).deletingLastPathComponent
        defer { cleanUp(dir) }
        let real = root + "/dotfiles-settings.json"
        try Data(#"{"model":"opus"}"#.utf8).write(to: URL(fileURLWithPath: real))
        try FileManager.default.setAttributes([.posixPermissions: 0o640], ofItemAtPath: real)
        try FileManager.default.createSymbolicLink(atPath: dir + "/settings.json", withDestinationPath: real)

        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)
        #expect((try? FileManager.default.destinationOfSymbolicLink(atPath: dir + "/settings.json")) == real)
        #expect(String(decoding: try read(real), as: UTF8.self).contains(AppIdentity.hookScriptName))
        #expect(permissions(real) == 0o640)
    }

    /// A settings.json copied from another account runs that account's
    /// wrapper; installing here must keep chaining to the same status line.
    @Test func copiedSettingsKeepTheStatusLineChain() throws {
        let first = try makeConfigDir(settings: realisticSettings)
        let second = try makeConfigDir(settings: nil)
        defer {
            cleanUp(first)
            cleanUp(second)
        }
        let original = try JSONTest.object(realisticSettings)

        #expect(HookInstaller.install(configDir: first, configuration: configuration) == .installed)
        try FileManager.default.copyItem(atPath: first + "/settings.json", toPath: second + "/settings.json")
        #expect(HookInstaller.install(configDir: second, configuration: configuration) == .installed)

        let json = try JSONTest.object(try read(second + "/settings.json"))
        #expect(json["statusLine"]?["command"]?.stringValue == HookInstaller.statusLineCommand(configDir: second, python: "python3"))
        let saved = HookInstaller.readSavedStatusLine(at: URL(fileURLWithPath: second + "/hooks/" + HookInstaller.previousStatusLineFileName))
        #expect(OrderedJSON.equivalent(saved, original["statusLine"]))

        #expect(HookInstaller.uninstall(configDir: second) == .removed)
        let restored = try JSONTest.object(try read(second + "/settings.json"))
        #expect(OrderedJSON.equivalent(restored["statusLine"], original["statusLine"]))
    }

    /// Two accounts whose settings.json is one file (a symlink): either one's
    /// status reads true, and uninstalling through either restores the
    /// original status line, whichever account's wrapper is installed.
    @Test func sharedSettingsFileWorksFromEitherAccount() throws {
        let first = try makeConfigDir(settings: realisticSettings)
        let second = try makeConfigDir(settings: nil)
        defer {
            cleanUp(first)
            cleanUp(second)
        }
        try FileManager.default.createSymbolicLink(atPath: second + "/settings.json", withDestinationPath: first + "/settings.json")
        #expect(AccountHookManager.settingsFileIdentity(configDir: first) == AccountHookManager.settingsFileIdentity(configDir: second))
        let original = try JSONTest.object(realisticSettings)

        #expect(HookInstaller.install(configDir: first, configuration: configuration) == .installed)
        let status = HookInstaller.readStatus(configDir: second)
        #expect(status.hooksInstalled && status.statusLineInstalled)
        #expect(!FileManager.default.fileExists(atPath: second + "/hooks"))

        #expect(HookInstaller.uninstall(configDir: second) == .removed)
        let restored = try JSONTest.object(try read(first + "/settings.json"))
        #expect(OrderedJSON.equivalent(restored["statusLine"], original["statusLine"]))
        #expect(HookInstaller.readStatus(configDir: first).hooksInstalled == false)
        #expect(HookInstaller.uninstall(configDir: first) == .alreadyCurrent)
        #expect(!FileManager.default.fileExists(atPath: first + "/hooks/" + HookInstaller.previousStatusLineFileName))
    }

    @Test func keepsOnlyTheNewestBackupsAndTheOriginal() throws {
        let dir = try makeConfigDir(settings: #"{"model":"opus"}"#)
        defer { cleanUp(dir) }
        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)
        for index in 0..<7 {
            let name = HookInstaller.backupPrefix + "20260101-00000\(index)-000" + HookInstaller.backupSuffix
            try Data("old \(index)".utf8).write(to: URL(fileURLWithPath: dir + "/" + name))
        }
        // Toggle the status line a few times: every write backs up, the
        // rotation keeps five, and the pre-install original stays.
        var configuration = configuration
        for round in 0..<4 {
            configuration.statusLineIntegration = round % 2 == 1
            #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)
        }
        #expect(backups(in: dir).count == HookInstaller.maxBackups)
        #expect(try read(dir + "/" + HookInstaller.originalBackupName) == Data(#"{"model":"opus"}"#.utf8))
    }

    /// The command Claude Code runs exits 0 (not the blocking 2) when the
    /// script is gone: a moved config folder must not break Claude Code.
    @Test(arguments: ["python3", "/usr/bin/python3", "/nonexistent/python3"])
    func registeredCommandsFailOpenWhenTheScriptIsMissing(python: String) throws {
        let dir = try makeConfigDir(settings: nil)
        defer { cleanUp(dir) }
        var configuration = configuration
        configuration.python = python
        #expect(HookInstaller.install(configDir: dir, configuration: configuration) == .installed)
        let json = try JSONTest.object(try read(dir + "/settings.json"))
        let hook = try #require(HookInstaller.firstHookCommand(in: json) { HookInstaller.isOurHook($0) })
        let statusLine = try #require(json["statusLine"]?["command"]?.stringValue)

        // As installed, with the app not running: exit 0.
        #expect(try TestShell.run(hook, stdin: "{}").status == 0)
        // The folder moved away: still exit 0, and nothing printed.
        try FileManager.default.removeItem(atPath: dir + "/hooks")
        for command in [hook, statusLine] {
            let result = try TestShell.run(command, stdin: "{}")
            #expect(result.status == 0)
            #expect(result.stdout.isEmpty)
        }
    }

    /// A save by Claude Code that lands while we plan is kept: we plan again
    /// from its bytes instead of writing over them.
    @Test func aConcurrentSaveIsNotLost() throws {
        let dir = try makeConfigDir(settings: #"{"model":"opus"}"#)
        defer { cleanUp(dir) }
        let settingsPath = dir + "/settings.json"
        var interfered = false
        let outcome = HookInstaller.applyChange(configDir: dir, writtenOutcome: .installed, beforeCommit: {
            guard !interfered else { return }
            interfered = true
            // Claude Code saves a permission rule meanwhile.
            try? Data(#"{"model":"opus","permissions":{"allow":["Bash(ls)"]}}"#.utf8)
                .write(to: URL(fileURLWithPath: settingsPath), options: .atomic)
        }) { context in
            HookInstaller.planInstall(existingData: context.data, hookCommand: "python3 '/x/hooks/superpowered-codenotch-hook.py'",
                                      version: nil, statusLine: .leave, savedPreviousStatusLine: nil)
        }
        #expect(outcome == .installed)
        #expect(interfered)
        let json = try JSONTest.object(try read(settingsPath))
        #expect(json["permissions"]?["allow"]?.items?.first?.stringValue == "Bash(ls)")
        #expect(json["hooks"] != nil)
    }
}

/// The guard that keeps tests (which run before any bootstrap, with the real
/// home) out of the real Claude folders.
struct InstallerGuardTests {
    @Test func realClaudeFoldersAreProtectedBeforeBootstrap() {
        guard !AppIdentity.isFrozen, let entry = getpwuid(getuid()), let dir = entry.pointee.pw_dir else { return }
        let realHome = String(cString: dir)
        for path in [realHome, realHome + "/.claude", realHome + "/.claude-work", realHome + "/.claude_x",
                     realHome + "/.config/claude", realHome + "/.config/claude-alt"] {
            #expect(HookInstaller.isProtectedBeforeBootstrap(configDir: path), "\(path)")
        }
        for path in [NSTemporaryDirectory() + "spcn-x/.claude", "/tmp/x/.claude", realHome + "/Documents/.claude-copy"] {
            #expect(!HookInstaller.isProtectedBeforeBootstrap(configDir: path), "\(path)")
        }
    }

    @Test func resolvesAnInterpreterPastTheShim() {
        let present: Set<String> = ["/Applications/Xcode.app/Contents/Developer/usr/bin/python3", "/opt/homebrew/bin/python3"]
        #expect(HookInstaller.resolvePython(developerDir: "/Applications/Xcode.app/Contents/Developer", isExecutable: present.contains)
            == "/Applications/Xcode.app/Contents/Developer/usr/bin/python3")
        #expect(HookInstaller.resolvePython(developerDir: nil, isExecutable: present.contains) == "/opt/homebrew/bin/python3")
        #expect(HookInstaller.resolvePython(developerDir: nil, isExecutable: { _ in false }) == "python3")
    }
}

/// Runs a command the way Claude Code runs a hook: `/bin/sh -c`, JSON on stdin.
nonisolated enum TestShell {
    struct Result {
        let status: Int32
        let stdout: String
    }

    static func run(_ command: String, stdin: String, environment: [String: String] = ["PATH": "/usr/bin:/bin"]) throws -> Result {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", command]
        process.environment = environment
        let input = Pipe()
        let output = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try process.run()
        try? input.fileHandleForWriting.write(contentsOf: Data(stdin.utf8))
        try? input.fileHandleForWriting.close()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return Result(status: process.terminationStatus, stdout: String(decoding: data, as: UTF8.self))
    }
}
