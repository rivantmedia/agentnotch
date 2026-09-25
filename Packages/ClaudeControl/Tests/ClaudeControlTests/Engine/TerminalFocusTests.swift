import AppKit
import Foundation
import Testing
@testable import ClaudeControl

// MARK: - AppleScript generation

struct TerminalScriptTests {
    @Test func devicePathNormalizesAndRejectsJunk() {
        #expect(TerminalScript.devicePath(forTTY: "ttys003") == "/dev/ttys003")
        #expect(TerminalScript.devicePath(forTTY: "/dev/ttys012") == "/dev/ttys012")
        #expect(TerminalScript.devicePath(forTTY: " ttys4 \n") == "/dev/ttys4")
        #expect(TerminalScript.devicePath(forTTY: "") == nil)
        #expect(TerminalScript.devicePath(forTTY: "??") == nil)
        #expect(TerminalScript.devicePath(forTTY: "ttys1\" & do shell script \"x") == nil)
        #expect(TerminalScript.devicePath(forTTY: "../ttys1") == nil)
        #expect(TerminalScript.devicePath(forTTY: "/dev/") == nil)
    }

    @Test func quotedEscapesBackslashesAndQuotes() {
        #expect(TerminalScript.quoted("plain") == "\"plain\"")
        #expect(TerminalScript.quoted("say \"hi\"") == "\"say \\\"hi\\\"\"")
        #expect(TerminalScript.quoted("C:\\path") == "\"C:\\\\path\"")
        #expect(TerminalScript.quoted("\\\"") == "\"\\\\\\\"\"")
    }

    @Test func singleLineCollapsesLineBreaksAndDropsControls() {
        #expect(TerminalScript.singleLine("fix the\nbug\r\nnow") == "fix the bug  now")
        #expect(TerminalScript.singleLine("tab\there") == "tab here")
        #expect(TerminalScript.singleLine("bell\u{07}ring\u{1B}[0m") == "bellring[0m")
        #expect(TerminalScript.singleLine("  padded  ") == "padded")
    }

    @Test func iTermFocusSelectsSessionByTTY() throws {
        let script = try #require(TerminalScript.focus(.iTerm2, tty: "ttys007"))
        #expect(script.contains("tell application id \"com.googlecode.iterm2\""))
        #expect(script.contains("if tty of s is \"/dev/ttys007\" then"))
        #expect(script.contains("select s"))
        #expect(script.contains("activate"))
        #expect(!script.contains("System Events"))
        #expect(!script.contains("keystroke"))
    }

    @Test func terminalFocusSelectsTabAndRaisesWindow() throws {
        let script = try #require(TerminalScript.focus(.terminalApp, tty: "/dev/ttys002"))
        #expect(script.contains("tell application id \"com.apple.Terminal\""))
        #expect(script.contains("if tty of t is \"/dev/ttys002\" then"))
        #expect(script.contains("set selected of t to true"))
        #expect(script.contains("set index of w to 1"))
        #expect(script.hasSuffix("return \"not_found\""))
    }

    @Test func sendWritesEscapedSingleLineText() throws {
        let iterm = try #require(TerminalScript.send("run \"tests\"\nplease", to: .iTerm2, tty: "ttys001"))
        #expect(iterm.contains("tell s to write text \"run \\\"tests\\\" please\" newline NO"))
        #expect(!iterm.contains("activate"))

        let terminal = try #require(TerminalScript.send("ok", to: .terminalApp, tty: "ttys001"))
        #expect(terminal.contains("do script \"ok\" in t"))
        #expect(!terminal.contains("System Events"))
    }

    /// Claude Code submits on a Return that arrives on its own; text and a
    /// line break in one burst is a paste, and a line feed is Ctrl+J (new
    /// line). So iTerm2 gets the text without a newline, then CR as a second
    /// script (S1: the messenger checks again between the two).
    @Test func iTermSendsReturnSeparatelyAfterTheText() throws {
        let script = try #require(TerminalScript.send("ship it", to: .iTerm2, tty: "ttys009"))
        let lines = script.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
        #expect(lines.contains("tell s to write text \"ship it\" newline NO"))
        #expect(!script.contains("enterKey"))
        let submit = try #require(TerminalScript.submit(to: .iTerm2, tty: "ttys009"))
        let submitLines = submit.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
        #expect(submitLines.first == "set enterKey to character id 13")
        #expect(submitLines.contains("tell s to write text enterKey newline NO"))
        #expect(submit.contains("if tty of s is \"/dev/ttys009\" then"))
        // Every write opts out of iTerm2's appended line feed.
        let writes = (lines + submitLines).filter { $0.contains("write text") }
        #expect(writes.count == 2)
        #expect(writes.allSatisfy { $0.hasSuffix("newline NO") })
        // Terminal.app's `do script` submits by itself: no second script.
        #expect(TerminalScript.submit(to: .terminalApp, tty: "ttys009") == nil)
    }

