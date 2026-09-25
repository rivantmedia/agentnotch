import os

/// An agent app has no window to print into, so anything worth diagnosing has
/// to go somewhere you can read it:
///
///     log stream --predicate 'subsystem == "com.rivantmedia.agentnotch"' --level debug
///
/// Fork: the subsystem is `Fork.logSubsystem`, not upstream's `com.vinz.codenotch`.
enum Log {
    static let usage = Logger(subsystem: Fork.logSubsystem, category: "usage") // Fork: own subsystem
    static let sessions = Logger(subsystem: Fork.logSubsystem, category: "sessions") // Fork: own subsystem
}
