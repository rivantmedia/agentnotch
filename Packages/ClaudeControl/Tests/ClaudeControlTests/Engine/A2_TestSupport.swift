import Foundation
@testable import ClaudeControl

extension TestPaths {
    /// A throwaway folder for the account, hook and configuration suites:
    /// always a fresh one under the temporary directory (never a fixed /tmp
    /// path shared between runs or agents). Callers remove it with `defer`.
    nonisolated static func temporaryRoot(_ label: String) -> String {
        let root = temporaryPath(label)
        try? FileManager.default.createDirectory(atPath: root, withIntermediateDirectories: true)
        return root
    }

    /// Like `temporaryRoot`, but nothing is created until the test needs it.
    nonisolated static func temporaryPath(_ label: String) -> String {
        // /var → /private/var, so paths compare equal to what the code resolves.
        let temporary = URL(fileURLWithPath: NSTemporaryDirectory()).resolvingSymlinksInPath().path
        return (temporary as NSString).appendingPathComponent("spcn-\(label)-\(UUID().uuidString.prefix(8))")
    }
}

/// `TestPaths`' script constants under the name A2's suites use (TestPaths
/// is nonisolated since the integration, so any suite may read it).
nonisolated enum ScriptPaths {
    /// Packages/ClaudeControl/Scripts, where the hook and status line scripts live.
    static let scripts = TestPaths.scripts

    /// A socket path baked into scripts the tests install; they always
    /// override it with SPCN_SOCKET.
    static let unusedSocket = TestPaths.unusedSocket
}

/// A UserDefaults domain of its own for one test, removed afterwards.
final class TestDefaults {
    let name = "spcn-tests-\(UUID().uuidString)"
    let defaults: UserDefaults

    init() {
        defaults = UserDefaults(suiteName: name)!
    }

    var store: ClaudeControlSettings.Store { ClaudeControlSettings.Store(defaults: defaults) }

    func remove() {
        defaults.removePersistentDomain(forName: name)
    }
}
