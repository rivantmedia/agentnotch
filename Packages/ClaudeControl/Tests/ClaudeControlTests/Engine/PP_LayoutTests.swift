import Foundation
import Testing
@testable import ClaudeControl

/// Only the two keys the app needs are ever parsed out of `.claude.json`.
struct PP_JSONFieldScannerTests {
    @Test func picksTopLevelValuesAndSkipsTheRest() throws {
        let text = #"""
        {"numStartups": 3, "projects": {"/a": {"history": [{"display": "say \"oauthAccount\": {\"x\": 1}"}], "n": [1, [2, {"k": "}"}]]}},
         "tips": "\\\" }", "oauthAccount": {"accountUuid": "u-1", "emailAddress": "me@x.dev"},
         "flag": true, "nothing": null, "count": -12.5e3,
         "cachedUsageUtilization": {"accountUuid": "u-1", "fetchedAtMs": 1790000000000}}
        """#
        let values = try #require(JSONFieldScanner.values(in: Data(text.utf8), keys: ["oauthAccount", "cachedUsageUtilization", "count"]))
        #expect(Set(values.keys) == ["oauthAccount", "cachedUsageUtilization", "count"])
        let account = try #require(JSONFieldScanner.objects(in: Data(text.utf8), keys: ["oauthAccount"])?["oauthAccount"] as? [String: Any])
        #expect(account["emailAddress"] as? String == "me@x.dev")
        #expect(String(decoding: try #require(values["count"]), as: UTF8.self) == "-12.5e3")
        // A key only mentioned inside a string or nested deeper is not a top-level key.
        #expect(JSONFieldScanner.values(in: Data(text.utf8), keys: ["x", "k", "display"])?.isEmpty == true)
    }

    @Test func refusesWhatIsNotAnObject() {
        #expect(JSONFieldScanner.values(in: Data("[1, 2]".utf8), keys: ["a"]) == nil)
        #expect(JSONFieldScanner.values(in: Data(#"{"a": {"b": 1}"#.utf8), keys: ["a"]) == nil) // cut off mid-write
        #expect(JSONFieldScanner.values(in: Data(#"{"a" 1}"#.utf8), keys: ["a"]) == nil)
        #expect(JSONFieldScanner.values(in: Data("{}".utf8), keys: ["a"]) == [:])
        #expect(JSONFieldScanner.values(in: Data("\u{FEFF}{\"a\":1}".utf8), keys: ["a"]).map { Array($0.keys) } == ["a"])
    }

    @Test func escapedKeysAndDuplicatesBehaveLikeAParser() throws {
        let values = try #require(JSONFieldScanner.objects(in: Data(#"{"oauthAccount": {"emailAddress": "a"}, "k": 1, "k": 2}"#.utf8),
                                                           keys: ["oauthAccount", "k"]))
        #expect((values["oauthAccount"] as? [String: Any])?["emailAddress"] as? String == "a")
        #expect(values["k"] as? Int == 2)
    }

    @Test func theGlobalConfigReaderUsesOnlyTheTwoKeys() throws {
        let data = Data(#"{"primaryApiKey": "sk-secret", "oauthAccount": {"accountUuid": "u", "emailAddress": "e@x.dev"}, "cachedUsageUtilization": {"accountUuid": "u", "fetchedAtMs": 1790000000000, "utilization": {"five_hour": {"utilization": 3, "resets_at": "2099-01-01T00:00:00Z"}}}}"#.utf8)
        let config = try #require(ClaudeGlobalConfig.parse(data))
        #expect(config.identity?.email == "e@x.dev")
        #expect(config.cachedUsage?.accountUuid == "u")
        #expect(ClaudeGlobalConfig.readKeys == ["oauthAccount", "cachedUsageUtilization"])
    }
}

/// Which folders Claude Code runs in, which are Claude Parallel Profiles
/// stores, and which are only infrastructure.
@MainActor
@Suite(.serialized)
struct PP_ClassificationTests {
    @Test func theUsersLayout() throws {
        let fake = ParallelProfilesHome("classify")
        defer { fake.cleanUp() }
        try fake.buildUserLayout()
        let snapshot = ConfigDirClassifier.readSnapshot(home: fake.home, isAlive: { _ in false })
        let layout = ConfigDirClassifier.classify(snapshot)
        #expect(layout.extensionDetected)
        #expect(layout.runDirs == [".claude", ".claude-windows/1bf3e8f92b11", ".claude-windows/801f9dd51396",
                                   ".claude-windows/b9fbb9ecd7cb"].map(fake.path))
        #expect(layout.stores == [".claude-claude", ".claude-paras", ".claude-paras-rivant-in"].map(fake.path))
        #expect(layout.infrastructure == [".claude-shared", ".claude-windows"].map(fake.path))
        #expect(layout.windowDirs.count == 3)
        // Links are read as links, and the default's login is ~/.claude.json.
        let defaultFolder = try #require(snapshot.folder(fake.path(".claude")))
        #expect(defaultFolder.isSignedIn)
        #expect(defaultFolder.linkTargets.contains(fake.path(".claude-shared/projects")))
    }

    @Test func discoveryAddsDefaultStandaloneWindowsAndStoresNeverTheSharedHistory() throws {
        let fake = ParallelProfilesHome("discover")
        defer { fake.cleanUp() }
        try fake.buildUserLayout()
        // A window that has not been stocked yet (no .claude.json) is not an account.
        try fake.linkShared(".claude-windows/0000aaaa1111")
        let found = AccountRegistry.discover(home: fake.home, isAlive: { _ in false })
        #expect(found.accounts == [".claude", ".claude-windows/1bf3e8f92b11", ".claude-windows/801f9dd51396",
                                   ".claude-windows/b9fbb9ecd7cb", ".claude-claude", ".claude-paras",
                                   ".claude-paras-rivant-in"].map(fake.path))
        #expect(found.suggestions.isEmpty)
    }

    @Test func storesAreFoundByMarkerOrManifestAlone() throws {
        let fake = ParallelProfilesHome("stores")
        defer { fake.cleanUp() }
        try fake.linkShared(".claude")
        try fake.addStore("a", uuid: "u-a", email: "a@x.dev", marker: true)   // marker, not in the manifest
        try fake.addStore("b", uuid: "u-b", email: "b@x.dev", marker: false)  // in the manifest only
        try fake.writeManifest(stores: [".claude-b"])
        let layout = ConfigDirClassifier.classify(ConfigDirClassifier.readSnapshot(home: fake.home, isAlive: { _ in false }))
        #expect(layout.kind(of: fake.path(".claude-a")) == .store)
        #expect(layout.kind(of: fake.path(".claude-b")) == .store)
        #expect(layout.extensionDetected)
    }

    /// Without the extension everything works as before: every folder is a
    /// run folder (grouped by identity later).
    @Test func withoutTheExtensionEveryFolderRuns() throws {
        let fake = ParallelProfilesHome("plain")
        defer { fake.cleanUp() }
        try fake.mkdir(".claude/projects")
        try fake.writeJSON(".claude.json", fake.login(uuid: "u-1", email: "me@x.dev"))
        try fake.mkdir(".claude-work/projects")
        try fake.writeJSON(".claude-work/.claude.json", fake.login(uuid: "u-2", email: "me@work.dev"))
        let snapshot = ConfigDirClassifier.readSnapshot(home: fake.home, isAlive: { _ in false })
        let layout = ConfigDirClassifier.classify(snapshot)
        #expect(!layout.extensionDetected)
        #expect(layout.runDirs == [fake.path(".claude"), fake.path(".claude-work")])
        #expect(layout.stores.isEmpty && layout.infrastructure.isEmpty)
    }

    /// A user who links a profile's history into `~/.claude` by hand still
    /// runs Claude Code in `~/.claude`; an unsigned link target is history.
    @Test func aLinkTargetIsInfrastructureUnlessItRunsItself() throws {
        let fake = ParallelProfilesHome("links")
        defer { fake.cleanUp() }
        try fake.mkdir(".claude/projects")
        try fake.mkdir(".claude/sessions")
        try fake.mkdir(".claude-work")
        try FileManager.default.createSymbolicLink(atPath: fake.path(".claude-work/projects"), withDestinationPath: fake.path(".claude/projects"))
        try fake.writeJSON(".claude-work/.claude.json", fake.login(uuid: "u-2", email: "me@work.dev"))
        try fake.mkdir(".claude-history/projects")
        try fake.mkdir(".claude-other")
        try FileManager.default.createSymbolicLink(atPath: fake.path(".claude-other/sessions"), withDestinationPath: "../.claude-history/projects/../sessions")
        try fake.mkdir(".claude-history/sessions")
        let layout = ConfigDirClassifier.classify(ConfigDirClassifier.readSnapshot(home: fake.home, isAlive: { _ in false }))
        #expect(layout.kind(of: fake.path(".claude")) == .run)
        #expect(layout.kind(of: fake.path(".claude-work")) == .run)
        #expect(layout.kind(of: fake.path(".claude-history")) == .infrastructure)
    }

    @Test func windowFoldersAreRecognised() {
        let home = "/Users/me"
        #expect(ParallelProfiles.isWindowDir("/Users/me/.claude-windows/801f9dd51396", home: home))
        #expect(ParallelProfiles.isWindowDir("/Users/me/.claude-windows/801f9dd51396/", home: home))
        #expect(!ParallelProfiles.isWindowDir("/Users/me/.claude-windows", home: home))
        #expect(!ParallelProfiles.isWindowDir("/Users/me/.claude-windows/.manifest.json", home: home))
        #expect(!ParallelProfiles.isWindowDir("/Users/me/.claude-windows/a/b", home: home))
        #expect(ParallelProfilesManifest.parse(Data(#"{"stores": ["/Users/me/.claude-a/"], "created": []}"#.utf8))?.stores == ["/Users/me/.claude-a"])
        #expect(ParallelProfilesManifest.parse(Data("[]".utf8)) == nil)
    }
}

/// New VS Code windows are noticed within seconds.
@MainActor
@Suite(.serialized)
struct PP_WindowWatchTests {
    @Test func aNewWindowOrAnAccountSwitchIsNoticed() throws {
        let fake = ParallelProfilesHome("watch")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let first = WindowDirWatch.fingerprint(home: fake.home)
        #expect(first.windows.count == 3 && first.manifest != nil)
        #expect(WindowDirWatch.change(from: nil, to: first) == .none)
        #expect(WindowDirWatch.change(from: first, to: WindowDirWatch.fingerprint(home: fake.home)) == .none)

        // A workspace opened: its folder appears, then gets its config.
        try fake.linkShared(".claude-windows/0a1b2c3d4e5f")
        let appeared = WindowDirWatch.fingerprint(home: fake.home)
        #expect(WindowDirWatch.change(from: first, to: appeared) == .folders)
        try fake.writeJSON(".claude-windows/0a1b2c3d4e5f/.claude.json", fake.login(uuid: ParallelProfilesHome.biiosUUID, email: ParallelProfilesHome.biios))
        let stocked = WindowDirWatch.fingerprint(home: fake.home)
        #expect(WindowDirWatch.change(from: appeared, to: stocked) == .folders)

        // The window switched account: only who is signed in changed.
        try fake.writeJSON(".claude-windows/0a1b2c3d4e5f/.claude.json", fake.login(uuid: ParallelProfilesHome.parasUUID, email: ParallelProfilesHome.paras, cachedFetchedAtMs: 1))
        #expect(WindowDirWatch.change(from: stocked, to: WindowDirWatch.fingerprint(home: fake.home)) == .identities)
    }
}
