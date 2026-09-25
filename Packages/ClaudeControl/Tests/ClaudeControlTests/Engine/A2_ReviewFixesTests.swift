import Foundation
import Testing
@testable import ClaudeControl

/// Regressions found in the A2 review: each test pins one defect.
@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
nonisolated struct ReviewedInstallerTests {
    private func configuration(statusLine: Bool, python: String = "python3") -> HookInstaller.Configuration {
        HookInstaller.Configuration(
            python: python,
            version: ClaudeCodeVersion(major: 2, minor: 1, patch: 280),
            statusLineIntegration: statusLine,
            hookScript: EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket),
            statusLineScript: EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket)
        )
    }

    private func makeConfigDir(settings: String?) throws -> String {
        let root = TestPaths.temporaryRoot("review")
        let dir = root + "/.claude-test"
        try FileManager.default.createDirectory(atPath: dir + "/projects", withIntermediateDirectories: true)
        if let settings {
            try Data(settings.utf8).write(to: URL(fileURLWithPath: dir + "/settings.json"))
        }
        return dir
    }

    private func settings(_ dir: String) throws -> OrderedJSON {
        try JSONTest.object(Data(contentsOf: URL(fileURLWithPath: dir + "/settings.json")))
    }

    /// A status line the user removed while ours wrapped nothing must not
    /// come back from an older backup: not chained by a rewrite of the
    /// wrapper, and not restored when the integration is turned off.
    @Test func aStatusLineTheUserRemovedStaysRemoved() throws {
        let dir = try makeConfigDir(settings: #"{"model":"opus","statusLine":{"type":"command","command":"echo old"}}"#)
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }

        // Wrap `echo old`, then give it back.
        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: true)) == .installed)
        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: false)) == .installed)
        #expect(try settings(dir)["statusLine"]?["command"]?.stringValue == "echo old")

        // The user deletes their status line, then turns the integration on:
        // ours wraps nothing.
        var edited = try settings(dir)
        edited.set("statusLine", nil)
        try Data(edited.serialized().utf8).write(to: URL(fileURLWithPath: dir + "/settings.json"))
        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: true)) == .installed)
        let previous = URL(fileURLWithPath: dir + "/hooks/" + HookInstaller.previousStatusLineFileName)
        #expect(try Data(contentsOf: previous) == HookInstaller.chainsNothing)

        // A rewrite of our entries (a new interpreter path) keeps chaining nothing.
        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: true, python: "/usr/bin/python3")) == .installed)
        #expect(try Data(contentsOf: previous) == HookInstaller.chainsNothing)

        // Even with the saved copy lost, the backups say there was none.
        try FileManager.default.removeItem(at: previous)
        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: true)) == .installed)
        #expect(!FileManager.default.fileExists(atPath: previous.path))

        // Off again: no status line, as the user left it.
        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: false)) == .installed)
        #expect(try settings(dir)["statusLine"] == nil)
    }

    /// The backups stand in for a lost saved copy with what the status line
    /// was right before a wrapper took it: the newest backup not taken while
    /// wrapped decides, even when it had none.
    @Test func backupsSayWhatTheStatusLineWasBeforeTheWrapper() throws {
        let dir = try makeConfigDir(settings: nil)
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }
        let settingsURL = URL(fileURLWithPath: dir + "/settings.json")
        let wrapper = #"{"statusLine":{"type":"command","command":"python3 '\#(dir)/hooks/superpowered-codenotch-statusline.py'"}}"#
        func backup(_ stamp: String, _ text: String) throws {
            try Data(text.utf8).write(to: URL(fileURLWithPath: dir + "/" + HookInstaller.backupPrefix + stamp + HookInstaller.backupSuffix))
        }
        func scan() -> OrderedJSON? {
            HookInstaller.newestStatusLine(inBackupsBeside: settingsURL,
                                           prefixes: [HookInstaller.backupPrefix, HookInstaller.originalBackupName], home: "/Users/me")
        }

        try Data(#"{"statusLine":{"type":"command","command":"first"}}"#.utf8)
            .write(to: URL(fileURLWithPath: dir + "/" + HookInstaller.originalBackupName))
        try backup("20260101-000000-000", #"{"statusLine":{"type":"command","command":"old"}}"#)
        try backup("20260102-000000-000", wrapper)
        #expect(scan()?["command"]?.stringValue == "old")

        // Newer: the user had no status line when ours wrapped again.
        try backup("20260103-000000-000", #"{"model":"opus"}"#)
        try backup("20260104-000000-000", wrapper)
        #expect(scan() == nil)

        // Only wrapped states left in the rotation: the original decides.
        for name in try FileManager.default.contentsOfDirectory(atPath: dir) where name.hasPrefix(HookInstaller.backupPrefix) {
            try FileManager.default.removeItem(atPath: dir + "/" + name)
        }
        try backup("20260105-000000-000", wrapper)
        #expect(scan()?["command"]?.stringValue == "first")
    }

    /// "Remove Vibe Island hooks" while ours chains to its status line: ours
    /// chains to nothing from then on, even after a rewrite and a turn-off
    /// (the backups hold Vibe Island's status line, which must not return).
    @Test func vibeIslandsStatusLineStaysGoneAfterRemoval() throws {
        let vibeIsland = #"{"type":"command","command":"$HOME/.vibe-island/bin/vibe-island-statusline"}"#
        let dir = try makeConfigDir(settings: #"{"statusLine":\#(vibeIsland)}"#)
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }

        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: true)) == .installed)
        let previous = URL(fileURLWithPath: dir + "/hooks/" + HookInstaller.previousStatusLineFileName)
        #expect(HookInstaller.readSavedStatusLine(at: previous)?["command"]?.stringValue?.contains("vibe-island") == true)

        #expect(HookInstaller.removeLegacyHooks(configDir: dir, kinds: [.vibeIsland]) == .alreadyCurrent)
        #expect(try Data(contentsOf: previous) == HookInstaller.chainsNothing)

        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: true, python: "/usr/bin/python3")) == .installed)
        #expect(try Data(contentsOf: previous) == HookInstaller.chainsNothing)
        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: false)) == .installed)
        #expect(try settings(dir)["statusLine"] == nil)
    }

    /// The wrapper treats the "nothing" marker as no previous command.
    @Test func theWrapperChainsNothingForTheMarker() throws {
        let dir = try makeConfigDir(settings: #"{"model":"opus"}"#)
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }
        #expect(HookInstaller.install(configDir: dir, configuration: configuration(statusLine: true)) == .installed)
        let previous = URL(fileURLWithPath: dir + "/hooks/" + HookInstaller.previousStatusLineFileName)
        #expect(try Data(contentsOf: previous) == HookInstaller.chainsNothing)

        let command = try #require(try settings(dir)["statusLine"]?["command"]?.stringValue)
        let result = try TestShell.run(command, stdin: #"{"session_id":"s"}"#,
                                       environment: ["PATH": "/usr/bin:/bin", "SPCN_DEV": "1", "SPCN_SOCKET": dir + "/missing.sock"])
        #expect(result.status == 0)
        #expect(result.stdout.isEmpty)
    }
}

