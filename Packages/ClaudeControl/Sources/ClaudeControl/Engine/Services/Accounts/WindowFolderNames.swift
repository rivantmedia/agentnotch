//
//  WindowFolderNames.swift
//  ClaudeControl
//
//  What a VS Code window's working copy is for, in words. Claude Parallel
//  Profiles names a window's folder `~/.claude-windows/<id>`, where `<id>`
//  is the first 12 hex digits of the SHA-1 of the window's workspace folder
//  path (or of its `.code-workspace` file's URI). A bare hash says nothing,
//  so the paths the app's sessions ran in (and the folders above them) are
//  hashed the same way: a match names the window after its project,
//  "VS Code · superpowered-vibe-notch". Nothing is read from disk. Pure.
//

import CryptoKit
import Foundation

nonisolated enum WindowFolderNames {
    /// The id Claude Parallel Profiles gives a window opened on `workspace`.
    static func windowId(forWorkspace workspace: String) -> String {
        Insecure.SHA1.hash(data: Data(workspace.utf8)).prefix(6).map { String(format: "%02x", $0) }.joined()
    }

    /// Window folder → the project it is a window of (the workspace
    /// folder's name), for every window folder one of `paths` (session
    /// working directories) is in or under.
    static func names(windowDirs: [String], paths: [String], home: String) -> [String: String] {
        let home = AccountPaths.normalize(home)
        var folderById: [String: String] = [:]
        for dir in windowDirs {
            folderById[(AccountPaths.normalize(dir) as NSString).lastPathComponent] = AccountPaths.normalize(dir)
        }
        guard !folderById.isEmpty else { return [:] }
        var names: [String: String] = [:]
        var tried = Set<String>()
        for path in paths {
            var candidate = AccountPaths.normalize(path)
            // The workspace is the folder itself or one above it (a session
            // may run in a subfolder), never home or anything above.
            while candidate.hasPrefix(home + "/") || (!candidate.hasPrefix(home) && candidate.count > 1) {
                guard tried.insert(candidate).inserted else { break }
                if let folder = folderById[windowId(forWorkspace: candidate)], names[folder] == nil {
                    names[folder] = (candidate as NSString).lastPathComponent
                    break
                }
                let parent = (candidate as NSString).deletingLastPathComponent
                if parent == candidate || parent.isEmpty { break }
                candidate = parent
            }
        }
        return names
    }

    /// "VS Code · superpowered-vibe-notch" for a named window folder, else
    /// the folder's path as given.
    static func label(_ folder: String, display: String, names: [String: String]) -> String {
        guard let name = names[AccountPaths.normalize(folder)] else { return display }
        return "VS Code · \(name)"
    }
}
