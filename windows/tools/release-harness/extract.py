#!/usr/bin/env python3
"""Reads a GitHub workflow file well enough to test release.yml without PyYAML.

The harness runs on the Mac's python3 (3.9) and on CI runners with nothing installed, so
this is a small indentation-based reader for the YAML the fork's workflows use: block
mappings and sequences, `|`/`>` block scalars, quoted and plain scalars, flow lists
(`[a, b]`) and comments. It is not a general YAML parser (no anchors, no flow mappings,
no multi-document files); anything it does not understand raises ParseError rather than
guessing, so a workflow written in another style fails the harness loudly.

Two views:
- load(text) -> Workflow: jobs with their `if`, needs, environment, permissions, env,
  outputs and steps (name, id, if, uses, with, env, run), all as plain values;
- raw_steps(text, job) -> [(label, [lines])]: each step's source lines exactly as written,
  for the byte-for-byte comparison in identical.py.

CLI:
  extract.py WORKFLOW                      summary of every job (JSON)
  extract.py WORKFLOW JOB                  that job (JSON)
  extract.py WORKFLOW JOB STEP --run       that step's run script, as bash would get it
"""

import json
import re
import sys

__all__ = ["ParseError", "parse", "load", "raw_steps", "Workflow", "Job", "Step"]


class ParseError(Exception):
    pass


# --- scalars -----------------------------------------------------------------------------

def _strip_comment(text):
    """Drops a trailing ` # comment` that is outside quotes."""
    quote = None
    for i, ch in enumerate(text):
        if quote:
            if ch == quote:
                quote = None
        elif ch in "'\"" and (i == 0 or text[i - 1] in " :[,"):
            quote = ch
        elif ch == "#" and (i == 0 or text[i - 1] in " \t"):
            return text[:i].rstrip()
    return text


def _scalar(text, where):
    text = _strip_comment(text.strip())
    if text == "":
        return None
    if text[0] == "'":
        if len(text) < 2 or text[-1] != "'":
            raise ParseError("%s: unterminated single-quoted string" % where)
        return text[1:-1].replace("''", "'")
    if text[0] == '"':
        if len(text) < 2 or text[-1] != '"':
            raise ParseError("%s: unterminated double-quoted string" % where)
        try:
            return json.loads(text)
        except ValueError:
            raise ParseError("%s: a double-quoted string this reader cannot decode" % where)
    if text[0] == "[":
        if text[-1] != "]":
            raise ParseError("%s: a flow list must end on its line" % where)
        inner = text[1:-1].strip()
        if not inner:
            return []
        return [_scalar(part, where) for part in inner.split(",")]
    if text[0] == "{":
        # Only the one-line, unnested form (`{ required: false }`).
        if text[-1] != "}" or "{" in text[1:] or "[" in text:
            raise ParseError("%s: only one-line flow mappings without nesting are read" % where)
        out = {}
        inner = text[1:-1].strip()
        for part in inner.split(",") if inner else []:
            key, sep, value = part.partition(":")
            if not sep or not key.strip():
                raise ParseError("%s: not a `key: value` pair in a flow mapping" % where)
            out[key.strip()] = _scalar(value, where)
        return out
    if text[0] in "&*!%@`":
        raise ParseError("%s: unsupported YAML (%r)" % (where, text[:20]))
    if text in ("true", "True", "TRUE"):
        return True
    if text in ("false", "False", "FALSE"):
        return False
    if text in ("null", "~"):
        return None
    if re.match(r"^-?[0-9]+$", text):
        return int(text)
    return text


# --- blocks ------------------------------------------------------------------------------

def _indent(line):
    return len(line) - len(line.lstrip(" "))


def _significant(line):
    stripped = line.strip()
    return stripped != "" and not stripped.startswith("#")


