import Foundation
import Sparkle
import Testing
@testable import Codenotch

/// Agent Notch updates itself only as the release workflow built it: that
/// build's Info.plist carries the fork's feed and EdDSA key, and nothing else
/// may switch Sparkle on. A local build that updated itself would be swapped
/// for the published release; a sealed or `.dev` copy must never fetch or
/// install anything; and a feed other than the fork's would replace this app
/// with another app. Nothing here starts Sparkle or touches the network.
@MainActor
struct UpdatesTests {
    /// A valid key's shape (32 bytes, base64), not a real one.
    private static let key = Data(repeating: 7, count: 32).base64EncodedString()

    /// The Info.plist `Scripts/spm-build-app.sh --with-updates` writes.
    private static let release: [String: Any] = [
        "SUFeedURL": Fork.updateFeedURL,
        "SUPublicEDKey": key,
    ]

    /// `info` defaults to the release build's (nil: a default argument can't
    /// read the suite's main-actor fixture).
    private func enabled(_ info: [String: Any]? = nil, bundleID: String? = Fork.bundleID,
                         sealed: Bool = false, underTest: Bool = false) -> Bool {
        Fork.updatesEnabled(info: info ?? Self.release, bundleID: bundleID, sealed: sealed, underTest: underTest)
    }

    private func release(_ key: String, _ value: Any?) -> [String: Any] {
        var info = Self.release
        info[key] = value
        return info
    }

    @Test func onlyTheReleaseBuildUpdatesItself() {
        #expect(enabled())
        // Sparkle trims the key before decoding it, so a trailing newline is fine.
        #expect(enabled(release("SUPublicEDKey", Self.key + "\n")))
    }

    /// Sparkle refuses to start with a key it can't decode, and says so in an
    /// alert on every launch; without a key it would trust the code signature
    /// alone, and an ad-hoc signature vouches for nothing.
    @Test func aMissingOrUnusableKeyKeepsUpdatesOff() {
        #expect(!enabled(release("SUPublicEDKey", nil)))
        #expect(!enabled(release("SUPublicEDKey", "")))
        #expect(!enabled(release("SUPublicEDKey", "not base64!")))
        #expect(!enabled(release("SUPublicEDKey", Data(count: 31).base64EncodedString())))
        #expect(!enabled(release("SUPublicEDKey", Data(count: 33).base64EncodedString())))
        #expect(!enabled(release("SUPublicEDKey", Data(count: 64).base64EncodedString())))
        #expect(!enabled(release("SUPublicEDKey", Data(count: 32))))
    }

    /// Upstream's feed (or anyone else's) would replace this app with theirs.
    @Test func anyFeedButTheForksKeepsUpdatesOff() {
        let feeds = [
            "https://hivinz.com/appcast.xml",
            "https://github.com/vinzdg/codenotch/releases/latest/download/appcast.xml",
            "http://github.com/rivantmedia/agentnotch/releases/latest/download/appcast.xml",
            "https://github.com/someone/agentnotch/releases/latest/download/appcast.xml",
            "https://github.com/rivantmedia/agentnotch/releases/latest/download/appcast.xml/",
            "",
        ]
        for feed in feeds {
            #expect(!enabled(release("SUFeedURL", feed)), "\(feed)")
        }
        #expect(!enabled(release("SUFeedURL", nil)))
    }

    /// A suffixed copy (`.sealed`, `.dev`) is a development build that must
    /// never become the release; upstream's app is never this app's to update.
    @Test func anyOtherBundleKeepsUpdatesOff() {
        for id in ["com.rivantmedia.agentnotch.sealed", "com.rivantmedia.agentnotch.dev",
                   "com.vinz.codenotch", "com.rivantmedia.agentnotch2", ""] {
            #expect(!enabled(bundleID: id), "\(id)")
        }
        #expect(!enabled(bundleID: nil))
    }

    @Test func sealedRunsAndTestHostsNeverUpdate() {
        #expect(!enabled(sealed: true))
        #expect(!enabled(underTest: true))
    }

