//
//  AccountsSection.swift
//  ClaudeControl
//
//  Every Claude account (a signed-in identity, whatever folders it lives
//  in): its colour, its name (the nickname Codenotch shows everywhere),
//  email and plan, where it runs (`~/.claude`, VS Code windows) and its
//  Claude Parallel Profiles stores, and what its settings.json files hold. Two switches: "Ring in notch" (Codenotch's
//  connected state) and "Track sessions and hooks". Then rename, install,
//  remove another app's hooks, copy the launch command, reveal, forget, and
//  adding an existing folder or a new account.
//

import SwiftUI

struct AccountsSection: View {
    let model: SettingsPaneModel
    let actions: SettingsPaneActions
    var initialNewAccountStep: NewAccountForm.Step = .closed
    /// Snapshots: every account's folders listed.
    var expandsFolders = false

    var body: some View {
        Section {
            if model.accounts.isEmpty {
                Caption("No Claude Code accounts yet. Add the folder Claude Code uses, or create a new account.")
            }
            ForEach(model.accounts) { account in
                AccountSettingsRow(
                    account: account,
                    canInstall: model.hookConsent == true && model.hooksEnabled && model.installsAllowed,
                    canWrite: model.hookConsent == true && model.installsAllowed,
                    isWorking: model.isHookWorkRunning,
                    actions: actions,
                    showsFolders: expandsFolders
                )
            }
            ForEach(model.suggestions) { suggestion in
                FolderSuggestionRow(suggestion: suggestion, canAdd: model.installsAllowed, actions: actions)
            }
            if !model.unsignedFolders.isEmpty {
                UnsignedFoldersRow(folders: model.unsignedFolders)
            }
            NewAccountForm(actions: actions, parallelProfiles: model.parallelProfiles, initialStep: initialNewAccountStep)
        } header: {
            Text("Accounts")
        } footer: {
            Caption(model.parallelProfiles
                    ? "Ring in notch draws the account's usage ring. Track sessions and hooks lists its sessions here; switching it off also removes this app's hooks from that account's VS Code workspaces and folders, and its ring and usage checks with them. ~/.claude keeps them while another account is tracked, since Claude Parallel Profiles copies whichever account you last used into it; that account's sessions there are then hidden and its prompts stay in the terminal."
                    : "Ring in notch draws the account's usage ring. Track sessions and hooks lists its sessions here; switching it off also removes this app's hooks from that account, and its ring and usage checks with them.")
        }
    }
}

// MARK: - Row

struct AccountSettingsRow: View {
    let account: AccountSettingsItem
    let canInstall: Bool
    let canWrite: Bool
    let isWorking: Bool
    let actions: SettingsPaneActions

    @State private var isRenaming = false
    @State private var draftName = ""
    @State private var isConfirmingForget = false
    @State private var didCopy = false
    @State private var showsFolders: Bool

