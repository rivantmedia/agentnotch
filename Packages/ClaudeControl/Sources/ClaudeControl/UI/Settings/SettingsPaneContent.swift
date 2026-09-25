//
//  SettingsPaneContent.swift
//  ClaudeControl
//
//  The "Claude Code" pane of Codenotch's settings, in Codenotch's own form
//  style: grouped sections of native switches and pickers with a line of
//  explanation under what needs one. Buttons take the style the settings
//  window sets over every pane.
//
//   1. Turn on Claude Code control (until answered)
//   2. Accounts
//   3. Hooks and status line
//   4. Usage
//   5. Sessions and attention
//   6. Notifications
//   7. Advanced
//

import SwiftUI

struct SettingsPaneContent: View {
    let model: SettingsPaneModel
    let actions: SettingsPaneActions
    /// Snapshots: open the new-account form in this state.
    var initialNewAccountStep: NewAccountForm.Step = .closed
    /// Snapshots: every account's folder list shown.
    var expandsFolders = false

    /// "Not now" was answered and the user asked to turn it on after all:
    /// show the card again, files and all, rather than writing blind.
    @State private var isReconsideringConsent = false

    var body: some View {
        Form {
            if model.hookConsent == nil || model.setup.vibeNotchRunning {
                consentSection
            }
            if model.hookConsent == true && !model.setup.newInstallFolders.isEmpty {
                scopeNoticeSection
            }
            AccountsSection(model: model, actions: actions, initialNewAccountStep: initialNewAccountStep,
                            expandsFolders: expandsFolders)
            hooksSection
            usageSection
            attentionSection
            notificationsSection
            advancedSection
        }
        .formStyle(.grouped)
    }

    // MARK: 1. Consent

    private var consentSection: some View {
        Section {
            SettingsConsentCard(
                files: model.consentFiles,
                scope: model.consentScope,
                cleansStores: model.takeoverCleansStores,
                cleanupFiles: model.takeoverCleanupFiles,
                isAnswered: model.hookConsent != nil,
                takesOverVibeNotch: model.setup.superpoweredVibeNotchHooksFound,
                keepsVibeNotchHooks: model.setup.vibeNotchHooksFound,
                isBlockedByVibeNotch: model.setup.vibeNotchRunning,
                onTurnOn: actions.turnOn,
                onNotNow: actions.notNow,
                onQuitVibeNotch: actions.quitVibeNotch
            )
        }
    }