    /// The appcast the release workflow attaches to the newest release, and
    /// that release's page.
    @Test func theFeedAndTheReleasesPageAreTheForksOwn() throws {
        let feed = try #require(URL(string: Fork.updateFeedURL))
        #expect(feed.scheme == "https")
        #expect(feed.host == "github.com")
        #expect(feed.path == "/rivantmedia/agentnotch/releases/latest/download/appcast.xml")
        let page = try #require(URL(string: Fork.releasesPageURL))
        #expect(page.scheme == "https")
        #expect(page.host == "github.com")
        #expect(page.path == "/rivantmedia/agentnotch/releases/latest")
        for url in [Fork.updateFeedURL, Fork.releasesPageURL] {
            #expect(!url.contains("hivinz"))
            #expect(!url.contains("vinzdg"))
        }
    }

    /// The gate wants the Info.plist's feed to the character, and the release
    /// build writes it from a copy of its own: a release built with any other
    /// spelling would never switch updates on, and whoever installed it would
    /// stay on it for good.
    @Test func theReleaseBuildWritesThisFeed() throws {
        let script = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Scripts/spm-build-app.sh")
        let text = try String(contentsOf: script, encoding: .utf8)
        #expect(text.contains("\"\(Fork.updateFeedURL)\""), "Scripts/spm-build-app.sh no longer writes \(Fork.updateFeedURL)")
    }

    /// Sparkle asks the delegate for the feed before user defaults and the
    /// Info.plist. Asked through the protocol, so a method that stopped
    /// matching the requirement (a rename after a Sparkle update) fails here
    /// instead of quietly letting `defaults write … SUFeedURL` choose the feed.
    /// The updater is made, never started.
    @Test func sparkleIsHandedTheForksFeed() {
        let delegate: SPUUpdaterDelegate = Updater()
        let sparkle = SPUUpdater(
            hostBundle: .main, applicationBundle: .main,
            userDriver: SPUStandardUserDriver(hostBundle: .main, delegate: nil), delegate: nil)
        #expect(delegate.feedURLString?(for: sparkle) == Fork.updateFeedURL)
    }

    /// The test process has no feed in its Info.plist, so it behaves as a
    /// source build: Sparkle is never created, and the switch reads off.
    @Test func thisProcessDoesNotUpdateItself() throws {
        // Required, not expected: past this line the updater would really start.
        try #require(!Fork.updatesEnabled)
        let updater = Updater()
        updater.start()
        updater.automatic = true
        #expect(updater.automatic == false)
        #expect(updater.lastChecked == nil)
    }

    /// A copy that doesn't update itself says where a new one comes from:
    /// this app's releases, never upstream's download site.
    @Test func checkingNowInASourceBuildPointsAtTheReleasesPage() throws {
        try #require(!Fork.updatesEnabled)
        let updater = Updater()
        updater.checkNow()
        guard case .failed(let why) = updater.outcome else {
            Issue.record("a source build's check must say why nothing happened, got \(updater.outcome)")
            return
        }
        #expect(why.contains(Fork.releasesPageURL), "\(why)")
        #expect(!why.contains("hivinz"), "\(why)")
        #expect(!why.contains(Fork.upstreamName), "\(why)")
        #expect(updater.outcome.message == why)
    }

    @Test func aStalledCheckPointsAtTheReleasesPage() {
        guard case .failed(let why) = Updater.outcome(afterTimeoutFrom: .checking) else {
            Issue.record("a stalled check must not stay on Checking…")
            return
        }
        #expect(why.contains(Fork.releasesPageURL), "\(why)")
        #expect(!why.contains("hivinz"), "\(why)")
        #expect(!why.contains(Fork.upstreamName), "\(why)")
    }

    /// Upstream's notes are keyed by upstream's versions, and this app numbers
    /// its own from 1.0.0: its first release would open with upstream's "The
    /// first release." The dialogue's own check is never even asked.
    @Test func upstreamsWhatsNewNeverIntroducesThisApp() {
        #expect(ReleaseNotes.note(for: "1.0.0") != nil, "the collision this guards against")
        #expect(!Fork.showsUpstreamReleaseNotes)
        var asked = false
        #expect(!Fork.showWhatsNewIfNeeded { asked = true; return true })
        #expect(!asked)
    }
}
