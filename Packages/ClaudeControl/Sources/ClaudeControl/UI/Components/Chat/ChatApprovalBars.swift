//
//  ChatApprovalBars.swift
//  ClaudeControl
//
//  The chat's bottom bars: a tool permission (the whole request, what
//  "Always" would save, Deny / Always allow / Allow), a plan from plan mode
//  (Keep planning / Approve plan), a dialog only the terminal can answer,
//  and the composer. Questions have their own panel (ChatQuestionPanel).
//

import SwiftUI

// MARK: - Permission suggestion text

/// Describes Claude Code's `permission_suggestions` entry that "Always allow"
/// applies (the terminal's "Yes, and don't ask again").
enum PermissionSuggestionText {
    nonisolated static func describe(_ suggestions: [AnyCodable]?) -> String? {
        guard let first = suggestions?.first?.value as? [String: Any],
              let type = first["type"] as? String else { return nil }
        let destination = destinationPhrase(first["destination"] as? String)

        switch type {
        case "addRules", "replaceRules":
            let rules = (first["rules"] as? [Any] ?? []).compactMap { rule -> String? in
                guard let dict = rule as? [String: Any], let tool = dict["toolName"] as? String, !tool.isEmpty else {
                    return nil
                }
                if let content = dict["ruleContent"] as? String, !content.isEmpty {
                    return "\(tool)(\(content))"
                }
                return tool
            }
            guard !rules.isEmpty else { return nil }
            return "Don't ask again for \(rules.joined(separator: ", "))\(destination.map { " \($0)" } ?? "")"
        case "setMode":
            guard let mode = first["mode"] as? String else { return nil }
            return "Switch to \(modeName(mode))\(destination.map { " \($0)" } ?? "")"
        case "addDirectories":
            let directories = (first["directories"] as? [Any] ?? []).compactMap { $0 as? String }
            guard !directories.isEmpty else { return nil }
            let names = directories.map { URL(fileURLWithPath: $0).lastPathComponent }
            return "Allow access to \(names.joined(separator: ", "))\(destination.map { " \($0)" } ?? "")"
        default:
            return nil
        }
    }

    nonisolated private static func destinationPhrase(_ destination: String?) -> String? {
        switch destination {
        case "session": return "for this session"
        case "localSettings": return "in this project (just you)"
        case "projectSettings": return "in this project (shared)"
        case "userSettings": return "in all projects"
        default: return nil
        }
    }

    nonisolated private static func modeName(_ mode: String) -> String {
        switch mode {
        case "acceptEdits": return "accept-edits mode"
        case "bypassPermissions": return "bypass-permissions mode"
        case "plan": return "plan mode"
        case "default": return "default mode"
        default: return "\(mode) mode"
        }
    }
}

/// What the chat and the row say about plans.
enum PlanApprovalCopy {
    /// What Claude reads back when the user keeps planning.
    static let keepPlanningReason = "The user reviewed the plan and wants to keep planning. Stay in plan mode and ask what to change before implementing."
}

// MARK: - Container

/// The shared frame of the bottom bars: the panel's padding, nothing drawn
/// behind (the hairline above separates it from the transcript).
struct ChatBar<Content: View>: View {
    @ViewBuilder let content: Content

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: theme.blockSpacing) {
            content
        }
        .padding(theme.padding)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A bar's title line: a signal-coloured mark and a line of primary ink.
struct ChatBarTitle: View {
    let text: String
    var kind: SessionGlyphKind = .needsInput