    /// The yes was given to an earlier build that didn't cover the VS Code
    /// workspaces' folders: said once, with the folders, and a way out.
    private var scopeNoticeSection: some View {
        Section {
            VStack(alignment: .leading, spacing: 8) {
                Text(ConsentCopy.scopeTitle)
                    .font(.headline)
                Text(ConsentCopy.scopeMessage(folderCount: model.setup.newInstallFolders.count))
                    .font(.callout)
                    .foregroundStyle(.ink(.secondary))
                    .fixedSize(horizontal: false, vertical: true)
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(model.scopeNoticeFolders, id: \.self) { folder in
                        Text(folder)
                            .font(.caption.monospaced())
                            .lineLimit(1)
                            .truncationMode(.head)
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Folders: \(model.scopeNoticeFolders.joined(separator: ", "))")
                HStack {
                    Spacer()
                    Button("Turn off", action: actions.turnOffAfterScopeNotice)
                    Button("OK", action: actions.acknowledgeScope)
                }
            }
            .padding(.vertical, 4)
        }
    }

    // MARK: 3. Hooks

    private var hooksSection: some View {
        Section("Hooks and status line") {
            if model.hookConsent == false {
                if isReconsideringConsent {
                    SettingsConsentCard(
                        files: model.consentFiles,
                        scope: model.consentScope,
                        cleansStores: model.takeoverCleansStores,
                        cleanupFiles: model.takeoverCleanupFiles,
                        isAnswered: false,
                        takesOverVibeNotch: model.setup.superpoweredVibeNotchHooksFound,
                        keepsVibeNotchHooks: model.setup.vibeNotchHooksFound,
                        isBlockedByVibeNotch: model.setup.vibeNotchRunning,
                        onTurnOn: {
                            isReconsideringConsent = false
                            actions.turnOn()
                        },
                        onNotNow: { isReconsideringConsent = false },
                        onQuitVibeNotch: actions.quitVibeNotch
                    )
                } else {
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Claude Code control is off")
                            Caption("Nothing is written to any settings.json. Sessions still show from Claude Code's session files, without approvals or \u{201C}done\u{201D}.")
                        }
                        Spacer()
                        Button("Turn on…") { isReconsideringConsent = true }
                    }
                }
            }
            Toggle(isOn: binding(model.hooksEnabled, actions.setHooksEnabled)) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Hooks in tracked accounts")
                    Caption(model.hooksSummary, ink: model.hooksEnabled && model.hooksInstalledCount < model.trackedCount ? .needsYou : .secondary)
                }
            }
            .disabled(model.hookConsent != true || !model.installsAllowed || model.isHookWorkRunning)

            Toggle(isOn: binding(model.statusLineIntegration, actions.setStatusLineIntegration)) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Live status line data")
                    Caption("Wraps each account's status line to read limits and context live. Turning it off restores your status line exactly.")
                }
            }
            .disabled(model.hookConsent != true || !model.installsAllowed)

            LabeledContent("Socket") {
                Text(model.socketPath)
                    .font(.caption.monospaced())
                    .foregroundStyle(.ink(.secondary))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }
            LabeledContent {
                HStack(spacing: 8) {
                    Text(model.claudeCodeVersion.map { "Version \($0)" } ?? "Not found yet")
                        .foregroundStyle(.ink(.secondary))
                    if model.claudeBinaryPath != nil {
                        Button("Find automatically", action: actions.resetClaudeBinary)
                    }
                    Button("Choose…", action: actions.chooseClaudeBinary)
                        .disabled(!model.installsAllowed)
                }
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Claude Code")
                    Caption(model.claudeBinaryPath.map { "Hooks are written for \($0)." }
                            ?? "Hooks are written for the oldest claude found, so every version reads them.")
                }
            }
            if let notice = model.hooksChangedNotice {
                Caption(notice)
            }
            if model.upstreamVibeNotchRunning {
                Caption("Vibe Notch is running. Where its hooks are still installed, both apps hear every session; remove its hooks per account above.",
                        ink: .needsYou)
            }
        }
    }

    // MARK: 4. Usage

    private var usageSection: some View {
        Section("Usage") {
            Picker(selection: binding(model.probeInterval, actions.setProbeInterval)) {
                Text("Off").tag(0)
                ForEach([5, 10, 15, 30], id: \.self) { Text("\($0) min").tag($0) }
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Check usage every")
                    Caption(model.probeInterval == 0
                            ? "Off: readings come only from live status lines and Claude Code's own cache."
                            : UsageCheckCopy.explanation(minutes: model.probeInterval))
                }
            }
            Toggle(isOn: binding(model.readsDesktopUsageCache, actions.setReadsDesktopUsageCache)) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Also read Claude Desktop's cached usage")
                    Caption("Claude Desktop keeps the limits it last saw on disk; no token is involved.")
                }
            }
            ForEach(model.accounts.filter(\.isTracked)) { account in
                LabeledContent {
                    Text(account.usageLine)
                        .foregroundStyle(.ink(.secondary))
                        .monospacedDigit()
                } label: {
                    HStack(spacing: 6) {
                        AccountDot(colorIndex: account.colorIndex, size: 7)
                        Text(account.name)
                    }
                }
            }
            HStack {
                Spacer()
                if model.isRefreshingUsage {
                    ProgressView().controlSize(.small)
                }
                Button("Refresh now", action: actions.refreshUsage)
                    .disabled(model.isRefreshingUsage)
            }
        }
    }

    // MARK: 5. Attention

    private var attentionSection: some View {
        Section("Sessions and attention") {
            Picker(selection: binding(model.autoOpen, actions.setAutoOpen)) {
                ForEach(AutoOpenPolicy.allCases, id: \.self) { Text($0.settingTitle).tag($0) }
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Open the sessions panel")
                    Caption(model.autoOpen == .never ? model.autoOpen.detail : "\(model.autoOpen.detail) Never over a full-screen app.")
                }
            }
            Picker(selection: binding(model.holdOpen, actions.setHoldOpen)) {
                Text("Auto").tag(HoldOpenPolicy.auto)
                Text("Always").tag(HoldOpenPolicy.always)
                Text("Never").tag(HoldOpenPolicy.never)
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Keep the notch open while a session needs you")
                    Caption("Auto keeps it open only beside the camera, where the folded notch can't show marks. There, finished and working sessions show only when you hover (or by their chime and peek).")
                }
            }
            Toggle("Counts on the Claude rings", isOn: binding(model.ringBadges, actions.setRingBadges))
            Toggle(isOn: binding(model.restingMarks, actions.setRestingMarks)) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Dots on the folded notch")
                    Caption("Not beside the camera, where the folded notch is the camera housing itself.")
                }
            }
            Toggle("Needs-you count on the Dock icon", isOn: binding(model.dockBadge, actions.setDockBadge))
            Picker("Clicking a Claude ring", selection: binding(model.ringClick, actions.setRingClick)) {
                Text("Opens its sessions").tag(RingClickAction.openPanel)
                Text("Refreshes its usage").tag(RingClickAction.refresh)
            }
            Picker(selection: binding(model.sessionClick, actions.setSessionClick)) {
                Text("Smart").tag(SessionClickAction.smart)
                Text("Opens the panel").tag(SessionClickAction.panel)
                Text("Shows the terminal").tag(SessionClickAction.terminal)
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Clicking a session in the hover card")
                    Caption("Smart opens the panel when the session needs you, and its terminal otherwise.")
                }
            }
            Picker("Panel shortcut", selection: binding(model.hotKey, actions.setHotKey)) {
                ForEach(PanelHotKey.allCases, id: \.self) { Text($0.displayName).tag($0) }
            }
            HStack {
                Spacer()
                Button("Open the sessions panel", action: actions.openSessionsPanel)
            }
        }
    }

    // MARK: 6. Notifications

    private var notificationsSection: some View {
        Section("Notifications") {
            Toggle("Banner when a session needs you", isOn: binding(model.notifyNeedsInput, actions.setNotifyNeedsInput))
            Toggle("Banner when a session is done", isOn: binding(model.notifyReadyForReview, actions.setNotifyReadyForReview))
            LabeledContent("macOS permission") {
                if model.notificationsDenied {
                    HStack(spacing: 8) {
                        Text("Off in System Settings")
                            .foregroundStyle(.ink(model.notifyNeedsInput || model.notifyReadyForReview ? .needsYou : .secondary))
                        Button("Open…", action: actions.openSystemNotificationSettings)
                    }
                } else {
                    Text("Allowed").foregroundStyle(.ink(.secondary))
                }
            }
            HStack(alignment: .firstTextBaseline) {
                Caption("Banners are silent: sounds and the notch's peek follow Codenotch's Notifications settings.")
                Spacer()
                Button("Sounds and peek…", action: actions.openNotificationsPane)
            }
        }
    }

    // MARK: 7. Advanced

    private var advancedSection: some View {
        Section("Advanced") {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Session state")
                    Caption("A plain-text line per session for bug reports: its title, state and tasks, and the start of a finished reply. Look it over before sharing it.")
                }
                Spacer()
                Button("Copy", action: actions.copyStateDump)
            }
            ConfirmingButtonRow(
                title: "Review queue",
                detail: "Marks every finished session reviewed.",
                action: "Reset…",
                confirm: "Mark all reviewed",
                perform: actions.resetReviewQueue
            )
        }
    }

    private func binding<Value>(_ value: Value, _ set: @escaping (Value) -> Void) -> Binding<Value> {
        Binding(get: { value }, set: set)
    }
}

