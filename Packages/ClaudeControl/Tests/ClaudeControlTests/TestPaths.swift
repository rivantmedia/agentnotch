import Foundation

/// Paths inside the package, found from this file (tests run from source).
nonisolated enum TestPaths {
    /// Packages/ClaudeControl
    static let packageRoot = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()   // ClaudeControlTests
        .deletingLastPathComponent()   // Tests
        .deletingLastPathComponent()   // ClaudeControl

    /// Packages/ClaudeControl/Scripts, where the hook and status line scripts live.
    static let scripts = packageRoot.appendingPathComponent("Scripts", isDirectory: true)

    /// A socket path baked into scripts the tests install; they always
    /// override it with SPCN_SOCKET.
    static let unusedSocket = "/tmp/spcn-tests-unused.sock"
}
