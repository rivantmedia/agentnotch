//
//  B_MovedChatSettingsTests.swift
//  ClaudeControlTests
//
//  UI suites moved out of Engine/ChatAndNotificationTests.swift at
//  integration (WP-B's request): AskUserQuestion answers, permission
//  suggestion text and the settings helpers all test UI types.
//

import Foundation
import Testing
@testable import ClaudeControl

// MARK: - AskUserQuestion answers

struct ChatQuestionTests {
    static let input: [String: AnyCodable] = [
        "questions": AnyCodable([
            [
                "question": "Which database?",
                "header": "DB",
                "multiSelect": false,
                "options": [
                    ["label": "Postgres", "description": "Relational"],
                    ["label": "SQLite"],
                    ["description": "no label, dropped"],
                ],
            ] as [String: Any],
            [
                "question": "Which features?",
                "multiSelect": true,
                "options": [["label": "Auth"], ["label": "Billing"], ["label": "Search"]],
            ] as [String: Any],
            ["header": "no question text, skipped"] as [String: Any],
        ] as [Any]),
    ]

    @Test func parsesQuestionsAndOptions() {
        let questions = ChatQuestion.parse(toolInput: Self.input)
        #expect(questions.count == 2)
        #expect(questions[0].question == "Which database?")
        #expect(questions[0].header == "DB")
        #expect(!questions[0].multiSelect)
        #expect(questions[0].options.map(\.label) == ["Postgres", "SQLite"])
        #expect(questions[0].options[0].description == "Relational")
        #expect(questions[0].options[1].description == nil)
        #expect(questions[1].multiSelect)
        #expect(questions[1].header == nil)
        #expect(questions[1].index == 1)
    }

    @Test func parsesDecodedHookJSON() throws {
        let json = #"{"questions":[{"question":"Ship it?","header":"Go","multiSelect":false,"options":[{"label":"Yes","description":"Now"},{"label":"No","description":"Later"}]}]}"#
        let decoded = try JSONDecoder().decode([String: AnyCodable].self, from: Data(json.utf8))
        let questions = ChatQuestion.parse(toolInput: decoded)
        #expect(questions.count == 1)
        #expect(questions[0].options.map(\.label) == ["Yes", "No"])
        #expect(questions[0].multiSelect == false)
    }

    @Test func missingOrMalformedInputGivesNoQuestions() {
        #expect(ChatQuestion.parse(toolInput: nil).isEmpty)
        #expect(ChatQuestion.parse(toolInput: ["questions": AnyCodable("nope")]).isEmpty)
    }

    @Test func singleSelectAnswers() {
        let question = ChatQuestion.parse(toolInput: Self.input)[0]
        var selection = ChatQuestionSelection()
        #expect(ChatQuestionAnswers.answer(for: question, selection: selection) == nil)

        selection.toggle("SQLite", multiSelect: false)
        selection.toggle("Postgres", multiSelect: false)
        #expect(ChatQuestionAnswers.answer(for: question, selection: selection) == "Postgres")

        // "Other" replaces the option and needs text.
        selection.toggleOther(multiSelect: false)
        #expect(selection.labels.isEmpty)
        #expect(ChatQuestionAnswers.answer(for: question, selection: selection) == nil)
        selection.otherText = "  DuckDB  "
        #expect(ChatQuestionAnswers.answer(for: question, selection: selection) == "DuckDB")

        // Picking an option again drops "Other".
        selection.toggle("SQLite", multiSelect: false)
        #expect(ChatQuestionAnswers.answer(for: question, selection: selection) == "SQLite")
    }

    @Test func multiSelectJoinsLabelsInOptionOrder() {
        let question = ChatQuestion.parse(toolInput: Self.input)[1]
        var selection = ChatQuestionSelection()
        selection.toggle("Search", multiSelect: true)
        selection.toggle("Auth", multiSelect: true)
        #expect(ChatQuestionAnswers.answer(for: question, selection: selection) == "Auth, Search")

        selection.toggle("Search", multiSelect: true)
        #expect(ChatQuestionAnswers.answer(for: question, selection: selection) == "Auth")

        selection.toggleOther(multiSelect: true)
        selection.otherText = "Exports"
        #expect(ChatQuestionAnswers.answer(for: question, selection: selection) == "Auth, Exports")

        selection.toggle("Auth", multiSelect: true)
        #expect(ChatQuestionAnswers.answer(for: question, selection: selection) == "Exports")
    }

