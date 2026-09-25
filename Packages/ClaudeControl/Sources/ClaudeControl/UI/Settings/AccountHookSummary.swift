//
//  AccountHookSummary.swift
//  ClaudeControl
//
//  One account's hooks at a glance: what its settings.json holds and, when
//  our hooks aren't there, why, in words that say what the user loses (its
//  sessions still show, but without approvals or "done").
//

import Foundation

nonisolated struct AccountHookSummary: Equatable, Sendable {
    enum Kind: Equatable, Sendable {
        case installed
        case notInstalled
        case unreadable
        case missingFolder
        /// Not tracked, and none of our hooks left behind.
        case hidden
        /// Not read yet.
        case unknown
    }

    let kind: Kind
    /// Short label, e.g. "Hooks installed".
    let title: String
    /// Longer explanation, if any.
    let detail: String?

    static func make(
        status: AccountHookStatus?,
        hooksEnabled: Bool,
        installsDisabled: Bool,
        isHidden: Bool = false
    ) -> AccountHookSummary {
        if isHidden && status?.hooksInstalled != true {
            return AccountHookSummary(kind: .hidden, title: "Not tracked", detail: nil)
        }
        guard let status else {
            return AccountHookSummary(kind: .unknown, title: "Checking hooks…", detail: nil)
        }
        if !status.configDirExists {
            return AccountHookSummary(kind: .missingFolder, title: "Folder missing", detail: "The folder doesn't exist any more.")
        }
        if !status.settingsReadable {
            return AccountHookSummary(
                kind: .unreadable,
                title: "settings.json unreadable",
                detail: "settings.json isn't valid JSON, so it is left untouched. Fix it, then reinstall."
            )
        }
        if status.hooksInstalled {
            let statusLine = status.statusLineInstalled ? "live status line on" : "live status line off"
            return AccountHookSummary(kind: .installed, title: "Hooks installed", detail: "Hooks installed · \(statusLine)")
        }
        let why: String
        if installsDisabled {
            why = "Installing is off for this run (--no-install)."
        } else if !hooksEnabled {
            why = "Hooks are turned off (see Hooks below)."
        } else if let error = status.lastError, !error.isEmpty {
            why = error
        } else {
            // They are found through Claude Code's own session files.
            why = "Its sessions still show; answer their prompts where Claude Code runs (VS Code or the terminal) until the hooks are in."
        }
        return AccountHookSummary(kind: .notInstalled, title: "Hooks not installed", detail: why)
    }
}
