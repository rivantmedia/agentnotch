//
//  ChatQuestionPanel.swift
//  ClaudeControl
//
//  Answers AskUserQuestion from the chat: each question with its header,
//  its options (radio buttons, or checkboxes for multi-select) and a
//  free-text "Other". Submit sends Claude Code the answers through the
//  pending hook, exactly as if they were picked in the terminal.
//

import SwiftUI

struct ChatQuestionPanel: View {
    let questions: [ChatQuestion]
    let canFocusTerminal: Bool
    let onSubmit: ([String: String]) -> Void
    let onGoToTerminal: () -> Void

    /// Tall enough for one typical question; more scroll.
    var maxQuestionsHeight: CGFloat = 300

    @State private var selections: [Int: ChatQuestionSelection]
    @State private var didSubmit = false

    init(
        questions: [ChatQuestion],
        canFocusTerminal: Bool,
        maxQuestionsHeight: CGFloat = 300,
        initialSelections: [Int: ChatQuestionSelection] = [:],
        onSubmit: @escaping ([String: String]) -> Void,
        onGoToTerminal: @escaping () -> Void
    ) {
        self.questions = questions
        self.canFocusTerminal = canFocusTerminal
        self.maxQuestionsHeight = maxQuestionsHeight
        self.onSubmit = onSubmit
        self.onGoToTerminal = onGoToTerminal
        self._selections = State(initialValue: initialSelections)
    }

    private var answers: [String: String]? {
        ChatQuestionAnswers.answers(for: questions, selections: selections)
    }

    var body: some View {
        ChatBar {
            ChatBarTitle(text: questions.count == 1 ? "Claude has a question" : "Claude has \(questions.count) questions")

            ChatAdaptiveScroll(maxHeight: maxQuestionsHeight) {
                VStack(alignment: .leading, spacing: 12) {
                    ForEach(questions) { question in
                        ChatQuestionBlock(question: question, selection: binding(for: question.index))
                    }
                }
                .padding(.vertical, 1)
            }

            HStack(spacing: 6) {
                Text(footerText)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .lineLimit(1)
                Spacer(minLength: 6)
                if canFocusTerminal {
                    Button("Show terminal", action: onGoToTerminal)
                        .buttonStyle(.claude(.secondary))
                        .help("Answer in the terminal instead (⌘J)")
                }
                Button(questions.count == 1 ? "Submit" : "Submit answers") {
                    guard let answers else { return }
                    didSubmit = true
                    onSubmit(answers)
                }
                .buttonStyle(.claude(.primary))
                .disabled(answers == nil || didSubmit)
            }
        }
    }

    private var footerText: String {
        if didSubmit { return "Sent to Claude" }
        let answered = questions.filter { ChatQuestionAnswers.answer(for: $0, selection: selections[$0.index]) != nil }.count
        if questions.count > 1 { return "\(answered) of \(questions.count) answered" }
        return answered == 1 ? "Ready to send" : "Pick an answer"
    }

    private func binding(for index: Int) -> Binding<ChatQuestionSelection> {
        Binding(
            get: { selections[index] ?? ChatQuestionSelection() },
            set: { selections[index] = $0 }
        )
    }
}

// MARK: - One question

private struct ChatQuestionBlock: View {
    let question: ChatQuestion
    @Binding var selection: ChatQuestionSelection

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 7) {
                if let header = question.header {
                    Text(header)
                        .claudeFont(.caption, weight: .semibold)
                        .foregroundStyle(.ink(.needsYou))
                        .fixedSize()
                }
                Text(question.question)
                    .claudeFont(.body, weight: .medium)
                    .foregroundStyle(.ink(.primary))
                    .fixedSize(horizontal: false, vertical: true)
                if question.multiSelect {
                    Text("Choose any")
                        .claudeFont(.caption)
                        .foregroundStyle(.ink(.secondary))
                        .fixedSize()
                }
            }

            VStack(alignment: .leading, spacing: 1) {
                ForEach(question.options) { option in
                    ChatOptionRow(
                        title: option.label,
                        detail: option.description,
                        isMultiSelect: question.multiSelect,
                        isSelected: selection.isSelected(option.label)
                    ) {
                        selection.toggle(option.label, multiSelect: question.multiSelect)
                    }
                }
                ChatOtherOptionRow(
                    isMultiSelect: question.multiSelect,
                    isSelected: selection.isOtherChosen,
                    text: $selection.otherText
                ) {
                    selection.toggleOther(multiSelect: question.multiSelect)
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(question.question)
    }
}

// MARK: - Option rows

/// A radio button, or a checkbox for multi-select.
private struct ChatSelectionMark: View {
    let isMultiSelect: Bool
    let isSelected: Bool

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        ZStack {
            if isMultiSelect {
                RoundedRectangle(cornerRadius: 3, style: .continuous)
                    .fill(isSelected ? theme.primaryFill : .clear)
                RoundedRectangle(cornerRadius: 3, style: .continuous)
                    .strokeBorder(isSelected ? .clear : theme.textSecondary, lineWidth: 1)
                if isSelected {
                    Image(systemName: "checkmark")
                        .font(.system(size: 7.5, weight: .heavy))
                        .foregroundStyle(.ink(.onPrimary))
                }
            } else {
                Circle()
                    .strokeBorder(isSelected ? theme.textPrimary : theme.textSecondary, lineWidth: isSelected ? 3.5 : 1)
            }
        }
        .frame(width: 11, height: 11)
        .accessibilityHidden(true)
    }
}

private struct ChatOptionRow: View {
    let title: String
    let detail: String?
    let isMultiSelect: Bool
    let isSelected: Bool
    let action: () -> Void

    @Environment(\.claudeControlTheme) private var theme
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                ChatSelectionMark(isMultiSelect: isMultiSelect, isSelected: isSelected)
                    .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 3.5 }
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .claudeFont(.body, weight: .medium)
                        .foregroundStyle(.ink(.primary))
                    if let detail {
                        Text(detail)
                            .claudeFont(.caption)
                            .foregroundStyle(.ink(.secondary))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(isSelected ? theme.rowSelection : isHovered ? theme.rowHover : .clear)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .accessibilityLabel(detail.map { "\(title), \($0)" } ?? title)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }
}

/// "Other", with a text field once chosen.
private struct ChatOtherOptionRow: View {
    let isMultiSelect: Bool
    let isSelected: Bool
    @Binding var text: String
    let action: () -> Void

    @Environment(\.claudeControlTheme) private var theme
    @State private var isHovered = false

    var body: some View {
        HStack(spacing: 8) {
            Button(action: action) {
                HStack(spacing: 8) {
                    ChatSelectionMark(isMultiSelect: isMultiSelect, isSelected: isSelected)
                    Text("Other")
                        .claudeFont(.body, weight: .medium)
                        .foregroundStyle(.ink(isSelected || isHovered ? .primary : .secondary))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .onHover { isHovered = $0 }
            .accessibilityLabel("Other")
            .accessibilityAddTraits(isSelected ? .isSelected : [])

            if isSelected {
                ClaudeTextField(placeholder: "Type your answer", text: $text)
                    .transition(.opacity)
            } else {
                Spacer(minLength: 0)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, isSelected ? 2 : 5)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(isSelected ? theme.rowSelection : isHovered ? theme.rowHover : .clear)
        )
    }
}
