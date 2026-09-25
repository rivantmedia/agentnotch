// swift-tools-version:6.2
//
// ClaudeControl: the Claude Code session and account layer of Agent
// Notch (hooks, sessions, review queue, permissions, chat, token-free usage),
// kept out of the upstream Codenotch sources so merges from upstream stay cheap.
// Ported from Superpowered Vibe Notch; Apache-2.0 (see LICENSE and NOTICE).
//
// One module, two folders:
// - Sources/ClaudeControl/Engine  models, services, the public facade. No SwiftUI
//                                 (Scripts/check-seams.sh greps for it).
// - Sources/ClaudeControl/UI      SwiftUI views built on the engine.
// Only what the app's bridge (Sources/ClaudeBridge) uses is `public`.
//
// - ClaudeControlSnapshots        renders the panel, chat and settings to PNGs
//                                 from fixtures: `swift run --package-path
//                                 Packages/ClaudeControl ClaudeControlSnapshots <dir>`
// - agentnotch-inspect-accounts         prints, read-only, the accounts the app
//                                 finds in a home folder (`--home <dir>`)
//
// Unlike the app target (Swift 5, minimal concurrency checking, like upstream)
// this package uses default MainActor isolation and the Swift 6.2 upcoming
// features, the same settings as the Superpowered Vibe Notch code it ports.
// Anything the app calls from off the main actor must be marked `nonisolated`.
//
// The hook and status line scripts are not SwiftPM resources: Scripts/*.py are
// embedded as Swift strings by Scripts/embed-scripts.sh into
// Engine/Scripts/EmbeddedScripts.swift (a test checks they are in step).
//
// Tests use Swift Testing, which ships with the Command Line Tools:
// `Scripts/spm-test.sh` from the repository root.
import PackageDescription

let isolationSettings: [SwiftSetting] = [
    .defaultIsolation(MainActor.self),
    .enableUpcomingFeature("InferSendableFromCaptures"),
    .enableUpcomingFeature("GlobalActorIsolatedTypesUsability"),
    .enableUpcomingFeature("DisableOutwardActorInference"),
    .enableUpcomingFeature("NonisolatedNonsendingByDefault"),
    .enableUpcomingFeature("InferIsolatedConformances"),
    .enableUpcomingFeature("MemberImportVisibility"),
]

let package = Package(
    name: "ClaudeControl",
    platforms: [.macOS("15.0")],
    products: [
        .library(name: "ClaudeControl", targets: ["ClaudeControl"]),
        .executable(name: "ClaudeControlSnapshots", targets: ["ClaudeControlSnapshots"]),
        .executable(name: "agentnotch-inspect-accounts", targets: ["agentnotch-inspect-accounts"]),
    ],
    dependencies: [
        .package(url: "https://github.com/swiftlang/swift-markdown", from: "0.5.0"),
    ],
    targets: [
        .target(
            name: "ClaudeControl",
            dependencies: [
                .product(name: "Markdown", package: "swift-markdown"),
            ],
            swiftSettings: isolationSettings
        ),
        .executableTarget(
            name: "ClaudeControlSnapshots",
            dependencies: ["ClaudeControl"],
            swiftSettings: isolationSettings
        ),
        // Read-only: what the app makes of this Mac's Claude folders
        // (accounts, their run folders and stores, install targets).
        .executableTarget(
            name: "agentnotch-inspect-accounts",
            dependencies: ["ClaudeControl"],
            swiftSettings: isolationSettings
        ),
        // Like the Superpowered Vibe Notch suites it ports: main-actor by
        // default, without the upcoming features.
        .testTarget(
            name: "ClaudeControlTests",
            dependencies: ["ClaudeControl"],
            swiftSettings: [.defaultIsolation(MainActor.self)]
        ),
    ],
    swiftLanguageModes: [.v5]
)
