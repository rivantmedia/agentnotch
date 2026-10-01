#!/usr/bin/env node
// Reads what Claude Code's packages say about hooks, the status line and the
// shell they run in, and writes the facts file the Windows engine compiles in
// (DESIGN-WIN §6.2, R1, R2).
//
//   node windows/scripts/check-claude-code-facts.mjs [--out <file>] [--cache <dir>]
//        [--versions a,b,c] [--offline]
//
// NOTHING FROM A CLAUDE CODE PACKAGE IS EVER RUN. `npm pack` downloads the
// tarball, the system `tar` unpacks one file, and this script reads that
// file's bytes as latin1 text and looks for fixed patterns. No import(),
// require(), eval or spawn ever touches the package's code. Node 22, no npm
// dependencies.
//
// Versions: the first and last stable release of every minor from 2.0.0 on,
// plus the newest (`selectVersions`), plus any version the existing output
// file already lists, so a boundary someone pinned by hand (the release that
// introduced exec form, say) stays checked. `--versions` replaces the whole
// list. A package that ships as a native binary (the main package has no
// cli.js; its `bin` is claude.exe) is read through its Windows platform
// package `@anthropic-ai/claude-code-win32-x64`, whose claude.exe embeds the
// same JavaScript.
//
// Every fact is true, false or null. null (unknown) is what a pattern that
// finds nothing yields, and the engine never counts it as support. false needs
// a positive anchor: the code that would carry the feature was found and
// doesn't. The bundles are minified, so no anchor uses a variable name; each
// is a string literal Claude Code ships for its users, then the structure
// around it.
//
// ANCHORS (each verified on the versions named; `npm pack` the version and
// look at the bytes to check one):
//
//  hook entry schema   the object that holds `type:"command"` and
//                      `command: ... .describe("Shell command to execute")`.
//                      Its keys at the top level of the object decide
//                      hook_exec_form (`args`), hook_shell_key (`shell`);
//                      whether it is a strict object (opener `strictObject(`
//                      or a following `.strict()`) decides
//                      hook_unknown_keys_rejected. If the object isn't found
//                      at all, hook_exec_form falls back to the user-facing
//                      warning "Exec form treats" (true), else null.
//                        2.0.0    v.object({type:v.literal("command")...})  keys type, command, timeout
//                        2.1.100  L.object({type:L.literal("command")...}) adds if, shell, async...
//                        2.1.285  d({type:A("command")...})                adds args, cloud...
//  statusLine schema   `statusLine:` followed by an object whose first keys are
//                      type:"command", command, and which is followed by
//                      `.optional().describe("Custom status line display
//                      configuration")`. A `shell` key among its keys decides
//                      status_line_shell_key.
//                        2.1.100, 2.1.285 verified (keys type, command, padding, refreshInterval, ...)
//  default shell       `.shell??X` (a hook entry's shell, else X). X is either
//                      a constant (`X="bash"`: 2.1.100) or a function
//                      `function X(){return Y()?"bash":"powershell"}` with the
//                      Git Bash install message nearby (2.1.285). Before
//                      `shell` existed (2.0.0) the hook is spawned with
//                      `shell:!0` (Node's own shell: true).
//  CLAUDE_PID          `function F(e){let n={CLAUDECODE:"1",...CLAUDE_PID:String(process.pid)}`
//                      (2.1.285) and, in the object that carries the hook's
//                      `CLAUDE_PROJECT_DIR:`, a spread `...F(`. false only when
//                      the hook environment is found (`CLAUDE_PROJECT_DIR:`
//                      next to `{...process.env` or `{...UR()`: 2.0.0,
//                      2.1.100) and the text `CLAUDE_PID` appears nowhere.
//  bash transform      the rewrite Claude Code applies to a command it runs
//                      through bash on Windows. 2.1.285: `if(W&&!P&&!S)c=F(c);`
//                      right before the CLAUDE_CODE_SHELL_PREFIX lookup, with `function F(e){...}`
//                      (unquote the first word, `.sh` gets `bash ` in front).
//                      2.1.100 to 2.1.140: the inline
//                      `if(D&&!f&&v.trim().match(/\.sh(\s|$|")/)){...bash ...}`
//                      (2.1.140 has one more `&&!x` term).
//
// The output (schema 1) is read by agentnotch-engine's hooks::facts.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const SCHEMA = 1;
export const GENERATOR = 'windows/scripts/check-claude-code-facts.mjs';
export const MAIN_PACKAGE = '@anthropic-ai/claude-code';
export const NATIVE_PACKAGE = '@anthropic-ai/claude-code-win32-x64';
/** The per-version keys, in the order they are written (WP2's field names). */
export const VERSION_KEYS = [
  'version',
  'hook_exec_form',
  'hook_shell_key',
  'hook_unknown_keys_rejected',
  'status_line_shell_key',
  'windows_default_shell',
  'claude_pid_exported',
  'bash_transform',
];
export const TOP_KEYS = ['schema', 'generator', 'generated_at', 'versions', 'exec_form_min', 'exec_form_evidence'];
export const BASH_TRANSFORM_MAX = 400;
export const BASH_TRANSFORM_NOTE = '[as read from the package; at most 400 characters; token-like text removed] ';

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, '..', '..');
export const DEFAULT_OUT = join(REPO, 'windows', 'agentnotch-engine', 'tests', 'fixtures', 'claude-code-facts.json');

