// Tests for windows/scripts/check-claude-code-facts.mjs: `node --test windows/tools/tests/facts.test.mjs`.
//
// The snippets are cut down from the real bundles (the version each mirrors is
// named), with minified names changed, so the patterns are tested on the shapes
// they were derived from. For each fact: found and true, found and false, and
// nothing there (null: unknown is never "false").
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  BASH_TRANSFORM_MAX,
  BASH_TRANSFORM_NOTE,
  GENERATOR,
  SCHEMA,
  TOP_KEYS,
  VERSION_KEYS,
  analyze,
  bashTransform,
  buildOutput,
  claudePidExported,
  compareVersions,
  hookEntrySchema,
  hookExecForm,
  hookShellKey,
  hookUnknownKeysRejected,
  matchingBracket,
  parseVersion,
  scrub,
  selectVersions,
  statusLineShellKey,
  topLevelKeys,
  utcDate,
  windowsDefaultShell,
} from '../../scripts/check-claude-code-facts.mjs';

const SCRIPT = fileURLToPath(new URL('../../scripts/check-claude-code-facts.mjs', import.meta.url));

// ---- snippets, as the bundles have them ----

/** 2.0.0: three keys, no shell, spawned with shell:!0. */
const V200 =
  'RH5=v.object({type:v.literal("command").describe(\'Hook type (currently only "command" is supported)\'),' +
  'command:v.string().describe("Shell command to execute"),' +
  'timeout:v.number().positive().optional().describe("Timeout in seconds for this specific command")}),' +
  'TH5=v.object({matcher:v.string().optional()});' +
  'async function ht1(A,B){let G=jB(),W=InQ(Y,[],{env:{...process.env,CLAUDE_PROJECT_DIR:G},cwd:e0(),shell:!0}),J=NC1(W)}';

/** 2.1.100: shell key, no args; the default shell is a constant; the rewrite is inline. */
const V2100 =
  'var jm7,RZ6="bash";var rj8=y(()=>{jm7=["bash","powershell"]});' +
  'function vp5(){let q=L.object({type:L.literal("command").describe("Shell command hook type"),' +
  'command:L.string().describe("Shell command to execute"),if:oj8(),' +
  'shell:L.enum(jm7).optional().describe("Shell interpreter. Defaults to bash."),' +
  'timeout:L.number().positive().optional().describe("Timeout in seconds for this specific command"),' +
  'once:L.boolean().optional()}),K=L.object({type:L.literal("prompt").describe("LLM prompt hook type")});return{q,K}}' +
  'statusLine:L.object({type:L.literal("command"),command:L.string(),padding:L.number().optional(),' +
  'refreshInterval:L.number().min(1).optional().catch(void 0)}).optional().describe("Custom status line display configuration"),' +
  'async function ui8(q){let D=k1()==="windows",Z=q.shell??RZ6,f=Z==="powershell",v=q.command;' +
  'if(D&&!f&&v.trim().match(/\\.sh(\\s|$|")/)){if(!v.trim().startsWith("bash "))v=`bash ${v}`}' +
  'let R=!f&&process.env.CLAUDE_CODE_SHELL_PREFIX?kb8(process.env.CLAUDE_CODE_SHELL_PREFIX,v):v,' +
  'E=q.timeout?q.timeout*1000:B2,h={...UR(),CLAUDE_PROJECT_DIR:G(T)};}';

