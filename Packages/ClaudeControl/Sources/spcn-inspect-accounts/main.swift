//
//  main.swift
//  spcn-inspect-accounts
//
//  Prints what Superpowered Codenotch makes of this Mac's Claude Code
//  folders: the accounts (one per signed-in identity) with the folders
//  Claude Code runs in and their Claude Parallel Profiles stores, the
//  infrastructure it ignores, where hooks would be installed, and what the
//  Superpowered Vibe Notch takeover would clean. Read-only (see
//  `ClaudeAccountInspection`): listings, link targets, the manifest, the
//  store marker's existence, and `oauthAccount` plus two fields of
//  `cachedUsageUtilization` from each `.claude.json`.
//
//      swift run --package-path Packages/ClaudeControl spcn-inspect-accounts [--home <dir>]
//

import ClaudeControl
import Foundation

var home = FileManager.default.homeDirectoryForCurrentUser.path
let arguments = CommandLine.arguments
if let index = arguments.firstIndex(of: "--home"), index + 1 < arguments.count {
    home = arguments[index + 1]
}
print(ClaudeAccountInspection.report(home: home))