/// Discovery and folder checks over a throwaway home.
nonisolated struct ReviewedAccountFolderTests {
    /// A copy of ~/.claude keeps the session files of what ran when it was
    /// made; none of those processes run, so it is only suggested.
    @Test func staleSessionFilesDoNotAddAFolder() throws {
        let home = TestPaths.temporaryRoot("review-home")
        defer { try? FileManager.default.removeItem(atPath: home) }
        for (folder, pid) in [(".claude-mine", 99_001), (".claude-live", 99_002)] {
            try FileManager.default.createDirectory(atPath: "\(home)/\(folder)/sessions", withIntermediateDirectories: true)
            try Data(#"{"pid":\#(pid)}"#.utf8).write(to: URL(fileURLWithPath: "\(home)/\(folder)/sessions/\(pid).json"))
        }
        let found = AccountRegistry.discover(home: home, isAlive: { $0 == 99_002 })
        #expect(found.accounts == [home + "/.claude-live"])
        #expect(found.suggestions == [AccountFolderSuggestion(configDir: home + "/.claude-mine", reason: .found)])
        #expect(!AccountRegistry.hasLiveSession(home + "/.claude-mine", isAlive: { $0 == 99_002 }))
        #expect(AccountRegistry.hasLiveSession(home + "/.claude-live", isAlive: { $0 == 99_002 }))
    }

    /// A link to the home folder (or to a folder holding it) is refused like
    /// the home folder itself.
    @Test func aLinkToHomeIsHome() throws {
        let root = TestPaths.temporaryRoot("review-link")
        defer { try? FileManager.default.removeItem(atPath: root) }
        let home = root + "/home"
        try FileManager.default.createDirectory(atPath: home + "/.claude", withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(atPath: home + "/.claude-link", withDestinationPath: home)
        try FileManager.default.createSymbolicLink(atPath: home + "/.claude-up", withDestinationPath: root)
        #expect(throws: AccountFolderError.homeFolder) {
            try AccountRegistry.checkFolder(home + "/.claude-link", home: home, accounts: [])
        }
        #expect(throws: AccountFolderError.containsAccounts) {
            try AccountRegistry.checkFolder(home + "/.claude-up", home: home, accounts: [])
        }
    }
}