/** 2.1.285: args, shell, a function for the default shell, a function for the rewrite, CLAUDE_PID. */
const V2285 =
  'Or set CLAUDE_CODE_GIT_BASH_PATH to your bash.exe location.`;function m1(){return Ta()?"bash":"powershell"}function kq(e){}' +
  'function ep(){let e=d({type:A("command").describe("Shell command hook type"),' +
  'command:o().describe("Shell command to execute"),' +
  'args:R(o()).optional().describe("Argument list for exec form (a {brace} and a ) inside text)."),if:Ht(),' +
  'shell:W(si).optional().describe("Shell interpreter."),timeout:T().positive().optional(),' +
  'cloud:W(["device","skip"]).optional().catch("skip")}),n=d({type:A("prompt").describe("LLM prompt hook type")});return e}' +
  'statusLine:()=>d({type:A("command"),command:o(),padding:T().optional(),' +
  'refreshInterval:T().min(1).optional().catch(void 0)}).optional().describe("Custom status line display configuration"),' +
  'function vFe(e){let n={CLAUDECODE:"1",CLAUDE_CODE_SESSION_ID:e.sessionId,CLAUDE_CODE_CHILD_SESSION:"1",CLAUDE_PID:String(process.pid)};return n}' +
  'async function run(e){let et=e.shell??m1(),ht=et==="powershell",St=e.args!==void 0;' +
  'if(Je&&!ht&&!St)Gt=cyn(Gt);let vn=ve!==void 0?ve.shellPrefix:a.CLAUDE_CODE_SHELL_PREFIX,' +
  'Qn={...ve!==void 0?Art({}):{...is()},...be,...vFe(g),CLAUDE_PROJECT_DIR:Ut(Et)}}' +
  'function cyn(e){let r=e.trim(),i="",n=0;while(n<r.length){let o=r[n];if(o==="\\\\"&&n+1<r.length)i+=r[n+1],n+=2;else break}' +
  'return i.endsWith(".sh")?`bash ${e}`:e}var CA=eO(1)';

// ---- versions ----

test('parseVersion accepts plain releases only', () => {
  assert.deepEqual(parseVersion('2.1.285'), [2, 1, 285]);
  assert.equal(parseVersion('2.1.285-beta.1'), null);
  assert.equal(parseVersion('v2.1.285'), null);
  assert.equal(parseVersion('2.1'), null);
  assert.equal(parseVersion('latest'), null);
});

test('compareVersions orders numerically, not as text', () => {
  const sorted = ['2.1.100', '2.1.9', '2.0.77', '2.10.0', '2.1.285'].sort(compareVersions);
  assert.deepEqual(sorted, ['2.0.77', '2.1.9', '2.1.100', '2.1.285', '2.10.0']);
});

test('selectVersions: first and last of every minor from 2.0.0, and the newest; pre-releases ignored', () => {
  const all = [
    '0.2.9', '1.0.128', '1.0.0', // before 2.0.0: not wanted
    '2.0.1', '2.0.0', '2.0.77', '2.0.30',
    '2.1.0', '2.1.5', '2.1.285', '2.1.284',
    '2.1.286-rc.1', '2.2.0-beta.1', // pre-releases
    '2.2.3', '2.2.1', '2.2.9',
    '2.3.0-alpha.2',
    'next', 'latest', // dist-tags, not versions
  ];
  assert.deepEqual(selectVersions(all), [
    '2.0.0', '2.0.77', '2.1.0', '2.1.285', '2.2.1', '2.2.9',
  ]);
});

test('selectVersions: a minor with one release lists it once; the newest is always there', () => {
  assert.deepEqual(selectVersions(['2.0.0', '2.1.0', '2.1.0', '2.2.7']), ['2.0.0', '2.1.0', '2.2.7']);
  assert.deepEqual(selectVersions(['2.0.0']), ['2.0.0']);
  assert.deepEqual(selectVersions(['1.0.0', '2.0.0-x']), []);
  assert.deepEqual(selectVersions([]), []);
});

// ---- the small readers ----

test('matchingBracket and topLevelKeys skip strings and nested brackets', () => {
  const text = 'f({a:1,"b":{c:2},d:[1,2],e:"x}y,z:3",f:g(1,{h:2}),`t:${1}`:0})';
  const open = text.indexOf('{');
  assert.equal(text[matchingBracket(text, open)], '}');
  assert.equal(matchingBracket(text, open), text.length - 2);
  assert.deepEqual(topLevelKeys(text, open).keys, ['a', 'b', 'd', 'e', 'f']);
  assert.equal(topLevelKeys('{a:1,b:"never closed', 0), null);
});

// ---- hook entry schema: exec form, shell, strictness ----

test('hook_exec_form: true with args, false without, null when the schema is not there', () => {
  assert.equal(hookExecForm(V2285), true);
  assert.equal(hookExecForm(V2100), false);
  assert.equal(hookExecForm(V200), false);
  assert.equal(hookExecForm('nothing of interest here'), null);
  assert.equal(hookExecForm(''), null);
});

