# Persisted-file fixtures

Made-up data in the encodings the Mac engine writes (Swift `JSONEncoder`:
`accounts.json` and `usage-state.json` pretty with sorted keys and ISO 8601
whole seconds; `review-state.json` compact with epoch-second doubles), and
the Windows-only `control-settings.json` and `hook-install.json` in the same
style. `tests/persist_mac_files.rs` reads each through `persist::*` and
writes it back JSON-equivalent. `review-state-spvn.json` is Superpowered Vibe
Notch's bare shape, which is still read.

The cloud files (`cloud-*.json`) are WP8's, with `cloud::files`, in the Mac's
`CloudJSON` encoding (compact, sorted keys, ISO 8601 with milliseconds);
`tests/cloud_mac_files.rs` round-trips each. `cloud-session.json` is the
website sign-in (`cloud::auth::AuthSession`; made-up tokens).