    init(account: AccountSettingsItem, canInstall: Bool, canWrite: Bool, isWorking: Bool,
         actions: SettingsPaneActions, showsFolders: Bool = false) {
        self.account = account
        self.canInstall = canInstall
        self.canWrite = canWrite
        self.isWorking = isWorking
        self.actions = actions
        self._showsFolders = State(initialValue: showsFolders)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(alignment: .center, spacing: 8) {
                AccountDot(colorIndex: account.colorIndex, size: 9)
                    .opacity(account.isTracked ? 1 : 0.45)
                if isRenaming {
                    TextField("Name", text: $draftName, prompt: Text(account.defaultName))
                        .labelsHidden()
                        .textFieldStyle(.roundedBorder)
                        .onSubmit(commitRename)
                        .frame(maxWidth: 220)
                    Button("Save", action: commitRename)
                    Button("Cancel") { isRenaming = false }
                } else {
                    Text(account.name)
                        .foregroundStyle(.ink(account.isTracked ? .primary : .secondary))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    if account.isDefault {
                        Caption(account.defaultCaption)
                            .help(account.defaultCaptionHelp ?? "")
                    }
                }
                Spacer(minLength: 8)
                Text("Ring in notch")
                    .font(.caption)
                    .foregroundStyle(.ink(account.isTracked ? .secondary : .tertiary))
                    .accessibilityHidden(true)
                // An untracked account has no ring and no usage checks: the
                // switch would do nothing (GUX-7).
                Toggle("Ring in notch", isOn: Binding(
                    get: { account.isTracked && account.isRingShown },
                    set: { actions.setRingShown(account.ringID, $0) }
                ))
                .toggleStyle(.switch)
                .controlSize(.small)
                .labelsHidden()
                .disabled(!account.isTracked)
                .help(account.isTracked ? "Ring in notch" : AccountSettingsItem.ringNeedsTrackingCaption)
                .accessibilityLabel("Ring in notch for \(account.name)")
                .accessibilityHint(account.isTracked ? "" : AccountSettingsItem.ringNeedsTrackingCaption)
            }

            VStack(alignment: .leading, spacing: 2) {
                Caption(account.folders.isEmpty && account.folderSummary == account.folder
                        ? "\(account.identity) · \(account.folder)" : account.identity)
                if !(account.folders.isEmpty && account.folderSummary == account.folder) {
                    Caption(account.folderSummary)
                    if !account.folders.isEmpty {
                        Button(showsFolders ? "Hide folders" : "Show folders") { showsFolders.toggle() }
                            .buttonStyle(.link)
                            .font(.caption)
                            .accessibilityLabel(showsFolders ? "Hide \(account.name)'s folders" : "Show \(account.name)'s folders")
                    }
                }
                if showsFolders {
                    AccountFoldersList(folders: account.folders)
                        .padding(.top, 2)
                }
                Caption(account.usageLine)
            }
            .padding(.leading, 17)

            FlowLayout(spacing: 5, lineSpacing: 5, fillsWidth: true) {
                StatusChip(text: account.hookTitle, ink: hookInk)
                if account.statusLineInstalled {
                    StatusChip(text: "Live status line", ink: .secondary)
                }
                ForEach(account.legacyHooks) { legacy in
                    StatusChip(text: "\(legacy.appName) hooks", ink: .needsYou)
                }
                if account.vibeIslandHooksPresent {
                    StatusChip(text: "Vibe Island hooks", ink: .secondary)
                        .help("Vibe Island is a separate app; its hooks are left alone.")
                }
            }
            .padding(.leading, 17)

            if account.hasLoginConflict {
                Caption(account.hasTerminalLaunch
                        ? "Sessions here ran with different CLAUDE_CONFIG_DIR spellings, which Claude Code treats as separate logins. Start it the same way each time: Copy launch command."
                        : "Sessions here ran with different CLAUDE_CONFIG_DIR spellings, which Claude Code treats as separate logins. Start it from its VS Code window rather than by hand.",
                        ink: .needsYou)
                    .padding(.leading, 17)
            }
            if let guidance = account.launchGuidance {
                Caption(guidance)
                    .padding(.leading, 17)
            }

            if let problem = account.hookProblem {
                Caption(problem, ink: account.hookState == .unreadable ? .critical : .needsYou)
                    .padding(.leading, 17)
            }

            Toggle(isOn: Binding(get: { account.isTracked }, set: { actions.setTracked(account.id, $0) })) {
                Text("Track sessions and hooks")
            }
            .toggleStyle(.switch)
            .controlSize(.small)
            .padding(.leading, 17)
            if !account.isTracked {
                Caption("Ring in notch needs Track sessions and hooks")
                    .padding(.leading, 17)
            }

            FlowLayout(spacing: 6, lineSpacing: 6, fillsWidth: true) {
                Button("Rename…") {
                    draftName = account.hasNickname ? account.name : ""
                    isRenaming = true
                }
                if account.isTracked && account.hookFolderCount > 0 {
                    Button(account.hookState == .installed ? "Reinstall hooks" : "Install hooks") {
                        actions.installHooks(account.id)
                    }
                    .disabled(!canInstall || isWorking || account.hookState == .missingFolder)
                }
                ForEach(account.legacyHooks) { legacy in
                    Button("Remove \(legacy.appName) hooks") {
                        actions.removeLegacyHooks(account.id, legacy)
                    }
                    .disabled(!legacy.canRemove(canWrite: canWrite, canInstall: canInstall)
                              || isWorking || account.hookState == .unreadable)
                    .help(legacy.removalHelp(canInstall: canInstall))
                }
                if account.hasTerminalLaunch {
                    Button(didCopy ? "Copied" : "Copy launch command") {
                        actions.copyLaunchCommand(account.launchCommand)
                        didCopy = true
                    }
                    .help(account.launchCommand)
                }
                Button("Reveal in Finder") { actions.reveal(account.id) }
                if account.canForget {
                    if isConfirmingForget {
                        Button("Forget and remove hooks", role: .destructive) {
                            isConfirmingForget = false
                            actions.forget(account.id)
                        }
                        Button("Cancel") { isConfirmingForget = false }
                    } else {
                        Button("Forget…") { isConfirmingForget = true }
                    }
                }
            }
            .padding(.leading, 17)
            .task(id: didCopy) {
                guard didCopy else { return }
                try? await Task.sleep(for: .seconds(1.5))
                didCopy = false
            }

            if isConfirmingForget {
                Caption(account.folders.isEmpty
                        ? "This app's hooks are removed from its settings.json and it stops being tracked. The folder and its sessions are left alone."
                        : "This app's hooks are removed from every folder it runs in and it stops being tracked, VS Code workspaces opened later included (~/.claude keeps them while another account is tracked). Its folders, stores and sessions are left alone.")
                    .padding(.leading, 17)
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(account.name), \(account.identity), \(account.hookTitle)")
    }

    private var hookInk: ClaudeInk.Token {
        switch account.hookState {
        case .installed: return .review
        case .notInstalled: return .needsYou
        case .unreadable: return .critical
        case .off, .missingFolder: return .secondary
        }
    }

    private func commitRename() {
        let name = draftName.trimmingCharacters(in: .whitespacesAndNewlines)
        actions.rename(account.ringID, name.isEmpty ? nil : name)
        isRenaming = false
    }
}

/// An account's folders, expanded: where it runs and its stores.
struct AccountFoldersList: View {
    let folders: [AccountSettingsItem.Folder]

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(folders) { folder in
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    // A project name is short and leads; a path keeps its
                    // end (the hash or folder name) when it must shorten.
                    Text(folder.title)
                        .font(folder.projectName == nil ? .caption.monospaced() : .caption)
                        .foregroundStyle(.ink(.secondary))
                        .lineLimit(1)
                        .truncationMode(folder.projectName == nil ? .head : .tail)
                        .layoutPriority(folder.projectName == nil ? 0 : 1)
                        .help(folder.path)
                    Text(folder.roleName)
                        .font(folder.projectName == nil ? .caption : .caption.monospaced())
                        .foregroundStyle(.ink(.tertiary))
                        .lineLimit(1)
                        .truncationMode(.head)
                    Spacer(minLength: 6)
                    Text(folder.state)
                        .font(.caption)
                        .foregroundStyle(.ink(folder.role == .store ? .tertiary : .secondary))
                }
                .accessibilityElement(children: .combine)
            }
        }
    }
}

