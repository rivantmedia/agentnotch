import Foundation
import Testing
@testable import ClaudeControl

/// Account names, hook health, the settings pane's wording and adding folders.
struct B_SettingsTests {
    private let home = "/Users/me"

    private func account(_ dir: String, label: String, email: String? = nil, plan: String? = nil,
                         tracked: Bool = true, hooks: ClaudeHookStatus = ClaudeHookStatus()) -> ClaudeAccountSummary {
        ClaudeAccountSummary(id: dir, ringID: ClaudeRingIdentity.ringID(configDir: dir, home: home), configDir: dir,
                             label: label, email: email, planName: plan, isDefault: dir == "\(home)/.claude",
                             isTracked: tracked, hooks: hooks, launchCommand: "claude")
    }

    // MARK: Labels

    @Test func distinctLabelsAreLeftAlone() {
        let labels = AccountLabels.disambiguated([
            account("\(home)/.claude", label: "Personal"), account("\(home)/.claude-work", label: "Work"),
        ], home: home)
        #expect(labels == ["\(home)/.claude": "Personal", "\(home)/.claude-work": "Work"])
    }

    @Test func sameEmailIsToldApartByPlanThenFolder() {
        let byPlan = AccountLabels.disambiguated([
            account("\(home)/.claude", label: "me@x.com", email: "me@x.com", plan: "Max 20x"),
            account("\(home)/.claude-work", label: "me@x.com", email: "me@x.com", plan: "Team"),
        ], home: home)
        #expect(byPlan["\(home)/.claude"] == "me@x.com · Max 20x")
        #expect(byPlan["\(home)/.claude-work"] == "me@x.com · Team")

        let byFolder = AccountLabels.disambiguated([
            account("\(home)/.claude", label: "Me", plan: "Pro"),
            account("\(home)/.claude-side", label: "me", plan: "Pro"),
        ], home: home)
        #expect(byFolder["\(home)/.claude"] == "Me · ~/.claude")
        #expect(byFolder["\(home)/.claude-side"] == "me · side")
    }

    @Test func clashingNicknamesUseTheEmail() {
        let labels = AccountLabels.disambiguated([
            account("\(home)/.claude", label: "Work", email: "a@x.com"),
            account("\(home)/.claude-2", label: "Work", email: "b@x.com"),
        ], home: home)
        #expect(labels["\(home)/.claude"] == "Work · a@x.com")
        #expect(labels["\(home)/.claude-2"] == "Work · b@x.com")
    }

    @Test func folderNames() {
        #expect(AccountLabels.folderName("\(home)/.claude-work", home: home) == "work")
        #expect(AccountLabels.folderName("\(home)/.claude", home: home) == "~/.claude")
        #expect(AccountLabels.folderName("/opt/claude", home: home) == "/opt/claude")
        #expect(AccountPathDisplay.abbreviated(home, home: home + "/") == "~")
    }

    @Test func rowsOnlyNameAccountsWhenThereIsMoreThanOne() {
        var model = SessionsPanelModel(sessions: [], accounts: [account("\(home)/.claude", label: "Personal")])
        #expect(!model.showsAccounts)
        model.accounts.append(account("\(home)/.claude-work", label: "Work", tracked: false))
        #expect(!model.showsAccounts)
        model.accounts.append(account("\(home)/.claude-side", label: "Side"))
        #expect(model.showsAccounts)
    }

    // MARK: Hook health

    @Test func hookHealthNamesTrackedAccountsWithoutHooksOnceHooksAreOn() {
        let accounts = [
            account("\(home)/.claude", label: "Personal", hooks: ClaudeHookStatus(hooksInstalled: true)),
            account("\(home)/.claude-work", label: "Work"),
            account("\(home)/.claude-old", label: "Old", tracked: false),
        ]
        let labels = AccountLabels.disambiguated(accounts, home: home)
        #expect(HookHealth.make(accounts: accounts, labels: labels, hooksEnabled: false).isHealthy)
        let health = HookHealth.make(accounts: accounts, labels: labels, hooksEnabled: true)
        #expect(health.accountsWithoutHooks == ["Work"])
        #expect(health.summary == "Hooks are missing in Work.")
        #expect(HookHealth(accountsWithoutHooks: ["A", "B"]).summary == "Hooks are missing in A and B.")
        #expect(HookHealth(accountsWithoutHooks: ["A", "B", "C"]).summary == "Hooks are missing in 3 accounts.")
    }

    @Test func theHooksSwitchSaysWhatIsTrue() {
        var model = SettingsPaneModel()
        #expect(model.hooksSummary == "Turn on Claude Code control first.")
        model.hookConsent = true
        model.installsAllowed = false
        #expect(model.hooksSummary == "Installing is off for this run (--no-install).")
        model.installsAllowed = true
        #expect(model.hooksSummary == "Off: no account has this app's hooks.")
        model.hooksEnabled = true
        let now = Date()
        func item(_ state: AccountSettingsItem.HookState, tracked: Bool = true) -> AccountSettingsItem {
            SettingsPaneItems.item(account("\(home)/.claude-\(UUID().uuidString)", label: "x", tracked: tracked,
                                           hooks: ClaudeHookStatus(hooksInstalled: state == .installed)),
                                   diskStatus: nil, nickname: nil, isRingShown: true, hooksEnabled: true,
                                   reading: nil, home: home, now: now)
        }
        model.accounts = [item(.installed), item(.notInstalled), item(.notInstalled, tracked: false)]
        #expect(model.hooksSummary == "Installed in 1 of 2 tracked accounts.")
        model.accounts = [item(.installed), item(.installed)]
        #expect(model.hooksSummary == "Installed in all 2 tracked accounts.")
    }