class _Reader(object):
    def __init__(self, text):
        if "\t" in text:
            # YAML forbids tabs for indentation; inside a run block they would be kept, but the
            # fork's workflows have none, so one is more likely a mistake than intended.
            for n, line in enumerate(text.split("\n"), 1):
                if "\t" in line[: _indent(line) + 1]:
                    raise ParseError("line %d: tab in indentation" % n)
        self.lines = text.split("\n")

    def next_significant(self, i):
        while i < len(self.lines) and not _significant(self.lines[i]):
            i += 1
        return i

    def node(self, i, min_indent):
        """The value whose first line is the next significant line at or after i, if that
        line is indented at least min_indent. Returns (value, next index)."""
        j = self.next_significant(i)
        if j >= len(self.lines) or _indent(self.lines[j]) < min_indent:
            return None, j
        line = self.lines[j]
        if line.strip() == "-" or line.strip().startswith("- "):
            return self.sequence(j, _indent(line))
        return self.mapping(j, _indent(line))

    def mapping(self, i, indent):
        out = {}
        while True:
            j = self.next_significant(i)
            if j >= len(self.lines):
                return out, j
            line = self.lines[j]
            ind = _indent(line)
            if ind < indent:
                return out, j
            if ind > indent:
                raise ParseError("line %d: unexpected indentation" % (j + 1))
            body = line[ind:]
            if body.startswith("- ") or body == "-":
                return out, j
            m = re.match(r"^([^\s:'\"#][^:]*?|'[^']*'|\"[^\"]*\"):(?:\s+(.*))?$", body)
            if not m:
                raise ParseError("line %d: not a `key: value` line: %r" % (j + 1, body[:40]))
            key = m.group(1)
            if key[0] in "'\"":
                key = key[1:-1]
            rest = (m.group(2) or "").rstrip()
            if key in out:
                raise ParseError("line %d: duplicate key %r" % (j + 1, key))
            where = "line %d" % (j + 1)
            bare = _strip_comment(rest)
            if bare == "":
                # A nested block; a sequence may sit at the key's own indentation.
                k = self.next_significant(j + 1)
                if k < len(self.lines) and _indent(self.lines[k]) == indent and (
                        self.lines[k].strip().startswith("- ") or self.lines[k].strip() == "-"):
                    value, i = self.sequence(k, indent)
                else:
                    value, i = self.node(j + 1, indent + 1)
            elif re.match(r"^[|>][+-]?$", bare):
                value, i = self.block_scalar(j + 1, indent, bare)
            else:
                value, i = _scalar(rest, where), j + 1
            out[key] = value

    def sequence(self, i, indent):
        out = []
        while True:
            j = self.next_significant(i)
            if j >= len(self.lines):
                return out, j
            line = self.lines[j]
            ind = _indent(line)
            if ind < indent:
                return out, j
            body = line[ind:]
            if ind > indent or not (body.startswith("- ") or body == "-"):
                if ind == indent:
                    return out, j
                raise ParseError("line %d: unexpected indentation" % (j + 1))
            item = body[2:] if body != "-" else ""
            if item.strip() == "":
                value, i = self.node(j + 1, indent + 1)
            elif re.match(r"^([^\s:'\"#\[][^:]*?|'[^']*'|\"[^\"]*\"):(\s|$)", item):
                # `- key: value`: a mapping whose first key sits two columns in. Re-read the
                # line as if the dash were a space, then the mapping goes on below it.
                saved = self.lines[j]
                self.lines[j] = " " * (indent + 2) + item
                try:
                    value, i = self.mapping(j, indent + 2)
                finally:
                    self.lines[j] = saved
            else:
                value, i = _scalar(item, "line %d" % (j + 1)), j + 1
            out.append(value)

    def block_scalar(self, i, parent_indent, style):
        """`|`, `|-`, `>`, `>-`: the lines indented deeper than the key, blank ones kept."""
        body = []
        j = i
        block_indent = None
        while j < len(self.lines):
            line = self.lines[j]
            if line.strip() == "":
                body.append("")
                j += 1
                continue
            ind = _indent(line)
            if ind <= parent_indent:
                break
            if block_indent is None:
                block_indent = ind
            if ind < block_indent:
                raise ParseError("line %d: block scalar line indented less than its first" % (j + 1))
            body.append(line[block_indent:])
            j += 1
        # Trailing blank lines belong to no one (clip and strip chomping drop them).
        while body and body[-1] == "":
            body.pop()
        end = j
        if style[0] == "|":
            text = "\n".join(body)
        else:
            # Folded: lines join with a space; more-indented and blank lines keep their breaks.
            text = ""
            for n, part in enumerate(body):
                if n == 0:
                    text = part
                elif part == "" or part.startswith(" ") or body[n - 1] == "" or body[n - 1].startswith(" "):
                    text += "\n" + part
                else:
                    text += " " + part
        if style.endswith("-"):
            return text, end
        if style.endswith("+"):
            raise ParseError("line %d: keep chomping (`+`) is not supported" % i)
        return (text + "\n") if body else "", end


def parse(text):
    """The whole document as nested dicts and lists."""
    reader = _Reader(text)
    value, i = reader.node(0, 0)
    i = reader.next_significant(i)
    if i < len(reader.lines):
        raise ParseError("line %d: could not read this line" % (i + 1))
    return value if value is not None else {}


# --- the workflow view -------------------------------------------------------------------