/// Run folders nobody has signed in to: no ring until someone does.
struct UnsignedFoldersRow: View {
    let folders: [String]

    /// A VS Code workspace's folder is signed in by picking an account for
    /// its window (or /login there); any other folder with /login.
    static func caption(_ folders: [String]) -> String {
        let list = folders.joined(separator: ", ")
        if folders.contains(where: { $0.contains("/.claude-windows/") || $0.hasPrefix("VS Code · ") }) {
            return "\(list): Claude Code runs here, but nobody is signed in yet. Pick an account for that window from the Claude Parallel Profiles status bar item, or /login there; it gets a ring then."
        }
        return "\(list): Claude Code runs here, but nobody is signed in yet. It gets a ring once someone signs in with /login."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Not signed in")
                .foregroundStyle(.ink(.secondary))
            Caption(Self.caption(folders))
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}

/// A folder that looks like an account but wasn't added by itself.
struct FolderSuggestionRow: View {
    let suggestion: FolderSuggestionItem
    let canAdd: Bool
    let actions: SettingsPaneActions

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Found \(suggestion.folder)")
                    .foregroundStyle(.ink(.secondary))
                Caption(suggestion.reason)
            }
            Spacer(minLength: 8)
            Button("Dismiss") { actions.dismissSuggestion(suggestion.configDir) }
            Button("Add") { actions.acceptSuggestion(suggestion.configDir) }
                .disabled(!canAdd)
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Suggested account folder \(suggestion.folder): \(suggestion.reason)")
    }
}

/// A short fact in a signal colour.
struct StatusChip: View {
    let text: String
    let ink: ClaudeInk.Token

