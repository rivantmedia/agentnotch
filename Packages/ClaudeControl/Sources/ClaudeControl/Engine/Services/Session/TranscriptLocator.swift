//
//  TranscriptLocator.swift
//  ClaudeIsland
//
//  Finds session and subagent transcript files. The hook's `transcript_path`
//  is authoritative; everything here is a fallback for sessions discovered
//  without one (session registry, status line, old hook scripts).
//
//  Claude Code stores a session at `<configDir>/projects/<slug>/<sessionId>.jsonl`
//  where slug = cwd with EVERY character outside [a-zA-Z0-9] replaced by "-"
//  (per UTF-16 unit, as JavaScript's replace does), truncated and suffixed with
//  a hash above 200 characters. Upstream only replaced "/" and ".", which breaks
//  paths containing "@", spaces, "_" and so on.
//

import Foundation

nonisolated enum TranscriptLocator {
    /// Claude Code's project folder name for a working directory (untruncated
    /// part only for very long paths; see `transcriptPath`).
    static func projectSlug(for cwd: String) -> String {
        var units: [UInt16] = []
        units.reserveCapacity(cwd.utf16.count)
        let dash = UInt16(UInt8(ascii: "-"))
        for unit in cwd.utf16 {
            let isAlphanumeric = (unit >= 0x30 && unit <= 0x39)  // 0-9
                || (unit >= 0x41 && unit <= 0x5A)  // A-Z
                || (unit >= 0x61 && unit <= 0x7A)  // a-z
            units.append(isAlphanumeric ? unit : dash)
        }
        return String(decoding: units, as: UTF16.self)
    }

    /// `<configDir>/projects/<slug>/<sessionId>.jsonl` as Claude Code would name
    /// it, without checking that it exists. Long slugs can't be reproduced
    /// exactly (the hash suffix), so callers should prefer `transcriptPath`.
    static func expectedTranscriptPath(sessionId: String, cwd: String, configDir: String) -> String {
        let projects = (AccountPaths.normalize(configDir) as NSString).appendingPathComponent("projects")
        let slug = projectSlug(for: cwd)
        return (projects as NSString).appendingPathComponent("\(slug)/\(sessionId).jsonl")
    }

    /// Locates an existing transcript: the hook-provided path if it exists, else
    /// the computed slug path, else a search of `<configDir>/projects/*/<sessionId>.jsonl`.
    static func transcriptPath(
        sessionId: String,
        cwd: String?,
        configDir: String,
        hint: String? = nil,
        fileManager: FileManager = .default
    ) -> String? {
        if let hint, !hint.isEmpty, fileManager.fileExists(atPath: hint) {
            return hint
        }
        if let cwd, !cwd.isEmpty {
            let expected = expectedTranscriptPath(sessionId: sessionId, cwd: cwd, configDir: configDir)
            if fileManager.fileExists(atPath: expected) {
                return expected
            }
        }
        return searchTranscript(sessionId: sessionId, configDir: configDir, fileManager: fileManager)
    }

    /// The file a path names, every symbolic link resolved: two config
    /// folders that share their history (Claude Parallel Profiles links
    /// `projects/` to `~/.claude-shared`) name one transcript two ways.
    static func realPath(_ path: String) -> String {
        URL(fileURLWithPath: AccountPaths.normalize(path)).resolvingSymlinksInPath().path
    }

    /// Whether two paths name the same file (spelled alike, or through links).
    static func isSameFile(_ lhs: String, _ rhs: String) -> Bool {
        AccountPaths.normalize(lhs) == AccountPaths.normalize(rhs) || realPath(lhs) == realPath(rhs)
    }

    /// Locates a transcript in any of `configDirs` (the session's own first),
    /// reading each physical `projects` folder once however many config
    /// folders link to it. Returns the path as spelled through the first
    /// config folder that has it, never the link target: the config folder
    /// is read back from it.
    static func transcriptPath(
        sessionId: String,
        cwd: String?,
        configDirs: [String],
        hint: String? = nil,
        fileManager: FileManager = .default
    ) -> String? {
        var seenProjects = Set<String>()
        for configDir in configDirs {
            let projects = realPath((AccountPaths.normalize(configDir) as NSString).appendingPathComponent("projects"))
            guard seenProjects.insert(projects).inserted else { continue }
            if let found = transcriptPath(sessionId: sessionId, cwd: cwd, configDir: configDir,
                                          hint: seenProjects.count == 1 ? hint : nil, fileManager: fileManager) {
                return found
            }
        }
        return nil
    }

    /// Scans every project folder of the account for `<sessionId>.jsonl`.
    static func searchTranscript(sessionId: String, configDir: String, fileManager: FileManager = .default) -> String? {
        guard isSafeFileName(sessionId) else { return nil }
        let projects = (AccountPaths.normalize(configDir) as NSString).appendingPathComponent("projects")
        guard let folders = try? fileManager.contentsOfDirectory(atPath: projects) else { return nil }
        let fileName = "\(sessionId).jsonl"
        for folder in folders {
            let candidate = (projects as NSString).appendingPathComponent("\(folder)/\(fileName)")
            if fileManager.fileExists(atPath: candidate) {
                return candidate
            }
        }
        return nil
    }

    /// Transcript of a subagent of the session whose main transcript is `transcriptPath`.
    ///
    /// Current Claude Code nests them under the session:
    ///   `<project>/<sessionId>/subagents/agent-<agentId>.jsonl`
    /// workflow agents one level deeper (`subagents/workflows/<id>/agent-<agentId>.jsonl`),
    /// and older versions stored them flat: `<project>/agent-<agentId>.jsonl`.
    /// Returns the nested path when nothing exists yet (the file may still be created).
    static func subagentTranscriptPath(transcriptPath: String, agentId: String, fileManager: FileManager = .default) -> String {
        let projectDir = (transcriptPath as NSString).deletingLastPathComponent
        let sessionId = ((transcriptPath as NSString).lastPathComponent as NSString).deletingPathExtension
        let fileName = "agent-\(agentId).jsonl"
        let subagentsDir = (projectDir as NSString).appendingPathComponent("\(sessionId)/subagents")
        let nested = (subagentsDir as NSString).appendingPathComponent(fileName)
        if fileManager.fileExists(atPath: nested) { return nested }

        let flat = (projectDir as NSString).appendingPathComponent(fileName)
        if fileManager.fileExists(atPath: flat) { return flat }

        if isSafeFileName(agentId), let enumerator = fileManager.enumerator(atPath: subagentsDir) {
            while let relative = enumerator.nextObject() as? String {
                if (relative as NSString).lastPathComponent == fileName {
                    return (subagentsDir as NSString).appendingPathComponent(relative)
                }
            }
        }
        return nested
    }

    /// Session and agent ids come from other processes; never let one escape a folder.
    private static func isSafeFileName(_ name: String) -> Bool {
        !name.isEmpty && !name.contains("/") && name != "." && name != ".."
    }
}
