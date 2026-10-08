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

The sealed hub (`hub::sealed_fixture`) serves the sealed demo
(`hub::sealed_demo`: the fixture accounts, sessions and usage in the real
stores, through the hub's own projections), made at its clock's "now".
`snapshot.json` and `settings.json` are that demo made at their
`generated_at_ms`, and `chat.json` is its chat sample; `tests/hub_sealed_demo.rs`
holds them equal, also at any other time with the times moved: every
`*_at_ms` and `since_ms` field, and upstream's `resets_at` / `fetched_at`;
durations and labels stay as written. A `calls.json` entry's reply is what a
fresh sealed hub answers; `error` names the `CallError` code.

Made by WP0 (the data follows the Mac's `SampleSessions`); WP7 maintains
them. A change to the demo or to a projection that changes what they show
changes them, with the node tests that read them.

`events.json` also holds the glue's own events, which the hub never emits:
`an:panel_place` (a `model::PanelPlace`: where an open panel moved to, `edge`
null when it floats), `an:panel_state` (a `PanelState`, to the notch's page),
and an `an:panel` whose request carries those four placement fields beside its
own (`edge`, `floating`, `width`, `tail_offset`). `settings.json` and
`snapshot.json` carry `attention.hotkey_ok` / `hotkey_message` (the hot key
service's report; `hotkey_ok` reads `true` when a payload lacks it).
