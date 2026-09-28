import AppKit
import Foundation
import SwiftUI
import Testing
@testable import ClaudeControl

/// The settings pane's Cloud section: what it says in each state, and that
/// it builds in each.
@Suite(.serialized)
struct CloudSettingsTests {
    private let now = Date(timeIntervalSince1970: 1_790_000_000)

    // MARK: Website

    /// The website is shown, never edited: its host (a port or path kept),
    /// with `http://` left on so a development server reads as one.
    @Test func theWebsiteRowShowsTheHost() {
        #expect(CloudSettingsCopy.websiteDisplay("https://agentnotch.rivant.in") == "agentnotch.rivant.in")
        #expect(CloudSettingsCopy.websiteDisplay("https://agentnotch.example.com:8443/app") == "agentnotch.example.com:8443/app")
        #expect(CloudSettingsCopy.websiteDisplay("http://localhost:3000") == "http://localhost:3000")
        #expect(CloudSettingsCopy.noWebsite == "None")
        #expect(CloudSettingsCopy.websiteOverridden == "Set by AGENTNOTCH_WEB_URL for this run.")
    }

    // MARK: Signing in

    @Test func signingInNeedsAWebsiteAndSaysWhereTheSignInIsKept() {
        // A build with no website says so plainly; there is nothing to type.
        #expect(CloudSettingsCopy.signInDetail(ClaudeCloudState()) == "This build has no website, so it can't sign in or sync.")
        let set = CloudSettingsCopy.signInDetail(ClaudeCloudState(websiteURL: "https://agentnotch.example.com"))
        #expect(set.hasPrefix("Opens Google's sign-in in your browser."))
        #expect(set.contains(CloudSettingsCopy.fileNote))
        #expect(CloudSettingsCopy.fileNote.contains("only your user can read"))
        #expect(CloudSettingsCopy.signInDetail(ClaudeCloudState(websiteURL: "https://a.example", auth: .signingIn))
                == "Finish signing in in your browser.")
    }

    @Test func aFailedSignInSaysWhy() {
        #expect(CloudSettingsCopy.signInProblem(ClaudeCloudState()) == nil)
        #expect(CloudSettingsCopy.signInProblem(ClaudeCloudState(auth: .error("The website isn't answering."))) == "The website isn't answering.")
        #expect(CloudSettingsCopy.signInProblem(ClaudeCloudState(lastError: "Signed out of the website. Sign in again."))
                == "Signed out of the website. Sign in again.")
    }

    // MARK: What is sent

    @Test func syncSaysWhatIsSentAndWhatNeverIs() {
        for reads in [true, false] {
            let text = CloudSettingsCopy.syncDetail(readsDesktopUsage: reads)
            for word in ["project folder names", "session titles", "models", "times", "token counts", "cost",
                         "usage limits", "each signed-in Claude account", "Claude Desktop"] {
                #expect(text.contains(word), "\(word)")
            }
            #expect(text.contains("Never file paths, prompts or your Claude login."))
        }
        #expect(CloudSettingsCopy.syncDetail(readsDesktopUsage: false).contains("once reading its cache is on"))
    }

    @Test func summariesSayWhatTheyCostAndSend() {
        let text = CloudSettingsCopy.summariesDetail
        #expect(text.contains("Runs Claude Code on this Mac with the session's own account"))
        #expect(text.contains("uses that account's usage"))
        #expect(text.contains("a few cents per session with Haiku"))
        #expect(text.contains("one- or two-sentence summary"))
        // Which sessions, the limits, and what is removed (review findings 4, 19 and 25).
        #expect(text.hasPrefix("Only for sessions that end after you turn this on."))
        #expect(text.contains("at most $0.10 a run, 20 an hour and 60 a day"))
        #expect(text.contains("none while that account's 5-hour limit is 80% used"))
        #expect(text.contains("file paths cut to their last part and anything like a key or password removed"))
        // Plainly: what turning it off deletes, and what stays (fix check).
        #expect(text.hasSuffix("Turning this off deletes the summaries not sent yet; summaries already sent stay on the website."))

        var cloud = ClaudeCloudState(auth: .signedIn(email: nil), syncEnabled: false, summariesEnabled: true)
        #expect(CloudSettingsCopy.summariesNote(cloud) == "Needs sync.")
        cloud.syncEnabled = true
        #expect(CloudSettingsCopy.summariesNote(cloud) == "Not in this run: it can't start Claude Code.")
        cloud.summariesAvailable = true
        #expect(CloudSettingsCopy.summariesNote(cloud) == nil)
        cloud.summarizedSessions = 1
        #expect(CloudSettingsCopy.summariesNote(cloud) == "1 session summarised on this Mac.")
        cloud.summarizedSessions = 12
        #expect(CloudSettingsCopy.summariesNote(cloud) == "12 sessions summarised on this Mac.")
    }

    // MARK: Status

    @Test func theSyncRowSaysWhenItLastWorkedAndWhatWaits() {
        var cloud = ClaudeCloudState(auth: .signedIn(email: "me@example.com"))
        #expect(CloudSettingsCopy.status(cloud, now: now) == .init(title: "Sync is off", detail: "Nothing is sent while it is off."))
        cloud.syncEnabled = true
        #expect(CloudSettingsCopy.status(cloud, now: now) == .init(title: "Not synced yet", detail: nil))
        cloud.lastSyncAt = now.addingTimeInterval(-180)
        cloud.pendingSessions = 1
        cloud.pendingUsage = 4
        #expect(CloudSettingsCopy.status(cloud, now: now)
                == .init(title: "Last synced 3m ago", detail: "1 session and 4 usage readings to send."))
        cloud.isSyncing = true
        #expect(CloudSettingsCopy.status(cloud, now: now).title == "Syncing…")
        cloud.isSyncing = false
        cloud.lastError = "The website is busy. Trying again in a minute."
        #expect(CloudSettingsCopy.status(cloud, now: now)
                == .init(title: "Last sync failed", detail: "The website is busy. Trying again in a minute.", isProblem: true))
        #expect(CloudSettingsCopy.waiting(sessions: 0, readings: 0) == nil)
        #expect(CloudSettingsCopy.waiting(sessions: 3, readings: 0) == "3 sessions to send.")
        #expect(CloudSettingsCopy.waiting(sessions: 0, readings: 1) == "1 usage reading to send.")
    }

    // MARK: The website's settings

    /// Regression (M5): the section points to the website's settings page
    /// for removing summaries or deleting synced data, built from the
    /// dashboard's address (the page beside it), else the website's.
    @Test func theSectionPointsToTheWebsitesSettingsForRemovingData() throws {
        func state(website: String? = nil, dashboard: String? = nil) -> ClaudeCloudState {
            ClaudeCloudState(websiteURL: website, auth: .signedIn(email: nil), dashboardURL: dashboard.flatMap(URL.init(string:)))
        }
        #expect(state(dashboard: "https://a.example/dashboard").settingsURL?.absoluteString == "https://a.example/settings")
        #expect(state(dashboard: "https://a.example/app/dashboard/").settingsURL?.absoluteString
                == "https://a.example/app/settings")
        #expect(state(website: "https://a.example").settingsURL?.absoluteString == "https://a.example/settings")
        #expect(state(website: "http://localhost:3000", dashboard: "http://localhost:3000/home").settingsURL?.absoluteString
                == "http://localhost:3000/settings")
        #expect(state().settingsURL == nil)
        #expect(state(website: "ftp://a.example").settingsURL == nil)
        #expect(UIFixtures.cloudSignedIn().settingsURL?.absoluteString == "https://agentnotch.example.com/settings")

        let url = try #require(URL(string: "https://a.example/settings"))
        let text = CloudSettingsCopy.dataSettings(url: url)
        #expect(String(text.characters) == "Remove summaries or delete synced data in the website's Settings.")
        let linked = text.runs.compactMap { run in run.link.map { (String(text[run.range].characters), $0) } }
        #expect(linked.count == 1 && linked.first?.0 == "Settings" && linked.first?.1 == url)
    }

    // MARK: Building

    @Test func theSectionBuildsInEveryState() {
        _ = NSApplication.shared
        let states: [ClaudeCloudState] = [
            UIFixtures.cloudSignedOut(), UIFixtures.cloudNoWebsite(), UIFixtures.cloudOverridden(), UIFixtures.cloudSignedIn(),
            ClaudeCloudState(websiteURL: UIFixtures.cloudWebsite, auth: .signingIn),
            ClaudeCloudState(websiteURL: UIFixtures.cloudWebsite, auth: .error("The website isn't answering.")),
            ClaudeCloudState(auth: .error("This build has no website to sign in to.")),
            ClaudeCloudState(websiteURL: UIFixtures.cloudWebsite, websiteIsOverridden: true, auth: .signedIn(email: nil),
                             syncEnabled: true, summariesEnabled: true, isSyncing: true, lastError: "Busy"),
        ]
        for (index, cloud) in states.enumerated() {
            var model = UIFixtures.settings()
            model.cloud = cloud
            let view = SettingsPaneContent(model: model, actions: SettingsPaneActions())
                .claudeControlTheme(.codenotchDark)
            let hosting = NSHostingView(rootView: view)
            hosting.frame = NSRect(x: 0, y: 0, width: 520, height: 900)
            hosting.layoutSubtreeIfNeeded()
            #expect(hosting.fittingSize.width > 0, "state \(index)")
        }
    }

    /// The live pane's fixtures: signed in for the full pane, signed out
    /// (the build's website, nothing typed) on a first run.
    @Test func fixturesCoverTheStatesTheSnapshotsShow() {
        #expect(UIFixtures.settings().cloud.isSignedIn)
        #expect(UIFixtures.settings().cloud.syncEnabled && !UIFixtures.settings().cloud.summariesEnabled)
        #expect(UIFixtures.settingsFirstRun().cloud == UIFixtures.cloudSignedOut())
        #expect(UIFixtures.cloudSignedOut().websiteURL != nil && !UIFixtures.cloudSignedOut().isSignedIn)
        #expect(UIFixtures.cloudNoWebsite().websiteURL == nil && !UIFixtures.cloudNoWebsite().isSignedIn)
        #expect(UIFixtures.cloudOverridden().websiteIsOverridden && !UIFixtures.cloudOverridden().isSignedIn)
        #expect(SettingsPaneModel().cloud == ClaudeCloudState())
    }
}
