// Runs hook and status line commands the way Claude Code does, so the smoke test proves the
// entries the installer wrote and not a reconstruction of them:
//   exec form    (entry has `args`): child_process.spawn(command, args, {windowsHide: true})
//   string form  (a `command` string): `bash -c <command>` and `powershell -NoProfile -Command <command>`
//                (an entry with a `shell` key runs only in that shell)
//
//   node run-hook.mjs (--settings <settings.json> --event <Name> [--index N]
//                      | --settings <settings.json> --status-line
//                      | --command <string> | --exec <file> [--arg <a>]...)
//        [--shell bash|powershell|both] [--bash <path>] [--powershell <path>]
//        [--stdin <file>] [--env K=V]... [--cwd <dir>] [--timeout <ms>]
//
// Prints {"runs":[{source, shell, command, args, exit, signal, timedOut, ms, stdout, stdout_b64,
// stderr}]} and exits 0 whatever the commands did: the verdict is the caller's. A shell that is
// not installed gives {skipped: true, reason}.
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const WIN = process.platform === 'win32';
export const DEFAULTS = {
  bash: WIN ? `${process.env.ProgramFiles ?? 'C:\\Program Files'}\\Git\\bin\\bash.exe` : 'bash',
  powershell: WIN ? 'powershell' : 'pwsh',
  timeoutMs: 10_000,
};

// What Claude Code gives a hook is set per run (--env); a copy inherited from whatever started
// this script would answer for the wrong session.
const NOT_INHERITED = ['CLAUDE_PID', 'CLAUDE_CONFIG_DIR', 'CLAUDE_CODE_ENTRYPOINT'];
function childEnv(extra) {
  const env = { ...process.env };
  for (const k of NOT_INHERITED) delete env[k];
  return { ...env, ...extra };
}

export function runProcess(file, args, { stdin, env, cwd, timeoutMs = DEFAULTS.timeoutMs } = {}) {
  return new Promise((done) => {
    const t0 = Date.now();
    const out = [];
    const err = [];
    let timedOut = false;
    let settled = false;
    const finish = (r) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      const stdout = Buffer.concat(out);
      done({
        exit: null, signal: null, timedOut, ms: Date.now() - t0,
        stdout: stdout.toString('utf8'), stdout_b64: stdout.toString('base64'),
        stderr: Buffer.concat(err).toString('utf8'), ...r,
      });
    };
    const child = spawn(file, args, { windowsHide: true, env: childEnv(env), cwd, stdio: ['pipe', 'pipe', 'pipe'] });
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill();
    }, timeoutMs);
    child.on('error', (e) => finish(e.code === 'ENOENT' ? { skipped: true, reason: `${file} not found` } : { error: String(e.message) }));
    child.stdout.on('data', (c) => out.push(c));
    child.stderr.on('data', (c) => err.push(c));
    child.stdin.on('error', () => {}); // the command may exit without reading its input
    child.stdin.end(stdin ?? '');
    child.on('close', (exit, signal) => finish({ exit, signal }));
  });
}

// Hook entries of one event, flattened: [{command, args?, shell?, timeout?}] in file order.
export function hookEntries(settings, event) {
  return (settings?.hooks?.[event] ?? []).flatMap((group) => (group.hooks ?? []).filter((h) => h.type === 'command' || h.command));
}

// The processes one entry turns into, as {shell, file, args}.
export function plan(entry, { shell = 'both', bash = DEFAULTS.bash, powershell = DEFAULTS.powershell } = {}) {
  if (Array.isArray(entry.args)) return [{ shell: 'exec', file: entry.command, args: entry.args.map(String) }];
  const shells = entry.shell ? [entry.shell] : shell === 'both' ? ['bash', 'powershell'] : [shell];
  return shells.map((s) => (s === 'powershell'
    ? { shell: s, file: powershell, args: ['-NoProfile', '-Command', entry.command] }
    : { shell: 'bash', file: bash, args: ['-c', entry.command] }));
}

