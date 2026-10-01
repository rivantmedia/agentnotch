// Drives one headless Claude Code session for the hermetic real-Claude-Code job (Maintainer
// decision Q3; real-claude.ps1 is the only caller, on the Windows CI runner only). It starts the
// pinned claude.exe in stream-json mode, sends the prompts one at a time (the next after the
// previous turn's `result`), and writes every line Claude Code prints to an events file. It never
// answers a permission itself: Claude Code races the PermissionRequest hooks against the
// `can_use_tool` request it prints here, and the hook (the app, answered from its panel) has to
// win. A state file tells the caller the process id and the session id as soon as they are known.
//
//   node drive-claude.mjs --claude <claude.exe> --cwd <dir> --events <file.jsonl> --state <file.json>
//        --prompt <text> [--prompt <text> ...] [--timeout <ms>] [--arg <claude argument> ...]
//
// The environment is the caller's, passed through unchanged (real-claude.ps1 builds it: the
// temporary profile, the fake key, the fake API's address). Prints one JSON summary line;
// exit 0 when every prompt got its result and Claude Code exited 0, else 1 (2: bad arguments).
//
// Why headless with `--permission-prompt-tool stdio` (read from Claude Code 2.1.285's bundle,
// never by running it): a print-mode session writes sessions\<pid>.json like an interactive one
// (the registration skips only child and spare sessions), and runs PermissionRequest hooks
// before the host is asked; AskUserQuestion and ExitPlanMode are enabled in print mode only when a
// permission prompt tool is named. `--include-hook-events` puts every hook's start and outcome
// in the stream.
import { spawn } from 'node:child_process';
import { appendFileSync, renameSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const BASE_ARGS = Object.freeze([
  '-p', '--input-format', 'stream-json', '--output-format', 'stream-json', '--verbose',
  '--include-hook-events', '--permission-prompt-tool', 'stdio',
]);

// One stream-json user message, as the Agent SDK sends it.
export function userMessage(text) {
  return `${JSON.stringify({ type: 'user', message: { role: 'user', content: [{ type: 'text', text }] }, parent_tool_use_id: null, session_id: '' })}\n`;
}

function writeJsonAtomically(file, value) {
  const temp = `${file}.tmp`;
  writeFileSync(temp, JSON.stringify(value));
  renameSync(temp, file);
}

export function driveClaude({ claude, args = [], cwd, env = process.env, prompts, eventsFile, stateFile, timeoutMs = 300000, spawnImpl = spawn }) {
  if (!prompts?.length) throw new Error('at least one prompt');
  return new Promise((done) => {
    const state = { pid: null, session_id: null, sent: 0, results: [], control_requests: 0, exit: null, signal: null, timedOut: false, error: null };
    const save = () => { if (stateFile) writeJsonAtomically(stateFile, state); };
    const record = (line) => { if (eventsFile) appendFileSync(eventsFile, `${line}\n`); };
    let child;
    try {
      child = spawnImpl(claude, [...BASE_ARGS, ...args], { cwd, env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
    } catch (e) {
      state.error = String(e.message ?? e);
      save();
      done(state);
      return;
    }
    state.pid = child.pid ?? null;
    save();
    const send = () => {
      child.stdin.write(userMessage(prompts[state.sent]));
      state.sent += 1;
      save();
    };
    const timer = setTimeout(() => {
      state.timedOut = true;
      save();
      child.kill();
    }, timeoutMs);

    let pending = '';
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      pending += chunk;
      let cut;
      while ((cut = pending.indexOf('\n')) >= 0) {
        const line = pending.slice(0, cut).replace(/\r$/, '');
        pending = pending.slice(cut + 1);
        if (!line.trim()) continue;
        record(line);
        let event;
        try {
          event = JSON.parse(line);
        } catch {
          continue;
        }
        if (event.type === 'system' && event.subtype === 'init' && event.session_id) {
          state.session_id = event.session_id;
          save();
        } else if (event.type === 'control_request') {
          // Left unanswered on purpose: the PermissionRequest hook must decide.
          state.control_requests += 1;
          save();
        } else if (event.type === 'result') {
          state.results.push({ subtype: event.subtype ?? null, is_error: event.is_error === true });
          if (state.sent < prompts.length) send();
          else {
            save();
            child.stdin.end();
          }
        }
      }
    });
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk) => { if (eventsFile) appendFileSync(`${eventsFile}.stderr.txt`, chunk); });
    child.stdin.on('error', () => { /* claude closed its stdin; the exit says why */ });
    child.on('error', (e) => { state.error = String(e.message ?? e); });
    child.on('close', (code, signal) => {
      clearTimeout(timer);
      state.exit = code;
      state.signal = signal;
      save();
      done(state);
    });
    send();
  });
}

export function succeeded(state, prompts) {
  return !state.error && !state.timedOut && state.exit === 0 && state.results.length === prompts && state.results.every((r) => !r.is_error);
}

function parse(argv) {
  const o = { prompts: [], args: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      if (i + 1 >= argv.length) throw new Error(`${a} needs a value`);
      return argv[++i];
    };
    if (a === '--claude') o.claude = next();
    else if (a === '--cwd') o.cwd = next();
    else if (a === '--events') o.events = next();
    else if (a === '--state') o.state = next();
    else if (a === '--prompt') o.prompts.push(next());
    else if (a === '--arg') o.args.push(next());
    else if (a === '--timeout') o.timeoutMs = Number(next());
    else throw new Error(`unknown argument ${a}`);
  }
  if (!o.claude || !o.cwd || !o.events || !o.state || !o.prompts.length) throw new Error('--claude, --cwd, --events, --state and --prompt are required');
  return o;
}

async function main(argv) {
  let o;
  try {
    o = parse(argv);
  } catch (e) {
    console.log(JSON.stringify({ error: String(e.message ?? e) }));
    return 2;
  }
  const state = await driveClaude({
    claude: o.claude, args: o.args, cwd: o.cwd, prompts: o.prompts, eventsFile: o.events, stateFile: o.state,
    ...(o.timeoutMs ? { timeoutMs: o.timeoutMs } : {}),
  });
  console.log(JSON.stringify(state));
  return succeeded(state, o.prompts.length) ? 0 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).then((c) => { process.exitCode = c; });
}
