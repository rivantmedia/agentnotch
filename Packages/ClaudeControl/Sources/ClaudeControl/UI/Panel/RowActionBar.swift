//
//  RowActionBar.swift
//  ClaudeControl
//
//  Answering a session from its row: Deny / Always / Allow for a permission,
//  one chip per option for a short question, Review plan / Approve for a
//  plan, "Answer…" for anything richer and "Show terminal" for a dialog only
//  the terminal can answer. Every button carries the request it was drawn
//  for, so a click can't land on a request that replaced it.
//

import SwiftUI

struct RowActionBar: View {
    let sessionId: String
    let actions: SessionPrimaryActions
    let canFocus: Bool
    let perform: (ClaudeKeyRouter.Command) -> Void

    var body: some View {
        switch actions {
        case .none:
            EmptyView()
        case let .permission(toolUseId, always, needsReview):
            permissionBar(toolUseId: toolUseId, always: always, needsReview: needsReview)
        case let .questionChips(toolUseId, question):
            AnswerChips(sessionId: sessionId, toolUseId: toolUseId, question: question, perform: perform)
        case .answerInChat:
            trailingButtons {
                Button("Answer…") { perform(.openChat(sessionId: sessionId)) }
                    .buttonStyle(.claude(.primary, compact: true))
                    .help("Answer in the chat (⌘⏎)")
            }
        case .plan(let toolUseId):
            trailingButtons {
                Button("Review plan") { perform(.openChat(sessionId: sessionId)) }
                    .buttonStyle(.claude(.secondary, compact: true))
                    .help("Read the whole plan (⏎)")
                Button("Approve") { perform(.approvePlan(sessionId: sessionId, toolUseId: toolUseId)) }
                    .buttonStyle(.claude(.primary, compact: true))
                    .help("Approve the plan and let Claude start (⌘⏎)")
            }
        case .answerInTerminal:
            HStack(spacing: 6) {
                Text("Answer in the terminal")
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                Spacer(minLength: 4)
                if canFocus {
                    Button("Show terminal") { perform(.jump(sessionId: sessionId)) }
                        .buttonStyle(.claude(.secondary, compact: true))
                        .help("Bring the session's terminal to the front (⌘J)")
                }
            }
        }
    }

    @ViewBuilder
    private func permissionBar(toolUseId: String, always: AlwaysAllowOffer?, needsReview: Bool) -> some View {
        let showsAlways = always?.isInline == true && !needsReview
        VStack(alignment: .trailing, spacing: 4) {
            if showsAlways, let description = always?.description {
                // Said out loud, not only in a tooltip: what "Always" saves,
                // and where.
                Text("Always: \(description)")
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .lineLimit(2)
                    .multilineTextAlignment(.trailing)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            } else if needsReview {
                Text("Too long to judge from here: review it whole first.")
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            trailingButtons {
                Button("Deny") { perform(.deny(sessionId: sessionId, toolUseId: toolUseId)) }
                    .buttonStyle(.claude(.secondary, compact: true))
                    .help("Deny (⌘⌫)")
                if showsAlways {
                    // A lasting rule: plain, never the eye-catching one, and
                    // amber stays "needs you" (GUX-15).
                    Button("Always") { perform(.alwaysAllow(sessionId: sessionId, toolUseId: toolUseId)) }
                        .buttonStyle(.claude(.secondary, compact: true))
                        .help("\(always?.description ?? "Allow, and don't ask again") (⌥⌘⏎)")
                }
                if needsReview {
                    Button("Review…") { perform(.openChat(sessionId: sessionId)) }
                        .buttonStyle(.claude(.primary, compact: true))
                        .help("Open the whole request in the chat (⌘⏎)")
                } else {
                    Button("Allow") { perform(.allow(sessionId: sessionId, toolUseId: toolUseId)) }
                        .buttonStyle(.claude(.primary, compact: true))
                        .help("Allow (⌘⏎)")
                        .padding(.leading, showsAlways ? 4 : 0)
                }
            }
        }
    }

    private func trailingButtons<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 5) {
            Spacer(minLength: 0)
            content()
        }
    }
}

// MARK: - Answer chips

/// "Which charting library should the dashboard use?" answered in one click:
/// a chip per option, numbered for the 1–4 keys, then "Other…" for a typed
/// answer in the chat.
private struct AnswerChips: View {
    let sessionId: String
    let toolUseId: String
    let question: ChatQuestion
    let perform: (ClaudeKeyRouter.Command) -> Void

    var body: some View {
        FlowLayout(spacing: 5, lineSpacing: 5, fillsWidth: true) {
            ForEach(Array(question.options.enumerated()), id: \.offset) { index, option in
                Button {
                    perform(.chooseOption(sessionId: sessionId, toolUseId: toolUseId, index: index))
                } label: {
                    HStack(spacing: 5) {
                        Text("\(index + 1)")
                            .claudeFont(.monoCaption)
                            .foregroundStyle(.ink(.needsYou, opacity: 0.7))
                        Text(option.label)
                    }
                }
                .buttonStyle(.claude(.tinted(.needsYou), compact: true))
                .help(option.description.map { "\($0) (\(index + 1))" } ?? "Answer \(option.label) (\(index + 1))")
                .accessibilityLabel(option.label)
            }
            Button("Other…") { perform(.openChat(sessionId: sessionId)) }
                .buttonStyle(.claude(.quiet, compact: true))
                .help("Type another answer in the chat")
        }
    }
}
