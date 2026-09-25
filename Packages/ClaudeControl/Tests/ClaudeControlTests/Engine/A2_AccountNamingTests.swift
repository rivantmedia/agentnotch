import Foundation
import Testing
@testable import ClaudeControl

/// Two accounts never read the same by default: names follow Codenotch's
/// rule for Claude rings, and badges are told apart by more than colour.
struct AccountNamingTests {
    let home = AccountPaths.homeDirectory

    private func account(_ folder: String, email: String? = nil, org: String? = nil, plan: String? = nil,
                         custom: String? = nil) -> ClaudeAccount {
        ClaudeAccount(configDir: home + "/" + folder, configDirEnv: folder == ".claude" ? nil : home + "/" + folder,
                      customLabel: custom, email: email, organizationName: org, subscriptionType: plan)
    }

    private func names(_ accounts: [ClaudeAccount]) -> [String: AccountNaming.Names] {
        AccountNaming.assign(accounts)
    }

    @Test func codenotchsWordForAnAddress() {
        #expect(AccountNaming.domainLabel(forAddress: "someone@gmail.com") == "Gmail")
        #expect(AccountNaming.domainLabel(forAddress: "vinz@acme.co.uk") == "Acme")
        #expect(AccountNaming.domainLabel(forAddress: "a@b@acme.com") == "Acme")
        #expect(AccountNaming.domainLabel(forAddress: "someone@IBM.com") == "IBM")
        for address in [nil, "", "no-at-sign", "someone@", "someone@.com", "someone@123.45"] as [String?] {
            #expect(AccountNaming.domainLabel(forAddress: address) == nil)
        }
    }

    @Test func baseNames() {
        #expect(AccountNaming.baseLabel(for: account(".claude", email: "me@gmail.com")) == "Claude Gmail")
        #expect(AccountNaming.baseLabel(for: account(".claude")) == "Claude")
        #expect(AccountNaming.baseLabel(for: account(".claude-work")) == "Claude (work)")
        #expect(AccountNaming.baseLabel(for: account(".claude_side")) == "Claude (side)")
        #expect(AccountNaming.baseLabel(for: ClaudeAccount(configDir: "/Volumes/X/claude-profile")) == "Claude (claude-profile)")
        #expect(AccountNaming.baseLabel(for: account(".claude-x", email: "odd@123.45")) == "Claude odd@123.45")
    }

    @Test func distinctAccountsKeepTheShortName() {
        let personal = account(".claude", email: "paulo@gmail.com")
        let work = account(".claude-work", email: "paulo@acme.com")
        let result = names([personal, work])
        #expect(result[personal.id]?.label == "Claude Gmail")
        #expect(result[work.id]?.label == "Claude Acme")
        #expect(result[personal.id]?.monogram == "GM")
        #expect(result[work.id]?.monogram == "AC")
    }

    /// Two gmail logins fall back to the whole address, as in Codenotch.
    @Test func sameDomainFallsBackToTheAddress() {
        let one = account(".claude", email: "paulo@gmail.com")
        let two = account(".claude-work", email: "eureka@gmail.com")
        let result = names([one, two])
        #expect(result[one.id]?.label == "Claude paulo@gmail.com")
        #expect(result[two.id]?.label == "Claude eureka@gmail.com")
        #expect(result[one.id]?.monogram == "PA")
        #expect(result[two.id]?.monogram == "EU")
    }

    /// The same address in two organizations: the organization tells them apart.
    @Test func sameAddressInTwoOrganizations() {
        let personal = account(".claude", email: "me@x.com", plan: "max")
        let team = account(".claude-team", email: "me@x.com", org: "Acme Team", plan: "team")
        let result = names([personal, team])
        #expect(result[team.id]?.label == "Claude me@x.com · Acme Team")
        #expect(result[personal.id]?.label == "Claude me@x.com · Max")
        #expect(result[personal.id]?.monogram != result[team.id]?.monogram)
    }

    /// Nothing but the folder to go on, and even that equal: the path.
    @Test func sameFolderNameElsewhere() {
        let one = ClaudeAccount(configDir: home + "/.claude-work")
        let two = ClaudeAccount(configDir: "/Volumes/Backup/.claude-work")
        let result = names([one, two])
        #expect(result[one.id]?.label == "Claude (~/.claude-work)")
        #expect(result[two.id]?.label == "Claude (/Volumes/Backup/.claude-work)")
        #expect(result[one.id]?.monogram != result[two.id]?.monogram)
    }

    /// A default name never equals someone's custom one; custom names stay.
    @Test func customNamesStayAndTakePart() {
        let named = account(".claude-a", custom: "Claude Gmail")
        let plain = account(".claude", email: "me@gmail.com")
        let result = names([named, plain])
        #expect(result[named.id]?.label == "Claude Gmail")
        #expect(result[plain.id]?.label == "Claude me@gmail.com")
    }

    @Test func badgesNeverCollide() {
        let accounts = (0..<12).map { account(".claude-p\($0)", email: "person\($0)@gmail.com") }
        let result = names(accounts)
        let monograms = accounts.compactMap { result[$0.id]?.monogram }
        #expect(Set(monograms).count == accounts.count)
    }

    /// The registry names every account it publishes.
    @MainActor
    @Test func theRegistryAppliesTheNames() {
        let named = AccountRegistry.named([account(".claude", email: "a@gmail.com"), account(".claude-w", email: "b@gmail.com")])
        #expect(named.map(\.label).sorted() == ["Claude a@gmail.com", "Claude b@gmail.com"])
        #expect(Set(named.map(\.monogram)).count == 2)
        // An untracked account is named as if it joined the tracked ones.
        var hidden = account(".claude-h", email: "c@gmail.com")
        hidden.isHidden = true
        let withHidden = AccountRegistry.named([account(".claude", email: "a@acme.com"), hidden])
        #expect(withHidden.first { $0.isHidden }?.label == "Claude Gmail")
    }
}
