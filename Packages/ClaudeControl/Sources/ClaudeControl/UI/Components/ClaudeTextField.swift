//
//  ClaudeTextField.swift
//  ClaudeControl
//
//  The panel's one text field: a soft rounded fill that brightens its
//  outline while it has the focus. With more than one line allowed, ⏎
//  submits and ⇧⏎ starts a new line. Snapshots draw its text statically.
//

import SwiftUI

struct ClaudeTextField: View {
    let placeholder: String
    @Binding var text: String
    var font: ClaudeFontToken = .body
    var lineLimit: ClosedRange<Int> = 1...1
    /// Mirrors the field's keyboard focus (nil: not tracked).
    var isFocused: Binding<Bool>? = nil
    /// The field was clicked: the panel should become key.
    var onFocusRequest: () -> Void = {}
    var onSubmit: () -> Void = {}
    /// Take the keyboard focus when shown (the chat's composer).
    var focusesOnAppear = false

    @Environment(\.claudeControlTheme) private var theme
    @Environment(\.claudeStaticRendering) private var isStatic
    @FocusState private var hasFocus: Bool

    var body: some View {
        Group {
            if isStatic {
                Text(text.isEmpty ? placeholder : text)
                    .foregroundStyle(.ink(text.isEmpty ? .secondary : .primary))
                    .lineLimit(lineLimit.upperBound)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                TextField(placeholder, text: $text, axis: lineLimit.upperBound > 1 ? .vertical : .horizontal)
                    .textFieldStyle(.plain)
                    .foregroundStyle(.ink(.primary))
                    .lineLimit(lineLimit)
                    .focused($hasFocus)
                    .onSubmit(onSubmit)
                    .onKeyPress(.return, phases: .down) { press in
                        guard lineLimit.upperBound > 1 else { return .ignored }
                        if press.modifiers.contains(.shift) || press.modifiers.contains(.option) {
                            text.append("\n")
                        } else {
                            onSubmit()
                        }
                        return .handled
                    }
                    .onChange(of: hasFocus) { _, focused in
                        isFocused?.wrappedValue = focused
                        if focused { onFocusRequest() }
                    }
                    .simultaneousGesture(TapGesture().onEnded(onFocusRequest))
                    .onAppear { if focusesOnAppear { hasFocus = true } }
            }
        }
        .claudeFont(font)
        .padding(.horizontal, 9)
        .padding(.vertical, 6)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous).fill(theme.controlFill)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .strokeBorder(hasFocus ? theme.rowSelectionStroke : .clear, lineWidth: theme.hairline)
        )
    }
}
