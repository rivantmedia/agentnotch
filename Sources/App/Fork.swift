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

    /// What upstream's copy calls the app.
    static let upstreamName = "Codenotch"

    /// Upstream's copy names the app "Codenotch" in some fifty strings and
    /// their twelve translations. Forking each one would mean re-forking it
    /// after every merge from upstream (and losing its translations until
    /// then), so `L10n.t` passes what it looks up through here (seam R1), and
    /// this app's own name goes in, in every language.
    ///
    /// - `key` is the catalog key: upstream's English with `%@`-style
    ///   placeholders. Copy about products that really are called Codenotch
    ///   (the phone app this one pairs with, upstream's Windows build and its
    ///   downloads) keeps the name.
    /// - `template` is the looked-up string before its arguments are filled
    ///   in. When it doesn't name Codenotch, an argument did (a project
    ///   folder, a device or account name): that is the user's data, and it
    ///   stays as it is. Only asked for when `text` names Codenotch.
    static func rebranded(_ text: String, locale: Locale, key: String, template: () -> String) -> String {
        guard namesUpstream(text), !namesUpstreamProduct(key) else { return text }
        let template = template()
        guard namesUpstream(template) else { return text }
        var result = text
        // A key without a translation falls back to upstream's English.
        if locale.language.languageCode == .english || template == key {
            // "an Agent Notch window", not "a Agent Notch window".
            result = Rebrand.englishArticle.stringByReplacingMatches(
                in: result, range: NSRange(result.startIndex..., in: result), withTemplate: "$1n \(Rebrand.nameTemplate)")
        } else if locale.language.languageCode == .french {
            // The name starts with a vowel, so "de" and "que" (lorsque,
            // puisque, …) elide: "Réglages d'Agent Notch", "tant qu'Agent Notch".
            result = Rebrand.frenchElision.stringByReplacingMatches(
                in: result, range: NSRange(result.startIndex..., in: result), withTemplate: "$1'\(Rebrand.nameTemplate)")
        }
        // Compounds join every word of the name: "Agent-Notch-Einstellungen".
        result = result.replacingOccurrences(
            of: upstreamName + "-", with: displayName.replacingOccurrences(of: " ", with: "-") + "-")
        return result.replacingOccurrences(of: upstreamName, with: displayName)
    }

    /// Whether `text` names upstream at all: the cheap check every lookup
    /// makes before anything else.
    static func namesUpstream(_ text: String) -> Bool {
        text.range(of: upstreamName, options: .literal) != nil
    }

    /// English copy naming a product other than this app that is called
    /// Codenotch. Phrases rather than keys, so a reworded string upstream
    /// still matches while it keeps naming the same product.
    static let upstreamProductPhrases = [
        "Codenotch app on your phone", "Codenotch on your phone", "Codenotch phone app",
        "Codenotch for Windows", "Codenotch-Setup", "hivinz.com",
    ]

    static func namesUpstreamProduct(_ key: String) -> Bool {
        upstreamProductPhrases.contains { key.range(of: $0, options: .literal) != nil }
    }

    /// Compiled once: `L10n.t` runs on the notch's repaint path.
    private enum Rebrand {
        static let nameTemplate = NSRegularExpression.escapedTemplate(for: displayName)
        static let englishArticle = try! NSRegularExpression(pattern: #"\b([aA]) \#(upstreamName)\b"#)
        static let frenchElision = try! NSRegularExpression(
            pattern: #"\b([dD]|[qQ]u|[lL]orsqu|[pP]uisqu|[jJ]usqu|[qQ]uoiqu)e \#(upstreamName)\b"#)
    }

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

    /// The fork's own Sparkle feed: the appcast the release workflow attaches
    /// to every GitHub release. `releases/latest/download` follows the newest
    /// published release, so the URL never changes. Never upstream's
    /// (hivinz.com): that feed would replace this app with the official
    /// Codenotch. `Updater` hands Sparkle this URL itself, which beats both the
    /// Info.plist and user defaults.
    static let updateFeedURL = "https://github.com/rivantmedia/agentnotch/releases/latest/download/appcast.xml"

    /// Where someone whose copy doesn't update itself gets a new one. The
    /// update copy names it; the app has no website address of its own.
    static let releasesPageURL = "https://github.com/rivantmedia/agentnotch/releases/latest"

    /// Whether this copy updates itself. Only the builds the release workflow
    /// makes (`Scripts/spm-build-app.sh --with-updates`) carry the feed and the
    /// EdDSA public key in their Info.plist; every other build strips them.
    /// A local build that updated itself would be swapped for the published
    /// release (Sparkle installs over whichever copy is running), and a
    /// sealed or `.dev` copy must never fetch or install anything. See `Updater`.
    static let updatesEnabled = updatesEnabled(
        info: Bundle.main.infoDictionary ?? [:], bundleID: Bundle.main.bundleIdentifier,
        sealed: isSealed, underTest: Runtime.isUnderTest)

    /// Pure, for tests. All of it has to hold:
    /// - not sealed, and not a test host;
    /// - exactly this app's bundle id, not a suffixed development copy;
    /// - the fork's own feed, as the release build writes it;
    /// - a public key Sparkle can use: with an invalid one it refuses to start
    ///   and puts up an alert on every launch, so such a copy stays off.
    static func updatesEnabled(info: [String: Any], bundleID: String?, sealed: Bool, underTest: Bool) -> Bool {
        guard !sealed, !underTest, bundleID == Fork.bundleID,
              info["SUFeedURL"] as? String == updateFeedURL,
              let key = info["SUPublicEDKey"] as? String else { return false }
        // Decoded the way Sparkle decodes it (SUSignatures.m): trimmed, then
        // strict base64 of a 32-byte Ed25519 public key.
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        return Data(base64Encoded: trimmed)?.count == 32
    }

    /// Upstream's What's New notes (`ReleaseNotes`) are keyed by upstream's
    /// versions, from 1.0.0 up, and this app numbers its own releases from
    /// 1.0.0 too (the VERSION file). Shown, they would introduce Agent Notch 1.0.0
    /// with upstream's "The first release." and 1.1.0 with upstream's 1.1.0
    /// changes. The fork's notes are on its releases page instead.
    static let showsUpstreamReleaseNotes = false

    /// The launch's What's New step (seam in AppDelegate): `showIfNeeded` is
    /// the dialogue's own check, asked only while upstream's notes are shown.
    /// Returns whether anything was put on screen, as `showIfNeeded` does, so
    /// the first launch's introduction still follows.
    @MainActor static func showWhatsNewIfNeeded(_ showIfNeeded: () -> Bool) -> Bool {
        showsUpstreamReleaseNotes && showIfNeeded()
    }

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