    /// The parts of the iTerm2 send script that don't need iTerm2's
    /// dictionary (CR variable, `tell me to delay` inside an app's tell
    /// block) compile. Compiling never launches the app or sends events.
    @Test func sendScriptScaffoldingCompiles() throws {
        guard FileManager.default.isExecutableFile(atPath: "/usr/bin/osacompile"),
              NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal") != nil else {
            return
        }
        let scaffold = """
        set enterKey to character id 13
        tell application id "com.apple.Terminal"
            repeat with w in windows
                set typed to enterKey
                return "\(TerminalScript.successMarker)"
            end repeat
        end tell
        return "\(TerminalScript.notFoundMarker)"
        """
        #expect(try Self.compiles(scaffold))
    }

    @Test func sendRejectsEmptyMessagesAndBadTTYs() {
        #expect(TerminalScript.send("   \n", to: .iTerm2, tty: "ttys001") == nil)
        #expect(TerminalScript.send("hi", to: .terminalApp, tty: "") == nil)
        #expect(TerminalScript.focus(.iTerm2, tty: "not a tty") == nil)
    }

    /// Compiles (never runs) the Terminal.app scripts against the real
    /// dictionary. Compiling doesn't launch Terminal or send Apple Events.
    @Test func terminalScriptsCompile() throws {
        guard FileManager.default.isExecutableFile(atPath: "/usr/bin/osacompile"),
              NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal") != nil else {
            return
        }
        let scripts = [
            try #require(TerminalScript.focus(.terminalApp, tty: "ttys003")),
            try #require(TerminalScript.send("echo \"hi\" \\ there", to: .terminalApp, tty: "ttys003")),
        ]
        for script in scripts {
            #expect(try Self.compiles(script))
        }
    }

    /// Same for iTerm2, when it is installed.
    @Test func iTermScriptsCompileWhenInstalled() throws {
        guard FileManager.default.isExecutableFile(atPath: "/usr/bin/osacompile"),
              NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.googlecode.iterm2") != nil else {
            return
        }
        let scripts = [
            try #require(TerminalScript.focus(.iTerm2, tty: "ttys003")),
            try #require(TerminalScript.send("hello", to: .iTerm2, tty: "ttys003")),
            try #require(TerminalScript.submit(to: .iTerm2, tty: "ttys003")),
        ]
        for script in scripts {
            #expect(try Self.compiles(script))
        }
    }

    /// Scripts run one after another even when requested at once, so two
    /// messages can't interleave. (Plain scripts that talk to no app.)
    @Test func runnerRunsScriptsOneAtATime() async throws {
        guard FileManager.default.isExecutableFile(atPath: "/usr/bin/osascript") else { return }
        let script = "delay 0.4\nreturn \"\(TerminalScript.successMarker)\""
        let start = Date()
        async let first = TerminalScriptRunner.shared.run(script, label: "test 1")
        async let second = TerminalScriptRunner.shared.run(script, label: "test 2")
        let outcomes = await [first, second]
        #expect(outcomes == [.succeeded, .succeeded])
        #expect(Date().timeIntervalSince(start) >= 0.8)

        let missing = await TerminalScriptRunner.shared.run("return \"\(TerminalScript.notFoundMarker)\"", label: "test 3")
        #expect(missing == .notFound)
    }

    private static func compiles(_ script: String) throws -> Bool {
        let file = FileManager.default.temporaryDirectory
            .appendingPathComponent("agentnotch-script-\(UUID().uuidString).applescript")
        try script.write(to: file, atomically: true, encoding: .utf8)
        defer { try? FileManager.default.removeItem(at: file) }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osacompile")
        process.arguments = ["-o", "/dev/null", file.path]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        return process.terminationStatus == 0
    }
}

// MARK: - Focus planning