// ---------------------------------------------------------------- versions

/** [major, minor, patch] of a plain release ("2.1.285"); null for anything else (a pre-release, a tag). */
export function parseVersion(text) {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(String(text));
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

export function compareVersions(a, b) {
  const x = parseVersion(a);
  const y = parseVersion(b);
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] - y[i];
  return 0;
}

/** The first and last release of every minor from 2.0.0 on, and the newest; pre-releases are ignored. */
export function selectVersions(all) {
  const stable = [...new Set(all)].filter((v) => {
    const p = parseVersion(v);
    return p && p[0] >= 2;
  });
  stable.sort(compareVersions);
  const byMinor = new Map();
  for (const v of stable) {
    const [major, minor] = parseVersion(v);
    const key = `${major}.${minor}`;
    const entry = byMinor.get(key) ?? { first: v, last: v };
    entry.last = v;
    byMinor.set(key, entry);
  }
  const picked = new Set();
  for (const { first, last } of byMinor.values()) {
    picked.add(first);
    picked.add(last);
  }
  if (stable.length > 0) picked.add(stable[stable.length - 1]);
  return [...picked].sort(compareVersions);
}

// ---------------------------------------------------------------- reading text

/** Every index of `needle` in `text`. */
function* indexesOf(text, needle) {
  let i = text.indexOf(needle);
  while (i !== -1) {
    yield i;
    i = text.indexOf(needle, i + 1);
  }
}

/** Index of the quote that closes the string literal opening at `start`, or -1. */
function skipString(text, start, end) {
  const quote = text[start];
  for (let i = start + 1; i < end; i++) {
    const c = text[i];
    if (c === '\\') i++;
    else if (c === quote) return i;
  }
  return -1;
}