    var body: some View {
        Text(text)
            .font(.caption.weight(.medium))
            .foregroundStyle(.ink(ink))
            .lineLimit(1)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(Capsule().fill(.ink(ink, opacity: 0.14)))
            .fixedSize()
    }
}

// MARK: - Adding accounts

/// "Add existing folder…" and "New account…", with the create flow inline.
struct NewAccountForm: View {
    enum Step: Equatable {
        case closed
        /// Claude Parallel Profiles adds accounts: how, and the terminal-only
        /// alternative.
        case parallelProfilesGuidance
        case naming(String)
        case failed(name: String, message: String)
        case created(folder: String, launchCommand: String)
    }

    let actions: SettingsPaneActions
    /// Claude Parallel Profiles manages accounts on this Mac.
    let parallelProfiles: Bool
    @State private var step: Step

    init(actions: SettingsPaneActions, parallelProfiles: Bool = false, initialStep: Step = .closed) {
        self.actions = actions
        self.parallelProfiles = parallelProfiles
        self._step = State(initialValue: initialStep)
    }

    var body: some View {
        switch step {
        case .closed:
            HStack {
                Spacer()
                Button("Add existing folder…", action: actions.addExistingFolder)
                Button("New account…") { step = parallelProfiles ? .parallelProfilesGuidance : .naming("") }
            }
        case .parallelProfilesGuidance:
            VStack(alignment: .leading, spacing: 6) {
                Text("Add an account in VS Code")
                    .font(.headline)
                Caption(NewAccountCopy.parallelProfiles)
                Caption(NewAccountCopy.terminalAlternative)
                HStack {
                    Spacer()
                    Button("Create a folder for the terminal…") { step = .naming("") }
                    Button("Done") { step = .closed }
                }
            }
        case .naming, .failed:
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    TextField("Account name", text: nameBinding, prompt: Text("Name, for example work"))
                        .labelsHidden()
                        .textFieldStyle(.roundedBorder)
                        .onSubmit(create)
                    Button("Cancel") { step = .closed }
                    Button("Create", action: create)
                        .disabled(nameBinding.wrappedValue.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                if case .failed(_, let message) = step {
                    Caption(message, ink: .critical)
                } else {
                    Caption("Creates ~/.claude-\(AccountRegistry.sanitizedAccountName(nameBinding.wrappedValue) ?? "name"), a separate Claude Code config folder you sign in to once.")
                }
            }
        case let .created(folder, launchCommand):
            VStack(alignment: .leading, spacing: 6) {
                Text("Created \(folder)")
                    .font(.headline)
                Caption("Run this in a terminal, then /login. It's on your clipboard.")
                Text(launchCommand)
                    .font(.callout.monospaced())
                    .textSelection(.enabled)
                    .lineLimit(2)
                HStack {
                    Spacer()
                    Button("Copy again") { actions.copyLaunchCommand(launchCommand) }
                    Button("Done") { step = .closed }
                }
            }
        }
    }

    private var nameBinding: Binding<String> {
        Binding(
            get: {
                switch step {
                case .naming(let name), .failed(let name, _): return name
                default: return ""
                }
            },
            set: { step = .naming($0) }
        )
    }

    private func create() {
        let name = nameBinding.wrappedValue
        guard !name.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        do {
            let command = try actions.createAccount(name)
            actions.copyLaunchCommand(command)
            let slug = AccountRegistry.sanitizedAccountName(name) ?? name
            step = .created(folder: "~/.claude-\(slug)", launchCommand: command)
        } catch {
            step = .failed(name: name, message: error.localizedDescription)
        }
    }
}

/// What "New account…" says when Claude Parallel Profiles manages accounts.
nonisolated enum NewAccountCopy {
    static let parallelProfiles = "Claude Parallel Profiles adds accounts: open a VS Code window, sign in with Claude Code's account menu or /login, and the extension saves the account. It appears here by itself, with its usage and sessions, and new windows get this app's hooks automatically. Signing in switches that window to the new account and reloads it (sessions running in it stop). The previous account stays saved, and you can switch any window back from the status bar."
    static let terminalAlternative = "To use another account from a terminal without VS Code, you can instead create a separate config folder (~/.claude-<name>) and sign in to it with CLAUDE_CONFIG_DIR."
}
