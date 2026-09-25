//
//  VibeNotchImport.swift
//  ClaudeControl
//
//  Superpowered Vibe Notch kept the same two files this engine keeps, in the
//  same format: the accounts (labels, colours, hidden folders) and the review
//  queue. The first live run copies them over where ours don't exist yet, so
//  switching apps loses neither. It happens once (`didImportVibeNotch`), and
//  only ever reads the other app's folder: nothing there is changed, and its
//  preferences are never read.
//
//  It runs the first time the engine's support folder is asked for (see
//  `AppIdentity.supportDirectory`), which is before anything loads either file.
//

import Foundation
import os.log

nonisolated enum VibeNotchImport {
    private static var logger: Logger {
        EngineLog.logger("Import")
    }

    /// Superpowered Vibe Notch's bundle id (for the running guard too).
    static let bundleIdentifier = "com.paraswtf.SuperpoweredVibeNotch"
    /// Its folder under ~/Library/Application Support.
    static let supportFolderName = "SuperpoweredVibeNotch"
    /// What is copied, when ours is missing.
    static let fileNames = ["accounts.json", "review-state.json"]

    static func sourceDirectory(home: String) -> URL {
        URL(fileURLWithPath: home, isDirectory: true)
            .appendingPathComponent("Library/Application Support", isDirectory: true)
            .appendingPathComponent(supportFolderName, isDirectory: true)
    }

    /// Copy the files once. Returns the names copied (empty when there was
    /// nothing to copy, or it already ran).
    @discardableResult
    static func runOnce(from source: URL, to destination: URL, settings: ClaudeControlSettings.Store) -> [String] {
        guard !settings.didImportVibeNotch else { return [] }
        let copied = copyMissing(from: source, to: destination)
        settings.didImportVibeNotch = true
        if !copied.isEmpty {
            logger.notice("Imported \(copied.joined(separator: ", "), privacy: .public) from Superpowered Vibe Notch")
        }
        return copied
    }

    /// Copy each of `fileNames` that exists in `source`, reads back as JSON
    /// in the shape we expect, and is missing from `destination`. Owner-only
    /// permissions on the copies. The source is only read.
    static func copyMissing(from source: URL, to destination: URL) -> [String] {
        let fm = FileManager.default
        var copied: [String] = []
        for name in fileNames {
            let target = destination.appendingPathComponent(name)
            guard !fm.fileExists(atPath: target.path),
                  let data = try? Data(contentsOf: source.appendingPathComponent(name)),
                  isImportable(data, name: name) else { continue }
            do {
                try fm.createDirectory(at: destination, withIntermediateDirectories: true,
                                       attributes: [.posixPermissions: 0o700])
                // Never replaces a file that appeared meanwhile (the folder is 0700).
                try data.write(to: target, options: .withoutOverwriting)
                try? fm.setAttributes([.posixPermissions: 0o600], ofItemAtPath: target.path)
                copied.append(name)
            } catch {
                logger.error("Couldn't import \(name, privacy: .public): \(error.localizedDescription, privacy: .public)")
            }
        }
        return copied
    }

    /// accounts.json must be a registry we can load; review-state.json a JSON object.
    static func isImportable(_ data: Data, name: String) -> Bool {
        switch name {
        case "accounts.json":
            return AccountRegistry.canLoad(data)
        default:
            return (try? JSONSerialization.jsonObject(with: data)) is [String: Any]
        }
    }
}