struct FocusPlannerTests {
    private let itermURL = URL(fileURLWithPath: "/Applications/iTerm.app")
    private let codeURL = URL(fileURLWithPath: "/Applications/Visual Studio Code.app")

    private func host(_ bundleId: String?, pid: Int32 = 500, url: URL? = nil) -> HostApp {
        HostApp(pid: pid, bundleIdentifier: bundleId, bundleURL: url, name: nil)
    }

    private func context(
        tmux: Bool = false,
        tty: String? = "ttys004",
        entrypoint: String? = "cli",
        host: HostApp? = nil,
        fallbackEditor: URL? = nil,
        cwd: String = "/Users/me/project",
        workspaceRoot: String? = "/Users/me/project"
    ) -> FocusContext {
        FocusContext(isInTmux: tmux, tty: tty, entrypoint: entrypoint, cwd: cwd, host: host,
                     fallbackEditorURL: fallbackEditor, workspaceRoot: workspaceRoot)
    }

    @Test func iTermSessionSelectsItsTabThenActivates() {
        let steps = FocusPlanner.plan(context(host: host("com.googlecode.iterm2", url: itermURL)))
        #expect(steps == [
            .scriptedTab(.iTerm2, tty: "ttys004"),
            .activate(pid: 500, bundleURL: itermURL),
        ])
    }

    @Test func terminalAppWithoutTTYOnlyActivates() {
        let steps = FocusPlanner.plan(context(tty: nil, host: host("com.apple.Terminal")))
        #expect(steps == [.activate(pid: 500, bundleURL: nil)])
    }

    @Test func tmuxGoesFirstAndSkipsTTYScripting() {
        // Inside tmux the process TTY is the pane's, not the terminal tab's.
        let steps = FocusPlanner.plan(context(tmux: true, host: host("com.apple.Terminal")))
        #expect(steps == [.tmux, .activate(pid: 500, bundleURL: nil)])
    }

    @Test func editorTerminalOpensTheFolder() {
        let steps = FocusPlanner.plan(context(host: host("com.microsoft.VSCode", url: codeURL)))
        #expect(steps == [
            .openFolder(appURL: codeURL, folder: "/Users/me/project"),
            .activate(pid: 500, bundleURL: codeURL),
        ])
    }

    /// BHV-8: a CLI started in a subfolder opens its workspace's root (the
    /// window that has it open), never the subfolder (a new window); with no
    /// root it only activates the editor.
    @Test func anEditorTerminalInASubfolderOpensTheWorkspaceRoot() {
        let code = host("com.microsoft.VSCode", url: codeURL)
        let sub = FocusPlanner.plan(context(host: code, cwd: "/Users/me/project/packages/api"))
        #expect(sub == [.openFolder(appURL: codeURL, folder: "/Users/me/project"), .activate(pid: 500, bundleURL: codeURL)])
        let none = FocusPlanner.plan(context(host: code, cwd: "/Users/me/scratch", workspaceRoot: nil))
        #expect(none == [.activate(pid: 500, bundleURL: codeURL)])
        // The extension always runs in its workspace root.
        let ext = FocusPlanner.plan(context(tty: nil, entrypoint: "claude-vscode", host: nil, fallbackEditor: codeURL,
                                           cwd: "/Users/me/project", workspaceRoot: nil))
        #expect(ext == [.openFolder(appURL: codeURL, folder: "/Users/me/project")])
    }

    @Test func theWorkspaceRootIsTheNearestGitFolderBelowHome() {
        let gits: Set<String> = ["/Users/me/project/.git", "/Users/me/.git"]
        func root(_ cwd: String) -> String? { WorkspaceRoot.find(from: cwd, home: "/Users/me", exists: gits.contains) }
        #expect(root("/Users/me/project/packages/api") == "/Users/me/project")
        #expect(root("/Users/me/project") == "/Users/me/project")
        // Home itself (a dotfiles repo) is never the workspace.
        #expect(root("/Users/me/notes") == nil)
        #expect(root("/opt/x") == nil)
        #expect(root("") == nil)
    }

    @Test func vsCodeExtensionUsesARunningEditor() {
        let steps = FocusPlanner.plan(context(tty: nil, entrypoint: "claude-vscode", host: nil, fallbackEditor: codeURL))
        #expect(steps == [.openFolder(appURL: codeURL, folder: "/Users/me/project")])
    }

    @Test func otherTerminalsAreActivated() {
        let ghostty = host("com.mitchellh.ghostty", pid: 42)
        #expect(FocusPlanner.plan(context(host: ghostty)) == [.activate(pid: 42, bundleURL: nil)])
    }

    @Test func nothingToFocusWithoutAnApp() {
        #expect(FocusPlanner.plan(context(host: nil)).isEmpty)
    }
}

