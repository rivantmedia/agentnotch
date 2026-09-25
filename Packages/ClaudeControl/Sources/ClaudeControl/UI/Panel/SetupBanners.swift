//
//  SetupBanners.swift
//  ClaudeControl
//
//  What stands between the panel and live sessions, said where it matters:
//  the consent card before any settings.json is touched, Superpowered Vibe
//  Notch still running, the hook socket failing, and accounts whose hooks
//  are missing (their sessions show, but never reach "Ready for review").
//

import SwiftUI

/// The banners the list shows, in order of what blocks the most.
struct SetupBanners: View {
    let setup: ClaudeSetupState
    let hookHealth: HookHealth
    let consentFiles: [String]
    /// Where it installs, in words (Claude Parallel Profiles only).
    var consentScope: String? = nil
    /// The takeover also cleans stores or the shared history.
    var takeoverCleansStores = false
    /// The settings.json files it cleans there.
    var takeoverCleanupFiles: [String] = []
    /// Answered in this panel already: hide the consent card at once.
    let didAnswerConsent: Bool
    let onTurnOn: () -> Void
    let onNotNow: () -> Void
    let onQuitVibeNotch: () -> Void
    let onOpenSettings: () -> Void
    /// The notice that the yes now covers VS Code workspaces' folders.
    var onAcknowledgeScope: () -> Void = {}
    var onTurnOffHooks: () -> Void = {}

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        VStack(spacing: theme.blockSpacing) {
            if setup.needsHookConsent && !didAnswerConsent {
                ConsentCard(
                    files: consentFiles,
                    scope: consentScope,
                    cleansStores: takeoverCleansStores,
                    cleanupFiles: takeoverCleanupFiles,
                    takesOverVibeNotch: setup.superpoweredVibeNotchHooksFound,
                    keepsVibeNotchHooks: setup.vibeNotchHooksFound,
                    isBlockedByVibeNotch: setup.vibeNotchRunning,
                    onTurnOn: onTurnOn,
                    onNotNow: onNotNow,
                    onQuitVibeNotch: onQuitVibeNotch
                )
            } else if setup.vibeNotchRunning {
                NoticeBanner(
                    icon: "exclamationmark.triangle.fill",
                    tint: .needsYou,
                    title: ConsentCopy.runningTitle,
                    message: ConsentCopy.running
                ) {
                    Button("Quit it", action: onQuitVibeNotch)
                        .buttonStyle(.claude(.tinted(.needsYou), compact: true))
                }
            }
            if !setup.newInstallFolders.isEmpty && !setup.needsHookConsent {
                NoticeBanner(
                    icon: "info.circle.fill",
                    tint: .secondary,
                    title: ConsentCopy.scopeTitle,
                    message: ConsentCopy.scopeMessage(folderCount: setup.newInstallFolders.count)
                ) {
                    VStack(alignment: .trailing, spacing: 4) {
                        Button("OK", action: onAcknowledgeScope)
                            .buttonStyle(.claude(.secondary, compact: true))
                        Button("Turn off", action: onTurnOffHooks)
                            .buttonStyle(.claude(.secondary, compact: true))
                    }
                }
            }
            if let error = setup.socketError {
                NoticeBanner(
                    icon: "bolt.horizontal.circle.fill",
                    tint: .critical,
                    title: "Not receiving hook events",
                    message: "\(error) Sessions still update from Claude Code's session files, without approvals."
                ) {
                    EmptyView()
                }
            }
            if hookHealth.controlOff && !setup.needsHookConsent {
                NoticeBanner(
                    icon: "pause.circle",
                    tint: .secondary,
                    title: HookHealth.controlOffTitle,
                    message: HookHealth.controlOffMessage
                ) {
                    Button("Turn on…", action: onOpenSettings)
                        .buttonStyle(.claude(.secondary, compact: true))
                }
            } else if !hookHealth.isHealthy && !setup.needsHookConsent {
                NoticeBanner(
                    icon: "exclamationmark.circle.fill",
                    tint: .needsYou,
                    title: hookHealth.summary,
                    message: hookHealth.consequence
                ) {
                    Button("Settings…", action: onOpenSettings)
                        .buttonStyle(.claude(.secondary, compact: true))
                }
            }
        }
    }
}

/// A one-thing-to-know banner: a coloured symbol, a line in primary ink, a
/// line of explanation and at most one action.
struct NoticeBanner<Action: View>: View {
    let icon: String
    let tint: ClaudeInk.Token
    let title: String
    let message: String
    @ViewBuilder let action: Action

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.ink(tint))
                .frame(width: 14)
                .padding(.top, 0.5)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .claudeFont(.body, weight: .semibold)
                    .foregroundStyle(.ink(.primary))
                    .fixedSize(horizontal: false, vertical: true)
                Text(message)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 4)
            action
        }
        .padding(9)
        .background(RoundedRectangle(cornerRadius: theme.rowCorner, style: .continuous).fill(theme.controlFill))
        .accessibilityElement(children: .combine)
    }
}

