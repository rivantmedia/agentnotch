//
//  ClaudeBinaryLocator.swift
//  ClaudeControl
//
//  Finds the `claude` executable and reads its version. Shared by the hook
//  installer (which hook events every installed Claude Code understands) and
//  the usage probe (which runs `claude -p` to ask for plan usage).
//
//  A GUI app inherits a minimal PATH, so `claude` is looked up in this order:
//  the binary the user chose in Settings, the path remembered from an earlier
//  launch, well-known install locations, and finally the user's own login
//  shell (`command -v claude`: non-interactive first, interactive only if that
//  finds nothing; its own process group, killed whole on timeout; a cooldown
//  after failure). Safe to call from any thread; everything here may block
//  briefly, so keep it off the main actor.
//

import Foundation
import os.log

/// Simple semantic version used to gate which hook events we register.
/// Claude Code rejects unknown hook keys, so we must only register events the
/// installed version knows about.
nonisolated struct ClaudeCodeVersion: Comparable, Hashable, Sendable, CustomStringConvertible {
    let major: Int
    let minor: Int
    let patch: Int

    init(major: Int, minor: Int, patch: Int) {
        self.major = major
        self.minor = minor
        self.patch = patch
    }

    var description: String { "\(major).\(minor).\(patch)" }

    static func < (lhs: ClaudeCodeVersion, rhs: ClaudeCodeVersion) -> Bool {
        (lhs.major, lhs.minor, lhs.patch) < (rhs.major, rhs.minor, rhs.patch)
    }

    /// Extracts the first `X.Y.Z` token from arbitrary version output.
    /// Accepts any prefix/suffix — works for "2.1.88", "v2.1.88", "2.1.88 (Claude Code)".
    static func parse(_ text: String) -> ClaudeCodeVersion? {
        let pattern = #"(\d+)\.(\d+)\.(\d+)"#
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return nil }
        let range = NSRange(text.startIndex..., in: text)
        guard let match = regex.firstMatch(in: text, range: range),
              match.numberOfRanges == 4,
              let majorRange = Range(match.range(at: 1), in: text),
              let minorRange = Range(match.range(at: 2), in: text),
              let patchRange = Range(match.range(at: 3), in: text),
              let major = Int(text[majorRange]),
              let minor = Int(text[minorRange]),
              let patch = Int(text[patchRange])
        else { return nil }
        return ClaudeCodeVersion(major: major, minor: minor, patch: patch)
    }
}