test('hook_exec_form: the warning text alone is a positive, a mention inside another object is not', () => {
  assert.equal(hookExecForm('t(`... Exec form treats "command" as a single executable name ...`)'), true);
  // `args` belongs to an unrelated object after the hook entry closed: not the hook entry's key
  const other = V2100 + 'x=L.object({args:L.array(L.string())})';
  assert.equal(hookExecForm(other), false);
});

test('hook_shell_key: true with shell, false without, null when absent', () => {
  assert.equal(hookShellKey(V2285), true);
  assert.equal(hookShellKey(V2100), true);
  assert.equal(hookShellKey(V200), false);
  assert.equal(hookShellKey('x'), null);
});

test('hook_unknown_keys_rejected: strictObject or .strict() reject, a plain object does not, absent is null', () => {
  assert.equal(hookUnknownKeysRejected(V2285), false);
  assert.equal(hookUnknownKeysRejected(V2100), false);
  assert.equal(hookUnknownKeysRejected(V200), false);
  const strictOpener = V2100.replace('L.object({type:L.literal("command")', 'L.strictObject({type:L.literal("command")');
  assert.notEqual(strictOpener, V2100);
  assert.equal(hookUnknownKeysRejected(strictOpener), true);
  const strictTail = V2100.replace('once:L.boolean().optional()})', 'once:L.boolean().optional()}).strict()');
  assert.notEqual(strictTail, V2100);
  assert.equal(hookUnknownKeysRejected(strictTail), true);
  assert.equal(hookUnknownKeysRejected('x'), null);
});

test('hookEntrySchema lists only the entry\'s own top-level keys', () => {
  assert.deepEqual(hookEntrySchema(V2285).keys, ['type', 'command', 'args', 'if', 'shell', 'timeout', 'cloud']);
  assert.deepEqual(hookEntrySchema(V200).keys, ['type', 'command', 'timeout']);
  assert.equal(hookEntrySchema('command:o().describe("Shell command to execute")'), null);
});

// ---- the status line ----

test('status_line_shell_key: false without shell, true with it, null when the schema is not there', () => {
  assert.equal(statusLineShellKey(V2285), false);
  assert.equal(statusLineShellKey(V2100), false);
  const withShell = V2285.replace('padding:T().optional(),', 'padding:T().optional(),shell:W(si).optional(),');
  assert.notEqual(withShell, V2285);
  assert.equal(statusLineShellKey(withShell), true);
  assert.equal(statusLineShellKey(V200), null);
  assert.equal(statusLineShellKey('statusLine:L.object({type:L.literal("command"),command:L.string()})'), null);
});

// ---- the default shell ----

test('windows_default_shell: conditional function, constant, Node shell, and nothing', () => {
  assert.equal(windowsDefaultShell(V2285), 'bash when Git Bash is found, else powershell');
  assert.equal(windowsDefaultShell(V2100), 'bash');
  assert.match(windowsDefaultShell(V200), /shell: true/);
  assert.equal(windowsDefaultShell('let a=1;'), null);
});

test('windows_default_shell: the conditional needs its Git Bash message, and a constant must be "bash"', () => {
  const noMessage = V2285.replace('CLAUDE_CODE_GIT_BASH_PATH', 'SOMETHING_ELSE');
  assert.equal(windowsDefaultShell(noMessage), null);
  const other = V2100.replace('RZ6="bash"', 'RZ6="zsh"');
  assert.equal(windowsDefaultShell(other), null);
});

// ---- CLAUDE_PID ----

test('claude_pid_exported: true when the env builder is spread into the hook env', () => {
  assert.equal(claudePidExported(V2285), true);
});

test('claude_pid_exported: false only when the hook env is there and the name is nowhere', () => {
  assert.equal(claudePidExported(V2100), false);
  assert.equal(claudePidExported(V200), false);
});

test('claude_pid_exported: unknown when the name exists but is not tied to the hook env, or nothing is found', () => {
  const elsewhere = V2100 + 'function pk(){return"${CLAUDE_PID:-}"}';
  assert.equal(claudePidExported(elsewhere), null);
  const notSpread = V2285.replace('...vFe(g),', '');
  assert.notEqual(notSpread, V2285);
  assert.equal(claudePidExported(notSpread), null);
  assert.equal(claudePidExported('nothing'), null);
});

