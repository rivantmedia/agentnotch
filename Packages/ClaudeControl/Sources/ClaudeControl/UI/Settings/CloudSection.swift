//
//  CloudSection.swift
//  ClaudeControl
//
//  The settings pane's "Cloud" section: the website sync goes to, signing
//  in to it with Google, what sync sends and never sends, session summaries
//  and what they cost, the last sync, and the way to the dashboard, to
//  sharing accounts there and to its settings (removing summaries, deleting
//  synced data). Nothing is uploaded until the user signs in and
//  turns sync on; the switches do nothing else.
//
//  Signed out: the website field, then "Sign in with Google" (disabled
//  until a website is saved; the field is locked while a sign-in runs, which
//  is bound to the website it started with). Signed in: the website (with
//  "Change…"), who is signed in, the two switches (off after every sign-in,
//  sign-out and website change), the last sync, sharing and signing out.
//

import SwiftUI

struct CloudSection: View {
    let cloud: ClaudeCloudState
    /// "Also read Claude Desktop's cached usage" (Usage) is on: its readings
    /// are sent too.
    let readsDesktopUsage: Bool
    let now: Date
    let actions: SettingsPaneActions

    @State private var draft: String
    @State private var isChangingWebsite: Bool
    /// Why the typed address wasn't saved.
    @State private var websiteProblem: String?

    /// - Parameter changingWebsite: Snapshots: open the field of a
    ///   signed-in pane.
    init(cloud: ClaudeCloudState, readsDesktopUsage: Bool, now: Date, actions: SettingsPaneActions,
         changingWebsite: Bool = false) {
        self.cloud = cloud
        self.readsDesktopUsage = readsDesktopUsage
        self.now = now
        self.actions = actions
        _draft = State(initialValue: cloud.websiteURL ?? "")
        _isChangingWebsite = State(initialValue: changingWebsite)
    }

    var body: some View {
        Section("Cloud") {
            websiteRow
            if cloud.isSignedIn {
                signedInRows
            } else {
                signInRow
            }
        }
        // Saved (and tidied: "example.com" becomes "https://example.com"),
        // or changed by the engine: the field shows what is kept.
        .onChange(of: cloud.websiteURL) { _, saved in
            draft = saved ?? ""
            websiteProblem = nil
            isChangingWebsite = false
        }
    }

    // MARK: Website