nonisolated enum ClaudeBinaryLocator {
    private static var logger: Logger { EngineLog.logger("ClaudeBinary") }

    private static let resolvedBinaryKey = ClaudeControlSettings.Key.prefix + "cache.claudeBinaryPath"
    private static let failedProbeKey = ClaudeControlSettings.Key.prefix + "cache.claudeBinaryProbeFailedAt"

    private static var defaults: UserDefaults { AppIdentity.defaults }

    /// How long to leave the login shell alone after it fails to find `claude`.
    /// Probing costs a shell startup, and the answer does not usually change
    /// between two launches a minute apart.
    private static let failedProbeCooldown: TimeInterval = 24 * 60 * 60

    /// How long one login shell may take.
    static let shellTimeout: TimeInterval = 5

    /// Serializes resolution so two callers (installer and usage probe) never
    /// both pay for a login shell at the same moment.
    private static let resolveLock = NSLock()

    /// `claude --version` results keyed by the binary's resolved (symlink-free)
    /// path. The native installer swaps a symlink on update, so a new version
    /// gets a new key and is detected afresh.
    nonisolated(unsafe) private static var versionCache: [String: ClaudeCodeVersion] = [:]
    private static let versionLock = NSLock()

    // MARK: - Locating

    /// The binary the user chose in Settings, when it is still executable.
    static var chosenPath: String? {
        guard let chosen = ClaudeControlSettings.claudeBinaryPath,
              FileManager.default.isExecutableFile(atPath: chosen) else { return nil }
        return chosen
    }

    /// Where we last found the `claude` binary, without probing for it. Nil
    /// when nothing has been resolved, or when the remembered path has since
    /// stopped being executable.
    static var rememberedPath: String? {
        if let chosenPath { return chosenPath }
        guard let remembered = defaults.string(forKey: resolvedBinaryKey),
              FileManager.default.isExecutableFile(atPath: remembered) else {
            return nil
        }
        return remembered
    }

    /// Drop everything we remember about where `claude` lives, so the next
    /// resolution starts from scratch (including a fresh login-shell probe).
    /// The Hooks toggle calls this, which makes flipping it off and on a
    /// genuine retry after installing Claude Code somewhere new.
    static func forget() {
        defaults.removeObject(forKey: resolvedBinaryKey)
        defaults.removeObject(forKey: failedProbeKey)
    }

    /// Well-known install locations, cheap to stat and checked before we pay for
    /// a shell. Deliberately does not cover version managers — nvm, fnm, volta,
    /// mise and bun globals put `claude` on a path only the user's shell knows.
    ///
    /// - Parameter configDirs: account config dirs; each may hold an old-style
    ///   local install at `<configDir>/local/claude`.
    static func fixedCandidates(home: String = AccountPaths.homeDirectory, configDirs: [String] = []) -> [String] {
        var candidates = [
            // The native installer's launcher (auto-updating symlink).
            home + "/.local/bin/claude",
            "/opt/homebrew/bin/claude",
            "/usr/local/bin/claude",
        ]
        let localInstallDirs = [home + "/.claude"] + configDirs
        for dir in localInstallDirs {
            let path = AccountPaths.normalize(dir) + "/local/claude"
            if !candidates.contains(path) {
                candidates.append(path)
            }
        }
        candidates += [
            home + "/.bun/bin/claude",
            "/opt/local/bin/claude",
            "/usr/bin/claude",
        ]
        return candidates
    }

    /// Find the `claude` executable: the user's choice, the remembered path,
    /// then well-known locations, then the user's own login shell. Blocks for
    /// up to a few seconds on the shell path; never call it on the main actor.
    static func resolve(configDirs: [String] = []) -> String? {
        resolveLock.lock()
        defer { resolveLock.unlock() }

        let fm = FileManager.default

        // A path chosen by the user, or resolved on an earlier launch. Costs
        // one stat, and saves the shell round-trip for everyone whose install
        // has not moved.
        if let remembered = rememberedPath {
            return remembered
        }

        if let known = fixedCandidates(configDirs: configDirs).first(where: { fm.isExecutableFile(atPath: $0) }) {
            defaults.set(known, forKey: resolvedBinaryKey)
            return known
        }

        defaults.removeObject(forKey: resolvedBinaryKey)

        // Nothing in the usual places. The remaining installs — an npm global
        // under a version manager, most commonly — are only on the PATH that the
        // user's shell builds, and a GUI app inherits none of it. Ask the shell.
        guard let resolved = resolveViaLoginShellIfAllowed() else {
            return nil
        }

        defaults.set(resolved, forKey: resolvedBinaryKey)
        return resolved
    }

    /// The login-shell probe behind its cooldown. Someone's login shell can be
    /// slow or hang outright, so one failure buys quiet until the cooldown
    /// expires rather than costing every launch the same stall.
    static func resolveViaLoginShellIfAllowed() -> String? {
        // Before bootstrap (tests, the snapshots tool) nobody's login shell
        // is started: it runs their rc files (S6).
        guard AppIdentity.isFrozen else {
            logger.notice("Not asking the login shell for claude: the engine isn't bootstrapped")
            return nil
        }
        if let lastFailure = defaults.object(forKey: failedProbeKey) as? Date,
           Date().timeIntervalSince(lastFailure) < failedProbeCooldown {
            logger.debug("Skipping shell probe — the last one failed recently")
            return nil
        }

        guard let resolved = resolveViaLoginShell() else {
            defaults.set(Date(), forKey: failedProbeKey)
            return nil
        }

        defaults.removeObject(forKey: failedProbeKey)
        return resolved
    }

    /// Ask the user's login shell where `claude` is: `-l -c` first, which
    /// reads only the login files, then `-i -l -c` (zsh reads nvm/fnm/mise
    /// setup from .zshrc, which only an interactive shell runs) if that
    /// found nothing. Each runs in its own process group with stdin from
    /// /dev/null, and on timeout the whole group is killed, so nothing an rc
    /// file started outlives it.
    ///
    /// Takes the shell as a parameter so it can be exercised against a stub —
    /// launching someone else's login shell is the riskiest thing in this
    /// file. Tests pass a path rather than setting `SHELL`, which is
    /// process-global and would not survive parallel execution.
    static func resolveViaLoginShell(
        shell: String = Foundation.ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh",
        timeout: TimeInterval = shellTimeout
    ) -> String? {
        guard FileManager.default.isExecutableFile(atPath: shell) else { return nil }

        for flags in [["-l", "-c"], ["-i", "-l", "-c"]] {
            // Interactive shells can print banners and prompts, hence the
            // defensive parse below.
            guard let result = ProcessRunner.run(
                executable: shell,
                arguments: flags + ["command -v claude"],
                timeout: timeout
            ), result.exitCode == 0,
                let path = parseShellResolvedPath(from: result.stdout) else { continue }
            logger.info("Resolved claude via \(shell, privacy: .public) \(flags.joined(separator: " "), privacy: .public): \(path, privacy: .public)")
            return path
        }
        return nil
    }

    /// Pull an executable path out of `command -v` output.
    ///
    /// An interactive shell may emit banners, prompt escapes or motd noise
    /// alongside the answer, and `command -v` returns a bare name for a shell
    /// function or alias. So: take the last line that is an absolute path we can
    /// actually execute, and ignore everything else.
    static func parseShellResolvedPath(
        from output: String,
        isExecutable: (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) -> String? {
        output
            .components(separatedBy: .newlines)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .last { $0.hasPrefix("/") && isExecutable($0) }
    }

    // MARK: - Environment

    /// `base` with PATH extended so `claude` can find its interpreter. An npm
    /// install is a `#!/usr/bin/env node` script, and under nvm/fnm/volta
    /// `node` sits next to `claude` — on a PATH a GUI app never inherits.
    static func environment(
        forBinaryAt path: String,
        base: [String: String] = Foundation.ProcessInfo.processInfo.environment
    ) -> [String: String] {
        var directories: [String] = []
        func add(_ directory: String) {
            if !directory.isEmpty && !directories.contains(directory) {
                directories.append(directory)
            }
        }
        add((path as NSString).deletingLastPathComponent)
        add((URL(fileURLWithPath: path).resolvingSymlinksInPath().path as NSString).deletingLastPathComponent)
        for directory in (base["PATH"] ?? "").split(separator: ":").map(String.init) {
            add(directory)
        }
        for directory in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            add(directory)
        }
        var environment = base
        environment["PATH"] = directories.joined(separator: ":")
        return environment
    }

    // MARK: - Before bootstrap

    /// Tests and the snapshots tool run the engine without `bootstrap`, with
    /// the real home. Then only a stub under the temporary directory may be
    /// run as `claude`, never the user's real one (S6).
    static func mayRunBeforeBootstrap(resolvedPath: String) -> Bool {
        if AppIdentity.isFrozen { return true }
        let temporary = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true).resolvingSymlinksInPath().path
        let path = URL(fileURLWithPath: resolvedPath).resolvingSymlinksInPath().path
        return path.hasPrefix(temporary.hasSuffix("/") ? temporary : temporary + "/")
    }

    // MARK: - Version

    /// Version of the `claude` binary at `path`, from `claude --version`.
    /// Cached per resolved binary, so repeated installs cost one stat.
    /// Returns nil on any failure — callers fall back to the baseline hook set,
    /// the safe direction to fail.
    static func version(ofBinaryAt path: String, timeout: TimeInterval = 10) -> ClaudeCodeVersion? {
        let key = URL(fileURLWithPath: path).resolvingSymlinksInPath().path
        guard mayRunBeforeBootstrap(resolvedPath: key) else {
            logger.notice("Not running \(path, privacy: .public) --version: the engine isn't bootstrapped")
            return nil
        }

        versionLock.lock()
        let cached = versionCache[key]
        versionLock.unlock()
        if let cached { return cached }

        guard let result = ProcessRunner.run(
            executable: path,
            arguments: ["--version"],
            environment: environment(forBinaryAt: path),
            timeout: timeout
        ),
              result.exitCode == 0,
              let version = ClaudeCodeVersion.parse(result.stdout) else {
            logger.info("Could not read the Claude Code version from \(path, privacy: .public)")
            return nil
        }

        logger.info("Detected Claude Code \(version.description, privacy: .public) at \(path, privacy: .public)")
        versionLock.lock()
        versionCache[key] = version
        versionLock.unlock()
        return version
    }

    /// The version of every `claude` found: the resolved one (the user's
    /// choice, or what their shell runs) and every well-known location that
    /// holds one. An older second install (an npm global left under nvm)
    /// may be the one a terminal runs, so the installer gates on the lowest.
    static func detectVersions(configDirs: [String] = []) -> [ClaudeCodeVersion] {
        let fm = FileManager.default
        var paths: [String] = []
        if let resolved = resolve(configDirs: configDirs) { paths.append(resolved) }
        for candidate in fixedCandidates(configDirs: configDirs) where fm.isExecutableFile(atPath: candidate) {
            paths.append(candidate)
        }
        var seen = Set<String>()
        return paths.compactMap { path in
            let key = URL(fileURLWithPath: path).resolvingSymlinksInPath().path
            guard seen.insert(key).inserted else { return nil }
            return version(ofBinaryAt: path)
        }
    }
}
