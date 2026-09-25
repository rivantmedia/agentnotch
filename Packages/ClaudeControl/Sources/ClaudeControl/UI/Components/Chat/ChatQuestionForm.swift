//
//  ChatQuestionForm.swift
//  ClaudeControl
//
//  The model behind answering AskUserQuestion from the panel: the questions
//  parsed from the pending permission's `tool_input`, the user's selections,
//  and the `answers` map Claude Code expects back (question text → chosen
//  label; several labels joined by ", "; free text for "Other").
//
//  The one AskUserQuestion parser: the row's answer chips, the chat's
//  question panel and the notification body all read questions through
//  `ChatQuestion.parse`, so an answer is keyed the same way wherever it is
//  given.
//

import Foundation

/// One option of an AskUserQuestion question.
nonisolated struct ChatQuestionOption: Equatable, Sendable, Identifiable {
    let label: String
    let description: String?

    var id: String { label }
}

/// One AskUserQuestion question.
nonisolated struct ChatQuestion: Equatable, Sendable, Identifiable {
    /// Position in the tool input (questions can repeat a text in theory).
    let index: Int
    /// The question, trimmed for display.
    let question: String
    /// The question exactly as Claude Code sent it: the key of the `answers`
    /// map, which Claude Code matches by text.
    let answerKey: String
    /// Short chip label, e.g. "Database".
    let header: String?
    let multiSelect: Bool
    let options: [ChatQuestionOption]

    var id: Int { index }

    /// Chips fit in a row only for this many options.
    static let maxInlineOptions = 4

    /// The question when it can be answered with one tap: a single,
    /// single-choice question with 1...4 options.
    static func inline(_ questions: [ChatQuestion]) -> ChatQuestion? {
        guard questions.count == 1, let question = questions.first,
              !question.multiSelect,
              (1...maxInlineOptions).contains(question.options.count) else { return nil }
        return question
    }

    /// Questions from AskUserQuestion's `tool_input`:
    /// `{"questions": [{"question", "header", "multiSelect", "options": [{"label", "description"}]}]}`.
    /// Malformed entries are skipped; options without a label are dropped.
    static func parse(toolInput: [String: AnyCodable]?) -> [ChatQuestion] {
        guard let raw = toolInput?["questions"]?.value as? [Any] else { return [] }
        var result: [ChatQuestion] = []
        for entry in raw {
            guard let dict = Self.dictionary(entry),
                  let rawText = dict["question"] as? String else { continue }
            let text = rawText.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { continue }
            let options: [ChatQuestionOption] = ((dict["options"] as? [Any]) ?? []).compactMap { option in
                // Bare strings are options too.
                if let label = (Self.unwrapped(option) as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) {
                    return label.isEmpty ? nil : ChatQuestionOption(label: label, description: nil)
                }
                guard let optionDict = Self.dictionary(option),
                      let label = (optionDict["label"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
                      !label.isEmpty else { return nil }
                let description = (optionDict["description"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
                return ChatQuestionOption(label: label, description: description?.isEmpty == false ? description : nil)
            }
            let header = (dict["header"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            result.append(ChatQuestion(
                index: result.count,
                question: text,
                answerKey: rawText,
                header: header?.isEmpty == false ? header : nil,
                multiSelect: Self.bool(dict["multiSelect"]),
                options: options
            ))
        }
        return result
    }

    private static func dictionary(_ value: Any) -> [String: Any]? {
        unwrapped(value) as? [String: Any]
    }

    private static func unwrapped(_ value: Any) -> Any {
        (value as? AnyCodable)?.value ?? value
    }

    private static func bool(_ value: Any?) -> Bool {
        switch value {
        case let bool as Bool: return bool
        case let number as NSNumber: return number.boolValue
        case let string as String: return string.lowercased() == "true"
        default: return false
        }
    }
}

/// What the user picked for one question.
nonisolated struct ChatQuestionSelection: Equatable, Sendable {
    /// Chosen option labels (one for single-select questions).
    var labels: [String] = []
    /// "Other" is chosen; its text is the (or an additional) answer.
    var isOtherChosen = false
    var otherText = ""

    /// Single-select: choose `label` (clears "Other"). Multi-select: toggle it.
    mutating func toggle(_ label: String, multiSelect: Bool) {
        if multiSelect {
            if let index = labels.firstIndex(of: label) {
                labels.remove(at: index)
            } else {
                labels.append(label)
            }
        } else {
            labels = [label]
            isOtherChosen = false
        }
    }

    /// Single-select: choose "Other" (clears the options). Multi-select: toggle it.
    mutating func toggleOther(multiSelect: Bool) {
        if multiSelect {
            isOtherChosen.toggle()
        } else {
            isOtherChosen = true
            labels = []
        }
    }

    func isSelected(_ label: String) -> Bool {
        labels.contains(label)
    }
}

enum ChatQuestionAnswers {
    /// The answer text for one question, or nil while it is unanswered.
    /// Multi-select labels keep the question's option order and are joined
    /// by ", ", followed by the "Other" text when given.
    nonisolated static func answer(for question: ChatQuestion, selection: ChatQuestionSelection?) -> String? {
        guard let selection else { return nil }
        let other = selection.isOtherChosen
            ? selection.otherText.trimmingCharacters(in: .whitespacesAndNewlines)
            : ""
        let ordered = question.options.map(\.label).filter { selection.labels.contains($0) }
            // Labels no longer among the options (shouldn't happen) still count.
            + selection.labels.filter { label in !question.options.contains { $0.label == label } }

        if question.multiSelect {
            var parts = ordered
            if !other.isEmpty { parts.append(other) }
            return parts.isEmpty ? nil : parts.joined(separator: ", ")
        }
        if selection.isOtherChosen {
            return other.isEmpty ? nil : other
        }
        return ordered.first
    }

    /// Claude Code's `answers` map (question text → answer), or nil until
    /// every question has an answer.
    nonisolated static func answers(
        for questions: [ChatQuestion],
        selections: [Int: ChatQuestionSelection]
    ) -> [String: String]? {
        guard !questions.isEmpty else { return nil }
        var result: [String: String] = [:]
        for question in questions {
            guard let answer = answer(for: question, selection: selections[question.index]) else { return nil }
            result[question.answerKey] = answer
        }
        return result
    }

    /// The `answers` map for choosing `label` with an answer chip.
    nonisolated static func answers(for question: ChatQuestion, choosing label: String) -> [String: String] {
        [question.answerKey: label]
    }
}
