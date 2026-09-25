//
//  WindowDirWatcher.swift
//  ClaudeControl
//
//  Claude Parallel Profiles makes a working copy for every VS Code window
//  (`~/.claude-windows/<id>`) the moment a workspace is opened, and restocks
//  it with another account when the window switches. Claude Code reloads its
//  hooks when settings.json changes, so a new window can have ours within
//  seconds of appearing: this polls the windows folder every few seconds
//  (a listing and a few `stat`s, no reads) and asks the registry to discover
//  again when a window appears or goes, a window's `.claude.json` changes
//  (who is signed in there may have), or the manifest changes. The hook
//  manager installs into the new folder on its next pass (after consent).
//  While the extension is there it also watches `~/.claude.json`, which it
//  rewrites whenever another window is focused: who `~/.claude` runs as is
//  re-read within seconds, not at the next 20-second usage poll.
//

import Foundation

nonisolated enum WindowDirWatch {
    /// What is watched, as comparable facts: every window folder's name and
    /// its `.claude.json` modification time and size, and the manifest's.
    struct Fingerprint: Equatable, Sendable {
        var windows: [String: FileStamp] = [:]
        var manifest: FileStamp?
        /// `~/.claude.json`, while the manifest exists.
        var defaultConfig: FileStamp?
    }

    struct FileStamp: Equatable, Sendable {
        var modifiedAt: Date?
        var size: Int
        var exists: Bool
    }

    static let interval: TimeInterval = 5

    static func stamp(_ path: String, fileManager: FileManager = .default) -> FileStamp {
        guard let attributes = try? fileManager.attributesOfItem(atPath: path) else {
            return FileStamp(modifiedAt: nil, size: -1, exists: false)
        }
        return FileStamp(modifiedAt: attributes[.modificationDate] as? Date,
                         size: (attributes[.size] as? NSNumber)?.intValue ?? -1, exists: true)
    }

    /// The windows folder as it is now (read-only).
    static func fingerprint(home: String, fileManager: FileManager = .default) -> Fingerprint {
        let root = ParallelProfiles.windowsRoot(home: home)
        var fingerprint = Fingerprint()
        for name in (try? fileManager.contentsOfDirectory(atPath: root)) ?? [] where !name.hasPrefix(".") {
            let config = ((root as NSString).appendingPathComponent(name) as NSString).appendingPathComponent(".claude.json")
            fingerprint.windows[name] = stamp(config, fileManager: fileManager)
        }
        let manifest = stamp(ParallelProfiles.manifestPath(home: home), fileManager: fileManager)
        fingerprint.manifest = manifest.exists ? manifest : nil
        if manifest.exists {
            fingerprint.defaultConfig = stamp((AccountPaths.normalize(home) as NSString).appendingPathComponent(".claude.json"),
                                              fileManager: fileManager)
        }
        return fingerprint
    }

    /// What changed between two looks: a window appeared or went (or the
    /// manifest changed), which needs a discovery, or only who is signed in
    /// to an existing window may have, which needs identities re-read.
    enum Change: Equatable, Sendable {
        case none
        case identities
        case folders
    }

    static func change(from old: Fingerprint?, to new: Fingerprint) -> Change {
        guard let old else { return .none }
        if Set(old.windows.keys) != Set(new.windows.keys) || old.manifest != new.manifest { return .folders }
        for (name, stamp) in new.windows where old.windows[name] != stamp {
            // A window without a config yet that now has one is a new account folder.
            if old.windows[name]?.exists != stamp.exists { return .folders }
            return .identities
        }
        // The extension mirrored another account into ~/.claude (or Claude
        // Code wrote it): who ~/.claude runs as is read again.
        if old.defaultConfig != new.defaultConfig { return .identities }
        return .none
    }
}