    @Test func answersMapNeedsEveryQuestion() {
        let questions = ChatQuestion.parse(toolInput: Self.input)
        var selections: [Int: ChatQuestionSelection] = [0: ChatQuestionSelection(labels: ["Postgres"])]
        #expect(ChatQuestionAnswers.answers(for: questions, selections: selections) == nil)

        selections[1] = ChatQuestionSelection(labels: ["Billing", "Auth"])
        #expect(ChatQuestionAnswers.answers(for: questions, selections: selections) == [
            "Which database?": "Postgres",
            "Which features?": "Auth, Billing",
        ])
        #expect(ChatQuestionAnswers.answers(for: [], selections: [:]) == nil)
    }
}

// MARK: - Permission suggestions

struct PermissionSuggestionTextTests {
    @Test func describesRules() {
        let suggestion = AnyCodable([
            "type": "addRules",
            "rules": [["toolName": "Bash", "ruleContent": "npm test:*"]],
            "behavior": "allow",
            "destination": "localSettings",
        ] as [String: Any])
        #expect(PermissionSuggestionText.describe([suggestion]) == "Don't ask again for Bash(npm test:*) in this project (just you)")

        let bare = AnyCodable(["type": "addRules", "rules": [["toolName": "WebFetch"]], "destination": "session"] as [String: Any])
        #expect(PermissionSuggestionText.describe([bare]) == "Don't ask again for WebFetch for this session")
    }

    @Test func describesModesAndDirectories() {
        let mode = AnyCodable(["type": "setMode", "mode": "acceptEdits", "destination": "session"] as [String: Any])
        #expect(PermissionSuggestionText.describe([mode]) == "Switch to accept-edits mode for this session")

        let dirs = AnyCodable(["type": "addDirectories", "directories": ["/Users/me/other"], "destination": "session"] as [String: Any])
        #expect(PermissionSuggestionText.describe([dirs]) == "Allow access to other for this session")
    }

    @Test func unknownShapesGiveNil() {
        #expect(PermissionSuggestionText.describe(nil) == nil)
        #expect(PermissionSuggestionText.describe([]) == nil)
        #expect(PermissionSuggestionText.describe([AnyCodable("x")]) == nil)
        #expect(PermissionSuggestionText.describe([AnyCodable(["type": "addRules", "rules": []] as [String: Any])]) == nil)
    }
}

// MARK: - Settings helpers

struct SettingsHelperTests {
    @Test func hookSummaryKinds() {
        var status = AccountHookStatus()
        status.configDirExists = true
        status.hooksInstalled = true
        status.statusLineInstalled = true
        #expect(AccountHookSummary.make(status: status, hooksEnabled: true, installsDisabled: false).kind == .installed)
        #expect(AccountHookSummary.make(status: status, hooksEnabled: true, installsDisabled: false).detail == "Hooks installed · live status line on")

        status.hooksInstalled = false
        let off = AccountHookSummary.make(status: status, hooksEnabled: false, installsDisabled: false)
        #expect(off.kind == .notInstalled)
        #expect(off.detail == "Hooks are turned off (see Hooks below).")
        #expect(AccountHookSummary.make(status: status, hooksEnabled: true, installsDisabled: true).detail?.contains("--no-install") == true)

        status.lastError = "Couldn't write settings.json"
        #expect(AccountHookSummary.make(status: status, hooksEnabled: true, installsDisabled: false).detail == "Couldn't write settings.json")

        #expect(AccountHookSummary.make(status: status, hooksEnabled: true, installsDisabled: false, isHidden: true).kind == .hidden)

        status.settingsReadable = false
        #expect(AccountHookSummary.make(status: status, hooksEnabled: true, installsDisabled: false).kind == .unreadable)
        status.configDirExists = false
        #expect(AccountHookSummary.make(status: status, hooksEnabled: true, installsDisabled: false).kind == .missingFolder)
        #expect(AccountHookSummary.make(status: nil, hooksEnabled: true, installsDisabled: false).kind == .unknown)
    }

    @Test func pathsUnderHomeAreAbbreviated() {
        #expect(AccountPathDisplay.abbreviated("/Users/me/.claude", home: "/Users/me") == "~/.claude")
        #expect(AccountPathDisplay.abbreviated("/Users/me", home: "/Users/me/") == "~")
        #expect(AccountPathDisplay.abbreviated("/Users/meow/.claude", home: "/Users/me") == "/Users/meow/.claude")
        #expect(AccountPathDisplay.abbreviated("/opt/claude", home: "/Users/me") == "/opt/claude")
    }

    @Test func contextMeterLevels() {
        #expect(ContextMeter.level(for: 42) == .normal)
        #expect(ContextMeter.level(for: 80) == .high)
        #expect(ContextMeter.level(for: 89.9) == .high)
        #expect(ContextMeter.level(for: 90) == .critical)
    }
}
