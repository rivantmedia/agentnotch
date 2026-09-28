//! Sealed mode: a development run that shows fixture data and reaches
//! nothing real (§4.13): no pipe server, no reads or writes of Claude
//! folders or `<support>`, no network, no subprocess, no toasts. It fails
//! closed: any non-empty value other than `0`, `false`, `no` or `off` seals
//! the run, so a typo never starts a live run against the real profile.

/// The variables that seal a run. `CODENOTCH_DEMO` is upstream's
/// screenshot switch; the Mac fork seals on it too, so a demo launch never
/// shows (or touches) real data.
pub const ENV_KEYS: [&str; 2] = ["AGENTNOTCH_SAFE_MODE", "CODENOTCH_DEMO"];

/// Values that clearly mean "not sealed"; unset and empty are off too.
pub const OFF_VALUES: [&str; 4] = ["0", "false", "no", "off"];
/// Values that clearly mean "sealed".
pub const ON_VALUES: [&str; 4] = ["1", "true", "yes", "on"];

fn normalised(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_lowercase();
    (!value.is_empty()).then_some(value)
}

/// Whether the environment asks for a sealed run.
pub fn is_sealed(get_env: impl Fn(&str) -> Option<String>) -> bool {
    ENV_KEYS.iter().any(|key| {
        normalised(get_env(key)).is_some_and(|value| !OFF_VALUES.contains(&value.as_str()))
    })
}

/// The first variable set to something neither clearly on nor clearly off
/// (it seals the run, but is worth a warning in the log), as `KEY=value`.
pub fn unrecognised(get_env: impl Fn(&str) -> Option<String>) -> Option<String> {
    ENV_KEYS.iter().find_map(|key| {
        let raw = get_env(key);
        let value = normalised(raw.clone())?;
        (!OFF_VALUES.contains(&value.as_str()) && !ON_VALUES.contains(&value.as_str()))
            .then(|| format!("{key}={}", raw.unwrap_or_default()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn fails_closed() {
        assert!(!is_sealed(env(&[])));
        for off in ["", " ", "0", "false", "No", "OFF"] {
            let pairs: &'static [(&str, &str)] =
                Box::leak(Box::new([("AGENTNOTCH_SAFE_MODE", off)]));
            assert!(!is_sealed(env(pairs)), "{off:?}");
        }
        for on in ["1", "true", "YES", "on", "enabled", "2", "tru"] {
            let pairs: &'static [(&str, &str)] =
                Box::leak(Box::new([("AGENTNOTCH_SAFE_MODE", on)]));
            assert!(is_sealed(env(pairs)), "{on:?}");
        }
        assert!(is_sealed(env(&[("CODENOTCH_DEMO", "1")])));
    }

    #[test]
    fn odd_values_are_reported() {
        assert_eq!(
            unrecognised(env(&[("AGENTNOTCH_SAFE_MODE", "tru")])),
            Some("AGENTNOTCH_SAFE_MODE=tru".into())
        );
        assert_eq!(unrecognised(env(&[("AGENTNOTCH_SAFE_MODE", "1")])), None);
        assert_eq!(unrecognised(env(&[("AGENTNOTCH_SAFE_MODE", "off")])), None);
    }
}
