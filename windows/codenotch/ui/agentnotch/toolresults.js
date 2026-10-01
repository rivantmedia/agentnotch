// What each tool returned, as the chat shows it under the tool's line (ToolResultViews.swift):
// file excerpts with line numbers, diffs, command output, matches, todos, subagent results,
// fetched pages and search hits, answers, MCP results. Code sits in the soft code box; added
// and removed lines take the review and critical colours.
//
// Tool output is untrusted text: every value goes through `agentnotchCommon.esc`, and nothing a
// tool returned is ever interpreted as markup.
//
// One IIFE, one global (`window.agentnotchToolResults`), no top-level let/const.
(function () {
  'use strict';

  var C = window.agentnotchCommon;
  var esc = C.esc;

  // ---- names ----------------------------------------------------------------------------

  var ALIASES = {
    AgentOutputTool: 'Await Agent', AskUserQuestion: 'Question', TodoWrite: 'Todo', TodoRead: 'Todo',
    WebFetch: 'Fetch', WebSearch: 'Search', NotebookEdit: 'Notebook', BashOutput: 'Bash', KillShell: 'Shell',
    EnterPlanMode: 'Plan', ExitPlanMode: 'Plan', SlashCommand: 'Command',
  };

  function titleCase(snake) {
    return String(snake).split('_').filter(Boolean).map(function (w) {
      return w.charAt(0).toUpperCase() + w.slice(1).toLowerCase();
    }).join(' ');
  }

  /** "mcp__deepwiki__ask_question" → "Deepwiki - Ask Question"; aliases for Claude's own tools. */
  function toolName(id) {
    var name = String(id == null ? '' : id);
    if (Object.prototype.hasOwnProperty.call(ALIASES, name)) return ALIASES[name];
    if (name.indexOf('mcp__') !== 0) return name;
    var rest = name.slice(5).replace(/^_+/, '');
    var cut = rest.indexOf('_');
    if (cut < 0) return titleCase(rest) || name;
    var server = titleCase(rest.slice(0, cut));
    var tool = rest.slice(cut + 1).replace(/^_/, '');
    return tool ? server + ' - ' + titleCase(tool) : server;
  }

  /** The last part of a Windows or Unix path (a loop: `/[\\/]+$/` is quadratic on a run of slashes). */
  function baseName(path) {
    var p = String(path == null ? '' : path);
    var end = p.length;
    while (end > 0 && (p.charAt(end - 1) === '/' || p.charAt(end - 1) === '\\')) end -= 1;
    p = p.slice(0, end);
    var cut = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
    return cut >= 0 ? p.slice(cut + 1) : p;
  }

  /** Swift's `capitalized`: each word starts upper case, the rest lower ("in_progress" is one word). */
  function capitalized(s) {
    return String(s == null ? '' : s).toLowerCase().replace(/(^|[^\p{L}\p{N}_'])(\p{L})/gu, function (m, before, letter) {
      return before + letter.toUpperCase();
    });
  }

  // ---- building blocks ------------------------------------------------------------------

  function note(text, cls) {
    return '<div class="an-note' + (cls ? ' ' + cls : '') + '">' + esc(text) + '</div>';
  }

  function resultText(text, lines) {
    return '<div class="an-result-text" style="-webkit-line-clamp:' + lines + '">' + esc(text) + '</div>';
  }

  function codeBox(header, body) {
    return '<div class="an-code">' + (header ? '<div class="an-code-head" title="' + esc(header) + '">' + esc(header) + '</div>' : '') +
      '<div class="an-code-body">' + body + '</div></div>';
  }

  function lines(text) {
    return String(text == null ? '' : text).split('\n');
  }

  function fileCode(filename, content, startLine, maxLines) {
    var all = lines(content);
    var start = Math.max(1, Number(startLine) || 1);
    var body = start > 1 ? '<div class="an-code-more an-code-gutterless">…</div>' : '';
    all.slice(0, maxLines).forEach(function (line, index) {
      body += '<div class="an-code-line"><span class="an-code-num">' + (start + index) + '</span><span class="an-code-text">' +
        (line ? esc(line) : '&nbsp;') + '</span></div>';
    });
    if (all.length > maxLines) body += '<div class="an-code-more an-code-gutterless">' + esc((all.length - maxLines) + ' more lines') + '</div>';
    return codeBox(filename, body);
  }

  function codePreview(content, maxLines, critical) {
    var all = lines(content);
    var body = '';
    all.slice(0, maxLines).forEach(function (line) {
      body += '<div class="an-code-line an-code-plain' + (critical ? ' an-critical' : '') + '"><span class="an-code-text">' +
        (line ? esc(line) : '&nbsp;') + '</span></div>';
    });
    if (all.length > maxLines) body += '<div class="an-code-more">' + esc((all.length - maxLines) + ' more lines') + '</div>';
    return codeBox(null, body);
  }

  function fileList(files, limit) {
    var out = '<div class="an-files">';
    (files || []).slice(0, limit).forEach(function (file) {
      out += '<div class="an-file" title="' + esc(file) + '">' + esc(baseName(file)) + '</div>';
    });
    if ((files || []).length > limit) out += note('and ' + (files.length - limit) + ' more');
    return out + '</div>';
  }

  // ---- diffs (LineDiff, DiffLineRow) ----------------------------------------------------

  var MAX_DIFF_LINES = 12;
  // Context and hunk lines are free, but a diff of many hunks must not grow without end.
  var MAX_DIFF_ROWS = 48;
  var MAX_TABLE_CELLS = 250000;

  function lcs(a, b) {
    if (!a.length || !b.length) return [];
    var rows = a.length + 1, cols = b.length + 1;
    var table = new Array(rows);
    for (var r = 0; r < rows; r++) table[r] = new Int32Array(cols);
    for (var i = 1; i < rows; i++) {
      for (var j = 1; j < cols; j++) {
        table[i][j] = a[i - 1] === b[j - 1] ? table[i - 1][j - 1] + 1 : Math.max(table[i - 1][j], table[i][j - 1]);
      }
    }
    var out = [];
    i = a.length;
    j = b.length;
    while (i > 0 && j > 0) {
      if (a[i - 1] === b[j - 1]) {
        out.push(a[i - 1]);
        i -= 1;
        j -= 1;
      } else if (table[i - 1][j] > table[i][j - 1]) i -= 1;
      else j -= 1;
    }
    return out.reverse();
  }

  /**
   * The changed lines between two texts (LineDiff.changes): shared start and end set aside,
   * then a longest-common-subsequence diff of what is left, old lines numbered in the old text
   * and new ones in the new. Past `limit` changed lines it stops and says it was cut.
   */
  function lineDiff(oldText, newText, limit) {
    var a = lines(oldText), b = lines(newText);
    var start = 0;
    while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
    var end = 0;
    while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end += 1;
    var oldMid = a.slice(start, a.length - end), newMid = b.slice(start, b.length - end);
    var common = oldMid.length * newMid.length <= MAX_TABLE_CELLS ? lcs(oldMid, newMid) : [];
    var out = [];
    var i = 0, j = 0, k = 0, truncated = false;
    while (i < oldMid.length || j < newMid.length) {
      var shared = k < common.length ? common[k] : null;
      if (i < oldMid.length && (shared === null || oldMid[i] !== shared)) {
        out.push({ kind: 'remove', text: oldMid[i], number: start + i + 1 });
        i += 1;
      } else if (j < newMid.length && (shared === null || newMid[j] !== shared)) {
        out.push({ kind: 'add', text: newMid[j], number: start + j + 1 });
        j += 1;
      } else {
        i += 1;
        j += 1;
        k += 1;
      }
      if (out.length > limit) {
        truncated = true;
        out.pop();
        break;
      }
    }
    return { lines: out, truncated: truncated };
  }

  function diffRow(kind, text, number) {
    // The kind is the engine's word, but it decides a class name: only the four are let through.
    kind = kind === 'add' || kind === 'remove' || kind === 'hunk' ? kind : 'context';
    var sign = kind === 'add' ? '+' : kind === 'remove' ? '−' : kind === 'hunk' ? '' : ' ';
    return '<div class="an-diff-line an-diff-' + kind + '">' +
      (number != null ? '<span class="an-code-num">' + esc(number) + '</span>' : '<span class="an-code-num"></span>') +
      '<span class="an-diff-sign">' + sign + '</span><span class="an-code-text">' + (text ? esc(text) : '&nbsp;') + '</span></div>';
  }

  /** The engine's diff lines (hunk, context, add, remove), up to 12 changed ones. */
  function diffView(diff, filename) {
    var body = '';
    var changed = 0;
    var cut = false;
    for (var i = 0; i < (diff || []).length; i++) {
      var d = diff[i] || {};
      if (i >= MAX_DIFF_ROWS) {
        cut = true;
        break;
      }
      if (d.kind === 'add' || d.kind === 'remove') {
        if (changed >= MAX_DIFF_LINES) {
          cut = true;
          break;
        }
        changed += 1;
      }
      var number = d.kind === 'remove' ? d.old_line : d.kind === 'hunk' ? null : d.new_line;
      body += diffRow(d.kind, d.text, number);
    }
    if (cut) body += '<div class="an-code-more">…</div>';
    return codeBox(filename, body);
  }

  /** An Edit's change from its input, before (or without) the engine's diff. */
  function inputDiff(input, filename) {
    var oldText = input && typeof input.old_string === 'string' ? input.old_string : '';
    var newText = input && typeof input.new_string === 'string' ? input.new_string : '';
    if (!oldText && !newText) return '';
    var d = lineDiff(oldText, newText, MAX_DIFF_LINES);
    var body = d.lines.map(function (l) {
      return diffRow(l.kind, l.text, l.number);
    }).join('');
    if (d.truncated) body += '<div class="an-code-more">…</div>';
    var name = filename || (input && input.file_path ? baseName(input.file_path) : 'file');
    return codeBox(name, body);
  }

  // ---- per tool -------------------------------------------------------------------------

  function taskDuration(ms) {
    var n = Math.floor(Number(ms) || 0);
    if (n >= 60000) return Math.floor(n / 60000) + 'm ' + Math.floor((n % 60000) / 1000) + 's';
    if (n >= 1000) return Math.floor(n / 1000) + 's';
    return n + 'ms';
  }

  function stack(parts) {
    var body = parts.filter(Boolean).join('');
    return body ? '<div class="an-result">' + body + '</div>' : '';
  }

  var RENDER = {
    read: function (r) {
      return r.content ? fileCode(baseName(r.file_path), r.content, r.start_line, 10) : '';
    },
    edit: function (r, input) {
      var diff = r.diff && r.diff.length ? diffView(r.diff, baseName(r.file_path)) : inputDiff(input, baseName(r.file_path));
      return stack([diff, r.user_modified ? note('You changed it before it was applied') : '']);
    },
    write: function (r) {
      var parts = [note((r.created ? 'Created ' : 'Wrote ') + baseName(r.file_path))];
      if (r.created && r.content) parts.push(codePreview(r.content, 8));
      else if (r.diff && r.diff.length) parts.push(diffView(r.diff, null));
      return stack(parts);
    },
    bash: function (r) {
      var parts = [];
      if (r.background_task_id) parts.push(note('Running in the background (' + r.background_task_id + ')'));
      if (r.return_code_interpretation) parts.push(note(r.return_code_interpretation));
      if (r.stdout) parts.push(codePreview(r.stdout, 15));
      if (r.stderr) parts.push(codePreview(r.stderr, 10, true));
      if (!r.stdout && !r.stderr && !r.background_task_id && !r.return_code_interpretation) parts.push(note('No output'));
      return stack(parts);
    },
    bash_output: function (r) {
      var head = '<div class="an-result-head">' + note(capitalized(r.status)) +
        (r.exit_code != null ? '<span class="an-mono-caption' + (r.exit_code === 0 ? '' : ' an-critical') + '">exit ' + esc(r.exit_code) + '</span>' : '') +
        '</div>';
      return stack([head, r.stdout ? codePreview(r.stdout, 10) : '', r.stderr ? codePreview(r.stderr, 5, true) : '']);
    },
    grep: function (r) {
      if (r.mode === 'content') return r.content ? codePreview(r.content, 15) : note('No matches');
      if (r.mode === 'count') return note(C.plural(r.num_files || 0, 'file') + ' with matches');
      return r.filenames && r.filenames.length ? fileList(r.filenames, 10) : note('No matches');
    },
    glob: function (r) {
      if (!r.filenames || !r.filenames.length) return note('No files');
      return stack([fileList(r.filenames, 10), r.truncated ? note('More were found than are listed') : '']);
    },
    todo: function (r) {
      return '<div class="an-todos">' + (r.items || []).map(function (t) {
        var status = t.status === 'completed' || t.status === 'in_progress' ? t.status : 'pending';
        var mark = status === 'completed' ? '✓' : status === 'in_progress' ? '›' : '·';
        return '<div class="an-todo an-todo-' + status + '"><span class="an-todo-mark">' + mark +
          '</span><span class="an-todo-text">' + esc(t.content) + '</span></div>';
      }).join('') + '</div>';
    },
    task: function (r) {
      var failed = r.status === 'failed' || r.status === 'error';
      var head = '<div class="an-result-head"><span class="an-task-status' + (failed ? ' an-critical' : '') + '">' +
        esc(capitalized(r.status)) + '</span>' +
        (r.total_duration_ms != null ? note(taskDuration(r.total_duration_ms)) : '') +
        (r.total_tool_use_count != null ? note(C.plural(r.total_tool_use_count, 'tool')) : '') + '</div>';
      return stack([head, r.content ? resultText(r.content, 5) : '']);
    },
    web_fetch: function (r) {
      var head = '<div class="an-result-head"><span class="an-mono-caption' + (r.code < 400 ? '' : ' an-critical') + '">' +
        esc(r.code) + '</span><span class="an-mono-caption an-ellipsis" title="' + esc(r.url) + '">' + esc(r.url) + '</span></div>';
      return stack([head, r.result ? resultText(r.result, 8) : '']);
    },
    web_search: function (r) {
      var results = r.results || [];
      if (!results.length) return note('No results');
      var out = results.slice(0, 5).map(function (item) {
        return '<div class="an-hit"><div class="an-hit-title">' + esc(item.title) + '</div>' +
          (item.snippet ? resultText(item.snippet, 2) : '') + '</div>';
      }).join('');
      if (results.length > 5) out += note('and ' + (results.length - 5) + ' more');
      return '<div class="an-hits">' + out + '</div>';
    },
    ask_user_question: function (r) {
      var answers = r.answers || {};
      return stack((r.questions || []).map(function (q, index) {
        var answer = answers[q.text] != null ? answers[q.text] : answers[String(index)];
        return '<div class="an-qa">' + resultText(q.text, 3) +
          (answer != null ? '<div class="an-qa-answer">↳ ' + esc(answer) + '</div>' : '') + '</div>';
      }));
    },
    kill_shell: function (r) {
      return note(r.message ? r.message : 'Shell ' + r.shell_id + ' stopped');
    },
    exit_plan_mode: function (r) {
      return stack([r.file_path ? note(baseName(r.file_path)) : '', r.plan ? resultText(r.plan, 6) : '']);
    },
    mcp: function (r) {
      var rows = '';
      if (r.raw && typeof r.raw === 'object' && !Array.isArray(r.raw)) {
        Object.keys(r.raw).slice(0, 5).forEach(function (key) {
          var v = r.raw[key];
          var text = typeof v === 'string' ? v : JSON.stringify(v);
          rows += '<div class="an-kv"><span class="an-kv-key">' + esc(key) + '</span><span class="an-kv-value">' +
            esc(String(text).slice(0, 100)) + '</span></div>';
        });
      }
      return stack([note(titleCase(r.server_name) + ' · ' + titleCase(r.tool_name)), rows]);
    },
    generic: function (r) {
      if (r.text) return codePreview(r.text, 15);
      if (r.raw != null) return codePreview(typeof r.raw === 'string' ? r.raw : JSON.stringify(r.raw, null, 2), 15);
      return note('Completed');
    },
  };

  /**
   * A tool item's result body, or '' when there is nothing to show. `item` is a chat item of
   * kind `tool` ({name, input, status, result}).
   */
  function render(item) {
    try {
      var result = item && item.result;
      var kind = result && typeof result.tool === 'string' ? result.tool : '';
      if (Object.prototype.hasOwnProperty.call(RENDER, kind)) return RENDER[kind](result, (item && item.input) || {});
      if (item && item.name === 'Edit') return inputDiff((item && item.input) || {}, null);
    } catch (e) {
      // A result of a shape the engine does not send: draw nothing rather than half a view.
      // The line carries the kind only, never the text.
      C.log('toolresults: could not draw a result');
    }
    return '';
  }

  window.agentnotchToolResults = {
    render: render,
    toolName: toolName,
    baseName: baseName,
    lineDiff: lineDiff,
    inputDiff: inputDiff,
    diffView: diffView,
    MAX_DIFF_LINES: MAX_DIFF_LINES,
  };
})();
