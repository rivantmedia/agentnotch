import AppKit
import Foundation
import SwiftUI
import Testing
@testable import ClaudeControl

/// The panel laid out for real, in a window off screen: every route builds,
/// and the height it asks the window for stays within [220, cap].
@Suite(.serialized)
struct B_PanelRenderTests {
    private static let width: CGFloat = 400

    /// Host `view` at `width`, let it lay out and report, then hand back the
    /// window so the caller can look at the state.
    @discardableResult
    private func host(_ view: some View, height: CGFloat = 900) -> NSWindow {
        _ = NSApplication.shared
        let hosting = NSHostingView(rootView: view.claudeControlTheme(.codenotchDark))
        hosting.sizingOptions = []
        let window = NSWindow(contentRect: NSRect(x: -12_000, y: -12_000, width: Self.width, height: height),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = hosting
        window.orderFront(nil)
        for _ in 0..<6 {
            hosting.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        }
        return window
    }

    private func panel(_ sessions: [SessionState], accounts: [ClaudeAccountSummary] = UIFixtures.accounts(),
                       route: ClaudePanelRoute = .sessions(ringID: nil)) -> (SessionsPanelContent<some View>, ClaudePanelState) {
        let state = ClaudePanelState(route: route)
        state.contentWidth = Self.width
        var model = SessionsPanelModel(sessions: sessions, accounts: accounts)
        model.now = UIFixtures.now
        model.readings = UIFixtures.readings()
        let view = SessionsPanelContent(model: model, state: state, actions: SessionsPanelActions(), chat: { session, hooks in
            ChatContent(session: session, history: UIFixtures.chatHistory(), isLoading: false, canFocus: true,
                        messageRoute: .tmux, account: nil, agentDescriptions: [:], sendFailure: nil,
                        state: state, hooks: hooks, onSend: { _ in })
        })
        return (view, state)
    }

    @Test func idealHeightStaysWithinBoundsForEveryFixture() {
        let fixtures: [(String, [SessionState])] = [
            ("empty", []),
            ("one", [SampleSessions.permission()]),
            ("regular", UIFixtures.regular()),
            ("every state", UIFixtures.everyState()),
            ("busy", UIFixtures.density()),
        ]
        for (name, sessions) in fixtures {
            let (view, state) = panel(sessions)
            let window = host(view)
            defer { window.close() }
            let cap = ClaudePanelGeometry.heightCap(.list)
            #expect(state.idealContentHeight >= ClaudePanelState.minimumContentHeight, "\(name)")
            #expect(state.idealContentHeight <= cap, "\(name)")
            if name == "busy" {
                #expect(state.idealContentHeight == cap, "26 sessions need the whole panel")
            }
            if name == "regular" {
                #expect(state.idealContentHeight > ClaudePanelState.minimumContentHeight, "the list reported its height")
            }
        }
    }

    @Test func everyRouteBuilds() {
        let sessions = UIFixtures.everyState()
        let routes: [ClaudePanelRoute] = [
            .sessions(ringID: nil),
            .sessions(ringID: UIFixtures.ringID(UIFixtures.work)),
            .sessions(ringID: "claude-nobody"),
            .session(id: SampleSessions.permission().sessionId),
            .session(id: SampleSessions.question().sessionId),
            .session(id: "ended-session"),
            .setup,
        ]
        for route in routes {
            let (view, state) = panel(sessions, route: route)
            let window = host(view)
            defer { window.close() }
            let cap = ClaudePanelGeometry.heightCap(state.mode)
            #expect(state.idealContentHeight >= ClaudePanelState.minimumContentHeight, "\(route)")
            #expect(state.idealContentHeight <= cap, "\(route)")
        }
    }

    @Test func settingsPaneBuildsInEveryState() {
        for model in [UIFixtures.settings(), UIFixtures.settingsFirstRun(), SettingsPaneModel()] {
            let window = host(SettingsPaneContent(model: model, actions: SettingsPaneActions()), height: 700)
            window.close()
        }
    }
}

/// Source rules the design sets for the UI (acceptance, WP-B item 2–3).
struct B_SourceRulesTests {
    private static let packageRoot = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

    private func sources(_ folder: String) throws -> [(String, String)] {
        let root = Self.packageRoot.appendingPathComponent(folder)
        let enumerator = try #require(FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil))
        return enumerator.compactMap { $0 as? URL }
            .filter { $0.pathExtension == "swift" }
            .compactMap { url in (try? String(contentsOf: url, encoding: .utf8)).map { (url.lastPathComponent, $0) } }
    }

    /// Code, without comments (they may name what they avoid).
    private func code(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line in line.range(of: "//").map { String(line[..<$0.lowerBound]) } ?? String(line) }
            .joined(separator: "\n")
    }

    @Test func nothingLoopsForever() throws {
        let files = try sources("Sources/ClaudeControl/UI")
        #expect(!files.isEmpty)
        for (name, text) in files {
            #expect(!code(text).contains("repeatForever"), "\(name)")
        }
    }

    @Test func noNotchViewModelOrAppTypes() throws {
        let forbidden = ["NotchViewModel", "NotchLayout.", "Palette.", "Typography.", "Preferences(", "ProviderSnapshot",
                         "AgentSession", "NotchFleet"]
        for folder in ["Sources/ClaudeControl", "Sources/ClaudeControlSnapshots"] {
            for (name, text) in try sources(folder) {
                for word in forbidden {
                    #expect(!code(text).contains(word), "\(name) mentions \(word)")
                }
            }
        }
    }

    @Test func coloursComeFromTheThemeOnly() throws {
        let literal = ["Color(red:", "Color.white", "Color.black", ".foregroundColor(", "Color.orange", "Color.red",
                       "Color.green", "Color.blue", "Color.gray"]
        for (name, text) in try sources("Sources/ClaudeControl/UI") where name != "ClaudeControlTheme.swift" {
            for word in literal {
                #expect(!code(text).contains(word), "\(name) uses \(word)")
            }
        }
    }

    @Test func symbolButtonsAreLabelled() throws {
        // Bare-symbol buttons go through ClaudeIconButton, whose label is
        // required and is both the tooltip and what VoiceOver reads.
        let files = try sources("Sources/ClaudeControl/UI")
        let buttons = try #require(files.first { $0.0 == "ClaudeButtons.swift" }?.1)
        #expect(buttons.contains(".accessibilityLabel(label)"))
        #expect(buttons.contains(".help(label)"))
        let composer = try #require(files.first { $0.0 == "ChatApprovalBars.swift" }?.1)
        #expect(composer.contains(".accessibilityLabel(\"Send\")"))
    }
}