class Step(object):
    def __init__(self, job, index, data):
        if not isinstance(data, dict):
            raise ParseError("job %s step %d is not a mapping" % (job, index + 1))
        self.job = job
        self.index = index
        self.data = data
        self.name = data.get("name")
        self.id = data.get("id")
        self.if_ = data.get("if")
        self.uses = data.get("uses")
        self.with_ = data.get("with") or {}
        self.env = data.get("env") or {}
        self.run = data.get("run")
        self.working_directory = data.get("working-directory")
        self.continue_on_error = data.get("continue-on-error")

    @property
    def label(self):
        return self.name or self.uses or ("step %d" % (self.index + 1))

    def as_dict(self):
        return {"name": self.name, "id": self.id, "if": self.if_, "uses": self.uses,
                "with": self.with_, "env": self.env, "run": self.run}


class Job(object):
    def __init__(self, job_id, data):
        if not isinstance(data, dict):
            raise ParseError("job %s is not a mapping" % job_id)
        self.id = job_id
        self.data = data
        self.name = data.get("name")
        self.if_ = data.get("if")
        needs = data.get("needs") or []
        self.needs = [needs] if isinstance(needs, str) else list(needs)
        self.environment = data.get("environment")
        self.permissions = data.get("permissions")
        self.env = data.get("env") or {}
        self.outputs = data.get("outputs") or {}
        self.uses = data.get("uses")
        self.with_ = data.get("with") or {}
        self.secrets = data.get("secrets")
        self.runs_on = data.get("runs-on")
        defaults = (data.get("defaults") or {}).get("run") or {}
        self.working_directory = defaults.get("working-directory")
        self.steps = [Step(job_id, n, s) for n, s in enumerate(data.get("steps") or [])]

    def step(self, name):
        for s in self.steps:
            if s.name == name:
                return s
        raise KeyError("job %s has no step named %r" % (self.id, name))

    def as_dict(self):
        return {"name": self.name, "if": self.if_, "needs": self.needs,
                "environment": self.environment, "permissions": self.permissions,
                "env": self.env, "outputs": self.outputs, "uses": self.uses,
                "with": self.with_, "secrets": self.secrets,
                "steps": [s.as_dict() for s in self.steps]}


class Workflow(object):
    def __init__(self, data, text):
        self.data = data
        self.text = text
        self.name = data.get("name")
        # YAML 1.1 reads a bare `on` as true; this reader keeps it a string key.
        self.on = data.get("on")
        self.permissions = data.get("permissions")
        self.env = data.get("env") or {}
        self.concurrency = data.get("concurrency")
        jobs = data.get("jobs")
        if not isinstance(jobs, dict):
            raise ParseError("no jobs")
        self.jobs = dict((k, Job(k, v)) for k, v in jobs.items())
        self.job_order = list(jobs.keys())

    def job(self, job_id):
        if job_id not in self.jobs:
            raise KeyError("no job %r" % job_id)
        return self.jobs[job_id]


def load(text):
    return Workflow(parse(text), text)


def raw_steps(text, job_id):
    """Each step of `job_id` as its exact source lines, comments outside the run block and
    blank lines at its end left out (they introduce the next step)."""
    lines = text.split("\n")
    try:
        start = lines.index("  %s:" % job_id)
    except ValueError:
        raise KeyError("no job %r" % job_id)
    steps = []
    current = None
    in_steps = False
    for line in lines[start + 1:]:
        if re.match(r"^  \S", line) or re.match(r"^\S", line):
            break
        if line == "    steps:":
            in_steps = True
            continue
        if not in_steps:
            continue
        if re.match(r"^    \S", line):
            break
        if line.startswith("      - "):
            current = [line]
            steps.append(current)
        elif current is not None:
            current.append(line)
    out = []
    for block in steps:
        while block and (block[-1].strip() == "" or
                         (block[-1].strip().startswith("#") and _indent(block[-1]) <= 8)):
            block.pop()
        label = None
        for line in block:
            m = re.match(r"^      (?:- |  )name: (.*)$", line)
            if m:
                label = m.group(1)
                break
        if label is None:
            m = re.match(r"^      - uses: (.*)$", block[0])
            label = m.group(1) if m else block[0].strip()
        out.append((label, block))
    return out


def _main(argv):
    if len(argv) < 2:
        sys.stderr.write(__doc__)
        return 2
    with open(argv[1]) as f:
        wf = load(f.read())
    if len(argv) == 2:
        summary = {}
        for job_id in wf.job_order:
            job = wf.jobs[job_id]
            summary[job_id] = {"if": job.if_, "needs": job.needs, "environment": job.environment,
                               "permissions": job.permissions,
                               "steps": [s.label for s in job.steps]}
        print(json.dumps(summary, indent=2))
        return 0
    job = wf.job(argv[2])
    if len(argv) == 3:
        print(json.dumps(job.as_dict(), indent=2))
        return 0
    step = job.step(argv[3])
    if "--run" in argv[4:]:
        sys.stdout.write(step.run or "")
    else:
        print(json.dumps(step.as_dict(), indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(_main(sys.argv))