    @Test func accountHookStateReadsTheDiskFirst() {
        let summary = account("\(home)/.claude", label: "P")
        var missing = AccountHookStatus()
        missing.configDirExists = false
        #expect(SettingsPaneItems.hookState(summary, diskStatus: missing, hooksEnabled: true) == .missingFolder)
        var unreadable = AccountHookStatus()
        unreadable.configDirExists = true
        unreadable.settingsReadable = false
        #expect(SettingsPaneItems.hookState(summary, diskStatus: unreadable, hooksEnabled: true) == .unreadable)
        #expect(SettingsPaneItems.hookState(summary, diskStatus: nil, hooksEnabled: true) == .notInstalled)
        #expect(SettingsPaneItems.hookState(summary, diskStatus: nil, hooksEnabled: false) == .off)
        let untracked = account("\(home)/.claude-x", label: "X", tracked: false)
        #expect(SettingsPaneItems.hookState(untracked, diskStatus: nil, hooksEnabled: true) == .off)
    }

    @Test func accountItemsCarryNicknameIdentityAndLegacyHooks() {
        let summary = account("\(home)/.claude-work", label: "me@x.com", email: "me@x.com", plan: "Team",
                              hooks: ClaudeHookStatus(hooksInstalled: true, vibeNotchHooksPresent: true,
                                                      superpoweredVibeNotchHooksPresent: true))
        let item = SettingsPaneItems.item(summary, diskStatus: nil, nickname: "Work", isRingShown: false,
                                          hooksEnabled: true, reading: nil, home: home, now: Date())
        #expect(item.name == "Work")
        #expect(item.hasNickname)
        #expect(item.defaultName == "me@x.com")
        #expect(item.identity == "me@x.com · Team")
        #expect(item.folder == "~/.claude-work")
        #expect(item.legacyHooks == [.vibeNotch, .superpoweredVibeNotch])
        #expect(item.usageLine == "Not checked while its ring is off")
        #expect(item.hookProblem == nil)
    }

    @Test func usageLinesUseFiveHourAndWeekly() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let reading = ClaudeRingReading(windows: [
            .init(id: "session", usedFraction: 0.342), .init(id: "weekly_all", usedFraction: 0.12),
        ], updatedAt: now.addingTimeInterval(-240), status: .ok)
        #expect(SettingsUsageLine.text(for: reading, isRingShown: true, now: now) == "5-hour 34% · weekly 12% · 4m ago")
        #expect(SettingsUsageLine.text(for: ClaudeRingReading(status: .waitingForFirstReading), isRingShown: true, now: now)
            == "Waiting for the first reading")
        #expect(SettingsUsageLine.text(for: ClaudeRingReading(status: .signInNeeded("Run claude, then /login.")), isRingShown: true, now: now)
            == "Run claude, then /login.")
        #expect(SettingsUsageLine.text(for: ClaudeRingReading(status: .failed("timed out")), isRingShown: true, now: now)
            == "Usage check failed: timed out")
        #expect(SettingsUsageLine.text(for: nil, isRingShown: true, now: now) == "No reading yet")
        // Integration: a reading older than its threshold says so.
        var old = reading
        old.updatedAt = now.addingTimeInterval(-2 * 3600)
        #expect(SettingsUsageLine.text(for: old, isRingShown: true, now: now).hasSuffix(", stale"))
    }

    // MARK: Adding a folder

    @Test func homeAndEverythingAboveItIsNeverAnAccount() {
        #expect(AddFolderCheck.evaluate(path: home, home: home, entries: [".claude.json", "projects"], knownConfigDirs: [])
            == .reject("That's your home folder, or a folder above it. Choose a Claude Code config folder instead, such as ~/.claude-work."))
        #expect(AddFolderCheck.evaluate(path: "/Users", home: home, entries: [], knownConfigDirs: []) != .add)
        #expect(AddFolderCheck.evaluate(path: "/", home: home, entries: [], knownConfigDirs: []) != .add)
        #expect(AddFolderCheck.evaluate(path: home + "/", home: home, entries: [], knownConfigDirs: []) != .add)
    }

    @Test func aClaudeFolderIsAddedAndAnythingElseIsAskedAbout() {
        let folder = "\(home)/.claude-research"
        #expect(AddFolderCheck.evaluate(path: folder, home: home, entries: ["projects", "settings.json"], knownConfigDirs: []) == .add)
        #expect(AddFolderCheck.evaluate(path: folder, home: home, entries: ["sessions"], knownConfigDirs: []) == .add)
        // `.claude.json` alone isn't enough: the home folder has one.
        guard case .confirm = AddFolderCheck.evaluate(path: folder, home: home, entries: [".claude.json"], knownConfigDirs: []) else {
            Issue.record("expected a question")
            return
        }
        guard case .reject = AddFolderCheck.evaluate(path: folder, home: home, entries: ["projects"], knownConfigDirs: [folder + "/"]) else {
            Issue.record("expected a refusal for a known folder")
            return
        }
    }
}