/** Index of the bracket that closes the one at `start` (strings are skipped), or -1. */
export function matchingBracket(text, start, limit = 60000) {
  const end = Math.min(text.length, start + limit);
  let depth = 0;
  for (let i = start; i < end; i++) {
    const c = text[i];
    if (c === '"' || c === "'" || c === '`') {
      i = skipString(text, i, end);
      if (i === -1) return -1;
    } else if (c === '{' || c === '(' || c === '[') depth++;
    else if (c === '}' || c === ')' || c === ']') {
      depth--;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/** The keys written directly inside an object literal's braces (`text[open]` is the `{`); null when it never closes. */
export function topLevelKeys(text, open) {
  const close = matchingBracket(text, open);
  if (close === -1) return null;
  const keys = [];
  let expectKey = true;
  for (let i = open + 1; i < close; i++) {
    const c = text[i];
    if (c === '"' || c === "'" || c === '`') {
      if (expectKey) {
        const m = /^(["'])([^"']*)\1:/.exec(text.slice(i, i + 80));
        if (m) keys.push(m[2]);
        expectKey = false;
      }
      i = skipString(text, i, close);
      if (i === -1) return null;
    } else if (c === '{' || c === '(' || c === '[') {
      expectKey = false;
      i = matchingBracket(text, i);
      if (i === -1 || i > close) return null;
    } else if (c === ',') expectKey = true;
    else if (expectKey) {
      const m = /^([\w$]+):/.exec(text.slice(i, i + 80));
      if (m) {
        keys.push(m[1]);
        i += m[1].length;
      }
      expectKey = false;
    }
  }
  return { keys, close };
}

// ---------------------------------------------------------------- the facts

const HOOK_ENTRY =
  /([\w$.]+)\(\{type:[\w$.]+\("command"\)(?:\.describe\((?:"[^"]*"|'[^']*')\))?,command:[\w$.]+\(\)\.describe\("Shell command to execute"\)$/;

/**
 * The hook entry schema: its top-level keys and whether it is a strict object,
 * or null when it isn't found. `strict` is true or false only when the closing
 * of the object was found too.
 */
export function hookEntrySchema(text) {
  const literal = '"Shell command to execute")';
  for (const at of indexesOf(text, literal)) {
    const from = Math.max(0, at - 700);
    const window = text.slice(from, at + literal.length);
    const m = HOOK_ENTRY.exec(window);
    if (!m) continue;
    const opener = from + m.index + m[1].length + 1; // the `{`
    const parsed = topLevelKeys(text, opener);
    if (!parsed) continue;
    const strict = /strictObject$/.test(m[1]) || text.startsWith(').strict()', parsed.close + 1);
    return { keys: parsed.keys, strict };
  }
  return null;
}

/** Whether hook entries accept `args` and run them without a shell (exec form). */
export function hookExecForm(text, schema = hookEntrySchema(text)) {
  if (schema) return schema.keys.includes('args');
  return text.includes('Exec form treats') ? true : null;
}

export function hookShellKey(text, schema = hookEntrySchema(text)) {
  return schema ? schema.keys.includes('shell') : null;
}

/** true when the hook entry schema refuses a key it doesn't know (a strict object), false when it drops it. */
export function hookUnknownKeysRejected(text, schema = hookEntrySchema(text)) {
  return schema ? schema.strict : null;
}

/** Whether the `statusLine` setting has a `shell` key. */
export function statusLineShellKey(text) {
  const tail = '.optional().describe("Custom status line display configuration")';
  for (const at of indexesOf(text, tail)) {
    const from = Math.max(0, at - 3000);
    const window = text.slice(from, at);
    const start = window.lastIndexOf('statusLine:');
    if (start === -1) continue;
    const m = /^statusLine:(?:\(\)=>)?[\w$.]*\(\{type:[\w$.]+\("command"\),command:/.exec(window.slice(start));
    if (!m) continue;
    const open = from + start + m[0].indexOf('({') + 1;
    const parsed = topLevelKeys(text, open);
    if (parsed && text.startsWith(')', parsed.close + 1) && text.startsWith(tail, parsed.close + 2)) {
      return parsed.keys.includes('shell');
    }
  }
  return null;
}

const SHELL_DEFAULT_BOTH = 'bash when Git Bash is found, else powershell';
const SHELL_DEFAULT_BASH = 'bash';
const SHELL_DEFAULT_NODE = "the system's default shell (spawned with shell: true)";

/** What a hook command string (and the status line) runs in on Windows when the entry names no shell. */
export function windowsDefaultShell(text) {
  for (const at of indexesOf(text, '.shell??')) {
    const m = /^\.shell\?\?([\w$]+)(\(\))?/.exec(text.slice(at, at + 60));
    if (!m) continue;
    const name = m[1].replaceAll('$', '\\$');
    if (m[2]) {
      // function X(){return Y()?"bash":"powershell"}, with the Git Bash install message beside it
      const def = new RegExp(`function ${name}\\(\\)\\{return [\\w$.]+\\(\\)\\?"bash":"powershell"\\}`).exec(text);
      if (def && text.includes('CLAUDE_CODE_GIT_BASH_PATH')) return SHELL_DEFAULT_BOTH;
    } else if (new RegExp(`[,;\\s]${name}="bash"[;,]`).test(text)) {
      return SHELL_DEFAULT_BASH;
    }
  }
  // No `shell` key yet: the hook is spawned by Node with `shell:!0`, a few tokens after its CLAUDE_PROJECT_DIR.
  for (const at of indexesOf(text, 'CLAUDE_PROJECT_DIR:')) {
    const window = text.slice(Math.max(0, at - 120), at + 600);
    if (/\[\],\{env:(?:[\w$]+|\{[^{}]*\}),cwd:[\w$.()]+,shell:!0\}/.test(window)) return SHELL_DEFAULT_NODE;
  }
  return null;
}

/** Whether `CLAUDE_PID` is in the environment a hook gets. */
export function claudePidExported(text) {
  const exported = 'CLAUDE_PID:String(process.pid)';
  for (const at of indexesOf(text, exported)) {
    const window = text.slice(Math.max(0, at - 400), at + exported.length);
    const m = /function ([\w$]+)\([\w$]+\)\{let [\w$]+=\{CLAUDECODE:"1",[^{}]*$/.exec(window);
    if (!m) continue;
    const spread = `...${m[1]}(`;
    for (const env of indexesOf(text, 'CLAUDE_PROJECT_DIR:')) {
      if (text.slice(Math.max(0, env - 260), env).includes(spread)) return true;
    }
  }
  // Not exported: the hook environment is there (the project dir next to its base) and the name appears nowhere.
  if (!text.includes('CLAUDE_PID')) {
    for (const env of indexesOf(text, 'CLAUDE_PROJECT_DIR:')) {
      if (/\{\.\.\.[\w$.]+(?:\(\))?,$/.test(text.slice(Math.max(0, env - 40), env))) return false;
    }
  }
  return null;
}

/** Text that must never reach the committed file: anything that reads like a credential or its plumbing. */
const SECRET_LIKE =
  /[\w.-]*(?:oauth|credential|secret|token|keychain|auth\s+login|api_?key|\.key\b|cred(?:read|enum)|renewal|renew\(|start_login|claude_auth|usage::start|secitem|ksec|generic-password|claudeusagecli|claudeprofile)[\w.-]*/gi;

/** The text with every secret-like word replaced; `changed` tells whether anything was. */
export function scrub(text) {
  let changed = false;
  const out = text.replace(SECRET_LIKE, () => {
    changed = true;
    return '[removed]';
  });
  return { text: out, changed };
}

/** The code that rewrites a command Claude Code runs through bash on Windows, as read (≤ 400 characters, scrubbed). */
export function bashTransform(text) {
  let found = null;
  // The rewrite as a function called right before the shell prefix is applied (2.1.213, 2.1.285).
  const call = /if\([\w$]+&&![\w$]+&&![\w$]+\)[\w$]+=([\w$]+)\([\w$]+\);let [\w$]+=[^;]{0,100}$/;
  for (const at of indexesOf(text, 'CLAUDE_CODE_SHELL_PREFIX')) {
    const m = call.exec(text.slice(Math.max(0, at - 200), at));
    if (!m) continue;
    const name = m[1].replaceAll('$', '\\$');
    const def = new RegExp(`function ${name}\\([\\w$]+\\)\\{`).exec(text);
    if (!def) continue;
    const open = def.index + def[0].length - 1;
    const close = matchingBracket(text, open, 4000);
    if (close === -1) continue;
    found = text.slice(def.index, close + 1);
    break;
  }
  // The same rewrite inline (2.1.100): `if(win&&!powershell&&cmd.trim().match(/\.sh(\s|$|")/)){...}`.
  if (found === null) {
    const inline = /if\([\w$]+(?:&&![\w$]+)+&&[\w$]+\.trim\(\)\.match\(\/\\\.sh/g;
    const m = inline.exec(text);
    if (m) {
      const open = text.indexOf('{', m.index);
      const close = open === -1 ? -1 : matchingBracket(text, open, 2000);
      if (close !== -1) found = text.slice(m.index, close + 1);
    }
  }
  if (found === null) return null;
  const scrubbed = scrub(found).text;
  const clipped = scrubbed.length > BASH_TRANSFORM_MAX ? scrubbed.slice(0, BASH_TRANSFORM_MAX) : scrubbed;
  return BASH_TRANSFORM_NOTE + clipped;
}

/** All facts of one bundle's text, in the written key order (after `version`). */
export function analyze(version, text) {
  const schema = hookEntrySchema(text);
  return {
    version,
    hook_exec_form: hookExecForm(text, schema),
    hook_shell_key: hookShellKey(text, schema),
    hook_unknown_keys_rejected: hookUnknownKeysRejected(text, schema),
    status_line_shell_key: statusLineShellKey(text),
    windows_default_shell: windowsDefaultShell(text),
    claude_pid_exported: claudePidExported(text),
    bash_transform: bashTransform(text),
  };
}

// ---------------------------------------------------------------- the output file

/** The file's content: stable key order, 2-space indent, trailing newline. */
export function buildOutput({ versions, generatedAt, execFormMin = null, execFormEvidence = null }) {
  const sorted = [...versions].sort((a, b) => compareVersions(a.version, b.version));
  const ordered = sorted.map((entry) => Object.fromEntries(VERSION_KEYS.map((k) => [k, entry[k] ?? null])));
  const doc = {
    schema: SCHEMA,
    generator: GENERATOR,
    generated_at: generatedAt,
    versions: ordered,
    exec_form_min: execFormMin,
    exec_form_evidence: execFormEvidence,
  };
  return `${JSON.stringify(doc, null, 2)}\n`;
}

/** The UTC date, `YYYY-MM-DD`. */
export function utcDate(now = new Date()) {
  return now.toISOString().slice(0, 10);
}

// ---------------------------------------------------------------- npm and tar (nothing from a package runs)

function sh(file, args, options = {}) {
  return execFileSync(file, args, { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'], ...options });
}

function tarballName(pkg, version) {
  return `${pkg.replace(/^@/, '').replace('/', '-')}-${version}.tgz`;
}

/** The tarball of `pkg@version` in the cache, packed with npm when missing (never when offline); null when it isn't there. */
function fetchTarball(pkg, version, cache, offline) {
  const file = join(cache, tarballName(pkg, version));
  if (existsSync(file)) return file;
  if (offline) return null;
  sh('npm', ['pack', `${pkg}@${version}`, '--pack-destination', cache, '--ignore-scripts', '--silent']);
  return existsSync(file) ? file : null;
}

function tarList(file) {
  return sh('tar', ['-tzf', file]).split('\n').filter(Boolean);
}

/** One member of a tarball as latin1 text (unpacked into a temporary folder, read, and deleted). */
function readMember(file, member) {
  const dir = mkdtempSync(join(tmpdir(), 'claude-facts-'));
  try {
    sh('tar', ['-xzf', file, '-C', dir, member]);
    return readFileSync(join(dir, member)).latin1Slice();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/** The text of the JavaScript of one version: cli.js, or the JavaScript embedded in claude.exe of the Windows package. */
export function readBundle(version, cache, offline) {
  const main = fetchTarball(MAIN_PACKAGE, version, cache, offline);
  if (!main) return null;
  const members = tarList(main);
  const cli = members.find((m) => m.endsWith('/cli.js'));
  if (cli) return { text: readMember(main, cli), source: 'cli.js' };
  const native = fetchTarball(NATIVE_PACKAGE, version, cache, offline);
  if (!native) return null;
  const exe = tarList(native).find((m) => m.endsWith('.exe'));
  if (!exe) return null;
  return { text: readMember(native, exe), source: exe.split('/').pop() };
}

function listPublished() {
  const parsed = JSON.parse(sh('npm', ['view', MAIN_PACKAGE, 'versions', '--json']));
  return Array.isArray(parsed) ? parsed : [parsed];
}

function listCached(cache) {
  const prefix = tarballName(MAIN_PACKAGE, '').slice(0, -'.tgz'.length);
  return readdirSync(cache)
    .filter((f) => f.startsWith(prefix) && f.endsWith('.tgz'))
    .map((f) => f.slice(prefix.length, -'.tgz'.length))
    .filter((v) => parseVersion(v)); // the platform packages share the prefix
}

// ---------------------------------------------------------------- main

function parseArgs(argv) {
  const opts = { out: DEFAULT_OUT, cache: null, versions: null, offline: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new Error(`${arg} needs a value`);
      return argv[++i];
    };
    if (arg === '--out') opts.out = resolve(value());
    else if (arg === '--cache') opts.cache = resolve(value());
    else if (arg === '--versions') opts.versions = value().split(',').map((v) => v.trim()).filter(Boolean);
    else if (arg === '--offline') opts.offline = true;
    else throw new Error(`unknown option ${arg}`);
  }
  if (opts.versions) {
    const bad = opts.versions.filter((v) => !parseVersion(v));
    if (bad.length) throw new Error(`not a release version: ${bad.join(', ')}`);
  }
  return opts;
}

/** What a previous output file says that this run keeps: the versions it lists and its explicit minimum. */
function readExisting(file) {
  try {
    const doc = JSON.parse(readFileSync(file, 'utf8'));
    return {
      versions: (doc.versions ?? []).map((v) => v.version).filter((v) => parseVersion(v)),
      execFormMin: doc.exec_form_min ?? null,
      execFormEvidence: doc.exec_form_evidence ?? null,
    };
  } catch {
    return { versions: [], execFormMin: null, execFormEvidence: null };
  }
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  const cache = opts.cache ?? mkdtempSync(join(tmpdir(), 'claude-facts-cache-'));
  mkdirSync(cache, { recursive: true });
  const existing = readExisting(opts.out);

  let wanted;
  if (opts.versions) wanted = opts.versions;
  else {
    const known = opts.offline ? listCached(cache) : listPublished();
    wanted = [...new Set([...selectVersions(known), ...existing.versions.filter((v) => known.includes(v))])];
  }
  wanted.sort(compareVersions);

  const versions = [];
  for (const version of wanted) {
    let bundle = null;
    try {
      bundle = readBundle(version, cache, opts.offline);
    } catch (error) {
      // A version npm no longer serves, or a package that can't be unpacked: say so and go on.
      console.error(`${version}: ${String(error.message).split('\n')[0]}`);
    }
    if (!bundle) {
      console.error(`${version}: no readable package${opts.offline ? ' in the cache (offline)' : ''}; left out`);
      continue;
    }
    const facts = analyze(version, bundle.text);
    versions.push(facts);
    console.error(`${version} (${bundle.source}): ${JSON.stringify({ ...facts, version: undefined, bash_transform: facts.bash_transform ? '…' : null })}`);
  }
  if (versions.length === 0) throw new Error('no version could be read');

  const body = buildOutput({
    versions,
    generatedAt: utcDate(),
    execFormMin: existing.execFormMin,
    execFormEvidence: existing.execFormEvidence,
  });
  mkdirSync(dirname(opts.out), { recursive: true });
  writeFileSync(opts.out, body);
  console.error(`wrote ${opts.out}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(`check-claude-code-facts: ${error.message}`);
    process.exit(1);
  }
}