// ---- the bash transform ----

test('bash_transform: the function form (2.1.285) and the inline form (2.1.100)', () => {
  const fn = bashTransform(V2285);
  assert.ok(fn.startsWith(BASH_TRANSFORM_NOTE));
  assert.ok(fn.slice(BASH_TRANSFORM_NOTE.length).startsWith('function cyn(e){let r=e.trim()'));
  assert.ok(fn.endsWith('`bash ${e}`:e}'));
  const inline = bashTransform(V2100);
  assert.ok(inline.slice(BASH_TRANSFORM_NOTE.length).startsWith('if(D&&!f&&v.trim().match('));
  assert.ok(inline.endsWith('`bash ${v}`}'));
  // 2.1.140 spells the condition with one more term
  const more = bashTransform(V2100.replace('if(D&&!f&&v.trim()', 'if(D&&!f&&!Z&&v.trim()'));
  assert.ok(more.includes('!f&&!Z&&v.trim()'));
});

test('bash_transform: the 2.1.213 spelling of the call (the prefix read from an env object)', () => {
  const text =
    'x=1;if(y&&!v&&!E)x=VMn(x);let P=!v&&!E&&Z.CLAUDE_CODE_SHELL_PREFIX?LKn(Z.CLAUDE_CODE_SHELL_PREFIX,x):x;' +
    'function VMn(e){let t=e.trim();return t.endsWith(".sh")?`bash ${e}`:e}var UQe';
  const value = bashTransform(text);
  assert.ok(value.slice(BASH_TRANSFORM_NOTE.length).startsWith('function VMn(e){let t=e.trim();'));
});

test('bash_transform: null when no rewrite is found', () => {
  assert.equal(bashTransform(V200), null);
  assert.equal(bashTransform(''), null);
});

test('bash_transform is cut to 400 characters and says so', () => {
  const long = 'x'.repeat(2000);
  const text = `if(Je&&!ht&&!St)Gt=cyn(Gt);let vn=ve!==void 0?ve.shellPrefix:a.CLAUDE_CODE_SHELL_PREFIX;function cyn(e){return"${long}"}`;
  const value = bashTransform(text);
  assert.equal(value.length, BASH_TRANSFORM_NOTE.length + BASH_TRANSFORM_MAX);
  assert.ok(value.startsWith(BASH_TRANSFORM_NOTE));
  assert.match(BASH_TRANSFORM_NOTE, /400/);
  assert.match(BASH_TRANSFORM_NOTE, /removed/);
});

test('scrub removes anything that reads like a credential, and nothing else', () => {
  // Built from pieces so this file never spells the patterns the repo's checks look for.
  const words = ['o' + 'auth', 'cred' + 'ential', 'sec' + 'ret', 'to' + 'ken', 'key' + 'chain', 'api_' + 'key'];
  for (const word of words) {
    const { text, changed } = scrub(`a ${word} b`);
    assert.equal(changed, true, word);
    assert.equal(text, 'a [removed] b', word);
  }
  const plain = 'function cyn(e){return i.endsWith(".sh")?`bash ${e}`:e}';
  assert.deepEqual(scrub(plain), { text: plain, changed: false });
  const inFunction = `function f(e){return"claude${'A' + 'iO' + 'auth'}"}`;
  assert.ok(!bashTransform(`if(Je&&!ht&&!St)Gt=f(Gt);let vn=ve!==void 0?ve.shellPrefix:a.CLAUDE_CODE_SHELL_PREFIX;${inFunction}`).includes('auth'));
});

// ---- analyze and the output ----

/** WP2's field names (agentnotch-engine hooks/facts.rs, schema 1). */
const WP2_VERSION_FIELDS = [
  'version',
  'hook_exec_form',
  'hook_shell_key',
  'hook_unknown_keys_rejected',
  'status_line_shell_key',
  'windows_default_shell',
  'claude_pid_exported',
  'bash_transform',
];
const WP2_TOP_FIELDS = ['schema', 'generator', 'generated_at', 'versions', 'exec_form_min', 'exec_form_evidence'];

