//
//  ClaudeControlSnapshots
//
//  Renders the panel, chat and settings views from fixtures to PNGs, so they
//  can be looked at without launching the app:
//
//      swift run --package-path Packages/ClaudeControl ClaudeControlSnapshots <dir> [name-filter]
//
//  Each sheet is hosted in a borderless window far off screen and drawn with
//  `cacheDisplay`, the way Codenotch's own render tests do it: unlike
//  ImageRenderer, that draws AppKit-backed controls (switches, pickers,
//  text fields, grouped forms) as they really look. Nothing real is read:
//  every view gets sample data.
//

import AppKit
@_spi(Snapshots) import ClaudeControl
import SwiftUI

let arguments = Array(CommandLine.arguments.dropFirst())
guard let outPath = arguments.first else {
    FileHandle.standardError.write(Data("usage: ClaudeControlSnapshots <output dir> [name filter]\n".utf8))
    exit(2)
}
let filter = arguments.dropFirst().first
let outDir = URL(fileURLWithPath: outPath, isDirectory: true)
try FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

/// Desktop visible around each card, so its corners read.
let margin: CGFloat = 16

@MainActor
func settle(_ seconds: TimeInterval = 0.25) {
    RunLoop.main.run(until: Date().addingTimeInterval(seconds))
}

@MainActor
func render(_ sheet: ClaudeControlSnapshotSheets.Sheet) -> Data? {
    let content = sheet.view
        .frame(width: sheet.width)
        .padding(margin)
        .background(Color(white: 0.16))
    let hosting = NSHostingView(rootView: content)
    if case .fitting = sheet.height {} else {
        // The window's size is ours to set: content taller than it is
        // clipped, as the panel window clips (and scrolls) it.
        hosting.sizingOptions = []
    }
    let window = NSWindow(contentRect: NSRect(x: -10_000, y: -10_000, width: sheet.width + 2 * margin, height: 800),
                          styleMask: [.borderless], backing: .buffered, defer: false)
    window.appearance = NSAppearance(named: .darkAqua)
    window.isReleasedWhenClosed = false
    window.contentView = hosting
    window.orderFront(nil)
    defer { window.close() }

    func resize(to height: CGFloat) {
        window.setContentSize(NSSize(width: sheet.width + 2 * margin, height: height + 2 * margin))
        hosting.frame = NSRect(origin: .zero, size: window.contentRect(forFrameRect: window.frame).size)
        hosting.layoutSubtreeIfNeeded()
    }

    switch sheet.height {
    case .fixed(let height):
        resize(to: height)
        settle()
    case .fitting:
        resize(to: 800)
        settle()
        resize(to: max(hosting.fittingSize.height - 2 * margin, 60))
        settle()
    case .reported(let state):
        // Lay out tall, let the content report the height it wants, then
        // render at that height, as the panel window would.
        resize(to: 1_600)
        settle(0.4)
        resize(to: state.idealContentHeight)
        settle()
    }

    guard let rep = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else { return nil }
    hosting.cacheDisplay(in: hosting.bounds, to: rep)
    return rep.representation(using: .png, properties: [:])
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

let written: [String] = try MainActor.assumeIsolated {
    var written: [String] = []
    for sheet in ClaudeControlSnapshotSheets.all() {
        let name = "\(sheet.name)-\(Int(sheet.width))"
        if let filter, !name.contains(filter) { continue }
        guard let data = render(sheet) else {
            FileHandle.standardError.write(Data("could not render \(name)\n".utf8))
            exit(1)
        }
        let file = outDir.appendingPathComponent("\(name).png")
        try data.write(to: file)
        written.append(file.path)
    }
    return written
}
print(written.joined(separator: "\n"))
