import AppKit
import ClaudeControl
import SwiftUI

/// The panel half of `--snapshot-claude <dir>` (design §11): the sheet type
/// and the ImageRenderer path for the panel chrome sheets
/// (`ClaudeAppSnapshots+Panel`). The flag itself is read, sealed only and
/// before any notch exists, by WP-C's `ClaudeNotchSnapshots` in
/// `ClaudeBridge.attach`, which renders its notch sheets and then these.
///
/// Rendered on the solid surface: glass does not draw offscreen (see
/// `codenotchHeadlessGlass`).
///
/// Fork-only file. Owned by WP-D.
@MainActor
enum ClaudeAppSnapshots {
    /// One PNG: `<name>.png`, of `view` at its own size.
    struct Sheet {
        let name: String
        let view: AnyView

        init<V: View>(_ name: String, _ view: V) {
            self.name = name
            self.view = AnyView(view)
        }
    }

    struct RenderError: Error, CustomStringConvertible {
        let sheet: String
        var description: String { "could not render \(sheet)" }
    }

    /// Write each sheet as a 2x PNG. Returns the files written.
    static func render(_ sheets: [Sheet], into directory: URL) throws -> [URL] {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return try sheets.map { sheet in
            guard let data = png(sheet.view) else { throw RenderError(sheet: sheet.name) }
            let file = directory.appendingPathComponent(sheet.name + ".png")
            try data.write(to: file)
            return file
        }
    }

    /// Drawn by AppKit in a window that is never shown, as the notch sheets
    /// are (`ClaudeNotchSnapshots.renderImage`): `ImageRenderer` draws a
    /// placeholder for AppKit-backed controls such as the header's gear menu.
    static func png(_ view: AnyView) -> Data? {
        let content = view.environment(\.colorScheme, .dark)
        if let image = ClaudeNotchSnapshots.renderImage(content) {
            return NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])
        }
        let renderer = ImageRenderer(content: content)
        renderer.scale = 2
        guard let image = renderer.nsImage, let tiff = image.tiffRepresentation else { return nil }
        return NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
    }
}
