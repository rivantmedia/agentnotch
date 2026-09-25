// swift-tools-version:6.2
//
// Agent Notch: a SwiftPM build of the app for machines that have
// only the Command Line Tools. Xcode users keep using project.yml + the
// Makefile; this manifest mirrors that project and must be kept in step with it
// (sources, packages, the ClaudeControl local package).
//
//     Scripts/spm-build-app.sh   builds and assembles "build/Agent Notch.app"
//     Scripts/spm-test.sh        runs the Swift Testing suites (Packages/ClaudeControl)
//     Scripts/spm-run-sealed.sh  launches a sealed copy briefly and kills it
//
// What the Command Line Tools cannot do, and how the scripts make up for it:
// - `Localizable.xcstrings` needs xcstringstool, so it is excluded here and
//   converted into `<lang>.lproj/Localizable.strings` by the bundle script.
// - `Assets.xcassets` needs actool, so it is excluded and its images are copied
//   loose into Resources (AppIcon becomes an .icns through iconutil).
// - The Xcode build reaches the vendored zstd decoder through a bridging header;
//   here it is a C target, imported under `#if SWIFT_PACKAGE`.
//
// The upstream XCTest suite (Tests/) is not declared: XCTest does not ship with
// the Command Line Tools. It still runs through `make test` under Xcode.
//
// Upstream's Package.resolved at the root is the Xcode project's pin file (see
// `make gen` / `make verify-deps`). SwiftPM reads the same pins; the build script
// puts the file back if SwiftPM only rewrote its originHash.
import PackageDescription

let package = Package(
    name: "AgentNotch",
    platforms: [.macOS("15.0")],
    dependencies: [
        // Same requirements as project.yml, so both builds resolve to the pins
        // in Package.resolved.
        .package(url: "https://github.com/apple/swift-nio", from: "2.102.0"),
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.6.0"),
        .package(path: "Packages/ClaudeControl"),
    ],
    targets: [
        // The vendored Zstandard decoder (Claude Desktop's HTTP cache is zstd).
        .target(
            name: "CodenotchZstd",
            path: "Sources/Vendor/zstd",
            exclude: ["LICENSE", "README.md"],
            publicHeadersPath: "."
        ),
        .executableTarget(
            name: "Codenotch",
            dependencies: [
                "CodenotchZstd",
                .product(name: "NIOHTTP1", package: "swift-nio"),
                .product(name: "NIOPosix", package: "swift-nio"),
                .product(name: "Sparkle", package: "Sparkle"),
                .product(name: "ClaudeControl", package: "ClaudeControl"),
            ],
            path: "Sources",
            exclude: [
                "Info.plist",
                "Assets.xcassets",
                "Resources",
                "Localizable.xcstrings",
                "Vendor",
                "Codenotch-Bridging-Header.h",
            ]
        ),
        // The fork's app-side Swift Testing suites (Scripts/spm-test.sh app).
        // Under Xcode, project.yml's CodenotchTests builds the same files.
        .testTarget(
            name: "CodenotchForkTests",
            dependencies: ["Codenotch"],
            path: "Tests/ForkSPM"
        ),
    ],
    // project.yml: SWIFT_VERSION 5.0, SWIFT_STRICT_CONCURRENCY minimal.
    swiftLanguageModes: [.v5]
)
