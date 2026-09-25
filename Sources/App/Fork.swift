import ClaudeControl
import Foundation

/// Agent Notch's identity, kept in one file of its own so merges
/// from upstream Codenotch touch as little as possible.
///
/// This fork has to live beside the official Codenotch on the same Mac, so
/// nothing it owns may share a name with upstream's: not the bundle id (and
/// with it the defaults domain, notification and login-item registrations),
/// not the log subsystem, not the Application Support folder, not the keychain
/// items it writes for itself. The official app is `com.vinz.codenotch`.
enum Fork {
    /// Must match `PRODUCT_BUNDLE_IDENTIFIER` in project.yml and the bundle id
    /// Scripts/spm-build-app.sh writes. Development builds may append a
    /// suffix (`.sealed`, `.dev`); see `ownsBundleIdentifier`.
    static let bundleID = "com.rivantmedia.agentnotch"

    /// Upstream's bundle id: never ours to quit, update, or erase.
    static let upstreamBundleID = "com.vinz.codenotch"

    static let displayName = "Agent Notch"

    /// `log stream --predicate 'subsystem == "com.rivantmedia.agentnotch"' --level debug`
    static let logSubsystem = bundleID

    /// Folder under `~/Library/Application Support`. Upstream uses
    /// "Codenotch"; sharing it would mix the two apps' phone-link pairings,
    /// custom icons, and the scratch folder of the usage probe.
    static let applicationSupportFolder = "Agent Notch"

    /// Keychain service for phone-link device secrets. Upstream's is
    /// `com.codenotch.phonelink.device`; the pairing registry lives in
    /// `applicationSupportFolder`, so its secrets must be ours alone as well,
    /// or unpairing here would delete the official app's device keys.
    static let phoneLinkKeychainService = bundleID + ".phonelink.device"

    /// Keychain items this app writes itself, the keys typed into Settings,
    /// which upstream files under the same service names as the official
    /// Codenotch. Shared, either app would read, replace and delete the other's
    /// keys (switching a provider off here would sign the official app out).
    /// `KeychainItem` files them under the fork's own names instead, so the
    /// fork starts with none, as it starts with none of upstream's settings.
    /// Items of other apps, which are only ever read (Claude Code's, Cursor's,
    /// Antigravity's, …), keep their names.
    static let ownedKeychainServices: Set<String> = [
        "ollama-api-key",                        // OllamaCredentials
        "lmstudio-api-token",                    // LMStudioCredentials
        "minimax-api-key",                       // MiniMaxCredentials
        "minimax-session-cookie",                // MiniMaxCredentials
        "com.vinzdg.codenotch.custom-endpoint",  // CustomEndpoint
    ]

    /// The service an item is really filed under: the fork's own name for the
    /// ones in `ownedKeychainServices` (`<bundleID>.ollama-api-key`,
    /// `<bundleID>.custom-endpoint`, …), anything else unchanged.
    static func keychainService(_ service: String) -> String {
        guard ownedKeychainServices.contains(service) else { return service }
        let upstreamPrefix = "com.vinzdg.codenotch."
        let name = service.hasPrefix(upstreamPrefix) ? String(service.dropFirst(upstreamPrefix.count)) : service
        return bundleID + "." + name
    }

    /// Sparkle stays off in this fork. Upstream's feed and signing key would
    /// silently replace this build with the official Codenotch, and the fork
    /// has no feed of its own. See `Updater`.
    static let updatesEnabled = false

    /// Upstream copies `com.vinz.usagenotch` (its name before the rename)
    /// into a fresh defaults domain. A fork starts clean instead and never
    /// reads another app's preferences.
    static let migratesUpstreamPreferences = false

    /// Defaults the fork changes, before `Preferences` reads anything (U2 in
    /// AppDelegate). Registered, not written: a choice the user makes still
    /// wins, and nothing lands in the domain until they make one.
    /// - The weekly ring draws outside the session ring (concentric rings, as
    ///   in Superpowered Vibe Notch). Upstream's `Preferences` reads
    ///   `defaults.string(forKey: "weeklyRing") ?? .off`, so a registered
    ///   value is what it sees.
    @MainActor static func prepareDefaults() {
        if migratesUpstreamPreferences { Preferences.migrateFromPreviousName() }
        UserDefaults.standard.register(defaults: ["weeklyRing": WeeklyRing.outside.rawValue])
    }

    /// Whether `identifier` is this app (or a suffixed development copy of it).
    /// Anything that quits, erases or otherwise acts on "the other copy" checks
    /// this first, so a misconfigured build can never act on upstream's app.
    static func ownsBundleIdentifier(_ identifier: String?) -> Bool {
        guard let identifier else { return false }
        return identifier == bundleID || identifier.hasPrefix(bundleID + ".")
    }

    /// Sealed development mode: fixture data, and nothing real is touched
    /// (keychain, sessions, transcripts, network, subprocesses). On with
    /// `AGENTNOTCH_SAFE_MODE=1` or `CODENOTCH_DEMO=1`. See `SealedMode`.
    static var isSealed: Bool { SealedMode.isOn }
}
