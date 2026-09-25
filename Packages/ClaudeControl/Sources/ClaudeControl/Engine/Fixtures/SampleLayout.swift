//
//  SampleLayout.swift
//  ClaudeControl
//
//  The fixture accounts as Claude Parallel Profiles lays them out, for
//  sealed runs and snapshots: two identities spread over `~/.claude`, three
//  account stores, three VS Code windows' working copies and the shared
//  history, which must come out as exactly two rings.
//
//  | folder                          | kind           | identity           |
//  |---------------------------------|----------------|--------------------|
//  | ~/.claude                       | run (default)  | me@personal.dev    |
//  | ~/.claude-windows/5d1e0a7b3c21  | run (window)   | me@personal.dev    |
//  | ~/.claude-windows/9b4f2e8d6a10  | run (window)   | me@company.com     |
//  | ~/.claude-windows/c07a3f5e1d94  | run (window)   | me@company.com     |
//  | ~/.claude-me                    | store          | me@personal.dev    |
//  | ~/.claude-company               | store          | me@company.com     |
//  | ~/.claude-company-team          | store          | me@company.com     |
//  | ~/.claude-shared                | infrastructure | —                  |
//
//  A third identity (`~/.claude-side`, a standalone folder) signs in a few
//  seconds into a sealed run. Nothing here is read from disk.
//

import Foundation

enum SampleLayout {
    static let personalUUID = "5b0f6f2e-7c1a-4d8e-9b3f-2a6c1d0e9f41"
    static let workUUID = "c3a9d2e7-4f18-4b6a-8e25-7d1f0c9b3a62"
    static let sideUUID = "e8d4b1c6-2a7f-4e93-b05d-9c3e6f1a8b27"

    static let personalWindow = "~/.claude-windows/5d1e0a7b3c21"
    static let workWindow = "~/.claude-windows/9b4f2e8d6a10"
    static let workSecondWindow = "~/.claude-windows/c07a3f5e1d94"
    static let personalStore = "~/.claude-me"
    static let workStore = "~/.claude-company"
    static let workSecondStore = "~/.claude-company-team"
    static let shared = "~/.claude-shared"

    /// A folder of the personal identity (`ClaudeAccount.sampleDefault`'s).
    static func personalFolder(_ dir: String, kind: ConfigDirKind) -> ClaudeAccount {
        var folder = ClaudeAccount(configDir: dir, configDirEnv: AccountPaths.normalize(dir), colorIndex: 0, kind: kind)
        folder.email = ClaudeAccount.sampleDefault.email
        folder.displayName = ClaudeAccount.sampleDefault.displayName
        folder.accountUuid = personalUUID
        folder.subscriptionType = ClaudeAccount.sampleDefault.subscriptionType
        folder.rateLimitTier = ClaudeAccount.sampleDefault.rateLimitTier
        return folder
    }

    /// A folder of the work identity (`ClaudeAccount.sampleWork`'s).
    static func workFolder(_ dir: String, kind: ConfigDirKind) -> ClaudeAccount {
        var folder = ClaudeAccount(configDir: dir, configDirEnv: AccountPaths.normalize(dir), colorIndex: 1, kind: kind)
        folder.email = ClaudeAccount.sampleWork.email
        folder.organizationName = ClaudeAccount.sampleWork.organizationName
        folder.accountUuid = workUUID
        folder.subscriptionType = ClaudeAccount.sampleWork.subscriptionType
        return folder
    }

    /// Every folder but the shared history, the two identities' own run
    /// folders (`SampleSessions.personal`, `.work`) first.
    static func folders(personal: ClaudeAccount, work: ClaudeAccount) -> [ClaudeAccount] {
        [
            personal,
            personalFolder(personalWindow, kind: .run),
            work,
            workFolder(workSecondWindow, kind: .run),
            personalFolder(personalStore, kind: .store),
            workFolder(workStore, kind: .store),
            workFolder(workSecondStore, kind: .store),
        ]
    }

    /// What classification says about the fixture folders.
    static var layout: ConfigDirLayout {
        let home = AccountPaths.homeDirectory
        var kinds: [String: ConfigDirKind] = [:]
        for dir in ["~/.claude", personalWindow, workWindow, workSecondWindow] { kinds[AccountPaths.normalize(dir)] = .run }
        for dir in [personalStore, workStore, workSecondStore] { kinds[AccountPaths.normalize(dir)] = .store }
        kinds[AccountPaths.normalize(shared)] = .infrastructure
        kinds[ParallelProfiles.windowsRoot(home: home)] = .infrastructure
        return ConfigDirLayout(home: home, kinds: kinds, extensionDetected: true)
    }
}

/// The fixture accounts' ring ids, for the app's sealed demo and snapshots.
@_spi(Sealed) public enum ClaudeFixtureRings {
    public static var personal: String { ClaudeRingIdentity.ringID(accountKey: SampleLayout.personalUUID) }
    public static var work: String { ClaudeRingIdentity.ringID(accountKey: SampleLayout.workUUID) }
    /// The account that signs in a few seconds into a sealed run.
    public static var side: String { ClaudeRingIdentity.ringID(accountKey: SampleLayout.sideUUID) }
}
