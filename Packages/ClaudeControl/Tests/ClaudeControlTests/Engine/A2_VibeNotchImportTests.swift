import Foundation
import Testing
@testable import ClaudeControl

/// The one-time import of Superpowered Vibe Notch's accounts and review
/// queue: only where ours are missing, only once, and the source is only read.
@MainActor
@Suite(.serialized)
struct VibeNotchImportTests {
    let root: String
    let source: URL
    let destination: URL
    let defaults = TestDefaults()

    nonisolated static let accountsJSON = """
    {
      "accounts" : [
        {
          "colorIndex" : 3,
          "configDir" : "/Users/u/.claude-work",
          "configDirEnv" : "/Users/u/.claude-work",
          "customLabel" : "Work",
          "id" : "/Users/u/.claude-work",
          "isHidden" : false,
          "lastSeenAt" : "2026-09-01T10:00:00Z",
          "source" : "hook"
        }
      ],
      "removedIds" : [
        "/Users/u/.claude-old"
      ],
      "version" : 1
    }
    """
    nonisolated static let reviewJSON = #"{"sess-1":{"completedAt":"2026-09-20T10:00:00Z","updatedAt":"2026-09-20T10:00:00Z"}}"#

    init() throws {
        root = TestPaths.temporaryRoot("import")
        source = URL(fileURLWithPath: root + "/home/Library/Application Support/SuperpoweredVibeNotch", isDirectory: true)
        destination = URL(fileURLWithPath: root + "/support", isDirectory: true)
        try FileManager.default.createDirectory(at: source, withIntermediateDirectories: true)
    }

    private func cleanUp() {
        try? FileManager.default.removeItem(atPath: root)
        defaults.remove()
    }

    private func writeSource(accounts: String = accountsJSON, review: String = reviewJSON) throws {
        try Data(accounts.utf8).write(to: source.appendingPathComponent("accounts.json"))
        try Data(review.utf8).write(to: source.appendingPathComponent("review-state.json"))
    }

    /// Paths, bytes and modification dates of everything in the source.
    private func sourceState() throws -> [String: String] {
        var state: [String: String] = [:]
        for name in try FileManager.default.contentsOfDirectory(atPath: source.path) {
            let path = source.appendingPathComponent(name).path
            let attributes = try FileManager.default.attributesOfItem(atPath: path)
            let bytes = try Data(contentsOf: URL(fileURLWithPath: path))
            state[name] = "\(bytes.base64EncodedString())|\(attributes[.modificationDate] ?? "")|\(attributes[.posixPermissions] ?? "")"
        }
        return state
    }

    @Test func sourceLocationIsTheOtherAppsSupportFolder() {
        defer { cleanUp() }
        #expect(VibeNotchImport.sourceDirectory(home: "/Users/u").path == "/Users/u/Library/Application Support/SuperpoweredVibeNotch")
    }

    @Test func copiesBothOnceAndLeavesTheSourceUnchanged() throws {
        defer { cleanUp() }
        try writeSource()
        let before = try sourceState()

        let copied = VibeNotchImport.runOnce(from: source, to: destination, settings: defaults.store)
        #expect(copied == ["accounts.json", "review-state.json"])
        #expect(try sourceState() == before)
        #expect(try Data(contentsOf: destination.appendingPathComponent("accounts.json")) == Data(Self.accountsJSON.utf8))
        #expect(try Data(contentsOf: destination.appendingPathComponent("review-state.json")) == Data(Self.reviewJSON.utf8))
        let permissions = try FileManager.default.attributesOfItem(atPath: destination.appendingPathComponent("accounts.json").path)[.posixPermissions]
        #expect((permissions as? NSNumber)?.intValue == 0o600)
        #expect(defaults.store.didImportVibeNotch)

        // Once: a second run copies nothing, even into an empty folder.
        try FileManager.default.removeItem(at: destination)
        #expect(VibeNotchImport.runOnce(from: source, to: destination, settings: defaults.store).isEmpty)
        #expect(!FileManager.default.fileExists(atPath: destination.path))
    }

    @Test func oursAreNeverReplaced() throws {
        defer { cleanUp() }
        try writeSource()
        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
        let ours = #"{"accounts":[],"removedIds":[],"version":1}"#
        try Data(ours.utf8).write(to: destination.appendingPathComponent("accounts.json"))

        #expect(VibeNotchImport.runOnce(from: source, to: destination, settings: defaults.store) == ["review-state.json"])
        #expect(try Data(contentsOf: destination.appendingPathComponent("accounts.json")) == Data(ours.utf8))
    }

    @Test func unreadableFilesAreSkipped() throws {
        defer { cleanUp() }
        try writeSource(accounts: #"{"accounts": "nope"}"#, review: "[1,2]")
        #expect(VibeNotchImport.runOnce(from: source, to: destination, settings: defaults.store).isEmpty)
        #expect(!FileManager.default.fileExists(atPath: destination.appendingPathComponent("accounts.json").path))
        #expect(defaults.store.didImportVibeNotch)
    }

    @Test func nothingToImportIsFine() throws {
        defer { cleanUp() }
        #expect(VibeNotchImport.runOnce(from: source.appendingPathComponent("missing"), to: destination, settings: defaults.store).isEmpty)
        #expect(defaults.store.didImportVibeNotch)
    }

    /// What was imported loads: labels, colours and forgotten folders carry over.
    @Test func theImportedRegistryLoads() throws {
        defer { cleanUp() }
        try writeSource()
        VibeNotchImport.runOnce(from: source, to: destination, settings: defaults.store)
        let registry = AccountRegistry(home: root + "/home", storeURL: destination.appendingPathComponent("accounts.json"),
                                       configReader: ClaudeGlobalConfigReader(), extraConfigDirs: [])
        let work = try #require(registry.account(id: "/Users/u/.claude-work"))
        #expect(work.label == "Work")
        #expect(work.colorIndex == 3)
        #expect(work.source == .hook)
    }
}