    @ViewBuilder
    private var websiteRow: some View {
        if cloud.websiteIsOverridden {
            LabeledContent {
                websiteText
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Website")
                    Caption(CloudSettingsCopy.websiteOverridden)
                }
            }
        } else if cloud.isSignedIn && !isChangingWebsite {
            LabeledContent {
                HStack(spacing: 8) {
                    websiteText
                    Button("Change…") { isChangingWebsite = true }
                }
            } label: {
                Text("Website")
            }
        } else {
            VStack(alignment: .leading, spacing: 6) {
                Text("Website")
                HStack(spacing: 8) {
                    TextField("Website", text: $draft, prompt: Text(CloudSettingsCopy.websitePlaceholder))
                        .labelsHidden()
                        .textFieldStyle(.roundedBorder)
                        .autocorrectionDisabled()
                        .onSubmit(saveWebsite)
                    if isChangingWebsite {
                        Button("Cancel") {
                            draft = cloud.websiteURL ?? ""
                            websiteProblem = nil
                            isChangingWebsite = false
                        }
                    }
                    Button("Save", action: saveWebsite)
                        .disabled(!canSave)
                }
                // A sign-in in progress belongs to the website it started with.
                .disabled(isSigningIn)
                if let websiteProblem {
                    Caption(websiteProblem, ink: .critical)
                } else {
                    Caption(isChangingWebsite ? CloudSettingsCopy.changeSignsOut : CloudSettingsCopy.websiteHelp)
                }
            }
        }
    }

    private var websiteText: some View {
        Text(cloud.websiteURL ?? "")
            .foregroundStyle(.ink(.secondary))
            .lineLimit(1)
            .truncationMode(.middle)
            .textSelection(.enabled)
    }

    private var trimmedDraft: String { draft.trimmingCharacters(in: .whitespacesAndNewlines) }

    private var isSigningIn: Bool { cloud.auth == .signingIn }

    private var canSave: Bool { !isSigningIn && trimmedDraft != (cloud.websiteURL ?? "") }

    private func saveWebsite() {
        let text = trimmedDraft
        guard canSave else { return }
        if !text.isEmpty, CloudSettingsCopy.normalizedWebsite(text) == nil {
            websiteProblem = CloudSettingsCopy.websiteInvalid
            return
        }
        websiteProblem = nil
        actions.saveCloudWebsite(text)
    }

    // MARK: Signed out

    private var signInRow: some View {
        let signingIn = cloud.auth == .signingIn
        return HStack(alignment: .center, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(signingIn ? "Signing in…" : "Not signed in")
                Caption(CloudSettingsCopy.signInDetail(cloud))
                if let problem = CloudSettingsCopy.signInProblem(cloud) {
                    Caption(problem, ink: .critical)
                }
            }
            Spacer(minLength: 8)
            if signingIn {
                ProgressView().controlSize(.small)
            }
            Button("Sign in with Google", action: actions.cloudSignIn)
                .disabled(cloud.websiteURL == nil || signingIn)
        }
    }

    // MARK: Signed in

    @ViewBuilder
    private var signedInRows: some View {
        ConfirmingButtonRow(
            title: cloud.email.map { "Signed in as \($0)" } ?? "Signed in",
            detail: CloudSettingsCopy.fileNote,
            action: "Sign out…",
            confirm: "Sign out",
            perform: actions.cloudSignOut
        )

        Toggle(isOn: binding(cloud.syncEnabled, actions.setCloudSync)) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Sync sessions and usage")
                Caption(CloudSettingsCopy.syncDetail(readsDesktopUsage: readsDesktopUsage))
            }
        }

        // Summaries ride on sync: with sync off nothing is sent, so none is written.
        Toggle(isOn: binding(cloud.syncEnabled && cloud.summariesEnabled, actions.setSessionSummaries)) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Summarise finished sessions with Claude")
                Caption(CloudSettingsCopy.summariesDetail)
                if let note = CloudSettingsCopy.summariesNote(cloud) {
                    Caption(note, ink: cloud.summariesEnabled && !cloud.summariesAvailable ? .needsYou : .secondary)
                }
            }
        }
        .disabled(!cloud.syncEnabled)

        let status = CloudSettingsCopy.status(cloud, now: now)
        HStack(alignment: .center, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(status.title)
                if let detail = status.detail {
                    Caption(detail, ink: status.isProblem ? .critical : .secondary)
                }
            }
            Spacer(minLength: 8)
            if cloud.isSyncing {
                ProgressView().controlSize(.small)
            }
            Button("Sync now", action: actions.syncCloudNow)
                .disabled(!cloud.syncEnabled || cloud.isSyncing)
        }

        HStack(alignment: .center, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Dashboard")
                Caption(CloudSettingsCopy.dashboard)
                if let settings = cloud.settingsURL {
                    // Its link goes through the pane's action (which a
                    // sealed run turns down), not straight to the browser.
                    Text(CloudSettingsCopy.dataSettings(url: settings))
                        .font(.caption)
                        .foregroundStyle(.ink(.secondary))
                        .fixedSize(horizontal: false, vertical: true)
                        .environment(\.openURL, OpenURLAction { _ in
                            actions.openCloudSettings()
                            return .handled
                        })
                }
            }
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 6) {
                Button("Open dashboard", action: actions.openCloudDashboard)
                Button("Share accounts…", action: actions.openCloudPools)
            }
            // The website says where its dashboard is once signed in.
            .disabled(cloud.dashboardURL == nil)
        }
    }

    private func binding<Value>(_ value: Value, _ set: @escaping (Value) -> Void) -> Binding<Value> {
        Binding(get: { value }, set: set)
    }
}

// MARK: - Copy