test('analyze gives every fact of schema 1 in order', () => {
  const facts = analyze('2.1.285', V2285);
  assert.deepEqual(Object.keys(facts), WP2_VERSION_FIELDS);
  assert.equal(facts.version, '2.1.285');
  assert.equal(facts.hook_exec_form, true);
  assert.equal(facts.hook_shell_key, true);
  assert.equal(facts.hook_unknown_keys_rejected, false);
  assert.equal(facts.status_line_shell_key, false);
  assert.equal(facts.claude_pid_exported, true);
  const none = analyze('9.9.9', 'no claude code in here');
  assert.deepEqual(Object.keys(none), WP2_VERSION_FIELDS);
  for (const key of WP2_VERSION_FIELDS.slice(1)) assert.equal(none[key], null, key);
});

test('the key lists the script writes are WP2\'s', () => {
  assert.deepEqual(VERSION_KEYS, WP2_VERSION_FIELDS);
  assert.deepEqual(TOP_KEYS, WP2_TOP_FIELDS);
  assert.equal(SCHEMA, 1);
  assert.equal(GENERATOR, 'windows/scripts/check-claude-code-facts.mjs');
});

test('buildOutput: schema 1, stable key order, 2-space indent, versions sorted by semver, unknown stays null', () => {
  const text = buildOutput({
    generatedAt: '2026-10-01',
    versions: [
      { version: '2.1.285', hook_exec_form: true, bash_transform: 'x', extra: 'dropped' },
      { version: '2.1.9', hook_exec_form: false },
      { version: '2.0.0' },
    ],
  });
  assert.ok(text.endsWith('}\n'));
  const doc = JSON.parse(text);
  assert.deepEqual(Object.keys(doc), WP2_TOP_FIELDS);
  assert.equal(doc.schema, 1);
  assert.equal(doc.generator, GENERATOR);
  assert.equal(doc.generated_at, '2026-10-01');
  assert.deepEqual(doc.versions.map((v) => v.version), ['2.0.0', '2.1.9', '2.1.285']);
  for (const v of doc.versions) assert.deepEqual(Object.keys(v), WP2_VERSION_FIELDS);
  assert.equal(doc.versions[0].hook_exec_form, null);
  assert.equal(doc.versions[1].hook_exec_form, false);
  assert.equal(doc.versions[2].bash_transform, 'x');
  assert.equal(doc.exec_form_min, null);
  assert.equal(doc.exec_form_evidence, null);
  assert.ok(text.includes('\n  "versions": [\n    {\n      "version": "2.0.0"'));
  // the same input, the same bytes
  assert.equal(
    text,
    buildOutput({
      generatedAt: '2026-10-01',
      versions: [{ version: '2.0.0' }, { version: '2.1.9', hook_exec_form: false }, { version: '2.1.285', hook_exec_form: true, bash_transform: 'x' }],
    }),
  );
});

test('buildOutput keeps an explicit exec_form_min and its evidence', () => {
  const doc = JSON.parse(
    buildOutput({ versions: [{ version: '2.1.139' }], generatedAt: '2026-10-01', execFormMin: '2.1.139', execFormEvidence: 'hermetic job' }),
  );
  assert.equal(doc.exec_form_min, '2.1.139');
  assert.equal(doc.exec_form_evidence, 'hermetic job');
});

test('utcDate is the UTC calendar date', () => {
  assert.equal(utcDate(new Date('2026-10-01T23:59:59Z')), '2026-10-01');
  assert.equal(utcDate(new Date('2026-12-31T00:00:00Z')), '2026-12-31');
});

// ---- the whole script, offline, on tiny fake packages ----

function pack(dir, name, version, files) {
  const root = mkdtempSync(join(dir, 'src-'));
  for (const [path, content] of Object.entries(files)) {
    const file = join(root, 'package', path);
    mkdirSync(join(file, '..'), { recursive: true });
    writeFileSync(file, content);
  }
  const tgz = join(dir, `${name.replace(/^@/, '').replace('/', '-')}-${version}.tgz`);
  execFileSync('tar', ['-czf', tgz, '-C', root, 'package']);
  rmSync(root, { recursive: true, force: true });
}