/// Regressions from the second A2 review pass.
@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
nonisolated struct SecondReviewInstallerTests {
    private let home = "/Users/me"

    private func configuration(statusLine: Bool = true) -> HookInstaller.Configuration {
        HookInstaller.Configuration(
            python: "python3",
            version: ClaudeCodeVersion(major: 2, minor: 1, patch: 280),
            statusLineIntegration: statusLine,
            hookScript: EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket),
            statusLineScript: EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket)
        )
    }

    private func makeConfigDir(settings: String?) throws -> String {
        let dir = TestPaths.temporaryRoot("review2") + "/.claude-test"
        try FileManager.default.createDirectory(atPath: dir + "/projects", withIntermediateDirectories: true)
        if let settings {
            try Data(settings.utf8).write(to: URL(fileURLWithPath: dir + "/settings.json"))
        }
        return dir
    }

    private func settings(_ dir: String) throws -> OrderedJSON {
        try JSONTest.object(Data(contentsOf: URL(fileURLWithPath: dir + "/settings.json")))
    }

    /// The hooks folder deleted (the saved status line with it): the next
    /// pass puts the scripts back and, although settings.json needs no
    /// change, the saved status line too, so the wrapper chains the user's
    /// status line again instead of printing nothing until something else
    /// rewrites settings.json.
    @Test func aLostSavedStatusLineIsPutBackWhileWrapped() throws {
        let dir = try makeConfigDir(settings: #"{"statusLine":{"type":"command","command":"printf mine","padding":1}}"#)
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }
        #expect(HookInstaller.install(configDir: dir, configuration: configuration()) == .installed)
        let previous = URL(fileURLWithPath: dir + "/hooks/" + HookInstaller.previousStatusLineFileName)
        let saved = try Data(contentsOf: previous)
        let settingsBefore = try Data(contentsOf: URL(fileURLWithPath: dir + "/settings.json"))

        try FileManager.default.removeItem(atPath: dir + "/hooks")
        #expect(!HookInstaller.readStatus(configDir: dir).hooksInstalled)

        #expect(HookInstaller.install(configDir: dir, configuration: configuration()) == .alreadyCurrent)
        #expect(try Data(contentsOf: URL(fileURLWithPath: dir + "/settings.json")) == settingsBefore)
        #expect(HookInstaller.readSavedStatusLine(at: previous)?.isEquivalent(to: try JSONTest.object(saved)) == true)
        let status = HookInstaller.readStatus(configDir: dir)
        #expect(status.hooksInstalled && status.statusLineInstalled)

        let command = try #require(try settings(dir)["statusLine"]?["command"]?.stringValue)
        let result = try TestShell.run(command, stdin: #"{"session_id":"s"}"#,
                                       environment: ["PATH": "/usr/bin:/bin", "SPCN_DEV": "1", "SPCN_SOCKET": dir + "/missing.sock"])
        #expect(result.stdout == "mine")

        // Nothing more to do on the next pass, and no staging files left behind.
        #expect(HookInstaller.install(configDir: dir, configuration: configuration()) == .alreadyCurrent)
        let leftovers = try FileManager.default.contentsOfDirectory(atPath: dir) + FileManager.default.contentsOfDirectory(atPath: dir + "/hooks")
        #expect(!leftovers.contains { $0.hasSuffix(".tmp") })
    }

    /// Superpowered Vibe Notch ran again after the takeover and wrapped
    /// ours. Taking over again puts back what ours chained (the user's
    /// current status line), not an older one from its backups, and never
    /// leaves either wrapper as the status line.
    @Test func takeoverAfterItWrappedOursFollowsOurChain() throws {
        let configDir = home + "/.claude"
        let ours = OrderedJSON.object([
            "type": .string("command"),
            "command": .string(HookInstaller.statusLineCommand(configDir: configDir, python: "python3")),
            "padding": .int(2),
        ])
        let current = OrderedJSON.object(["type": .string("command"), "command": .string("echo current"), "refreshInterval": .int(5)])
        let stale = OrderedJSON.object(["type": .string("command"), "command": .string("echo stale")])
        let data = Data(TakeoverFixture.settings(configDir: configDir).utf8)

        // Its saved copy is our wrapper: follow ours to what it chains.
        var json = try JSONTest.written(HookInstaller.planLegacyRemoval(
            existingData: data, kinds: [.superpoweredVibeNotch],
            vibeNotchPreviousStatusLine: ours, vibeNotchBackupStatusLine: stale,
            savedPreviousStatusLine: current, home: home))
        #expect(json["statusLine"]?["command"]?.stringValue == "echo current")
        #expect(OrderedJSON.equivalent(json["statusLine"]?["refreshInterval"], .int(5)))
        #expect(OrderedJSON.equivalent(json["statusLine"]?["padding"], .int(2)))
        #expect(JSONTest.commands(in: json, event: "Stop").isEmpty)

        // Our saved copy lost too: our backups say what ours chained.
        json = try JSONTest.written(HookInstaller.planLegacyRemoval(
            existingData: data, kinds: [.superpoweredVibeNotch],
            vibeNotchPreviousStatusLine: ours, vibeNotchBackupStatusLine: stale,
            savedPreviousStatusLine: nil, backupStatusLine: current, home: home))
        #expect(json["statusLine"]?["command"]?.stringValue == "echo current")

        // Ours chained nothing: no status line (not the stale one).
        json = try JSONTest.written(HookInstaller.planLegacyRemoval(
            existingData: data, kinds: [.superpoweredVibeNotch],
            vibeNotchPreviousStatusLine: ours, vibeNotchBackupStatusLine: stale,
            savedPreviousStatusLine: .object([]), home: home))
        #expect(json["statusLine"] == nil)
    }

    /// A write for the hooks alone (an older Claude Code showed up) leaves
    /// our status line entry's bytes as they were, however they are laid out.
    @Test func aHooksOnlyWriteKeepsTheStatusLineBytes() throws {
        let configDir = home + "/.claude"
        let hook = HookInstaller.hookCommand(configDir: configDir, python: "python3")
        let wrapper = HookInstaller.statusLineCommand(configDir: configDir, python: "python3")
        let saved = OrderedJSON.object(["type": .string("command"), "command": .string("echo hi")])
        let installed = try JSONTest.written(HookInstaller.planInstall(
            existingData: Data(#"{"model": "opus", "statusLine": {"type": "command", "command": "echo hi"}}"#.utf8),
            hookCommand: hook, version: ClaudeCodeVersion(major: 2, minor: 1, patch: 280),
            statusLine: .wrap(command: wrapper), savedPreviousStatusLine: nil, home: home))
        // Laid out by someone else: the entry on one line, in the middle of the file.
        var edited = installed
        let entry = try #require(installed["statusLine"])
        edited.set("statusLine", nil)
        let compactEntry = entry.serialized(indent: "", unit: "")
        var text = edited.serialized()
        text.insert(contentsOf: "\n  \"statusLine\": \(compactEntry),", at: text.index(after: text.startIndex))

        let lowered = HookInstaller.planInstall(
            existingData: Data(text.utf8), hookCommand: hook, version: ClaudeCodeVersion(major: 2, minor: 1, patch: 50),
            statusLine: .wrap(command: wrapper), savedPreviousStatusLine: saved, home: home)
        guard case .write(let data) = lowered.settings else {
            Issue.record("expected a write, got \(lowered.settings)")
            return
        }
        let written = String(decoding: data, as: UTF8.self)
        #expect(written.contains("\"statusLine\": \(compactEntry),"))
        #expect(try JSONTest.object(data)["hooks"]?["PermissionDenied"] == nil)
    }

    /// The same on disk, with its saved copy gone: its newest backup was
    /// taken while ours was the status line, which is what it replaced.
    @Test func takeoverFromItsBackupsFollowsOurWrapper() throws {
        let dir = try makeConfigDir(settings: nil)
        defer { try? FileManager.default.removeItem(atPath: (dir as NSString).deletingLastPathComponent) }
        try Data(TakeoverFixture.settings(configDir: dir).utf8).write(to: URL(fileURLWithPath: dir + "/settings.json"))
        try TakeoverFixture.writeVibeNotchFiles(configDir: dir, previous: nil)
        let ours = #"{"statusLine":{"type":"command","command":"python3 '\#(dir)/hooks/\#(AppIdentity.statusLineScriptName)'","padding":2}}"#
        try Data(#"{"statusLine":{"type":"command","command":"echo stale"}}"#.utf8)
            .write(to: URL(fileURLWithPath: dir + "/settings.json.superpowered-notch-20260101-000000-000.bak"))
        try Data(ours.utf8).write(to: URL(fileURLWithPath: dir + "/settings.json.superpowered-notch-20260102-000000-000.bak"))
        try Data("{\"type\":\"command\",\"command\":\"echo current\"}\n".utf8)
            .write(to: URL(fileURLWithPath: dir + "/hooks/" + HookInstaller.previousStatusLineFileName))

        #expect(HookInstaller.removeLegacyHooks(configDir: dir, kinds: [.superpoweredVibeNotch]) == .removed)
        #expect(try settings(dir)["statusLine"]?["command"]?.stringValue == "echo current")

        // And ours then wraps it again, chaining the same.
        #expect(HookInstaller.install(configDir: dir, configuration: configuration()) == .installed)
        #expect(HookInstaller.isOurStatusLine(try settings(dir)["statusLine"]))
        let previous = URL(fileURLWithPath: dir + "/hooks/" + HookInstaller.previousStatusLineFileName)
        #expect(HookInstaller.readSavedStatusLine(at: previous)?["command"]?.stringValue == "echo current")
    }
}

/// Names and discovery, second review pass.
struct SecondReviewAccountTests {
    let home = AccountPaths.homeDirectory

    /// A custom name keeps its own badge letters, and a default badge
    /// never takes the same ones (the badge shown is the registry's).
    @MainActor
    @Test func customNamesKeepTheirBadgeAndOthersAvoidIt() {
        let named = ClaudeAccount(configDir: home + "/.claude-a", configDirEnv: home + "/.claude-a", customLabel: "Gmail stuff")
        let plain = ClaudeAccount(configDir: home + "/.claude", email: "me@gmail.com")
        let twin = ClaudeAccount(configDir: home + "/.claude-b", configDirEnv: home + "/.claude-b", customLabel: "Gmail too",
                                 email: "you@acme.com")
        let result = AccountNaming.assign([named, plain, twin])
        #expect(result[named.id]?.monogram == "GM")
        #expect(result[plain.id]?.monogram == "ME")
        #expect(result[twin.id]?.monogram == "AC")

        let published = AccountRegistry.named([plain, named, twin])
        let badges = published.map(\.monogram)
        #expect(Set(badges).count == 3)
        #expect(published.first { $0.id == named.id }?.monogram == "GM")
    }

    /// Discovery never adds the home folder, or a folder holding it, through
    /// a `~/.claude-*` link (home's own `.claude.json` is signed in, so the
    /// link would look like a live account and get hooks in `~/settings.json`),
    /// nor as an extra folder; sightings and a saved list are held to the same.
    @MainActor
    @Test func aLinkToHomeIsNeverDiscovered() throws {
        let root = TestPaths.temporaryRoot("review2-link")
        defer { try? FileManager.default.removeItem(atPath: root) }
        let home = root + "/home"
        try FileManager.default.createDirectory(atPath: home + "/.claude/projects", withIntermediateDirectories: true)
        try Data(#"{"oauthAccount":{"emailAddress":"me@gmail.com"}}"#.utf8).write(to: URL(fileURLWithPath: home + "/.claude.json"))
        try FileManager.default.createSymbolicLink(atPath: home + "/.claude-link", withDestinationPath: home)
        try FileManager.default.createSymbolicLink(atPath: home + "/.claude-up", withDestinationPath: root)
        try FileManager.default.createDirectory(atPath: home + "/.claude-work/projects", withIntermediateDirectories: true)
        try Data(#"{"oauthAccount":{"emailAddress":"me@acme.com"}}"#.utf8)
            .write(to: URL(fileURLWithPath: home + "/.claude-work/.claude.json"))

        let found = AccountRegistry.discover(home: home, extraDirs: ["/", root, home + "/.claude-link"], isAlive: { _ in false })
        #expect(found.accounts == [home + "/.claude", home + "/.claude-work"])
        #expect(found.suggestions.isEmpty)
        #expect(!AccountRegistry.canBeAccount(home + "/.claude-link", home: home))
        #expect(!AccountRegistry.canBeAccount(home + "/.claude-up", home: home))
        #expect(AccountRegistry.canBeAccount(home + "/.claude-work", home: home))

        let registry = AccountRegistry(home: home, storeURL: URL(fileURLWithPath: root + "/accounts.json"),
                                       configReader: ClaudeGlobalConfigReader(), extraConfigDirs: [])
        registry.record(AccountSighting(configDir: home + "/.claude-link", configDirEnv: home + "/.claude-link", sessionId: "s", at: Date()))
        #expect(registry.account(id: home + "/.claude-link") == nil)
    }
}

/// Process groups, second review pass.
nonisolated struct SecondReviewProcessTests {
    /// On timeout the leader dies of its SIGTERM, but the group can outlive
    /// it: a member that missed the signal (forked while it went out, or
    /// ignoring it, as here) is killed too, rather than left running and
    /// holding the output pipe. (The race version made
    /// `aTimeoutStopsEverythingTheShellStarted` fail now and then.)
    @Test(.timeLimit(.minutes(1)))
    func aTimeoutLeavesNothingOfTheGroup() throws {
        let dir = TestPaths.temporaryRoot("review2-group")
        defer { try? FileManager.default.removeItem(atPath: dir) }
        let marker = dir + "/survived"
        let script = dir + "/stub"
        // The member ignores SIGTERM from its first line; the leader dies of it.
        // (Under heavy load the SIGTERM may land before the trap is set; the
        // member then simply dies with the leader, which passes too.)
        try "#!/bin/sh\n(trap '' TERM; sleep 2; touch '\(marker)') &\nsleep 120\n"
            .write(toFile: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script)

        #expect(ProcessRunner.run(executable: script, arguments: [], timeout: 1) == nil)
        Thread.sleep(forTimeInterval: 3)
        #expect(!FileManager.default.fileExists(atPath: marker))
    }
}

/// The JSON reader, second review pass.
nonisolated struct SecondReviewJSONTests {
    /// A saved status line is parsed without JSONSerialization in front of
    /// it: absurd nesting is refused, not recursed into until the stack runs
    /// out (a crash), even on a secondary thread's small stack.
    @Test func deepNestingIsRefusedNotACrash() async throws {
        let limit = OrderedJSON.maxDepth
        func nested(_ depth: Int) -> Data {
            Data((String(repeating: "[", count: depth) + String(repeating: "]", count: depth)).utf8)
        }
        let results = await Task.detached {
            (try? OrderedJSON.parse(nested(limit))) != nil
                && (try? OrderedJSON.parse(nested(limit + 1))) == nil
                && (try? OrderedJSON.parse(nested(100_000))) == nil
                && (try? OrderedJSON.parse(Data((String(repeating: #"{"a":"#, count: 100_000) + "1" + String(repeating: "}", count: 100_000)).utf8))) == nil
        }.value
        #expect(results)
    }
}