// MARK: - Message routes

struct MessageRouteTests {
    private func host(_ bundleId: String) -> HostApp {
        HostApp(pid: 9, bundleIdentifier: bundleId, bundleURL: nil, name: nil)
    }

    @Test func tmuxWinsOverTheHostingTerminal() {
        #expect(MessageRoute.route(isInTmux: true, tty: "ttys001", host: host("com.googlecode.iterm2")) == .tmux)
    }

    @Test func scriptableTerminalsTakeMessages() {
        #expect(MessageRoute.route(isInTmux: false, tty: "ttys001", host: host("com.googlecode.iterm2")) == .scripted(.iTerm2))
        #expect(MessageRoute.route(isInTmux: false, tty: "/dev/ttys001", host: host("com.apple.Terminal")) == .scripted(.terminalApp))
    }

    @Test func otherHostsOrMissingTTYCannot() {
        #expect(MessageRoute.route(isInTmux: false, tty: "ttys001", host: host("com.mitchellh.ghostty")) == nil)
        #expect(MessageRoute.route(isInTmux: false, tty: nil, host: host("com.apple.Terminal")) == nil)
        #expect(MessageRoute.route(isInTmux: true, tty: nil, host: nil) == nil)
        #expect(MessageRoute.route(isInTmux: false, tty: "ttys001", host: nil) == nil)
    }
}

// MARK: - Process tree, tmux clients, terminal registry

struct TerminalDetectionTests {
    @Test func ancestorsWalkUpAndStopAtCycles() {
        let tree: [Int: ProcessEntry] = [
            100: .init(pid: 100, ppid: 90, command: "claude", tty: "ttys001"),
            90: .init(pid: 90, ppid: 80, command: "-zsh", tty: "ttys001"),
            80: .init(pid: 80, ppid: 1, command: "/Applications/iTerm.app/Contents/MacOS/iTerm2", tty: nil),
        ]
        #expect(SessionHostResolver.ancestors(of: 100, tree: tree) == [90, 80])

        let cyclic: [Int: ProcessEntry] = [
            10: .init(pid: 10, ppid: 11, command: "a", tty: nil),
            11: .init(pid: 11, ppid: 10, command: "b", tty: nil),
        ]
        #expect(SessionHostResolver.ancestors(of: 10, tree: cyclic) == [11])
        #expect(SessionHostResolver.ancestors(of: 999, tree: tree).isEmpty)
    }