export async function runEntry(entry, options = {}, source = 'literal') {
  const runs = [];
  for (const p of plan(entry, options)) {
    const r = await runProcess(p.file, p.args, options);
    runs.push({ source, shell: p.shell, command: entry.command, ...(Array.isArray(entry.args) ? { args: entry.args } : {}), ...r });
  }
  return runs;
}

export async function runFromSettings(settingsFile, { event, index, statusLine = false, ...options }) {
  // A Windows editor's settings.json starts with a BOM, which Claude Code reads past and JSON.parse does not.
  const settings = JSON.parse(readFileSync(settingsFile, 'utf8').replace(/^\uFEFF/, ''));
  let picked;
  if (statusLine) {
    if (!settings.statusLine?.command) throw new Error(`${settingsFile} has no statusLine command`);
    picked = [[settings.statusLine, 'statusLine']];
  } else {
    const entries = hookEntries(settings, event);
    if (entries.length === 0) throw new Error(`${settingsFile} has no ${event} hook entries`);
    if (index !== undefined && !entries[index]) throw new Error(`${event} has ${entries.length} entries, no index ${index}`);
    picked = entries.map((e, i) => [e, `${event}[${i}]`]).filter((_, i) => index === undefined || i === index);
  }
  const runs = [];
  for (const [entry, source] of picked) runs.push(...(await runEntry(entry, options, source)));
  return runs;
}

function parse(argv) {
  const o = { env: {}, args: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      if (i + 1 >= argv.length) throw new Error(`${a} needs a value`);
      return argv[++i];
    };
    if (a === '--settings') o.settings = next();
    else if (a === '--event') o.event = next();
    else if (a === '--index') o.index = Number(next());
    else if (a === '--status-line') o.statusLine = true;
    else if (a === '--command') o.command = next();
    else if (a === '--exec') o.exec = next();
    else if (a === '--arg') o.args.push(next());
    else if (a === '--shell') o.shell = next();
    else if (a === '--bash') o.bash = next();
    else if (a === '--powershell') o.powershell = next();
    else if (a === '--stdin') o.stdinFile = next();
    else if (a === '--cwd') o.cwd = next();
    else if (a === '--timeout') o.timeoutMs = Number(next());
    else if (a === '--env') {
      const kv = next();
      const eq = kv.indexOf('=');
      if (eq < 1) throw new Error(`--env wants K=V, got ${kv}`);
      o.env[kv.slice(0, eq)] = kv.slice(eq + 1);
    } else throw new Error(`unknown argument ${a}`);
  }
  if (o.shell && !['bash', 'powershell', 'both'].includes(o.shell)) throw new Error('--shell is bash, powershell or both');
  return o;
}

async function main(argv) {
  try {
    const o = parse(argv);
    const options = {
      shell: o.shell, bash: o.bash, powershell: o.powershell, cwd: o.cwd, timeoutMs: o.timeoutMs, env: o.env,
      stdin: o.stdinFile ? readFileSync(o.stdinFile) : undefined,
    };
    for (const k of Object.keys(options)) if (options[k] === undefined) delete options[k];
    let runs;
    if (o.settings) {
      if (!o.statusLine && !o.event) throw new Error('--settings needs --event or --status-line');
      runs = await runFromSettings(o.settings, { event: o.event, index: o.index, statusLine: o.statusLine, ...options });
    } else if (o.exec) runs = await runEntry({ command: o.exec, args: o.args }, options, 'exec');
    else if (o.command) runs = await runEntry({ command: o.command }, options, 'literal');
    else throw new Error('give --settings, --command or --exec');
    console.log(JSON.stringify({ runs }));
    return 0;
  } catch (e) {
    console.log(JSON.stringify({ error: String(e.message ?? e), runs: [] }));
    return 2;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).then((c) => { process.exitCode = c; });
}
