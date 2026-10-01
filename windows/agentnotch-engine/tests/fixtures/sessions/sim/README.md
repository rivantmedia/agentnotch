# Simulator scenarios

The scenarios of `Packages/ClaudeControl/DevTools/simulate-sessions.py`
(`permission`, `question`, `tasks`, `review`, `ratelimit`, `statusline`,
`registry`, `goal`, `bgagent`, `bgworkflow`, and the extra `burst`) as data.
`tests/sessions_e2e.rs` plays each file: every hook event and status line
becomes a real frame (`build_hook_message`, `encode_hook_message`,
`build_statusline_message`, `encode_frame`), goes through `MemoryTransport`,
is decoded by the test-only decoder in `tests/sessions_support/frames.rs`
(ingress is WP1's) and reaches the `SessionStore`, whose jobs (registry
reads, transcript syncs, chat loads, the review file's write) run in the
test, with the store's own clock (`Tick` at `next_deadline`, the registries
read every 3 s). `ratelimit` checks only the session side; usage is WP4's.

## A file

```json
{
  "scenario": "name", "about": "...", "python": "scenario_name",
  "after": ["other scenario"],        // played first, in the same app (statusline)
  "hook_latency_ms": 100,             // simulated time one hook takes (default 100)
  "sessions": [ ... ],
  "steps": [ ... ]
}
```

A session is `{"name", "account": "personal"|"work", "cwd", "title", "prefix"}`
(`personal` is `<root>/.claude`, `work` is `<root>/.claude-work`, both under a
temporary root). It gets a stand-in process (`FakeProcesses`), a transcript
with the Python's two opening lines and a registry file path. With `"count": N`
and `"accounts": [...]` it makes `N` sessions named `<name>0..`, a group
(`{i}` in `cwd` and `title` is the index).

## Steps

Every step may carry `"advance_ms"`: the clock moves on afterwards, ticking at
every deadline the store names and reading the registries every 3 s.

| `op` | keys | does |
|---|---|---|
| `hook` | `session`, `event`, `fields`, `mirror` | Claude Code's stdin is `hook_event_name`, `session_id`, `transcript_path`, `cwd`, `permission_mode` plus `fields` (the Python's `payload`). The environment is `CLAUDE_PID`, `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_SESSION_ATTENDED`, `CLAUDE_CODE_ENTRYPOINT`. A `PermissionRequest` stays held until an `answer`. Unless `mirror` is false the registry file follows like Claude Code's (SessionStart idle, UserPromptSubmit busy, Stop idle/busy/shell by its background tasks, StopFailure idle, a main-agent PermissionRequest waiting after 0.2 s). Then `hook_latency_ms` pass. |
| `status_line` | `session`, `used_5h`, `used_7d`, `context_pct`, `cost` | the status line wrapper's stdin and frame |
| `transcript` | `session`, `lines` | appends `{"assistant_turn": {text, context_tokens, tool_uses}}` or `{"tool_result": {id, text, structured}}` lines |
| `registry` | `session`, `status`, `waiting_for`, `kind`, `entrypoint`, `name`, `name_source` | writes `<config>/sessions/<pid>.json` |
| `answer` | `session`, `tool`, `agent`, `answer`, `hook_output_contains` | the user answers a held request (`answer` is the engine's `Answer` JSON); the hook gets its response frame over the transport and the text the hook exe would print is checked |
| `advance` | `advance_ms` | only moves the clock |
| `expect` | `session` or `group` (+`count`), checks | the Python's `EXPECT` lines |
| `each` | `group`, `turns`, `steps` | for each turn, for each session of the group, the steps with `{turn}` and `{i}` replaced (the burst) |

## Checks of an `expect`

`tracked`, `state` (`needs_you`, `failed`, `ready_for_review`, `working`,
`idle`), `bucket`, `reason` (`permission:Bash`, `question`,
`dialog:permission prompt`, `error:Rate limited`, or null), `phase`
(`waiting_for_approval:Bash`, `waiting_for_input`, `processing`, `idle`),
`account`, `tasks` (`"1/3"`), `active_label`, `context_pct`, `model`,
`title`, `registry_status`, `last_assistant_message`,
`last_assistant_message_starts_with`, `last_message`,
`background_description` (`"1 workflow"`), `completion_pending`, `pending`
(`["permission:Bash"]`), `always_allow`, `held` (hook connections still held
open), `hook_output`, `ready_for_review_news` and `needs_you_news` (how many
attention transitions the notification rule counted), `review_file` and
`review_file_message_starts_with` (the persisted `review-state.json`).
