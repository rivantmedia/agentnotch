# Protocol fixtures

- `v1/*.json`: hook → app frames of protocol 1 (hook events, status line,
  control). **Kept forever**: hook copies in run folders can be older than
  the app, so every later server must accept these
  (`agentnotch-engine/tests/ingress_protocol_compat.rs`). Never edit one; a
  new shape gets a new file.
- `stdin/*.json`: what Claude Code writes on the hook's stdin; the builder
  makes `v1/<same name>.json` of each (`tests/proto_messages.rs`).
- `v1-responses/*.json` + `*.stdout`: an app → hook response frame for a
  stdin sample, and the exact bytes the hook prints for it. The `.stdout`
  files were produced by the Mac hook script's own `permission_output` and
  `json.dumps`; an empty file means "print nothing".
