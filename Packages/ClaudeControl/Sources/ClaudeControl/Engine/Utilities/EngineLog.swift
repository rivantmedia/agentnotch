//
//  EngineLog.swift
//  ClaudeControl
//
//  One subsystem for everything the engine logs: the host's bundle
//  identifier from the frozen configuration, so
//  `log stream --predicate 'subsystem == "<bundle id>"'` shows the whole
//  engine, and a sealed build logs under its own sealed id. Loggers are made
//  on use (they are cheap), never cached in a `static let` that could capture
//  the identity before `ClaudeControlHub.bootstrap` froze it.
//

import Foundation
import os.log

nonisolated enum EngineLog {
    static var subsystem: String { AppIdentity.bundleIdentifier }

    static func logger(_ category: String) -> Logger {
        Logger(subsystem: subsystem, category: category)
    }

    /// A dispatch queue label under the same identity, e.g.
    /// `<bundle id>.claude.desktop-usage`.
    static func queueLabel(_ name: String) -> String {
        "\(subsystem).claude.\(name)"
    }
}
