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
`cloud-ledger.json` (`cloud::ledger::SessionLedger`, v2: sessions by
`<sessionId>|<accountKey>`, accounts, owners with a nobody stretch, and the
unattributed sightings) and `cloud-folder-logins.json`
(`cloud::folder_logins::CloudFolderLogins`, v1) follow it; both are made up. `cloud-summaries.json`
(`cloud::summary::store::SessionSummaryStore`, v2: summaries and failed
attempts by ledger entry key, run times, `enabledAt`) and
`cloud-usage-outbox.json` (`cloud::recorder::UsageHistoryRecorder`, v1:
pending readings and the last per `accountKey|source`, explicit null
`resetsAt`) are made up in the same encoding.
`cloud-sync-state.json` (`cloud::pass::CloudSyncMemory`, v2: what the website
has, by ledger entry key, with `ended`, the transcript's stamp and the
payload version; absent optionals left out) is made up in the same encoding.