/// What the Cloud section says. Pure, so the wording is tested.
nonisolated enum CloudSettingsCopy {
    static let websitePlaceholder = "https://your-website.example"
    static let websiteHelp = "Where sync goes: an https:// address, or http://localhost for development."
    static let websiteInvalid = "Use an https:// address, or http://localhost for development."
    static let websiteOverridden = "Set by AGENTNOTCH_WEB_URL for this run."
    static let changeSignsOut = "Saving another website signs you out of this one."
    /// Said once: the app's own sign-in to its website, not Claude's.
    static let fileNote = "The website sign-in is kept in a file only your user can read."
    static let dashboard = "Your sessions and usage from every Mac and account you sync. Share an account there with a code: everyone in its pool sees what it was used for."
    /// Under the dashboard: where what was sent is taken off the website.
    static let dataSettingsText = "Remove summaries or delete synced data in the website's Settings."
    static let dataSettingsLink = "Settings"

    /// `dataSettingsText` with "Settings" linking to `url` (`<site>/settings`).
    static func dataSettings(url: URL) -> AttributedString {
        var text = AttributedString(dataSettingsText)
        if let range = text.range(of: dataSettingsLink, options: .backwards) {
            text[range].link = url
        }
        return text
    }

    /// The address as it would be saved, or nil when the app won't use it.
    static func normalizedWebsite(_ text: String) -> String? {
        CloudWebsite.validated(text)?.absoluteString
    }

    /// Under "Not signed in".
    static func signInDetail(_ cloud: ClaudeCloudState) -> String {
        if cloud.auth == .signingIn { return "Finish signing in in your browser." }
        guard cloud.websiteURL != nil else { return "Save a website first. " + fileNote }
        return "Opens Google's sign-in in your browser. " + fileNote
    }

    /// Why the last sign-in failed, or why the app was signed out.
    static func signInProblem(_ cloud: ClaudeCloudState) -> String? {
        if case .error(let message) = cloud.auth { return message }
        return cloud.lastError
    }

    /// What "Sync sessions and usage" sends, and what it never does.
    static func syncDetail(readsDesktopUsage: Bool) -> String {
        let desktop = readsDesktopUsage ? "Claude Desktop's readings too" : "Claude Desktop's too, once reading its cache is on"
        return "Sends project folder names, session titles, models, times, token counts, cost, and usage limits "
            + "for each signed-in Claude account (\(desktop)). Never file paths, prompts or your Claude login."
    }

    /// What a summary costs, which sessions get one, and what is sent.
    static let summariesDetail = "Only for sessions that end after you turn this on. Runs Claude Code on this Mac "
        + "with the session's own account, so it uses that account's usage: a few cents per session with Haiku, at "
        + "most $0.10 a run, 20 an hour and 60 a day, and none while that account's 5-hour limit is 80% used. Sends "
        + "the one- or two-sentence summary with the session, with file paths cut to their last part and anything "
        + "like a key or password removed. Turning this off deletes the summaries not sent yet; summaries already "
        + "sent stay on the website."

    /// Why summaries don't run now, or how many were written.
    static func summariesNote(_ cloud: ClaudeCloudState) -> String? {
        if !cloud.syncEnabled { return "Needs sync." }
        if cloud.summariesEnabled && !cloud.summariesAvailable { return "Not in this run: it can't start Claude Code." }
        guard cloud.summariesEnabled, cloud.summarizedSessions > 0 else { return nil }
        return cloud.summarizedSessions == 1 ? "1 session summarised on this Mac."
            : "\(cloud.summarizedSessions) sessions summarised on this Mac."
    }

    struct Status: Equatable {
        var title: String
        var detail: String?
        var isProblem = false
    }

    /// The sync row: when it last worked, what went wrong, what waits.
    static func status(_ cloud: ClaudeCloudState, now: Date) -> Status {
        guard cloud.syncEnabled else { return Status(title: "Sync is off", detail: "Nothing is sent while it is off.") }
        let title: String
        if cloud.isSyncing {
            title = "Syncing…"
        } else if cloud.lastError != nil {
            title = "Last sync failed"
        } else if let last = cloud.lastSyncAt {
            title = "Last synced \(UsageFormatter.age(of: last, now: now))"
        } else {
            title = "Not synced yet"
        }
        if let error = cloud.lastError, !cloud.isSyncing {
            return Status(title: title, detail: error, isProblem: true)
        }
        return Status(title: title, detail: waiting(sessions: cloud.pendingSessions, readings: cloud.pendingUsage))
    }

    /// "3 sessions and 12 usage readings to send", or nil.
    static func waiting(sessions: Int, readings: Int) -> String? {
        var parts: [String] = []
        if sessions > 0 { parts.append(sessions == 1 ? "1 session" : "\(sessions) sessions") }
        if readings > 0 { parts.append(readings == 1 ? "1 usage reading" : "\(readings) usage readings") }
        guard !parts.isEmpty else { return nil }
        return parts.joined(separator: " and ") + " to send."
    }
}
