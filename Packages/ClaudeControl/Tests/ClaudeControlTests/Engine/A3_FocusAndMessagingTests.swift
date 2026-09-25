import Foundation
import Testing
@testable import ClaudeControl

/// The focus chain's order: tmux (+yabai) → iTerm2 / Terminal by TTY → the
/// host's own tab focus (Ghostty, cmux) → VS Code family by folder → activate.
struct A3_FocusChainOrderTests {
    private func host(_ bundle: String, pid: Int32 = 900) -> HostApp {
        HostApp(pid: pid, bundleIdentifier: bundle, bundleURL: URL(fileURLWithPath: "/Applications/X.app"), name: "X")
    }

    private func context(
        host: HostApp?,
        tmux: Bool = false,
        tty: String? = "ttys004",
        entrypoint: String? = "cli",
        cwd: String = "/Users/me/code/app",
        external: Bool = true,
        fallbackEditor: URL? = nil
    ) -> FocusContext {
        FocusContext(pid: 4242, isInTmux: tmux, tty: tty, entrypoint: entrypoint, cwd: cwd, host: host,
                     fallbackEditorURL: fallbackEditor, hasExternalTabFocus: external)
    }

    private func kinds(_ steps: [FocusStep]) -> [String] {
        steps.map { step in
            switch step {
            case .tmux: return "tmux"
            case .scriptedTab(let terminal, _): return "tab:\(terminal.rawValue)"
            case .externalTab(let request, _, _): return "external:\(request.bundleID ?? "?")"
            case .openFolder: return "folder"
            case .activate: return "activate"
            }
        }
    }

    @Test func tmuxComesFirst() {
        let steps = FocusPlanner.plan(context(host: nil, tmux: true))
        #expect(kinds(steps) == ["tmux"])
        // Inside tmux the process TTY is the pane's: no tab scripting by it.
        let inITerm = FocusPlanner.plan(context(host: host(TerminalAppRegistry.iTerm2BundleId), tmux: true))
        #expect(kinds(inITerm) == ["tmux", "activate"])
    }

    @Test func scriptableTerminalsSelectTheTabByTTY() {
        #expect(kinds(FocusPlanner.plan(context(host: host(TerminalAppRegistry.iTerm2BundleId)))) == ["tab:iTerm2", "activate"])
        #expect(kinds(FocusPlanner.plan(context(host: host(TerminalAppRegistry.terminalAppBundleId)))) == ["tab:terminalApp", "activate"])
        // Without a usable TTY, only the app.
        #expect(kinds(FocusPlanner.plan(context(host: host(TerminalAppRegistry.iTerm2BundleId), tty: "??"))) == ["activate"])
    }

    @Test func ghosttyAndCmuxAskTheHost() throws {
        for bundle in [TerminalAppRegistry.ghosttyBundleId, TerminalAppRegistry.cmuxBundleId] {
            let steps = FocusPlanner.plan(context(host: host(bundle)))
            #expect(kinds(steps) == ["external:\(bundle)", "activate"])
            guard case .externalTab(let request, let hostPid, _) = try #require(steps.first) else { continue }
            #expect(request.pid == 4242 && request.tty == "ttys004" && request.cwd == "/Users/me/code/app")
            #expect(hostPid == 900)
            // A host without tab focus: just the app.
            #expect(kinds(FocusPlanner.plan(context(host: host(bundle), external: false))) == ["activate"])
        }
    }

    @Test func vsCodeFamilyOpensTheFolder() {
        let editor = host("com.microsoft.VSCode")
        var inRepo = context(host: editor)
        inRepo.workspaceRoot = "/Users/me/code/app"
        #expect(kinds(FocusPlanner.plan(inRepo)) == ["folder", "activate"])
        // No workspace root known (BHV-8): the editor is activated, no folder opened.
        #expect(kinds(FocusPlanner.plan(context(host: editor))) == ["activate"])
        // The extension with no app in its process tree uses a running editor.
        let url = URL(fileURLWithPath: "/Applications/Cursor.app")
        let steps = FocusPlanner.plan(context(host: nil, entrypoint: "claude-vscode", fallbackEditor: url))
        #expect(steps == [.openFolder(appURL: url, folder: "/Users/me/code/app")])
    }

    @Test func otherAppsAreActivatedAndNothingIsAnEmptyPlan() {
        #expect(kinds(FocusPlanner.plan(context(host: host("net.kovidgoyal.kitty")))) == ["activate"])
        #expect(FocusPlanner.plan(context(host: nil)).isEmpty)
    }

    @Test func externalFocusRunsOffTheMainActorSafely() {
        // The request the host gets carries only what it needs to find the tab.
        let request = ClaudeExternalTabRequest(bundleID: TerminalAppRegistry.ghosttyBundleId, pid: 1, tty: nil, cwd: "/x")
        #expect(TerminalAppRegistry.externalTabFocusBundleIdentifiers.contains(request.bundleID ?? ""))
        #expect(HostApp(pid: 1, bundleIdentifier: TerminalAppRegistry.cmuxBundleId, bundleURL: nil, name: nil).usesExternalTabFocus)
        #expect(!HostApp(pid: 1, bundleIdentifier: TerminalAppRegistry.iTerm2BundleId, bundleURL: nil, name: nil).usesExternalTabFocus)
    }
}