/// "Turn on Claude Code control": what it will edit, and nothing written
/// until the user says so.
struct ConsentCard: View {
    let files: [String]
    /// Where it installs, in words (Claude Parallel Profiles only).
    var scope: String? = nil
    /// The takeover also cleans stores or the shared history.
    var cleansStores = false
    /// The settings.json files it cleans there (stores, ~/.claude-shared).
    var cleanupFiles: [String] = []
    /// Superpowered Vibe Notch's hooks are registered: turning on replaces them.
    let takesOverVibeNotch: Bool
    /// Upstream Vibe Notch's hooks are registered: they stay (GUX-8).
    var keepsVibeNotchHooks: Bool = false
    /// Superpowered Vibe Notch is running: it would write its hooks straight
    /// back, so turning on waits until it quits.
    let isBlockedByVibeNotch: Bool
    let onTurnOn: () -> Void
    let onNotNow: () -> Void
    let onQuitVibeNotch: () -> Void

    @Environment(\.claudeControlTheme) private var theme

    /// Files listed before "and N more".
    private static let maxListedFiles = 4

    var body: some View {
        VStack(alignment: .leading, spacing: theme.blockSpacing) {
            HStack(spacing: 7) {
                Image(systemName: "terminal.fill")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.ink(.accent))
                    .accessibilityHidden(true)
                Text(ConsentCopy.title)
                    .claudeFont(.rowTitle, weight: .semibold)
                    .foregroundStyle(.ink(.primary))
            }
            Text(ConsentCopy.explanation)
                .claudeFont(.caption)
                .foregroundStyle(.ink(.secondary))
                .fixedSize(horizontal: false, vertical: true)
            if let scope {
                Text(scope)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.primary))
                    .fixedSize(horizontal: false, vertical: true)
            }

            if !files.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(files.prefix(Self.maxListedFiles), id: \.self) { file in
                        Text(file)
                            .claudeFont(.monoCaption)
                            .foregroundStyle(.ink(.primary))
                            .lineLimit(1)
                            .truncationMode(.head)
                    }
                    if files.count > Self.maxListedFiles {
                        Text("and \(files.count - Self.maxListedFiles) more")
                            .claudeFont(.caption)
                            .foregroundStyle(.ink(.secondary))
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Files it edits: \(files.joined(separator: ", "))")
            }

            if takesOverVibeNotch {
                Text(cleansStores ? ConsentCopy.takeover + " " + ConsentCopy.takeoverStores : ConsentCopy.takeover)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .fixedSize(horizontal: false, vertical: true)
                if cleansStores && !cleanupFiles.isEmpty {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(cleanupFiles, id: \.self) { file in
                            Text(file)
                                .claudeFont(.monoCaption)
                                .foregroundStyle(.ink(.secondary))
                                .lineLimit(1)
                                .truncationMode(.head)
                        }
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel("Files it cleans: \(cleanupFiles.joined(separator: ", "))")
                }
            }
            if keepsVibeNotchHooks {
                Text(ConsentCopy.vibeNotchStays)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .fixedSize(horizontal: false, vertical: true)
            }

            if isBlockedByVibeNotch {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(ConsentCopy.blocked)
                        .claudeFont(.caption)
                        .foregroundStyle(.ink(.needsYou))
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 4)
                    Button("Quit it", action: onQuitVibeNotch)
                        .buttonStyle(.claude(.tinted(.needsYou), compact: true))
                }
            }

            HStack(spacing: 6) {
                Spacer(minLength: 0)
                Button("Not now", action: onNotNow)
                    .buttonStyle(.claude(.secondary))
                Button(takesOverVibeNotch ? "Take over and turn on" : "Turn on", action: onTurnOn)
                    .buttonStyle(.claude(.primary))
                    .disabled(isBlockedByVibeNotch)
            }
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: theme.rowCorner, style: .continuous).fill(theme.controlFill))
    }
}

/// What the panel's and the settings pane's consent cards say, word for word.
nonisolated enum ConsentCopy {
    static let title = "Turn on Claude Code control"
    static let explanation = "To show every session live and let you answer prompts from here, this app adds its hooks and a status-line wrapper to the settings.json of each folder Claude Code runs in. Nothing is written until you turn it on, and turning it off puts your status line back exactly."
    static let takeover = "Superpowered Vibe Notch's hooks are replaced, not doubled up, and the status line it wrapped is put back first."
    static let takeoverStores = "Superpowered Vibe Notch also wrote into the account stores and ~/.claude-shared: its entries, its scripts and the settings.json files it created there are removed; nothing else is changed."
    static let vibeNotchStays = "Vibe Notch's hooks stay; remove them per account in Settings › Claude Code."
    static let blocked = "Superpowered Vibe Notch is running and would write its hooks straight back. Quit it first."
    static let runningTitle = "Superpowered Vibe Notch is running"
    /// The notice for a yes given to an earlier build, which didn't cover
    /// the VS Code workspaces' folders.
    static let scopeTitle = "Claude Code control now covers your VS Code workspaces"
    static func scopeMessage(folderCount: Int) -> String {
        let folders = folderCount == 1 ? "1 VS Code workspace's folder" : "\(folderCount) VS Code workspaces' folders"
        return "This version puts its hooks and status line in \(folders) too, and in new ones as they appear: Claude Parallel Profiles runs Claude Code there. Each settings.json has a backup beside it. Account stores never get hooks."
    }
    static let running = "It installs its own hooks for the same sessions. Quit it to hand them over to this app."
}
