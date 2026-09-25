//
//  AccountTag.swift
//  ClaudeControl
//
//  Which account a session runs under: the account's colour dot and its
//  name, never the colour alone. Shown only when more than one account is in
//  use.
//

import SwiftUI

struct AccountTag: View {
    let label: String
    let colorIndex: Int
    var font: ClaudeFontToken = .caption

    var body: some View {
        HStack(spacing: 4) {
            AccountDot(colorIndex: colorIndex)
            Text(label)
                .claudeFont(font)
                .foregroundStyle(.ink(.secondary))
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Account \(label)")
    }
}

/// A filled circle in the account's colour.
struct AccountDot: View {
    let colorIndex: Int
    var size: CGFloat = 5

    var body: some View {
        Circle()
            .fill(.ink(.account(colorIndex)))
            .frame(width: size, height: size)
    }
}