    @Test func helperProcessesMapToTheirApps() {
        #expect(SessionHostResolver.appBundlePath(forCommand: "/Applications/iTerm.app/Contents/MacOS/iTerm2") == "/Applications/iTerm.app")
        #expect(SessionHostResolver.appBundlePath(
            forCommand: "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)"
        ) == "/Applications/Visual Studio Code.app")
        #expect(SessionHostResolver.appBundlePath(forCommand: "-zsh") == nil)
        #expect(SessionHostResolver.appBundlePath(forCommand: "/usr/bin/login") == nil)

        #expect(SessionHostResolver.bundleIdentifierHint(
            forCommand: "/Users/me/Library/Application Support/iTerm2/iTermServer-3.5.4"
        ) == "com.googlecode.iterm2")
        #expect(SessionHostResolver.bundleIdentifierHint(forCommand: "wezterm-mux-server") == "com.github.wez.wezterm")
        #expect(SessionHostResolver.bundleIdentifierHint(forCommand: "/bin/zsh") == nil)
    }

    /// Resolution against one snapshot of running apps: nearest regular app
    /// by pid first, then helpers matched to their app by bundle.
    @Test func hostResolutionUsesTheRunningAppIndex() {
        let ghostty = HostApp(pid: 300, bundleIdentifier: "com.mitchellh.ghostty",
                              bundleURL: URL(fileURLWithPath: "/Applications/Ghostty.app"), name: "Ghostty")
        let iterm = HostApp(pid: 400, bundleIdentifier: "com.googlecode.iterm2",
                            bundleURL: URL(fileURLWithPath: "/Applications/iTerm.app"), name: "iTerm2")
        let apps = RunningAppIndex(apps: [ghostty, iterm])
        var tree: [Int: ProcessEntry] = [:]
        let processes: [(pid: Int, ppid: Int, command: String)] = [
            // Ghostty: claude → zsh → login → Ghostty
            (300, 1, "/Applications/Ghostty.app/Contents/MacOS/ghostty"),
            (301, 300, "/usr/bin/login"),
            (302, 301, "-zsh"),
            (303, 302, "claude"),
            // iTerm2: claude → zsh → login → iTermServer (a launchd child)
            (400, 1, "/Applications/iTerm.app/Contents/MacOS/iTerm2"),
            (410, 1, "/Applications/iTerm.app/Contents/MacOS/iTermServer-3.5.4"),
            (411, 410, "/usr/bin/login"),
            (412, 411, "-zsh"),
            (413, 412, "claude"),
            // tmux server: no app above it
            (500, 1, "tmux"),
            (501, 500, "-zsh"),
            (502, 501, "claude"),
        ]
        for process in processes {
            tree[process.pid] = ProcessEntry(
                pid: process.pid, ppid: process.ppid, command: process.command, tty: nil
            )
        }
        #expect(SessionHostResolver.hostApp(forPid: 303, tree: tree, apps: apps) == ghostty)
        #expect(SessionHostResolver.hostApp(forPid: 413, tree: tree, apps: apps) == iterm)
        #expect(SessionHostResolver.hostApp(forPid: 502, tree: tree, apps: apps) == nil)
        #expect(SessionHostResolver.hostApp(forPid: 999, tree: tree, apps: apps) == nil)
    }

    @Test func tmuxClientsParse() {
        let output = "4242 /dev/ttys003\n 77 /dev/ttys010\nbogus\n12 \n"
        #expect(TmuxClient.parse(listClientsOutput: output) == [
            TmuxClient(pid: 4242, tty: "/dev/ttys003"),
            TmuxClient(pid: 77, tty: "/dev/ttys010"),
        ])
    }

    @Test func terminalNamesMatchExactlyNotBySubstring() {
        #expect(TerminalAppRegistry.isTerminal("Terminal"))
        #expect(TerminalAppRegistry.isTerminal("iTerm2"))
        #expect(TerminalAppRegistry.isTerminal("/Applications/iTerm.app/Contents/MacOS/iTerm2"))
        #expect(TerminalAppRegistry.isTerminal("/Applications/Ghostty.app/Contents/MacOS/ghostty"))
        #expect(TerminalAppRegistry.isTerminal("/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)"))
        #expect(TerminalAppRegistry.isTerminal("/Applications/Warp.app/Contents/MacOS/stable"))

        // These used to match "st", "Code", "Terminal" as substrings.
        #expect(!TerminalAppRegistry.isTerminal("System Settings"))
        #expect(!TerminalAppRegistry.isTerminal("Xcode"))
        #expect(!TerminalAppRegistry.isTerminal("Postman"))
        #expect(!TerminalAppRegistry.isTerminal("/Applications/Xcode.app/Contents/MacOS/Xcode"))
        #expect(!TerminalAppRegistry.isTerminal("claude"))
        #expect(!TerminalAppRegistry.isTerminal(""))
    }

    @Test func bundleClassification() {
        #expect(TerminalAppRegistry.isTerminalBundle("com.googlecode.iterm2"))
        #expect(TerminalAppRegistry.isTerminalBundle("com.todesktop.230313mzl4w4u92"))
        #expect(!TerminalAppRegistry.isTerminalBundle("com.apple.Safari"))
        #expect(TerminalAppRegistry.isEditorBundle("com.vscodium"))
        #expect(!TerminalAppRegistry.isEditorBundle("com.apple.Terminal"))

        let iterm = HostApp(pid: 1, bundleIdentifier: "com.googlecode.iterm2", bundleURL: nil, name: nil)
        #expect(iterm.scriptableTerminal == .iTerm2)
        #expect(!iterm.isEditor)
        let cursor = HostApp(pid: 1, bundleIdentifier: "com.todesktop.230313mzl4w4u92", bundleURL: nil, name: nil)
        #expect(cursor.isEditor)
        #expect(cursor.scriptableTerminal == nil)
    }
}
