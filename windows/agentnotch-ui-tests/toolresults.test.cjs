'use strict';
// ui/agentnotch/toolresults.js on its own (the port of ToolResultViews.swift): the view each
// tool kind gets, the caps that keep long output short, the diff, and hostile text. The Mac
// tests it ports: B_ToolSummaryTests' LineDiff case (the summary text after a tool's name is
// the engine's `summary` field on Windows, tested with the engine).

const test = require('node:test');
const assert = require('node:assert/strict');
const dom = require('./lib/dom.cjs');
const audit = require('./lib/audit.cjs');
const harness = require('./lib/harness.cjs');
const { load, plain } = require('./lib/scripts.cjs');

const { window } = load(['common', 'toolresults']);
const T = window.agentnotchToolResults;

const frag = (html) => dom.parseFragment(html);
const tool = (result, extra) => Object.assign({ id: 't', kind: 'tool', name: 'X', summary: '', status: 'success', input: {}, result, subagent: null }, extra);
const view = (result, extra) => T.render(tool(result, extra));
const texts = (root, selector) => root.querySelectorAll(selector).map((el) => el.textContent);
const N = (n, word) => Array.from({ length: n }, (_, i) => word + (i + 1));

// ---- shape ------------------------------------------------------------------------------------

test('toolresults.js defines exactly one global and no top-level lexical binding', () => {
  const fs = require('node:fs');
  const path = require('node:path');
  const vm = require('node:vm');
  const dir = path.join(harness.UI, 'agentnotch');
  const source = fs.readFileSync(path.join(dir, 'toolresults.js'), 'utf8');
  assert.doesNotMatch(source, /^(let|const|class)\s/m);
  assert.match(source, /^\(function \(\) \{\n {2}'use strict';/m);
  const context = vm.createContext({});
  const win = vm.runInContext('this', context);
  Object.assign(win, { window: win, document: dom.parseDocument('<html><head></head><body></body></html>') });
  vm.runInContext(fs.readFileSync(path.join(dir, 'common.js'), 'utf8'), context);
  const before = new Set(Object.getOwnPropertyNames(win));
  vm.runInContext(source, context);
  assert.deepEqual(Object.getOwnPropertyNames(win).filter((n) => !before.has(n)), ['agentnotchToolResults']);
  assert.equal(vm.runInContext("typeof RENDER === 'undefined' && typeof esc === 'undefined' && typeof lcs === 'undefined'", context), true);
});

// ---- names ------------------------------------------------------------------------------------

test('toolName: aliases for Claude tools, MCP ids as "Server - Tool", others unchanged', () => {
  assert.equal(T.toolName('mcp__deepwiki__ask_question'), 'Deepwiki - Ask Question');
  assert.equal(T.toolName('mcp__github__list_pull_requests'), 'Github - List Pull Requests');
  assert.equal(T.toolName('mcp__solo'), 'Solo');
  assert.equal(T.toolName('mcp__a_b'), 'A - B');
  for (const [id, shown] of Object.entries({
    AgentOutputTool: 'Await Agent', AskUserQuestion: 'Question', TodoWrite: 'Todo', TodoRead: 'Todo', WebFetch: 'Fetch',
    WebSearch: 'Search', NotebookEdit: 'Notebook', BashOutput: 'Bash', KillShell: 'Shell', EnterPlanMode: 'Plan',
    ExitPlanMode: 'Plan', SlashCommand: 'Command',
  })) assert.equal(T.toolName(id), shown, id);
  for (const id of ['Bash', 'Read', 'Edit', 'Task', 'Agent', 'Mystery']) assert.equal(T.toolName(id), id);
  assert.equal(T.toolName(''), '');
  assert.equal(T.toolName(null), '');
  assert.equal(T.toolName('constructor'), 'constructor', 'an inherited property is not an alias');
  assert.equal(T.toolName('__proto__'), '__proto__');
});

test('baseName: the last part of a Windows or Unix path', () => {
  assert.equal(T.baseName('C:\\Users\\me\\code\\redirect.ts'), 'redirect.ts');
  assert.equal(T.baseName('/a/b/app.ts'), 'app.ts');
  assert.equal(T.baseName('/a/b/'), 'b');
  assert.equal(T.baseName('plain'), 'plain');
  assert.equal(T.baseName(''), '');
  assert.equal(T.baseName(null), '');
});

// ---- LineDiff (B_ToolSummaryTests.diffShowsRemovalsThenAdditionsAndStopsAtTheLimit) -----------

test('the line diff shows removals then additions and stops at the limit', () => {
  const diff = T.lineDiff('a\nb\nc', 'a\nB\nc', 12);
  assert.deepEqual(plain(diff.lines), [{ kind: 'remove', text: 'b', number: 2 }, { kind: 'add', text: 'B', number: 2 }]);
  assert.equal(diff.truncated, false);
  const long = T.lineDiff(Array.from({ length: 20 }, (_, i) => String(i + 1)).join('\n'), '', 5);
  assert.equal(long.lines.length, 5);
  assert.equal(long.truncated, true);
  assert.deepEqual(plain(T.lineDiff('', '', 5).lines), []);
});

test('the line diff sets the shared start and end aside and numbers each side in its own text', () => {
  const d = T.lineDiff('1\n2\n3\n4\n5', '1\n2\nX\nY\n4\n5', 12);
  assert.deepEqual(plain(d.lines), [
    { kind: 'remove', text: '3', number: 3 },
    { kind: 'add', text: 'X', number: 3 },
    { kind: 'add', text: 'Y', number: 4 },
  ]);
  const insert = T.lineDiff('a\nc', 'a\nb\nc', 12);
  assert.deepEqual(plain(insert.lines), [{ kind: 'add', text: 'b', number: 2 }]);
  assert.deepEqual(plain(T.lineDiff('same\nsame', 'same\nsame', 12).lines), []);
  // A middle bigger than the table lists old lines as removed and new lines as added.
  const big = T.lineDiff(N(600, 'o').join('\n'), N(600, 'n').join('\n'), 5000);
  assert.equal(big.lines.filter((l) => l.kind === 'remove').length, 600);
  assert.equal(big.lines.filter((l) => l.kind === 'add').length, 600);
  // Out of order lines keep the longest common subsequence.
  const moved = T.lineDiff('a\nb\nc\nd', 'a\nc\nb\nd', 12);
  assert.equal(moved.lines.length, 2);
});

// ---- dispatcher ---------------------------------------------------------------------------------

test('no result draws nothing, except an Edit, which shows its change from its input', () => {
  assert.equal(T.render(tool(null)), '');
  assert.equal(T.render(null), '');
  assert.equal(T.render(undefined), '');
  assert.equal(T.render({}), '');
  assert.equal(T.render(tool(null, { name: 'Bash' })), '');
  const edit = T.render(tool(null, { name: 'Edit', input: { file_path: 'C:\\a\\b\\app.ts', old_string: 'x', new_string: 'y' } }));
  const root = frag(edit);
  assert.equal(root.querySelector('.an-code-head').textContent, 'app.ts');
  assert.deepEqual(texts(root, '.an-diff-remove .an-code-text'), ['x']);
  assert.deepEqual(texts(root, '.an-diff-add .an-code-text'), ['y']);
  assert.equal(T.render(tool(null, { name: 'Edit', input: {} })), '');
  assert.equal(T.render(tool(null, { name: 'Edit', input: { old_string: 'x', new_string: 'y' } })).includes('>file<'), true, 'no path: the header says "file"');
});

test('a result of an unknown kind draws nothing; one with the wrong shape does not throw', () => {
  for (const result of [{ tool: 'nonsense' }, { tool: 'constructor' }, { tool: '__proto__' }, { tool: 'toString' }, { tool: 7 }, {}, 'text', 5, []]) {
    assert.equal(T.render(tool(result)), '', JSON.stringify(result));
  }
  for (const result of [{ tool: 'grep', filenames: 5 }, { tool: 'todo', items: 'x' }, { tool: 'glob', filenames: { a: 1 } }]) {
    assert.doesNotThrow(() => T.render(tool(result)));
    assert.deepEqual(audit.problems(T.render(tool(result))), [], JSON.stringify(result));
  }
});

// ---- read ---------------------------------------------------------------------------------------

test('read: the file name as header, line numbers from the start line, 10 lines, "N more lines"', () => {
  const html = view({ tool: 'read', file_path: 'C:\\Users\\me\\redirect.ts', content: 'a\n\nc', num_lines: 3, start_line: 11, total_lines: 40 });
  assert.equal(html,
    '<div class="an-result">'.slice(0, 0) +
    '<div class="an-code"><div class="an-code-head" title="redirect.ts">redirect.ts</div><div class="an-code-body">' +
    '<div class="an-code-more an-code-gutterless">…</div>' +
    '<div class="an-code-line"><span class="an-code-num">11</span><span class="an-code-text">a</span></div>' +
    '<div class="an-code-line"><span class="an-code-num">12</span><span class="an-code-text">&nbsp;</span></div>' +
    '<div class="an-code-line"><span class="an-code-num">13</span><span class="an-code-text">c</span></div>' +
    '</div></div>');
  const many = frag(view({ tool: 'read', file_path: '/x/y.txt', content: N(25, 'l').join('\n'), num_lines: 25, start_line: 1, total_lines: 25 }));
  assert.equal(many.querySelectorAll('.an-code-line').length, 10);
  assert.deepEqual(texts(many, '.an-code-num'), Array.from({ length: 10 }, (_, i) => String(i + 1)));
  assert.equal(many.querySelector('.an-code-more').textContent, '15 more lines');
  assert.equal(many.querySelector('.an-code-more').getAttribute('class'), 'an-code-more an-code-gutterless');
  assert.equal(view({ tool: 'read', file_path: '/x', content: '', num_lines: 0, start_line: 1, total_lines: 0 }), '');
  // Exactly ten lines: no "more".
  assert.equal(frag(view({ tool: 'read', file_path: '/x', content: N(10, 'l').join('\n'), start_line: 1 })).querySelector('.an-code-more'), null);
  // A start line of 1 draws no leading ellipsis; a missing or bad one counts as 1.
  assert.equal(frag(view({ tool: 'read', file_path: '/x', content: 'a', start_line: 1 })).querySelector('.an-code-more'), null);
  assert.equal(frag(view({ tool: 'read', file_path: '/x', content: 'a', start_line: 'x' })).querySelector('.an-code-num').textContent, '1');
});

// ---- edit and write -----------------------------------------------------------------------------

const EDIT_DIFF = [
  { kind: 'hunk', text: '@@ -12,3 +12,4 @@', old_line: null, new_line: null },
  { kind: 'context', text: "  const next = params.get('next');", old_line: 12, new_line: 12 },
  { kind: 'remove', text: '  return redirect(next);', old_line: 13, new_line: null },
  { kind: 'add', text: "  if (!isSafe(next)) return redirect('/');", old_line: null, new_line: 13 },
  { kind: 'add', text: '  return redirect(next);', old_line: null, new_line: 14 },
];

test('edit: the engine diff with hunk, context, removed and added rows, numbered, in a box headed by the file', () => {
  const root = frag(view({ tool: 'edit', file_path: 'C:\\a\\redirect.ts', replace_all: false, user_modified: false, diff: EDIT_DIFF }));
  assert.equal(root.querySelector('.an-code-head').textContent, 'redirect.ts');
  const rows = root.querySelectorAll('.an-diff-line');
  assert.deepEqual(rows.map((r) => r.getAttribute('class')), [
    'an-diff-line an-diff-hunk', 'an-diff-line an-diff-context', 'an-diff-line an-diff-remove', 'an-diff-line an-diff-add', 'an-diff-line an-diff-add',
  ]);
  assert.deepEqual(rows.map((r) => r.querySelector('.an-code-num').textContent), ['', '12', '13', '13', '14']);
  assert.deepEqual(rows.map((r) => r.querySelector('.an-diff-sign').textContent), ['', ' ', '−', '+', '+']);
  assert.deepEqual(rows.map((r) => r.querySelector('.an-code-text').textContent), [
    '@@ -12,3 +12,4 @@', "  const next = params.get('next');", '  return redirect(next);', "  if (!isSafe(next)) return redirect('/');", '  return redirect(next);',
  ]);
  assert.equal(root.querySelector('.an-note'), null);
});

test('edit: "You changed it before it was applied" when the user edited it; up to 12 changed lines, then "…"', () => {
  const html = view({ tool: 'edit', file_path: '/a/b.ts', replace_all: false, user_modified: true, diff: EDIT_DIFF });
  assert.equal(texts(frag(html), '.an-note').join('|'), 'You changed it before it was applied');
  const lots = Array.from({ length: 30 }, (_, i) => ({ kind: i % 2 ? 'add' : 'remove', text: 'l' + i, old_line: i, new_line: i }));
  const root = frag(view({ tool: 'edit', file_path: '/a/b.ts', user_modified: false, diff: lots }));
  assert.equal(root.querySelectorAll('.an-diff-line').length, 12);
  assert.equal(root.querySelector('.an-code-more').textContent, '…');
  const exactly = frag(view({ tool: 'edit', file_path: '/a/b.ts', user_modified: false, diff: lots.slice(0, 12) }));
  assert.equal(exactly.querySelector('.an-code-more'), null, 'exactly 12 changed lines are not cut');
  // Context between hunks is bounded too.
  const context = Array.from({ length: 200 }, (_, i) => ({ kind: 'context', text: 'c' + i, old_line: i, new_line: i }));
  const bounded = frag(view({ tool: 'edit', file_path: '/a/b.ts', user_modified: false, diff: context }));
  assert.equal(bounded.querySelectorAll('.an-diff-line').length, 48);
  assert.equal(bounded.querySelector('.an-code-more').textContent, '…');
});

test('edit: without an engine diff the change comes from the input; an unknown row kind is context', () => {
  const html = view({ tool: 'edit', file_path: 'C:\\a\\b\\redirect.ts', diff: [], user_modified: false },
    { input: { old_string: 'a\nb\nc', new_string: 'a\nB\nc', file_path: 'C:\\a\\b\\redirect.ts' } });
  const root = frag(html);
  assert.equal(root.querySelector('.an-code-head').textContent, 'redirect.ts');
  assert.deepEqual(texts(root, '.an-diff-line .an-code-text'), ['b', 'B']);
  assert.deepEqual(texts(root, '.an-diff-line .an-code-num'), ['2', '2']);
  const odd = frag(view({ tool: 'edit', file_path: '/a/b', diff: [{ kind: 'add an-critical', text: 'x', old_line: null, new_line: 1 }] }));
  assert.equal(odd.querySelector('.an-diff-line').getAttribute('class'), 'an-diff-line an-diff-context');
});

test('write: "Created <file>" with 8 lines of the new file, or "Wrote <file>" with the diff', () => {
  const created = frag(view({ tool: 'write', file_path: 'C:\\p\\new.ts', created: true, content: N(12, 'row').join('\n'), diff: [] }));
  assert.deepEqual(texts(created, '.an-note'), ['Created new.ts']);
  assert.equal(created.querySelector('.an-code-more').textContent, '4 more lines');
  assert.equal(created.querySelectorAll('.an-code-line').length, 8);
  assert.equal(created.querySelector('.an-code-head'), null, 'a preview has no header');
  const wrote = frag(view({ tool: 'write', file_path: '/p/old.ts', created: false, content: 'ignored', diff: EDIT_DIFF }));
  assert.equal(wrote.querySelector('.an-note').textContent, 'Wrote old.ts');
  assert.equal(wrote.querySelectorAll('.an-diff-line').length, 5);
  assert.equal(wrote.querySelector('.an-code-head'), null);
  // Created with no content: just the note.
  assert.equal(view({ tool: 'write', file_path: '/p/e.ts', created: true, content: '', diff: [] }),
    '<div class="an-result"><div class="an-note">Created e.ts</div></div>');
  // Wrote, no diff: just the note.
  assert.equal(view({ tool: 'write', file_path: '/p/e.ts', created: false, content: 'x', diff: [] }),
    '<div class="an-result"><div class="an-note">Wrote e.ts</div></div>');
});

// ---- bash ---------------------------------------------------------------------------------------

test('bash: stdout up to 15 lines, stderr up to 10 lines in the critical ink, notes for the rest', () => {
  const ok = view({ tool: 'bash', stdout: 'PASS a.spec.ts\n  4 passed', stderr: '', interrupted: false, return_code_interpretation: null, background_task_id: null });
  assert.equal(ok,
    '<div class="an-result"><div class="an-code"><div class="an-code-body">' +
    '<div class="an-code-line an-code-plain"><span class="an-code-text">PASS a.spec.ts</span></div>' +
    '<div class="an-code-line an-code-plain"><span class="an-code-text">  4 passed</span></div></div></div></div>');
  const both = frag(view({ tool: 'bash', stdout: N(20, 'o').join('\n'), stderr: N(14, 'e').join('\n') }));
  const boxes = both.querySelectorAll('.an-code');
  assert.equal(boxes.length, 2);
  assert.equal(boxes[0].querySelectorAll('.an-code-line').length, 15);
  assert.equal(boxes[0].querySelector('.an-code-more').textContent, '5 more lines');
  assert.equal(boxes[1].querySelectorAll('.an-code-line').length, 10);
  assert.equal(boxes[1].querySelector('.an-code-more').textContent, '4 more lines');
  assert.equal(boxes[0].querySelectorAll('.an-critical').length, 0);
  assert.equal(boxes[1].querySelectorAll('.an-critical').length, 10);
  assert.equal(both.querySelectorAll('.an-note').length, 0);
});

test('bash: a background id, an exit interpretation and "No output"', () => {
  assert.deepEqual(texts(frag(view({ tool: 'bash', stdout: '', stderr: '', background_task_id: 'bg_7' })), '.an-note'),
    ['Running in the background (bg_7)']);
  assert.deepEqual(texts(frag(view({ tool: 'bash', stdout: 'x', stderr: '', return_code_interpretation: 'Exit code 1' })), '.an-note'),
    ['Exit code 1']);
  assert.equal(view({ tool: 'bash', stdout: '', stderr: '' }), '<div class="an-result"><div class="an-note">No output</div></div>');
  assert.equal(view({ tool: 'bash', stdout: '', stderr: '', background_task_id: null, return_code_interpretation: null }),
    '<div class="an-result"><div class="an-note">No output</div></div>');
  // The Mac draws no note for an interrupted command (the tool's own line says so).
  assert.equal(view({ tool: 'bash', stdout: '', stderr: '', interrupted: true }),
    '<div class="an-result"><div class="an-note">No output</div></div>');
  const both = frag(view({ tool: 'bash', stdout: 'out', stderr: '', background_task_id: 'b', return_code_interpretation: 'why' }));
  assert.deepEqual(texts(both, '.an-note'), ['Running in the background (b)', 'why']);
});

test('bash output: the status, the exit code (critical unless 0), stdout 10 lines, stderr 5', () => {
  const root = frag(view({ tool: 'bash_output', shell_id: 's1', status: 'running', stdout: N(12, 'o').join('\n'), stderr: N(7, 'e').join('\n'), exit_code: null }));
  assert.equal(root.querySelector('.an-result-head .an-note').textContent, 'Running');
  assert.equal(root.querySelector('.an-mono-caption'), null);
  const boxes = root.querySelectorAll('.an-code');
  assert.equal(boxes[0].querySelectorAll('.an-code-line').length, 10);
  assert.equal(boxes[1].querySelectorAll('.an-code-line').length, 5);
  assert.equal(boxes[1].querySelector('.an-code-more').textContent, '2 more lines');
  const done = frag(view({ tool: 'bash_output', shell_id: 's1', status: 'completed', stdout: '', stderr: '', exit_code: 0 }));
  assert.equal(done.querySelector('.an-mono-caption').textContent, 'exit 0');
  assert.equal(done.querySelector('.an-mono-caption').getAttribute('class'), 'an-mono-caption');
  const bad = frag(view({ tool: 'bash_output', shell_id: 's1', status: 'killed', stdout: '', stderr: '', exit_code: 137 }));
  assert.equal(bad.querySelector('.an-mono-caption').textContent, 'exit 137');
  assert.equal(bad.querySelector('.an-mono-caption').getAttribute('class'), 'an-mono-caption an-critical');
  assert.equal(frag(view({ tool: 'bash_output', status: 'in_progress', stdout: '', stderr: '' })).querySelector('.an-note').textContent, 'In_progress');
  assert.equal(frag(view({ tool: 'bash_output', status: 'not found', stdout: '', stderr: '' })).querySelector('.an-note').textContent, 'Not Found');
});

test('kill shell: the engine message, or "Shell <id> stopped"', () => {
  assert.equal(view({ tool: 'kill_shell', shell_id: 'bg_2', message: 'Killed it' }), '<div class="an-note">Killed it</div>');
  assert.equal(view({ tool: 'kill_shell', shell_id: 'bg_2', message: '' }), '<div class="an-note">Shell bg_2 stopped</div>');
});

// ---- searches -----------------------------------------------------------------------------------

test('grep: file names (10), content (15 lines), a count, or "No matches"', () => {
  const files = frag(view({ tool: 'grep', mode: 'files_with_matches', filenames: N(13, 'C:\\src\\dir\\f').map((f) => f + '.ts'), num_files: 13 }));
  assert.equal(files.querySelectorAll('.an-file').length, 10);
  assert.equal(files.querySelector('.an-file').textContent, 'f1.ts');
  assert.equal(files.querySelector('.an-file').getAttribute('title'), 'C:\\src\\dir\\f1.ts');
  assert.equal(files.querySelector('.an-files > .an-note').textContent, 'and 3 more');
  assert.equal(frag(view({ tool: 'grep', mode: 'files_with_matches', filenames: N(10, 'f'), num_files: 10 })).querySelector('.an-note'), null);
  const content = frag(view({ tool: 'grep', mode: 'content', filenames: [], num_files: 0, content: N(20, 'm').join('\n'), num_lines: 20 }));
  assert.equal(content.querySelectorAll('.an-code-line').length, 15);
  assert.equal(content.querySelector('.an-code-more').textContent, '5 more lines');
  assert.equal(view({ tool: 'grep', mode: 'count', filenames: [], num_files: 1 }), '<div class="an-note">1 file with matches</div>');
  assert.equal(view({ tool: 'grep', mode: 'count', filenames: [], num_files: 3 }), '<div class="an-note">3 files with matches</div>');
  assert.equal(view({ tool: 'grep', mode: 'count', filenames: [], num_files: 0 }), '<div class="an-note">0 files with matches</div>');
  for (const empty of [
    { tool: 'grep', mode: 'files_with_matches', filenames: [], num_files: 0 },
    { tool: 'grep', mode: 'content', filenames: [], num_files: 0, content: null },
    { tool: 'grep', mode: 'content', filenames: [], num_files: 0, content: '' },
  ]) assert.equal(view(empty), '<div class="an-note">No matches</div>');
  // An unknown mode lists the files, as the Mac's default does.
  assert.equal(frag(view({ tool: 'grep', mode: 'weird', filenames: ['a/b.ts'], num_files: 1 })).querySelector('.an-file').textContent, 'b.ts');
});

test('glob: file names (10) and a note when the engine cut the list', () => {
  assert.equal(view({ tool: 'glob', filenames: [], num_files: 0, truncated: false }), '<div class="an-note">No files</div>');
  const root = frag(view({ tool: 'glob', filenames: N(14, 'a/b/g').map((f) => f + '.rs'), num_files: 14, truncated: true }));
  assert.equal(root.querySelectorAll('.an-file').length, 10);
  assert.deepEqual(texts(root, '.an-note'), ['and 4 more', 'More were found than are listed']);
  const short = frag(view({ tool: 'glob', filenames: ['x.rs'], num_files: 1, truncated: false }));
  assert.equal(short.querySelectorAll('.an-note').length, 0);
});

test('web fetch: the status code (critical from 400), the address, 8 lines of the page', () => {
  const ok = frag(view({ tool: 'web_fetch', url: 'https://example.com/a', code: 200, code_text: 'OK', bytes: 10, duration_ms: 5, result: 'The page says hello.' }));
  assert.equal(ok.querySelectorAll('.an-mono-caption')[0].textContent, '200');
  assert.equal(ok.querySelectorAll('.an-mono-caption')[0].getAttribute('class'), 'an-mono-caption');
  assert.equal(ok.querySelectorAll('.an-mono-caption')[1].textContent, 'https://example.com/a');
  assert.equal(ok.querySelectorAll('.an-mono-caption')[1].getAttribute('title'), 'https://example.com/a');
  assert.equal(ok.querySelector('.an-result-text').textContent, 'The page says hello.');
  assert.match(ok.querySelector('.an-result-text').getAttribute('style'), /-webkit-line-clamp:8/);
  const bad = frag(view({ tool: 'web_fetch', url: 'https://example.com/x', code: 404, code_text: 'Not Found', bytes: 0, duration_ms: 1, result: '' }));
  assert.equal(bad.querySelectorAll('.an-mono-caption')[0].getAttribute('class'), 'an-mono-caption an-critical');
  assert.equal(bad.querySelector('.an-result-text'), null);
  assert.equal(frag(view({ tool: 'web_fetch', url: 'u', code: 399, result: '' })).querySelector('.an-critical'), null);
  assert.ok(frag(view({ tool: 'web_fetch', url: 'u', code: 400, result: '' })).querySelector('.an-critical'));
  // The address is text, never a link.
  assert.equal(ok.querySelector('a'), null);
});

test('web search: 5 hits with a title and 2 lines of snippet, "and N more", "No results"', () => {
  assert.equal(view({ tool: 'web_search', query: 'q', results: [] }), '<div class="an-note">No results</div>');
  const hits = Array.from({ length: 8 }, (_, i) => ({ title: 'Title ' + i, url: 'https://e.com/' + i, snippet: i === 1 ? '' : 'snippet ' + i }));
  const root = frag(view({ tool: 'web_search', query: 'q', results: hits }));
  assert.deepEqual(texts(root, '.an-hit-title'), ['Title 0', 'Title 1', 'Title 2', 'Title 3', 'Title 4']);
  assert.equal(root.querySelectorAll('.an-hit')[1].querySelector('.an-result-text'), null, 'no snippet, no line');
  assert.match(root.querySelectorAll('.an-hit')[0].querySelector('.an-result-text').getAttribute('style'), /-webkit-line-clamp:2/);
  assert.equal(root.querySelector('.an-hits > .an-note').textContent, 'and 3 more');
  assert.equal(root.querySelector('a'), null, 'the hit address is not drawn as a link');
  assert.equal(frag(view({ tool: 'web_search', query: 'q', results: hits.slice(0, 5) })).querySelector('.an-note'), null);
});

// ---- tasks and agents ---------------------------------------------------------------------------

test('todo: a mark per status; an unknown status counts as pending', () => {
  const root = frag(view({ tool: 'todo', items: [
    { content: 'done', status: 'completed', active_form: null },
    { content: 'doing', status: 'in_progress', active_form: 'Doing' },
    { content: 'later', status: 'pending', active_form: null },
    { content: 'odd', status: 'x an-critical', active_form: null },
  ] }));
  assert.deepEqual(root.querySelectorAll('.an-todo').map((e) => e.getAttribute('class')),
    ['an-todo an-todo-completed', 'an-todo an-todo-in_progress', 'an-todo an-todo-pending', 'an-todo an-todo-pending']);
  assert.deepEqual(texts(root, '.an-todo-mark'), ['✓', '›', '·', '·']);
  assert.deepEqual(texts(root, '.an-todo-text'), ['done', 'doing', 'later', 'odd']);
  assert.equal(view({ tool: 'todo', items: [] }), '<div class="an-todos"></div>');
});

test('task: the status (critical when failed), duration, tool count and 5 lines of the result', () => {
  const root = frag(view({ tool: 'task', agent_id: 'a1', status: 'completed', content: 'Found 3 call sites.', total_duration_ms: 41000, total_tokens: 18234, total_tool_use_count: 6 }));
  assert.equal(root.querySelector('.an-task-status').textContent, 'Completed');
  assert.equal(root.querySelector('.an-task-status').getAttribute('class'), 'an-task-status');
  assert.deepEqual(texts(root, '.an-result-head .an-note'), ['41s', '6 tools']);
  assert.equal(root.querySelector('.an-result-text').textContent, 'Found 3 call sites.');
  assert.match(root.querySelector('.an-result-text').getAttribute('style'), /-webkit-line-clamp:5/);
  for (const status of ['failed', 'error']) {
    assert.equal(frag(view({ tool: 'task', status, content: '' })).querySelector('.an-task-status').getAttribute('class'), 'an-task-status an-critical');
  }
  const duration = (ms) => frag(view({ tool: 'task', status: 'completed', content: '', total_duration_ms: ms })).querySelector('.an-note').textContent;
  assert.equal(duration(0), '0ms');
  assert.equal(duration(999), '999ms');
  assert.equal(duration(1000), '1s');
  assert.equal(duration(59999), '59s');
  assert.equal(duration(60000), '1m 0s');
  assert.equal(duration(125000), '2m 5s');
  assert.equal(frag(view({ tool: 'task', status: 'completed', content: '', total_tool_use_count: 1 })).querySelector('.an-note').textContent, '1 tool');
  assert.equal(frag(view({ tool: 'task', status: 'completed', content: 'x' })).querySelectorAll('.an-note').length, 0);
});

test('question: each question with its answer as "↳ answer"; the answer key is the question text', () => {
  const root = frag(view({ tool: 'ask_user_question',
    questions: [{ text: 'Which database?', header: 'DB', multi_select: false, options: [] }, { text: 'Add tests?', header: null, multi_select: false, options: [] }, { text: 'Unanswered?', options: [] }],
    answers: { 'Which database?': 'Postgres', 'Add tests?': 'Yes' } }));
  assert.deepEqual(texts(root, '.an-qa > .an-result-text'), ['Which database?', 'Add tests?', 'Unanswered?']);
  assert.deepEqual(texts(root, '.an-qa-answer'), ['↳ Postgres', '↳ Yes']);
  // The Mac keys answers by the question's index; both are read.
  const byIndex = frag(view({ tool: 'ask_user_question', questions: [{ text: 'Q?', options: [] }], answers: { 0: 'first' } }));
  assert.equal(byIndex.querySelector('.an-qa-answer').textContent, '↳ first');
  assert.equal(view({ tool: 'ask_user_question', questions: [], answers: {} }), '');
});

test('plan: the plan file name and 6 lines of the plan', () => {
  const root = frag(view({ tool: 'exit_plan_mode', plan: '1. Do it\n2. Ship it', file_path: 'C:\\plans\\fix-login.md' }));
  assert.deepEqual(texts(root, '.an-note'), ['fix-login.md']);
  assert.equal(root.querySelector('.an-result-text').textContent, '1. Do it\n2. Ship it');
  assert.match(root.querySelector('.an-result-text').getAttribute('style'), /-webkit-line-clamp:6/);
  assert.equal(frag(view({ tool: 'exit_plan_mode', plan: null, file_path: null })).textContent, '');
  assert.equal(view({ tool: 'exit_plan_mode', plan: null, file_path: null }), '');
  assert.equal(view({ tool: 'exit_plan_mode', plan: '', file_path: null }), '');
});

test('MCP: "Server · Tool" and up to five key/value pairs, each value cut at 100 characters', () => {
  const root = frag(view({ tool: 'mcp', server_name: 'deep_wiki', tool_name: 'ask_question', raw: { a: 'one', b: 2, c: { d: [1, 2] }, e: true, f: null, g: 'sixth' } }));
  assert.equal(root.querySelector('.an-note').textContent, 'Deep Wiki · Ask Question');
  assert.deepEqual(texts(root, '.an-kv-key'), ['a', 'b', 'c', 'e', 'f']);
  assert.deepEqual(texts(root, '.an-kv-value'), ['one', '2', '{"d":[1,2]}', 'true', 'null']);
  const long = frag(view({ tool: 'mcp', server_name: 's', tool_name: 't', raw: { k: 'x'.repeat(500) } }));
  assert.equal(long.querySelector('.an-kv-value').textContent.length, 100);
  for (const raw of [null, 'text', 5, ['a'], undefined]) {
    const only = frag(view({ tool: 'mcp', server_name: 's', tool_name: 't', raw }));
    assert.equal(only.querySelectorAll('.an-kv').length, 0);
    assert.equal(only.querySelector('.an-note').textContent, 'S · T');
  }
});

test('generic: the text (15 lines), else the raw value as JSON, else "Completed"', () => {
  const text = frag(view({ tool: 'generic', text: N(20, 'g').join('\n'), raw: null }));
  assert.equal(text.querySelectorAll('.an-code-line').length, 15);
  assert.equal(text.querySelector('.an-code-more').textContent, '5 more lines');
  const raw = frag(view({ tool: 'generic', text: null, raw: { ok: true } }));
  assert.deepEqual(texts(raw, '.an-code-text'), ['{', '  "ok": true', '}']);
  assert.deepEqual(texts(frag(view({ tool: 'generic', text: null, raw: 'plain string' })), '.an-code-text'), ['plain string']);
  assert.deepEqual(texts(frag(view({ tool: 'generic', text: null, raw: 0 })), '.an-code-text'), ['0']);
  for (const none of [{ tool: 'generic', text: null, raw: null }, { tool: 'generic' }, { tool: 'generic', text: '', raw: null }]) {
    assert.equal(view(none), '<div class="an-note">Completed</div>');
  }
});

// ---- the fixtures ---------------------------------------------------------------------------------

function toolItems() {
  const items = [];
  for (const item of harness.fixture('chat.json').items) if (item.kind === 'tool') items.push(['chat.json', item]);
  for (const entry of harness.fixture('events.json')) {
    if (entry.event !== 'an:chat' || !entry.payload) continue;
    for (const item of entry.payload.items) if (item.kind === 'tool') items.push(['events.json an:chat', item]);
  }
  return items;
}

test('the contract fixtures: every tool item of chat.json and the an:chat patch renders clean', () => {
  const items = toolItems();
  assert.ok(items.length >= 5, 'the fixtures hold tool items');
  const seen = [];
  for (const [where, item] of items) {
    const html = T.render(item);
    assert.deepEqual(audit.problems(html), [], `${where} ${item.id}`);
    seen.push([item.id, item.result ? item.result.tool : 'none', html.length > 0]);
  }
  // Read, Edit and the task have a view; the Bash waiting for approval has no result yet; the patch's Bash has one.
  assert.deepEqual(seen.map((s) => s.join(':')), [
    'toolu_read_1:read:true', 'toolu_edit_1:edit:true', 'toolu_agent_1:task:true', 'toolu_sample_bash:none:false', 'toolu_sample_bash:bash:true',
  ]);
});

test('the fixtures show their text: file lines, the diff rows, the agent answer, the test output', () => {
  const byId = new Map(toolItems().map(([where, item]) => [item.id + '/' + (item.result ? item.result.tool : 'none'), T.render(item)]));
  const read = frag(byId.get('toolu_read_1/read'));
  assert.equal(read.querySelector('.an-code-head').textContent, 'redirect.ts');
  assert.deepEqual(texts(read, '.an-code-num'), ['11', '12', '13', '14']);
  assert.equal(texts(read, '.an-code-text')[1], "  const next = params.get('next');");
  const edit = frag(byId.get('toolu_edit_1/edit'));
  assert.equal(edit.querySelectorAll('.an-diff-add').length, 2);
  assert.equal(edit.querySelectorAll('.an-diff-remove').length, 1);
  assert.equal(frag(byId.get('toolu_agent_1/task')).querySelector('.an-result-text').textContent, 'Found 3 call sites.');
  assert.equal(texts(frag(byId.get('toolu_sample_bash/bash')), '.an-code-text').join('|'), 'PASS auth/redirect.spec.ts|  4 passed');
  assert.equal(byId.get('toolu_sample_bash/none'), '');
});

// ---- hand-made items for the kinds the fixtures lack ----------------------------------------------

const HAND_MADE = [
  ['write created', { tool: 'write', file_path: 'C:\\a\\n.ts', created: true, content: 'x\ny', diff: [] }],
  ['write wrote', { tool: 'write', file_path: 'C:\\a\\n.ts', created: false, content: '', diff: EDIT_DIFF }],
  ['bash stderr', { tool: 'bash', stdout: 'o', stderr: 'e', interrupted: false, return_code_interpretation: 'Exit code 2', background_task_id: 'b1' }],
  ['bash output', { tool: 'bash_output', shell_id: 'b1', status: 'completed', stdout: 'o', stderr: 'e', exit_code: 1 }],
  ['grep files', { tool: 'grep', mode: 'files_with_matches', filenames: ['a\\b.ts'], num_files: 1 }],
  ['grep content', { tool: 'grep', mode: 'content', filenames: [], num_files: 1, content: 'a.ts:1: hit', num_lines: 1 }],
  ['grep count', { tool: 'grep', mode: 'count', filenames: [], num_files: 2 }],
  ['glob', { tool: 'glob', filenames: ['a.rs', 'b.rs'], num_files: 2, truncated: true }],
  ['todo', { tool: 'todo', items: [{ content: 'a', status: 'completed', active_form: null }] }],
  ['web fetch', { tool: 'web_fetch', url: 'https://e.com', code: 200, code_text: 'OK', bytes: 1, duration_ms: 1, result: 'r' }],
  ['web search', { tool: 'web_search', query: 'q', results: [{ title: 't', url: 'https://e.com', snippet: 's' }] }],
  ['question', { tool: 'ask_user_question', questions: [{ text: 'Q?', options: [] }], answers: { 'Q?': 'A' } }],
  ['bash output', { tool: 'bash_output', shell_id: 's', status: 'running', stdout: '', stderr: '', exit_code: null }],
  ['kill shell', { tool: 'kill_shell', shell_id: 's', message: '' }],
  ['plan', { tool: 'exit_plan_mode', plan: 'p', file_path: '/p/plan.md' }],
  ['mcp', { tool: 'mcp', server_name: 'srv', tool_name: 'do_it', raw: { k: 'v' } }],
  ['generic', { tool: 'generic', text: 't', raw: null }],
];

test('every kind of the engine has a view and the hand-made items render clean', () => {
  const kinds = new Set(HAND_MADE.map(([, r]) => r.tool).concat(['read', 'edit', 'task']));
  assert.deepEqual([...kinds].sort(), ['ask_user_question', 'bash', 'bash_output', 'edit', 'exit_plan_mode', 'generic', 'glob', 'grep', 'kill_shell', 'mcp', 'read', 'task', 'todo', 'web_fetch', 'web_search', 'write']);
  for (const [name, result] of HAND_MADE) {
    const html = view(result);
    assert.ok(html.length > 0, name);
    assert.deepEqual(audit.problems(html), [], name);
  }
});

// ---- hostile text ---------------------------------------------------------------------------------

/** Every string field of a result replaced by `s`, for every kind. */
function hostileResults(s) {
  const big = s;
  return [
    { tool: 'read', file_path: s, content: s, num_lines: 1, start_line: 1, total_lines: 1 },
    { tool: 'read', file_path: s, content: s + '\n' + s, num_lines: 2, start_line: 5, total_lines: 9 },
    { tool: 'edit', file_path: s, replace_all: false, user_modified: true, diff: [{ kind: 'hunk', text: s, old_line: null, new_line: null }, { kind: 'context', text: s, old_line: 1, new_line: 1 }, { kind: 'remove', text: s, old_line: 2, new_line: null }, { kind: 'add', text: s, old_line: null, new_line: 2 }] },
    { tool: 'edit', file_path: s, diff: [] },
    { tool: 'write', file_path: s, created: true, content: s, diff: [] },
    { tool: 'write', file_path: s, created: false, content: s, diff: [{ kind: s, text: s, old_line: 1, new_line: 1 }] },
    { tool: 'bash', stdout: s, stderr: s, interrupted: false, return_code_interpretation: s, background_task_id: s },
    { tool: 'bash_output', shell_id: s, status: s, stdout: s, stderr: s, exit_code: 1 },
    { tool: 'grep', mode: 'files_with_matches', filenames: [s, s], num_files: 2 },
    { tool: 'grep', mode: 'content', filenames: [], num_files: 1, content: s, num_lines: 1 },
    { tool: 'grep', mode: s, filenames: [s], num_files: 1 },
    { tool: 'glob', filenames: [s], num_files: 1, truncated: true },
    { tool: 'todo', items: [{ content: s, status: s, active_form: s }] },
    { tool: 'task', agent_id: s, status: s, content: s, total_duration_ms: 1, total_tokens: 1, total_tool_use_count: 1 },
    { tool: 'web_fetch', url: s, code: 500, code_text: s, bytes: 1, duration_ms: 1, result: big },
    { tool: 'web_search', query: s, results: [{ title: s, url: s, snippet: s }] },
    { tool: 'ask_user_question', questions: [{ text: s, header: s, multi_select: false, options: [{ label: s, description: s }] }], answers: { [s]: s, 0: s } },
    { tool: 'kill_shell', shell_id: s, message: s },
    { tool: 'kill_shell', shell_id: s, message: '' },
    { tool: 'exit_plan_mode', plan: s, file_path: s },
    { tool: 'mcp', server_name: s, tool_name: s, raw: { [s]: s, nested: { [s]: s } } },
    { tool: 'generic', text: s, raw: null },
    { tool: 'generic', text: null, raw: { [s]: s } },
    { tool: s, x: s },
  ];
}

const NAMES = ['Bash', 'Edit', 'Read'];

test('hostile strings, a 10 000-character line and U+202E through every tool kind: no injected markup, no link', () => {
  const inputs = [...audit.HOSTILE, 'L'.repeat(10000), '‮gnp.exe', '‮' + 'x'.repeat(10000)];
  for (const hostile of inputs) {
    for (const result of hostileResults(hostile)) {
      for (const name of NAMES) {
        const item = tool(result, { name, input: { old_string: hostile, new_string: hostile + '2', file_path: hostile, command: hostile } });
        const html = T.render(item);
        assert.deepEqual(audit.problems(html), [], `${result.tool}/${name}: ${hostile.slice(0, 40)}`);
        assert.equal(frag(html).querySelectorAll('a').length, 0);
      }
    }
  }
});

test('hostile text survives as text: what the result holds is what the page shows', () => {
  const hostile = '<img src=x onerror=alert(1)>"><script>alert(1)</script>‮evil';
  const cases = [
    [{ tool: 'read', file_path: 'f', content: hostile, start_line: 1 }, '.an-code-text', hostile],
    [{ tool: 'bash', stdout: hostile, stderr: '' }, '.an-code-text', hostile],
    [{ tool: 'bash', stdout: '', stderr: hostile }, '.an-code-text', hostile],
    [{ tool: 'edit', file_path: 'f', diff: [{ kind: 'add', text: hostile, old_line: null, new_line: 1 }] }, '.an-code-text', hostile],
    [{ tool: 'grep', mode: 'content', filenames: [], content: hostile }, '.an-code-text', hostile],
    [{ tool: 'grep', mode: 'files_with_matches', filenames: ['C:\\x\\' + hostile.replace(/\//g, '|')] }, '.an-file', hostile.replace(/\//g, '|')],
    [{ tool: 'web_fetch', url: hostile, code: 200, result: hostile }, '.an-result-text', hostile],
    [{ tool: 'web_search', results: [{ title: hostile, snippet: hostile }] }, '.an-hit-title', hostile],
    [{ tool: 'task', status: 'completed', content: hostile }, '.an-result-text', hostile],
    [{ tool: 'todo', items: [{ content: hostile, status: 'pending' }] }, '.an-todo-text', hostile],
    [{ tool: 'ask_user_question', questions: [{ text: hostile }], answers: {} }, '.an-result-text', hostile],
    [{ tool: 'exit_plan_mode', plan: hostile }, '.an-result-text', hostile],
    [{ tool: 'kill_shell', message: hostile }, '.an-note', hostile],
    [{ tool: 'generic', text: hostile }, '.an-code-text', hostile],
  ];
  for (const [result, selector, expected] of cases) {
    const found = texts(frag(view(result)), selector);
    assert.ok(found.includes(expected), `${result.tool} ${selector}: ${JSON.stringify(found)}`);
  }
  // A file name keeps its whole text in the title, the last path part in the line.
  const file = frag(view({ tool: 'glob', filenames: ['C:\\dir\\' + hostile], truncated: false }));
  assert.equal(file.querySelector('.an-file').getAttribute('title'), 'C:\\dir\\' + hostile);
});

test('a 10 000-character line is drawn whole, in one row; the row cap counts lines, not characters', () => {
  const line = 'W'.repeat(10000);
  const root = frag(view({ tool: 'bash', stdout: line + '\n' + line, stderr: '' }));
  assert.deepEqual(texts(root, '.an-code-text'), [line, line]);
  const many = frag(view({ tool: 'bash', stdout: (line + '\n').repeat(40), stderr: '' }));
  assert.equal(many.querySelectorAll('.an-code-line').length, 15);
  const big = 'x\n'.repeat(200000);
  const started = Date.now();
  const cap = frag(view({ tool: 'bash', stdout: big, stderr: big }));
  assert.ok(Date.now() - started < 2000);
  assert.equal(cap.querySelectorAll('.an-code-line').length, 25);
});

test('a large edit is diffed in bounded time', () => {
  const a = Array.from({ length: 3000 }, (_, i) => 'old ' + i).join('\n');
  const b = Array.from({ length: 3000 }, (_, i) => 'new ' + i).join('\n');
  const started = Date.now();
  const html = T.render(tool(null, { name: 'Edit', input: { old_string: a, new_string: b, file_path: 'f.txt' } }));
  assert.ok(Date.now() - started < 2000);
  assert.equal(frag(html).querySelectorAll('.an-diff-line').length, 12);
});
