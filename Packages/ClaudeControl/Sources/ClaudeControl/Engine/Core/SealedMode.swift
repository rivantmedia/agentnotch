import Foundation

/// Whether this process runs sealed: a development mode that shows fixture
/// data and reaches nothing real.
///
/// Sealed means no keychain access of any kind (not even listing item
/// attributes), no reads of Claude, Codex or other agents' sessions and
/// transcripts, no provider network calls, no `claude`/`codex` subprocesses,
/// and no hook server. It exists so an agent or a developer can launch the app
/// next to someone's real setup without touching it.
///
/// Turned on by either variable:
/// - `SPCN_SAFE_MODE` (Superpowered Codenotch)
/// - `CODENOTCH_DEMO` (upstream's screenshot mode, sealed in this fork)
///
/// It fails closed: any non-empty value other than `0`, `false`, `no` or
/// `off` (any case) seals the run, so `SPCN_SAFE_MODE=true` or `=yes` is
/// sealed too, and a typo never starts a live run against the real home.
/// A value other than `1`, `true` or `yes` is reported at launch.
///
/// `nonisolated` so the app's actors and synchronous code can ask it without
/// hopping to the main actor.
public nonisolated enum SealedMode {
    /// The environment variables that turn sealed mode on.
    public static let environmentKeys = ["SPCN_SAFE_MODE", "CODENOTCH_DEMO"]

    /// Pure, for tests: whether `environment` asks for sealed mode.
    public static func isOn(environment: [String: String]) -> Bool {
        environmentKeys.contains { key in
            guard let value = normalised(environment[key]) else { return false }
            return !offValues.contains(value)
        }
    }

    /// Values that clearly mean "not sealed". Unset or empty is off too.
    static let offValues: Set<String> = ["0", "false", "no", "off"]
    /// Values that clearly mean "sealed".
    static let onValues: Set<String> = ["1", "true", "yes", "on"]

    /// The first variable set to something neither clearly on nor clearly
    /// off (which seals the run, but is worth a warning), as `KEY=value`.
    static func unrecognised(environment: [String: String]) -> String? {
        for key in environmentKeys {
            guard let value = normalised(environment[key]) else { continue }
            if !offValues.contains(value) && !onValues.contains(value) {
                return "\(key)=\(environment[key] ?? "")"
            }
        }
        return nil
    }

    private static func normalised(_ value: String?) -> String? {
        guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased(),
              !value.isEmpty else { return nil }
        return value
    }

    /// Whether this process was launched sealed. Read once: the environment
    /// does not change under a running process.
    public static let isOn: Bool = isOn(environment: Foundation.ProcessInfo.processInfo.environment)
}