extension AutoOpenPolicy {
    /// The words the settings pane and the panel's gear menu both use.
    var settingTitle: String {
        switch self {
        case .never: return "Never"
        case .needsInput: return "When a session needs you"
        case .needsInputOrDone: return "When one needs you or is done"
        }
    }
}

// MARK: - Building blocks

/// The line of explanation under a setting.
struct Caption: View {
    let text: String
    var ink: ClaudeInk.Token = .secondary

    init(_ text: String, ink: ClaudeInk.Token = .secondary) {
        self.text = text
        self.ink = ink
    }

    var body: some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(.ink(ink))
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// A button that asks once more before doing something it can't undo.
struct ConfirmingButtonRow: View {
    let title: String
    let detail: String
    let action: String
    let confirm: String
    let perform: () -> Void

    @State private var isConfirming = false

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                Caption(detail)
            }
            Spacer()
            if isConfirming {
                Button("Cancel") { isConfirming = false }
                Button(confirm, role: .destructive) {
                    isConfirming = false
                    perform()
                }
            } else {
                Button(action) { isConfirming = true }
            }
        }
    }
}

/// "Turn on Claude Code control": what it will edit, and nothing written
/// until the user says so.
struct SettingsConsentCard: View {
    let files: [String]
    /// Where it installs, in words (Claude Parallel Profiles only).
    var scope: String?
    /// The takeover also cleans stores or the shared history.
    var cleansStores = false
    /// The settings.json files it cleans there.
    var cleanupFiles: [String] = []
    let isAnswered: Bool
    let takesOverVibeNotch: Bool
    /// Upstream Vibe Notch's hooks are there too; they stay (GUX-8).
    var keepsVibeNotchHooks: Bool = false
    let isBlockedByVibeNotch: Bool
    let onTurnOn: () -> Void
    let onNotNow: () -> Void
    let onQuitVibeNotch: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(isAnswered ? ConsentCopy.runningTitle : ConsentCopy.title)
                .font(.headline)
            Text(isAnswered ? ConsentCopy.running : ConsentCopy.explanation)
                .font(.callout)
                .foregroundStyle(.ink(.secondary))
                .fixedSize(horizontal: false, vertical: true)
            if !isAnswered, let scope {
                Text(scope)
                    .font(.callout)
                    .foregroundStyle(.ink(.primary))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !isAnswered && !files.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(files, id: \.self) { file in
                        Text(file)
                            .font(.caption.monospaced())
                            .lineLimit(1)
                            .truncationMode(.head)
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Files it edits: \(files.joined(separator: ", "))")
            }
            if takesOverVibeNotch && !isAnswered {
                Caption(cleansStores ? ConsentCopy.takeover + " " + ConsentCopy.takeoverStores : ConsentCopy.takeover)
                if cleansStores && !cleanupFiles.isEmpty {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(cleanupFiles, id: \.self) { file in
                            Text(file)
                                .font(.caption.monospaced())
                                .foregroundStyle(.ink(.secondary))
                                .lineLimit(1)
                                .truncationMode(.head)
                        }
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel("Files it cleans: \(cleanupFiles.joined(separator: ", "))")
                }
            }
            if keepsVibeNotchHooks && !isAnswered {
                Caption(ConsentCopy.vibeNotchStays)
            }
            HStack {
                if isBlockedByVibeNotch {
                    Caption(isAnswered ? "" : ConsentCopy.blocked, ink: .needsYou)
                    Spacer()
                    Button("Quit Superpowered Vibe Notch", action: onQuitVibeNotch)
                } else {
                    Spacer()
                }
                if !isAnswered {
                    Button("Not now", action: onNotNow)
                    // Emphasis only, never the default button: a stray Return in
                    // the settings window must not edit every settings.json (GUX-10).
                    Button(takesOverVibeNotch ? "Take over and turn on" : "Turn on", action: onTurnOn)
                        .buttonStyle(.borderedProminent)
                        .disabled(isBlockedByVibeNotch)
                }
            }
        }
        .padding(.vertical, 4)
    }
}

/// How the usage check works, said where it can be switched off (S5): it
/// runs Claude Code itself, which may update its own files, and finding
/// `claude` may ask the login shell.
nonisolated enum UsageCheckCopy {
    static func explanation(minutes: Int) -> String {
        "Every \(minutes) min, only when nothing fresher has arrived, this app runs Claude Code's own usage check "
            + "in each signed-in account (Claude Code may update its own files there); this app never reads your "
            + "login token. To find claude it may ask your login shell once."
    }
}
