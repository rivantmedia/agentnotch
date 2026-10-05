// A hook of the hermetic real-Claude-Code job's own (real-claude.ps1 writes it into the temporary
// settings.json beside the app's entries, once in string form and once in exec form): it appends
// what Claude Code handed the hook to a JSON-lines file. That is the evidence that Claude Code
// exports CLAUDE_PID and CLAUDE_CONFIG_DIR to its hooks on Windows, and that an exec-form entry
// (`command` + `args`) runs at all, for EXEC_FORM_MIN. It prints nothing and always exits 0, so
// it never changes what Claude Code does.
//
//   node env-probe.mjs <out.jsonl> <form>
import { appendFileSync } from 'node:fs';

const [out, form = 'unknown'] = process.argv.slice(2);
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (c) => { input += c; });
process.stdin.on('end', () => {
  let event = {};
  try {
    event = JSON.parse(input);
  } catch {
    // not JSON: recorded as such below
  }
  const line = {
    form,
    hook_event_name: event.hook_event_name ?? null,
    tool_name: event.tool_name ?? null,
    session_id: event.session_id ?? null,
    claude_pid: process.env.CLAUDE_PID ?? null,
    claude_config_dir: process.env.CLAUDE_CONFIG_DIR ?? null,
    entrypoint: process.env.CLAUDE_CODE_ENTRYPOINT ?? null,
    parent_pid: process.ppid,
  };
  try {
    if (out) appendFileSync(out, `${JSON.stringify(line)}\n`);
  } catch {
    // a hook of ours never fails Claude Code's turn
  }
  process.exit(0);
});
