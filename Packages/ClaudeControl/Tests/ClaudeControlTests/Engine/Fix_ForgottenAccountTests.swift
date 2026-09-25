import Foundation
import Testing
@testable import ClaudeControl

/// BHV-3: a forgotten account's still-running sessions don't move to the
/// default ring: the store drops them, and anything that comes back from
/// it is listed nowhere and announces nothing.
@MainActor
struct Fix_ForgottenAccountTests {
    @Test func aRemovedAccountIsForgottenUntilAddedBack() throws {
        let home = NSTemporaryDirectory() + "spcn-fix-forget-\(UUID().uuidString)"
        defer { try? FileManager.default.removeItem(atPath: home) }
        try FileManager.default.createDirectory(atPath: home + "/.claude-temp", withIntermediateDirectories: true)
        let registry = AccountRegistry(home: home, storeURL: URL(fileURLWithPath: home + "-support/accounts.json"),
                                       configReader: ClaudeGlobalConfigReader(), extraConfigDirs: [])
        let account = try registry.addAccount(configDir: home + "/.claude-temp")
        #expect(!registry.isForgotten(account.id))
        registry.remove(id: account.id)
        #expect(registry.isForgotten(account.id))
        #expect(registry.forgottenIds == [account.id])
        _ = try registry.addAccount(configDir: home + "/.claude-temp")
        #expect(!registry.isForgotten(account.id))
        #expect(registry.forgottenIds.isEmpty)
        try? FileManager.default.removeItem(atPath: home + "-support")
    }

    @Test func theStoreDropsAForgottenAccountsSessions() async throws {
        let temp = try TemporaryAccount(prefix: "spcn-fix-forget-store")
        let store = SessionStore.forTests(
            reviewStore: ReviewStateStore(fileURL: temp.reviewFile, writeDelay: 0, createsFolder: false),
            effects: .none, completionTiming: .immediate)
        var kept = SessionState(sessionId: "kept", cwd: "/tmp/a")
        kept.accountId = "/Users/x/.claude"
        var gone = SessionState(sessionId: "gone", cwd: "/tmp/b")
        gone.accountId = "/Users/x/.claude-temp"
        await store.replaceAllWithFixtures([kept, gone])
        await store.process(.dropAccountSessions(accountId: "/Users/x/.claude-temp"))
        #expect(await store.session(for: "gone") == nil)
        #expect(await store.session(for: "kept") != nil)
    }

    @Test func thePanelListsNoSessionOfAForgottenAccount() {
        var kept = SessionState(sessionId: "kept", cwd: "/tmp/a")
        kept.accountId = "/Users/x/.claude"
        var gone = SessionState(sessionId: "gone", cwd: "/tmp/b")
        gone.accountId = "/Users/x/.claude-temp"
        var model = SessionsPanelModel(sessions: [kept, gone], accounts: [])
        #expect(model.visibleSessions(ringFilter: nil).count == 2)
        model.forgottenAccountIds = ["/Users/x/.claude-temp"]
        #expect(model.visibleSessions(ringFilter: nil).map(\.sessionId) == ["kept"])
    }
}
