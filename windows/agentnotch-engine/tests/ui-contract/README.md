# UI contract fixtures

Shared by the Rust engine and the node tests of `ui/agentnotch/*`
(DESIGN-WIN §3.6, §7.6). Every field is present in every object (`null`
when absent); `tests/ui_contract.rs` deserialises each file into its Rust
type and requires the same keys and values back.

| File | Rust type | Pushed as |
|---|---|---|
| `snapshot.json` | `model::HubSnapshot` | `an:snapshot`, reply to `snapshot` |
| `settings.json` | `model::SettingsSnapshot` | `an:settings`, reply to `settings` |
| `chat.json` | `model::ChatUpdate` (a reset) | `an:chat` |
| `calls.json` | `hub::Call` + the sealed hub's reply | `invoke('an_call', {method, args})` |
| `events.json` | each `an:*` event's payload (`payload_fixture` names a file above) | |
| `rebrand-vectors.json` | `{input, expected}` for `core::rebrand` and `rebrand.js` | |

The sealed hub (`hub::sealed_fixture`) serves these files, with their times
shifted so `generated_at_ms` is "now": every `*_at_ms` and `since_ms` field,
and upstream's `resets_at` / `fetched_at`; durations stay as written. A
`calls.json` entry's reply is what a fresh sealed hub answers; `error` names
the `CallError` code.

Made by WP0 (the data follows the Mac's `SampleSessions`); WP7 maintains
them, and its sealed demo's snapshot must equal `snapshot.json`.