/// Typing into a terminal only when Claude's prompt is what reads it.
struct A3_MessageSafetyTests {
    private func process(tty: String? = "ttys004", foreground: Bool = true, stopped: Bool = false) -> ProcessStatus {
        ProcessStatus(pid: 4242, ppid: 1, tty: tty, isForeground: foreground, isStopped: stopped)
    }

    @Test func dialogsBlockTyping() {
        let blocked: [NeedsInputReason] = [.permission(tool: "Bash"), .question, .planApproval, .elicitation(nil), .dialog("waiting")]
        for reason in blocked {
            #expect(MessageSafety.blockReason(attention: .needsInput(reason), expectedTTY: "ttys004", process: process()) != nil, "\(reason)")
        }
        // A failed turn has nothing open: the prompt is back.
        #expect(MessageSafety.blockReason(attention: .needsInput(.error("Rate limited")), expectedTTY: "ttys004", process: process()) == nil)
        #expect(MessageSafety.blockReason(attention: .readyForReview, expectedTTY: "ttys004", process: process()) == nil)
        #expect(MessageSafety.blockReason(attention: .idle, expectedTTY: "/dev/ttys004", process: process()) == nil)
    }

    @Test func theProcessMustOwnItsTerminalsForeground() {
        let attention = SessionAttention.idle
        #expect(MessageSafety.blockReason(attention: attention, expectedTTY: "ttys004", process: nil) != nil)
        #expect(MessageSafety.blockReason(attention: attention, expectedTTY: "ttys004", process: process(tty: "ttys009")) != nil)
        #expect(MessageSafety.blockReason(attention: attention, expectedTTY: "ttys004", process: process(tty: nil)) != nil)
        #expect(MessageSafety.blockReason(attention: attention, expectedTTY: nil, process: process()) != nil)
        #expect(MessageSafety.blockReason(attention: attention, expectedTTY: "ttys004", process: process(stopped: true))?.contains("suspended") == true)
        #expect(MessageSafety.blockReason(attention: attention, expectedTTY: "ttys004", process: process(foreground: false))?.contains("foreground") == true)
    }

    @Test func ourOwnProcessIsSeenByTheKernel() throws {
        let status = try #require(ProcessTreeBuilder.shared.status(ofPid: Int(getpid())))
        #expect(status.pid == Int(getpid()))
        #expect(!status.isStopped)
        #expect(ProcessTreeBuilder.shared.status(ofPid: 0) == nil)
        #expect(ProcessTreeBuilder.shared.status(ofPid: Int(Int32.max)) == nil)
    }
}

/// The kernel-backed process table and the helper runner.
nonisolated struct A3_ProcessTests {
    @Test func theProcessTableHasUsAndOurParent() throws {
        let tree = ProcessTreeBuilder.shared.buildTree()
        let me = try #require(tree[Int(getpid())])
        #expect(me.ppid == Int(getppid()))
        #expect(!me.command.isEmpty)
        #expect(ProcessTreeBuilder.shared.isDescendant(targetPid: Int(getpid()), ofAncestor: Int(getppid()), tree: tree))
        #expect(ProcessTreeBuilder.shared.isInTmux(pid: Int(getpid())) == ProcessTreeBuilder.shared.isInTmux(pid: Int(getpid()), tree: tree))
    }

    @Test(.timeLimit(.minutes(1)))
    func largeOutputNeverWedgesTheRunner() async throws {
        // More than a pipe buffer on both streams at once.
        let outcome = await HelperProcess.run("/bin/sh", arguments: [
            "-c", "head -c 300000 /dev/zero; head -c 200000 /dev/zero >&2",
        ], timeout: 20)
        let output = try outcome.get()
        #expect(output.succeeded)
        #expect(output.stdout.count == 300_000)
        // stderr is kept only up to its cap.
        #expect(output.stderr.count >= 64 * 1024 && output.stderr.count < 200_000)
    }

    @Test(.timeLimit(.minutes(1)))
    func aHangingChildIsKilledAtTheTimeout() async throws {
        let started = Date()
        let output = try await HelperProcess.run("/bin/sleep", arguments: ["3600"], timeout: 1).get()
        #expect(output.timedOut)
        #expect(!output.succeeded)
        #expect(Date().timeIntervalSince(started) < 30)
    }

    @Test func failuresAreReported() async throws {
        let missing = await HelperProcess.run("/nonexistent/tool", arguments: [])
        #expect(missing == .failure(.notFound("/nonexistent/tool")))
        let failed = try await HelperProcess.run("/bin/sh", arguments: ["-c", "echo nope >&2; exit 3"]).get()
        #expect(!failed.succeeded && failed.status == 3 && failed.stderrText == "nope\n")
        let mapped = ProcessExecutor.result(of: .success(failed), executable: "/bin/sh", arguments: [])
        #expect(mapped == .failure(.executionFailed(command: "/bin/sh", exitCode: 3, stderr: "nope\n")))
    }
}