function runScript(args) {
  return execFileSync(process.execPath, [SCRIPT, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}

test('the script reads cli.js and a native package offline, runs nothing, and writes schema 1', () => {
  const work = mkdtempSync(join(tmpdir(), 'facts-test-'));
  try {
    const cache = join(work, 'cache');
    mkdirSync(cache);
    const marker = join(work, 'executed');
    // Anything that ran this would leave the marker behind.
    const trap = `require("fs").writeFileSync(${JSON.stringify(marker)},"x");process.exit(0);\n`;
    pack(cache, '@anthropic-ai/claude-code', '2.0.0', { 'cli.js': trap + V200, 'package.json': '{"bin":{"claude":"cli.js"}}' });
    pack(cache, '@anthropic-ai/claude-code', '2.1.100', { 'cli.js': trap + V2100, 'package.json': '{"bin":{"claude":"cli.js"}}' });
    pack(cache, '@anthropic-ai/claude-code', '2.1.285', {
      'package.json': '{"bin":{"claude":"bin/claude.exe"},"optionalDependencies":{"@anthropic-ai/claude-code-win32-x64":"2.1.285"}}',
      'bin/claude.exe': 'x',
    });
    pack(cache, '@anthropic-ai/claude-code-win32-x64', '2.1.285', {
      'package.json': '{}',
      'claude.exe': `MZ\u0000\u0001${trap}${V2285}\u0000\u0002binary tail`,
    });
    const out = join(work, 'out', 'facts.json');
    runScript(['--offline', '--cache', cache, '--versions', '2.1.285,2.0.0,2.1.100', '--out', out]);
    const doc = JSON.parse(readFileSync(out, 'utf8'));
    assert.deepEqual(Object.keys(doc), WP2_TOP_FIELDS);
    assert.deepEqual(doc.versions.map((v) => v.version), ['2.0.0', '2.1.100', '2.1.285']);
    assert.deepEqual(doc.versions.map((v) => v.hook_exec_form), [false, false, true]);
    assert.deepEqual(doc.versions.map((v) => v.claude_pid_exported), [false, false, true]);
    assert.equal(existsSync(marker), false, 'a package file was executed');
    // deterministic apart from generated_at
    const out2 = join(work, 'out', 'again.json');
    runScript(['--offline', '--cache', cache, '--versions', '2.0.0,2.1.100,2.1.285', '--out', out2]);
    assert.equal(readFileSync(out, 'utf8'), readFileSync(out2, 'utf8'));
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test('the script without --versions picks from the cache, keeps versions the file lists, and leaves out what is missing', () => {
  const work = mkdtempSync(join(tmpdir(), 'facts-test-'));
  try {
    const cache = join(work, 'cache');
    mkdirSync(cache);
    for (const v of ['2.0.0', '2.0.5', '2.0.9', '2.1.0', '2.1.3']) {
      pack(cache, '@anthropic-ai/claude-code', v, { 'cli.js': V2100, 'package.json': '{"bin":{"claude":"cli.js"}}' });
    }
    const out = join(work, 'facts.json');
    writeFileSync(out, buildOutput({ versions: [{ version: '2.0.5' }, { version: '2.5.5' }], generatedAt: '2026-01-01', execFormMin: '2.1.3', execFormEvidence: 'by hand' }));
    runScript(['--offline', '--cache', cache, '--out', out]);
    const doc = JSON.parse(readFileSync(out, 'utf8'));
    // first/last of 2.0 and 2.1, plus 2.0.5 from the old file; 2.5.5 isn't known, so it is dropped
    assert.deepEqual(doc.versions.map((v) => v.version), ['2.0.0', '2.0.5', '2.0.9', '2.1.0', '2.1.3']);
    assert.equal(doc.exec_form_min, '2.1.3');
    assert.equal(doc.exec_form_evidence, 'by hand');
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test('the script refuses a bad version and an unknown option', () => {
  assert.throws(() => runScript(['--offline', '--versions', '2.1.x']), /not a release version/);
  assert.throws(() => runScript(['--bogus']), /unknown option/);
});
