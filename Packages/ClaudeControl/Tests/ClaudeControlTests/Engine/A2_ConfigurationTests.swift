import Foundation
import Testing
@testable import ClaudeControl

/// `ClaudeControlConfiguration.live` and every `SPCN_*` switch, parsed in
/// one place with one rule for "on".
struct ConfigurationTests {
    private func live(_ environment: [String: String], _ arguments: [String] = ["app"]) -> ClaudeControlConfiguration {
        ClaudeControlConfiguration.live(appDisplayName: "Superpowered Codenotch",
                                        bundleIdentifier: "com.paraswtf.superpowered-codenotch",
                                        supportFolderName: "Superpowered Codenotch",
                                        environment: environment, arguments: arguments)
    }

    @Test func defaults() {
        let config = live(["HOME": "/Users/u"])
        #expect(config.mode == .live)
        #expect(config.homeDirectory == "/Users/u")
        #expect(config.supportDirectory.path == "/Users/u/Library/Application Support/Superpowered Codenotch/Claude")
        #expect(config.socketPath == "/Users/u/Library/Application Support/Superpowered Codenotch/Claude/hook.sock")
        #expect(config.installsAllowed && config.notificationsAllowed && config.probesAllowed)
        #expect(config.extraConfigDirs.isEmpty)
        #expect(config.hookScriptName == "superpowered-codenotch-hook.py")
        #expect(config.statusLineScriptName == "superpowered-codenotch-statusline.py")
    }

    @Test func environmentOverrides() {
        let config = live([
            "HOME": "/Users/u",
            "SPCN_SUPPORT_DIR": "~/dev/support",
            "SPCN_SOCKET": "~/dev/hook.sock",
            "SPCN_NO_INSTALL": "yes",
            "SPCN_NO_NOTIFICATIONS": " TRUE ",
            "SPCN_EXTRA_CONFIG_DIRS": "~/a: /b/c ::",
        ])
        #expect(config.supportDirectory.path == "/Users/u/dev/support")
        #expect(config.socketPath == "/Users/u/dev/hook.sock")
        #expect(!config.installsAllowed)
        #expect(!config.notificationsAllowed)
        #expect(config.extraConfigDirs == ["/Users/u/a", "/b/c"])
    }

    @Test func noInstallAsAnArgument() {
        #expect(!live(["HOME": "/Users/u"], ["app", "--no-install"]).installsAllowed)
    }

    @Test(arguments: ["1", "true", "TRUE", "yes", " Yes "])
    func truthyValues(value: String) {
        #expect(DevFlags.truthy(value))
        #expect(!live(["HOME": "/Users/u", "SPCN_NO_INSTALL": value]).installsAllowed)
    }

    @Test(arguments: ["0", "", "false", "no", "on", "2"])
    func falsyValues(value: String) {
        #expect(!DevFlags.truthy(value))
        #expect(live(["HOME": "/Users/u", "SPCN_NO_INSTALL": value]).installsAllowed)
    }

    @Test func flagsReadArgumentsOrEnvironment() {
        #expect(DevFlags.flag("--dump-state", env: "SPCN_DUMP_STATE", environment: [:], arguments: ["app", "--dump-state"]))
        #expect(DevFlags.flag("--dump-state", env: "SPCN_DUMP_STATE", environment: ["SPCN_DUMP_STATE": "yes"], arguments: ["app"]))
        #expect(!DevFlags.flag("--dump-state", env: "SPCN_DUMP_STATE", environment: ["SPCN_DUMP_STATE": "0"], arguments: ["app"]))
    }

    @Test func pathLists() {
        #expect(DevFlags.pathList(nil, home: "/h").isEmpty)
        #expect(DevFlags.pathList("~:~/x:/y/", home: "/h") == ["/h", "/h/x", "/y/"])
    }

    /// `sockaddr_un` holds 103 bytes of path: a support folder deep enough
    /// to overflow it moves the socket to `/tmp/spcn-<uid>/hook.sock`.
    @Test func aSocketPathTooLongFallsBackToTmp() {
        let deep = "/Users/" + String(repeating: "x", count: 60) + "/deep"
        let config = live(["HOME": deep])
        #expect(config.supportDirectory.path.hasPrefix(deep))
        #expect(config.socketPath == "/tmp/spcn-\(getuid())/hook.sock")

        let limit = ClaudeControlConfiguration.maxSocketPathBytes
        let justFits = URL(fileURLWithPath: "/" + String(repeating: "a", count: limit - "/hook.sock".count - 1))
        #expect(ClaudeControlConfiguration.socketPath(in: justFits, userID: 501).utf8.count == limit)
        #expect(ClaudeControlConfiguration.socketPath(in: justFits, userID: 501) == justFits.path + "/hook.sock")
        let oneOver = URL(fileURLWithPath: justFits.path + "b")
        #expect(ClaudeControlConfiguration.socketPath(in: oneOver, userID: 501) == "/tmp/spcn-501/hook.sock")
    }

    @Test func sealedReachesNothingReal() {
        let config = ClaudeControlConfiguration.sealed(appDisplayName: "T", bundleIdentifier: "com.example.t")
        #expect(config.mode == .sealed)
        #expect(!config.installsAllowed && !config.notificationsAllowed && !config.probesAllowed)
        #expect(config.homeDirectory.hasPrefix(NSTemporaryDirectory()))
        #expect(config.supportDirectory.path.hasPrefix(URL(fileURLWithPath: NSTemporaryDirectory()).path))
    }
}

// The socket folder's rules (created 0700; for the shared /tmp fallback a
// real folder, ours, closed to others) are tested with the server that
// applies them: A1_SocketLifecycleTests (HookSocketDirectory).