    var body: some View {
        HStack(spacing: 7) {
            StatusRing(kind: kind)
            Text(text)
                .claudeFont(.body, weight: .semibold)
                .foregroundStyle(.ink(.primary))
                .lineLimit(1)
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

// MARK: - Permission

/// "Bash wants to run", the whole request, and the answers.
struct ChatApprovalBar: View {
    let tool: String
    let request: String?
    let alwaysAllow: AlwaysAllowOffer?
    let onApprove: () -> Void
    let onAlwaysAllow: () -> Void
    let onDeny: () -> Void

    @Environment(\.claudeControlTheme) private var theme

    private var toolName: String { MCPToolFormatter.formatToolName(tool) }

    var body: some View {
        ChatBar {
            ChatBarTitle(text: "\(toolName) needs your permission")
            if let request, !request.isEmpty {
                ChatAdaptiveScroll(maxHeight: 132) {
                    Text(request)
                        .claudeFont(.mono)
                        .foregroundStyle(.ink(.primary))
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                }
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(theme.controlFill))
            }
            if let description = alwaysAllow?.description {
                Text("Always allow: \(description).")
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 6) {
                Spacer(minLength: 0)
                Button("Deny", action: onDeny)
                    .buttonStyle(.claude(.secondary))
                    .help("Deny (⌘⌫)")
                if alwaysAllow != nil {
                    Button("Always allow", action: onAlwaysAllow)
                        .buttonStyle(.claude(.secondary))
                        .help("\(alwaysAllow?.description ?? "Allow, and don't ask again") (⌥⌘⏎)")
                }
                Button("Allow", action: onApprove)
                    .buttonStyle(.claude(.primary))
                    .help("Allow (⌘⏎)")
                    .padding(.leading, alwaysAllow != nil ? 4 : 0)
            }
        }
    }
}

// MARK: - Plan

/// ExitPlanMode: the plan, "Keep planning" and "Approve plan".
struct ChatPlanApprovalBar: View {
    let plan: String?
    /// Tallest the plan gets.
    var maxPlanHeight: CGFloat = 280
    let onApprove: () -> Void
    let onKeepPlanning: () -> Void

    @Environment(\.claudeControlTheme) private var theme

    private var hasPlan: Bool {
        !(plan?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
    }

    var body: some View {
        ChatBar {
            ChatBarTitle(text: "Plan ready for approval")
            if let plan, hasPlan {
                ChatAdaptiveScroll(maxHeight: maxPlanHeight) {
                    MarkdownText(plan, token: .body)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(9)
                }
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(theme.controlFill))
            }
            HStack(spacing: 6) {
                Text("Approving lets Claude start on it.")
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .lineLimit(1)
                Spacer(minLength: 6)
                Button("Keep planning", action: onKeepPlanning)
                    .buttonStyle(.claude(.secondary))
                    .help("Stay in plan mode and say what to change (⌘⌫)")
                Button("Approve plan", action: onApprove)
                    .buttonStyle(.claude(.primary))
                    .help("Approve the plan (⌘⏎)")
            }
        }
    }
}

// MARK: - Terminal only

/// Something only the terminal can answer, or a session the panel can't
/// type into: say so, and offer the terminal.
struct ChatTerminalOnlyBar: View {
    let title: String?
    let message: String
    let canFocus: Bool
    let onFocus: () -> Void

    var body: some View {
        ChatBar {
            if let title {
                ChatBarTitle(text: title)
            }
            HStack(alignment: .center, spacing: 8) {
                Text(message)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if canFocus {
                    Button("Show terminal", action: onFocus)
                        .buttonStyle(.claude(title == nil ? .secondary : .primary))
                        .help("Bring the session's terminal to the front (⌘J)")
                }
            }
        }
    }
}

// MARK: - Composer

/// A reply typed into the session's terminal: ⏎ sends, ⇧⏎ starts a new line.
struct ChatComposer: View {
    @Binding var draft: String
    let failure: String?
    @Binding var isFocused: Bool
    let onFocusRequest: () -> Void
    let onSend: (String) -> Void

    @Environment(\.claudeControlTheme) private var theme

    private var trimmed: String { draft.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        ChatBar {
            if let failure {
                Text(failure)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.critical))
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(alignment: .bottom, spacing: 8) {
                ClaudeTextField(
                    placeholder: "Reply to Claude",
                    text: $draft,
                    font: .chat,
                    lineLimit: 1...5,
                    isFocused: $isFocused,
                    onFocusRequest: onFocusRequest,
                    onSubmit: send,
                    focusesOnAppear: true
                )
                Button(action: send) {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(.ink(trimmed.isEmpty ? .tertiary : .onPrimary))
                        .frame(width: 26, height: 26)
                        .background(Circle().fill(trimmed.isEmpty ? theme.controlFill : theme.primaryFill))
                        .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .disabled(trimmed.isEmpty)
                .help("Send (⏎). ⇧⏎ starts a new line.")
                .accessibilityLabel("Send")
            }
        }
    }

    private func send() {
        let text = trimmed
        guard !text.isEmpty else { return }
        onSend(text)
    }
}
