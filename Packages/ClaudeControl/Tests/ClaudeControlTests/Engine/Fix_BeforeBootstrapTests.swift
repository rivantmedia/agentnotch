import Foundation
import Testing
@testable import ClaudeControl

/// S6/CS-5: before `bootstrap` (every test, the snapshots tool) the engine
/// sees the real home, so each side effect that could reach it refuses:
/// `claude` never runs, no login shell starts, no folder is created there.
/// Injected runners and temporary homes still work (the rest of the suite).
struct Fix_BeforeBootstrapTests {
    @Test func theTestProcessIsNotBootstrapped() {
        #expect(!AppIdentity.isFrozen)
    }

    @Test func theDefaultProbeRunnerNeverRunsClaude() async {
        let request = UsageStore.ProbeRequest(accountId: "claude", configDirEnv: nil,
                                              configDirs: [NSHomeDirectory() + "/.claude"],
                                              workingDirectory: URL(fileURLWithPath: NSTemporaryDirectory()))
        let outcome = await UsageStore.runClaudeCodeProbe(request)
        guard case .failed(let reason) = outcome else {
            Issue.record("expected a refusal, got \(outcome)")
            return
        }
        #expect(reason.contains("bootstrapped"))
    }

    @Test func noLoginShellIsStarted() {
        #expect(ClaudeBinaryLocator.resolveViaLoginShellIfAllowed() == nil)
    }

    @Test func onlyAStubUnderTheTemporaryDirectoryMayRunAsClaude() {
        #expect(!ClaudeBinaryLocator.mayRunBeforeBootstrap(resolvedPath: "/opt/homebrew/bin/claude"))
        #expect(!ClaudeBinaryLocator.mayRunBeforeBootstrap(resolvedPath: NSHomeDirectory() + "/.local/bin/claude"))
        #expect(ClaudeBinaryLocator.version(ofBinaryAt: "/usr/bin/true") == nil)
        let stub = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("spcn-stub-claude").path
        #expect(ClaudeBinaryLocator.mayRunBeforeBootstrap(resolvedPath: stub))
    }

    /// The folder `createAccount` would make in the real home is one the
    /// installer's guard protects, which `createAccount` now asks first.
    @Test func aNewAccountFolderInTheRealHomeIsProtected() throws {
        let entry = try #require(getpwuid(getuid()))
        let realHome = String(cString: entry.pointee.pw_dir)
        #expect(HookInstaller.isProtectedBeforeBootstrap(configDir: realHome + "/.claude-spcn-guard-probe"))
        #expect(!HookInstaller.isProtectedBeforeBootstrap(configDir: NSTemporaryDirectory() + "home/.claude-x"))
    }
}
