//
//  SealedModeBadge.swift
//  ClaudeControl
//
//  A small label that says the app is running sealed, so a screenshot or a
//  screen recording taken in that mode is never mistaken for live data.
//

import SwiftUI

public struct SealedModeBadge: View {
    private let isShown: Bool

    public init() {
        isShown = SealedMode.isOn
    }

    /// Snapshots: draw it whatever mode the process is in.
    init(isShown: Bool) {
        self.isShown = isShown
    }

    public var body: some View {
        if isShown {
            Text("Sealed")
                .claudeFont(.caption, weight: .semibold)
                .foregroundStyle(.ink(.accent))
                .padding(.horizontal, 6)
                .padding(.vertical, 1.5)
                .background(Capsule().fill(.ink(.accent, opacity: 0.16)))
                .fixedSize()
                .accessibilityLabel("Sealed mode: showing fixture data")
        }
    }
}
