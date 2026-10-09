#!/usr/bin/env python3
"""The release workflow harness: runs release.yml's shell steps against a fake GitHub.

release.yml can only really run on main, after the Windows port is merged, and then it
publishes. So its logic is proven here, before that: every scenario in scenarios/*.json
plays one Release run through the job graph as GitHub would (job and step `if:`
expressions evaluated with the operators release.yml uses, needs results, outputs,
artifacts handed between jobs), and runs the real shell steps of `plan`, `keys`,
`sign-windows` and `publish` in bash, unchanged, with:

- a fake gh (fake-gh) answering from the scenario's state and refusing anything else,
  and a fake curl (fake-curl) serving only the feeds a published release would; both
  record every call. They come first on PATH, and each step first checks that `gh` and
  `curl` resolve to them, so nothing reaches GitHub. Each job's token is named after its
  `contents` permission, and the fake gh shows a read token what GitHub shows it: no
  drafts, and no writes;
- "Re-run failed jobs" when a scenario has `rerun`: the failed jobs and the jobs after
  them run again against what the first attempt left, plus any releases published in the
  meantime (`rerun.addReleases`), with the other jobs' outputs (plan's among them) as they
  were;
- the real windows/agentnotch-release binary from the workspace (built if missing) in
  `keys` and `sign-windows`, fed only the DESIGN-WIN Appendix B test seed;
- the jobs that build (release-tool, mac, windows) simulated: they hand over the files
  their real counterparts would (dummy bytes, the real tool binary), or fail on demand.

`website` is never run (it talks to GitHub's OIDC endpoint and the website); its `if:` is
evaluated and checked. Then structural checks over release.yml: where secrets and
environments may appear, that the seed only reaches a hash-checked tool on stdin, and
which jobs may write.

Usage: run.py [-v] [--workflow RELEASE_YML] [SCENARIO-NAME-PART ...]
Prints one line per scenario and per structural check; exit 0 when all pass, 1 otherwise.
Needs bash, jq and python3 (3.9+); cargo only when the tool has not been built yet.
"""

import base64
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
WINDOWS = os.path.join(ROOT, "windows")
RELEASE_YML = os.path.join(ROOT, ".github", "workflows", "release.yml")
sys.path.insert(0, HERE)

import extract  # noqa: E402

# DESIGN-WIN Appendix B: a test seed only (base64 of the bytes 0x00..0x1f), never a real key.
TEST_SEED = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="
TEST_KEY_ID = "B5A5638361FBD019"
# Some other Sparkle public key (32 bytes), for "the key changed" scenarios.
OTHER_SPARKLE_KEY = base64.b64encode(bytes(range(32, 64))).decode()
REPO = "rivantmedia/agentnotch"
SHA = "c0ffee" * 6 + "abcd"
OTHER_SHA = "badc0de" * 5 + "12345"
PUBLIC_KEY_HEADER = "# Agent Notch's update public key (harness copy)\n"
OTHER_PIN = ("# a pin of another key\n"
             "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEJCQUUzNDY2ODFGQTAyOEQK"
             "UldTTkF2cUJaalN1dTR5bSt0endrUjNXajF1QmFKa25xa2RQZlo1azZtaCtqZjN6OHR2NXdRUnMK\n")
JOB_ORDER = ["checks", "plan", "release-tool", "keys", "mac", "windows", "sign-windows", "publish", "website"]
RUN_JOBS = ("plan", "keys", "sign-windows", "publish")


# --- expressions -------------------------------------------------------------------------

class ExprError(Exception):
    pass


_TOKEN = re.compile(r"\s*(?:(?P<str>'(?:[^']|'')*')|(?P<num>-?[0-9]+(?:\.[0-9]+)?)|"
                    r"(?P<op>==|!=|&&|\|\||<=|>=|[!()\[\].,<>])|(?P<id>[A-Za-z_][A-Za-z0-9_-]*))")


def _tokens(text):
    out, i = [], 0
    while i < len(text):
        if text[i:].strip() == "":
            break
        m = _TOKEN.match(text, i)
        if not m:
            raise ExprError("cannot read %r at %r" % (text, text[i:i + 10]))
        i = m.end()
        kind = m.lastgroup
        out.append((kind, m.group(kind)))
    return out


def _truthy(v):
    return not (v is None or v is False or v == "" or (isinstance(v, (int, float)) and not isinstance(v, bool) and v == 0))


def _number(v):
    if v is None:
        return 0.0
    if isinstance(v, bool):
        return 1.0 if v else 0.0
    if isinstance(v, (int, float)):
        return float(v)
    if isinstance(v, str):
        s = v.strip()
        if s == "":
            return 0.0
        try:
            return float(int(s, 16)) if s.lower().startswith("0x") else float(s)
        except ValueError:
            return float("nan")
    return float("nan")


def _equal(a, b):
    """GitHub's loose equality: same types compare directly (strings ignoring case),
    different ones as numbers."""
    if isinstance(a, str) and isinstance(b, str):
        return a.lower() == b.lower()
    if a is None and b is None:
        return True
    if isinstance(a, bool) and isinstance(b, bool):
        return a == b
    if isinstance(a, (dict, list)) or isinstance(b, (dict, list)):
        return a is b
    x, y = _number(a), _number(b)
    return x == y  # NaN never equals anything


def to_string(v):
    if v is None:
        return ""
    if v is True:
        return "true"
    if v is False:
        return "false"
    if isinstance(v, float) and v.is_integer():
        return str(int(v))
    if isinstance(v, (dict, list)):
        return json.dumps(v)
    return str(v)


class Expr(object):
    """Evaluates one GitHub Actions expression against `ctx` (nested dicts) and `status`
    (a dict of the status functions' values)."""

    def __init__(self, text, ctx, status):
        self.toks = _tokens(text)
        self.i = 0
        self.ctx = ctx
        self.status = status
        self.text = text

    def peek(self, value=None):
        if self.i >= len(self.toks):
            return None
        tok = self.toks[self.i]
        if value is not None and tok[1] != value:
            return None
        return tok

    def take(self, value=None):
        tok = self.peek(value)
        if tok is None:
            raise ExprError("expected %r in %r" % (value, self.text))
        self.i += 1
        return tok

    def evaluate(self):
        v = self.or_()
        if self.i != len(self.toks):
            raise ExprError("unexpected %r in %r" % (self.toks[self.i][1], self.text))
        return v

    def or_(self):
        v = self.and_()
        while self.peek("||"):
            self.take()
            w = self.and_()
            v = v if _truthy(v) else w
        return v

    def and_(self):
        v = self.eq()
        while self.peek("&&"):
            self.take()
            w = self.eq()
            v = w if _truthy(v) else v
        return v

    def eq(self):
        v = self.unary()
        while self.peek("==") or self.peek("!="):
            op = self.take()[1]
            w = self.unary()
            v = _equal(v, w) if op == "==" else not _equal(v, w)
        return v

    def unary(self):
        if self.peek("!"):
            self.take()
            return not _truthy(self.unary())
        return self.primary()

    def primary(self):
        kind, value = self.take()
        if value == "(":
            v = self.or_()
            self.take(")")
            return v
        if kind == "str":
            return value[1:-1].replace("''", "'")
        if kind == "num":
            return float(value)
        if kind != "id":
            raise ExprError("unexpected %r in %r" % (value, self.text))
        if value in ("true", "false"):
            return value == "true"
        if value == "null":
            return None
        if self.peek("("):
            self.take()
            args = []
            while not self.peek(")"):
                args.append(self.or_())
                if self.peek(","):
                    self.take()
            self.take(")")
            return self.call(value, args)
        v = self.ctx.get(value)
        while self.peek(".") or self.peek("["):
            if self.take()[1] == ".":
                v = _member(v, self.take()[1])
            else:
                key = self.or_()
                self.take("]")
                v = _member(v, to_string(key))
        return v

    def call(self, name, args):
        if name in ("success", "failure", "cancelled", "always"):
            if args:
                raise ExprError("%s() takes no arguments" % name)
            return self.status[name]
        if name == "contains":
            hay, needle = args
            if isinstance(hay, list):
                return any(_equal(x, needle) for x in hay)
            return to_string(needle).lower() in to_string(hay).lower()
        if name == "startsWith":
            return to_string(args[0]).lower().startswith(to_string(args[1]).lower())
        raise ExprError("the harness does not know %s()" % name)


def _member(v, key):
    if not isinstance(v, dict):
        return None
    if key in v:
        return v[key]
    for k in v:
        if k.lower() == key.lower():
            return v[k]
    return None


def evaluate(text, ctx, status):
    text = text.strip()
    if text.startswith("${{") and text.endswith("}}") and text.count("${{") == 1:
        text = text[3:-2]
    return Expr(text, ctx, status).evaluate()


def uses_status(text):
    return re.search(r"\b(success|failure|cancelled|always)\s*\(", text) is not None


def interpolate(value, ctx, status):
    """`${{ … }}` replaced in a string (env and with values)."""
    if not isinstance(value, str):
        return to_string(value)
    return re.sub(r"\$\{\{(.*?)\}\}", lambda m: to_string(evaluate(m.group(1), ctx, status)), value)


# --- one Release run ----------------------------------------------------------------------

class StepResult(object):
    def __init__(self, label, result, output="", annotations=None):
        self.label = label
        self.result = result
        self.output = output
        self.annotations = annotations or []


class JobResult(object):
    def __init__(self, job_id, result):
        self.id = job_id
        self.result = result
        self.outputs = {}
        self.steps = []
        self.note = ""

    @property
    def annotations(self):
        out = []
        for s in self.steps:
            out.extend(s.annotations)
        return out


def _annotations(output):
    out = []
    for line in output.split("\n"):
        m = re.match(r"^::(error|warning|notice)(?: [^:]*)?::(.*)$", line)
        if m:
            out.append((m.group(1), m.group(2)))
    return out


def _read_outputs(path):
    out = {}
    if not os.path.exists(path):
        return out
    with open(path) as f:
        lines = f.read().split("\n")
    i = 0
    while i < len(lines):
        line = lines[i]
        m = re.match(r"^([A-Za-z0-9_-]+)<<(.+)$", line)
        if m:
            delim, body = m.group(2), []
            i += 1
            while i < len(lines) and lines[i] != delim:
                body.append(lines[i])
                i += 1
            if i >= len(lines):
                raise RuntimeError("GITHUB_OUTPUT: %s<<%s never closed" % (m.group(1), delim))
            out[m.group(1)] = "\n".join(body)
        elif "=" in line:
            k, v = line.split("=", 1)
            out[k] = v
        elif line.strip():
            raise RuntimeError("GITHUB_OUTPUT: unreadable line %r" % line)
        i += 1
    return out


def _sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


class Harness(object):
    def __init__(self, workflow, tool, base, verbose):
        self.wf = workflow
        self.tool = tool
        self.base = base
        self.verbose = verbose
        self.bash = shutil.which("bash")
        self.shims = os.path.join(base, "shims")
        os.makedirs(self.shims)
        for name, source in (("gh", "fake-gh"), ("curl", "fake-curl")):
            shutil.copy(os.path.join(HERE, source), os.path.join(self.shims, name))
        self._script("sleep", '#!/bin/sh\n# The feed check waits between tries; the harness need not.\nexit 0\n')
        if not shutil.which("sha256sum"):
            # The Mac has shasum, not GNU's sha256sum; same output format.
            self._script("sha256sum", '#!/bin/sh\nexec shasum -a 256 "$@"\n')
        for name in os.listdir(self.shims):
            os.chmod(os.path.join(self.shims, name), 0o755)
        derived = self._tool(["derive-public"], stdin=TEST_SEED)
        self.keys = json.loads(derived)
        if self.keys.get("key_id") != TEST_KEY_ID:
            raise SystemExit("the release tool derived key id %s from the Appendix B seed, not %s"
                             % (self.keys.get("key_id"), TEST_KEY_ID))
        self.feeds = {"test-key": self._previous_feed_test_key(), "other-key": self._previous_feed_other_key()}

    def _script(self, name, text):
        with open(os.path.join(self.shims, name), "w") as f:
            f.write(text)

    def _tool(self, args, stdin=None, cwd=None):
        p = subprocess.run([self.tool] + args, input=stdin, cwd=cwd, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, universal_newlines=True)
        if p.returncode != 0:
            raise SystemExit("agentnotch-release %s failed: %s" % (args[0], p.stderr.strip()))
        return p.stdout

    def _previous_feed_test_key(self):
        """latest.json of a previous release, signed with the test seed's Windows key."""
        d = os.path.join(self.base, "previous-test-key")
        os.makedirs(d)
        exe = "AgentNotch-1.0.1-Setup.exe"
        with open(os.path.join(d, exe), "wb") as f:
            f.write(b"MZ previous installer (harness)\n")
        self._tool(["sign", "--file", exe, "--version", "1.0.1", "--out", exe + ".sig"], stdin=TEST_SEED, cwd=d)
        self._tool(["feed", "--version", "1.0.1", "--tag", "agentnotch-v1.0.1", "--repo", REPO,
                    "--installer", exe, "--sig", exe + ".sig", "--pubkey", self.keys["pubkey"], "--file", exe,
                    "--notes-url", "https://github.com/%s/releases/tag/agentnotch-v1.0.1" % REPO,
                    "--out", "latest.json"], cwd=d)
        return os.path.join(d, "latest.json")

    def _previous_feed_other_key(self):
        """latest.json signed with another key: the Tauri signer fixture of the tool's tests."""
        src = os.path.join(WINDOWS, "agentnotch-release", "tests", "fixtures", "tauri-signer")
        d = os.path.join(self.base, "previous-other-key")
        os.makedirs(d)
        exe = "AgentNotch-0.9.0-Setup.exe"
        for name in (exe, exe + ".sig"):
            shutil.copy(os.path.join(src, name), d)
        with open(os.path.join(src, "throwaway-key.pub")) as f:
            pubkey = f.read().strip()
        self._tool(["feed", "--version", "0.9.0", "--tag", "agentnotch-v0.9.0", "--repo", REPO,
                    "--installer", exe, "--sig", exe + ".sig", "--pubkey", pubkey, "--file", exe,
                    "--notes-url", "https://github.com/%s/releases/tag/agentnotch-v0.9.0" % REPO,
                    "--out", "latest.json"], cwd=d)
        if self._tool(["key-id-of-feed", "latest.json"], cwd=d).strip() == TEST_KEY_ID:
            raise SystemExit("the other-key feed has the test key's id")
        return os.path.join(d, "latest.json")

    # A scenario's GitHub: the repository at this commit, the releases, refs and contents.
    def setup(self, sc, d):
        version = sc.get("version", "1.1.0")
        mac_key = {"test": self.keys["sparkle_public_key"], "other": OTHER_SPARKLE_KEY,
                   "none": None}[sc.get("macKey", "test")]
        repo_files = {"VERSION": version + "\n",
                      "Scripts/sparkle-public-ed-key.txt": PUBLIC_KEY_HEADER + (mac_key + "\n" if mac_key else "")}
        pin = sc.get("pin")
        if pin == "same":
            repo_files["Scripts/tauri-update-public-key.txt"] = "# pinned\n" + self.keys["pubkey"] + "\n"
        elif pin == "other":
            repo_files["Scripts/tauri-update-public-key.txt"] = OTHER_PIN
        files = os.path.join(d, "gh-files")
        os.makedirs(files)
        state = {"releases": [], "deleted": [], "contents": {}, "refs": [], "tagObjects": {},
                 "faults": sc.get("faults", {})}

        def add(release):
            # Release ids, as GitHub's: the fake gh keeps each release's files under r<id>.
            release["id"] = 101 + len(state["releases"])
            os.makedirs(os.path.join(files, "r%d" % release["id"]))
            state["releases"].append(release)
            return release["id"]

        prev = sc.get("previous")
        if prev:
            ptag = "agentnotch-v" + prev.get("version", "1.0.1")
            assets = ["AgentNotch-%s.dmg" % prev.get("version", "1.0.1"), "AgentNotch-%s.zip" % prev.get("version", "1.0.1"),
                      "appcast.xml"]
            if prev.get("feed"):
                assets += ["AgentNotch-%s-Setup.exe" % prev.get("version", "1.0.1"),
                           "AgentNotch-%s-Setup.exe.sig" % prev.get("version", "1.0.1"), "latest.json"]
            pid = add({"tagName": ptag, "name": "Agent Notch " + prev.get("version", "1.0.1"),
                       "isDraft": False, "isPrerelease": False, "isLatest": True,
                       "assets": [{"name": a, "size": 1, "state": "uploaded"} for a in assets]})
            if prev.get("feed"):
                shutil.copy(self.feeds[prev["feed"]], os.path.join(files, "r%d" % pid, "latest.json"))
            released_key = {"same": mac_key, "other": OTHER_SPARKLE_KEY,
                            "test": self.keys["sparkle_public_key"]}[prev.get("macKey", "same")]
            state["contents"]["Scripts/sparkle-public-ed-key.txt@" + ptag] = PUBLIC_KEY_HEADER + (released_key or "") + "\n"
        # Upstream's tags and other names are in the list too; plan must ignore them.
        add({"tagName": "v1.9.0", "name": "Codenotch 1.9.0", "isDraft": False,
             "isPrerelease": False, "isLatest": False, "assets": []})
        # A draft of the next version, which no run of this one may touch.
        add({"tagName": "agentnotch-v9.9.9", "name": "Agent Notch 9.9.9", "isDraft": True,
             "isPrerelease": False, "isLatest": False, "assets": []})
        tag = "agentnotch-v" + version
        drafts = sc.get("draft") or []
        # "own": named as this workflow names its drafts; "foreign": named otherwise.
        for draft in [drafts] if isinstance(drafts, str) else drafts:
            add({"tagName": tag, "name": "Agent Notch " + version if draft == "own" else "Something else",
                 "isDraft": True, "isPrerelease": False, "isLatest": False,
                 "assets": [{"name": "AgentNotch-%s.dmg" % version, "size": 3, "state": "uploaded"}]})
        if sc.get("published"):
            add({"tagName": tag, "name": "Agent Notch " + version, "isDraft": False,
                 "isPrerelease": False, "isLatest": True, "assets": []})
            for r in state["releases"]:
                if r["tagName"] != tag:
                    r["isLatest"] = False
        existing = sc.get("tag")
        if existing == "this":
            state["refs"].append({"ref": "refs/tags/" + tag, "object": {"type": "commit", "sha": SHA}})
        elif existing == "other":
            state["refs"].append({"ref": "refs/tags/" + tag, "object": {"type": "commit", "sha": OTHER_SHA}})
        elif existing == "annotated-this":
            state["refs"].append({"ref": "refs/tags/" + tag, "object": {"type": "tag", "sha": "a" * 40}})
            state["tagObjects"]["a" * 40] = {"object": {"type": "commit", "sha": SHA}}
        # A longer tag with the same prefix (matching-refs is a prefix match).
        state["refs"].append({"ref": "refs/tags/" + tag + "-rc1", "object": {"type": "commit", "sha": OTHER_SHA}})
        state_path = os.path.join(d, "gh-state.json")
        with open(state_path, "w") as f:
            json.dump(state, f, indent=1)
        return repo_files, state_path, files, state

    def run(self, sc):
        d = tempfile.mkdtemp(prefix="sc-", dir=self.base)
        repo_files, state_path, gh_files, initial = self.setup(sc, d)
        version = sc.get("version", "1.1.0")
        event = sc.get("event", "push")
        inputs = {}
        if event == "workflow_dispatch":
            inputs = {"dry_run": False, "rotate_update_key": False, "skip_windows": False}
            inputs.update(sc.get("inputs", {}))
        run = {
            "dir": d, "repo_files": repo_files, "artifacts": os.path.join(d, "artifacts"),
            "uploads": [], "state": state_path, "gh_files": gh_files,
            "gh_log": os.path.join(d, "gh.log"), "curl_log": os.path.join(d, "curl.log"),
            "ctx": {
                "github": {"event_name": event, "repository": REPO, "ref": sc.get("ref", "refs/heads/main"),
                           "sha": SHA, "server_url": "https://github.com", "workflow": "Release"},
                "inputs": inputs,
                "secrets": {"SPARKLE_ED_PRIVATE_KEY": TEST_SEED},
                "vars": {},
            },
            "jobs": {}, "version": version, "sc": sc, "attempt": 1, "first": None, "initial": initial,
        }
        for name in ("gh_log", "curl_log"):
            open(run[name], "w").close()
        os.makedirs(run["artifacts"])
        for job_id in JOB_ORDER:
            run["jobs"][job_id] = self.job(run, job_id)
        rerun = sc.get("rerun")
        if rerun is not None:
            self.rerun_failed_jobs(run, rerun)
        return run

    def rerun_failed_jobs(self, run, rerun):
        """GitHub's "Re-run failed jobs": the failed jobs and every job after them run again,
        in the same run (its artifacts, and the outputs of the jobs that succeeded, plan's
        among them, as the first attempt left them); GitHub's state is whatever the first
        attempt left, with the scenario's faults for the second attempt and the releases it says
        were published in between (`addReleases`: release objects as setup() makes them, without
        ids; one with isLatest takes that mark from the others)."""
        run["first"] = dict(run["jobs"])
        again = set(j for j in JOB_ORDER if run["jobs"][j].result in ("failure", "cancelled"))
        for job_id in JOB_ORDER:
            if any(n in again for n in self.wf.job(job_id).needs):
                again.add(job_id)
        with open(run["state"]) as f:
            state = json.load(f)
        state["faults"] = rerun.get("faults", {})
        taken = set(r["id"] for r in state["releases"] + state.get("deleted", []))
        for extra in rerun.get("addReleases", []):
            extra = dict(extra)
            extra["id"] = min(i for i in range(201, 1000) if i not in taken)
            taken.add(extra["id"])
            os.makedirs(os.path.join(run["gh_files"], "r%d" % extra["id"]))
            if extra.get("isLatest"):
                for r in state["releases"]:
                    r["isLatest"] = False
            state["releases"].append(extra)
        with open(run["state"], "w") as f:
            json.dump(state, f, indent=1)
        run["attempt"] = 2
        run["rerun_calls_from"] = len(_calls(run["gh_log"]))
        run["rerun_jobs"] = [j for j in JOB_ORDER if j in again]
        for job_id in run["rerun_jobs"]:
            run["jobs"][job_id] = self.job(run, job_id)

    def job(self, run, job_id):
        job = self.wf.job(job_id)
        needs = {}
        for n in job.needs:
            r = run["jobs"][n]
            needs[n] = {"result": r.result, "outputs": r.outputs}
        ctx = dict(run["ctx"])
        ctx["needs"] = needs
        # The job's GITHUB_TOKEN, named after what it may do with contents: the fake gh shows
        # a read token no drafts and refuses its writes, as GitHub does.
        perms = job.permissions if job.permissions is not None else (self.wf.permissions or {})
        ctx["github"] = dict(ctx["github"], token="fake-github-token-%s" % (
            "write" if (perms or {}).get("contents") == "write" else "read"))
        status = {"success": all(n["result"] == "success" for n in needs.values()),
                  "failure": any(n["result"] == "failure" for n in needs.values()),
                  "cancelled": False, "always": True}
        cond = job.if_ if job.if_ is not None else "success()"
        if not isinstance(cond, str):
            cond = to_string(cond)
        if not uses_status(cond):
            cond = "success() && (%s)" % cond
        if not _truthy(evaluate(cond, ctx, status)):
            return JobResult(job_id, "skipped")
        result = JobResult(job_id, "success")
        sc = run["sc"]
        forced = (sc.get("jobs") or {}).get(job_id)
        if job_id == "checks":
            result.result = forced or "success"
        elif job_id == "release-tool":
            self.fake_release_tool(run, result)
        elif job_id == "mac":
            self.fake_mac(run, result)
        elif job_id == "windows":
            self.fake_windows(run, ctx, status, job, result)
        elif job_id == "website":
            result.result = "would run"
            result.note = "not run by the harness (OIDC and the website)"
        elif job_id in RUN_JOBS:
            self.run_steps(run, job, ctx, result)
        else:
            raise SystemExit("the harness does not know job %s" % job_id)
        return result

    # The jobs that build, simulated with what they hand over.
    def fake_release_tool(self, run, result):
        out = os.path.join(run["artifacts"], "release-tool")
        os.makedirs(out)
        path = os.path.join(out, "agentnotch-release")
        shutil.copy(self.tool, path)
        result.outputs = {"sha256": _sha256(path)}
        if run["sc"].get("toolTampered"):
            # The artifact changed after release-tool hashed it.
            with open(path, "ab") as f:
                f.write(b"\0")
        run["uploads"].append(("release-tool", ["agentnotch-release"]))

    def fake_mac(self, run, result):
        mac = run["sc"].get("mac", {})
        if mac.get("result", "success") != "success":
            result.result = mac["result"]
            return
        v = run["version"]
        dmg = mac.get("dmg", "AgentNotch-%s.dmg" % v)
        zipname = "AgentNotch-%s.zip" % v
        notarized = mac.get("notarized", "0")
        out = os.path.join(run["artifacts"], "mac-release")
        os.makedirs(out)
        with open(os.path.join(out, dmg), "wb") as f:
            f.write(b"dmg bytes (harness)\n" * 3)
        with open(os.path.join(out, zipname), "wb") as f:
            f.write(b"PK zip bytes (harness)\n")
        with open(os.path.join(out, "appcast.xml"), "w") as f:
            f.write('<?xml version="1.0"?>\n<rss><channel><item><sparkle:version>%s</sparkle:version>'
                    '</item></channel></rss>\n' % v)
        with open(os.path.join(out, "release-info.env"), "w") as f:
            f.write("DMG=%s\nZIP=%s\nNOTARIZED=%s\n" % (dmg, zipname, notarized))
        result.outputs = {"DMG": dmg, "ZIP": zipname, "NOTARIZED": notarized}
        run["uploads"].append(("mac-release", sorted(os.listdir(out))))

    def fake_windows(self, run, ctx, status, job, result):
        inputs = dict((k, interpolate(v, ctx, status)) for k, v in job.with_.items())
        result.outputs = {}
        result.note = "with " + json.dumps(inputs, sort_keys=True)
        problems = []
        if inputs.get("release") != "true":
            problems.append("release is %r" % inputs.get("release"))
        if inputs.get("updater_pubkey") != self.keys["pubkey"]:
            problems.append("updater_pubkey is not the derived key")
        if inputs.get("expected_key_id") != TEST_KEY_ID:
            problems.append("expected_key_id is %r" % inputs.get("expected_key_id"))
        if problems:
            result.result = "failure"
            result.note += "; " + ", ".join(problems)
            return
        win = run["sc"].get("windows", {})
        if win.get("result", "success") != "success":
            result.result = win["result"]
            return
        v = run["version"]
        out = os.path.join(run["artifacts"], "windows-unsigned")
        os.makedirs(out)
        installer = win.get("installer", "AgentNotch-%s-Setup.exe" % v)
        with open(os.path.join(out, installer), "wb") as f:
            f.write(b"MZ installer bytes (harness)\n" * 5)
        # Written on Windows: CRLF line ends.
        info = "VERSION=%s\r\nINSTALLER=%s\r\nAUTHENTICODE=%s\r\nSIGNER=\r\nUPDATES=on\r\n" % (
            win.get("version", v), installer, win.get("authenticode", "unsigned"))
        with open(os.path.join(out, "windows-release-info.env"), "w", newline="") as f:
            f.write(info)
        run["uploads"].append(("windows-unsigned", sorted(os.listdir(out))))

    # The jobs whose shell steps run for real.
    def run_steps(self, run, job, ctx, result):
        jd = os.path.join(run["dir"], job.id + ("" if run["attempt"] == 1 else "-attempt-%d" % run["attempt"]))
        workspace = os.path.join(jd, "workspace")
        temp = os.path.join(jd, "runner-temp")
        home = os.path.join(jd, "home")
        for p in (workspace, temp, home):
            os.makedirs(p)
        summary = os.path.join(jd, "step-summary.md")
        open(summary, "w").close()
        ctx = dict(ctx)
        ctx["runner"] = {"temp": temp, "os": "Linux"}
        ctx["steps"] = {}
        status = {"success": True, "failure": False, "cancelled": False, "always": True}
        ctx["env"] = {}
        job_env = {}
        for k, v in job.env.items():
            job_env[k] = interpolate(v, ctx, status)
        ctx["env"] = dict(job_env)
        failed = False
        for step in job.steps:
            status = {"success": not failed, "failure": failed, "cancelled": False, "always": True}
            cond = step.if_ if step.if_ is not None else "success()"
            cond = to_string(cond) if not isinstance(cond, str) else cond
            if not uses_status(cond):
                cond = "success() && (%s)" % cond
            if not _truthy(evaluate(cond, ctx, status)):
                result.steps.append(StepResult(step.label, "skipped"))
                if step.id:
                    ctx["steps"][step.id] = {"outputs": {}, "outcome": "skipped", "conclusion": "skipped"}
                continue
            step_env = dict(job_env)
            for k, v in step.env.items():
                step_env[k] = interpolate(v, dict(ctx, env=step_env), status)
            if step.uses:
                res, outputs = self.action(run, job, step, ctx, status, workspace, temp)
            else:
                res, outputs = self.shell(run, job, step, step_env, workspace, temp, home, summary)
            result.steps.append(res)
            if step.id:
                ctx["steps"][step.id] = {"outputs": outputs, "outcome": res.result, "conclusion": res.result}
            if res.result == "failure":
                failed = True
        result.result = "failure" if failed else "success"
        status = {"success": not failed, "failure": failed, "cancelled": False, "always": True}
        if not failed:
            result.outputs = dict((k, interpolate(v, ctx, status)) for k, v in job.outputs.items())
        with open(summary) as f:
            result.summary = f.read()

    def shell(self, run, job, step, env, workspace, temp, home, summary):
        jd = os.path.dirname(workspace)
        n = len(os.listdir(jd))
        script = os.path.join(jd, "step-%02d.sh" % n)
        with open(script, "w") as f:
            f.write(step.run)
        wrapper = os.path.join(jd, "step-%02d-wrapper.sh" % n)
        with open(wrapper, "w") as f:
            f.write('gh_at="$(command -v gh || true)"; curl_at="$(command -v curl || true)"\n'
                    'if [[ "$gh_at" != "$HARNESS_SHIMS/gh" || "$curl_at" != "$HARNESS_SHIMS/curl" ]]; then\n'
                    '  echo "harness: gh is $gh_at and curl is $curl_at, not the fakes; nothing runs" >&2\n'
                    '  exit 97\n'
                    'fi\n'
                    '. "$HARNESS_STEP"\n')
        outputs_file = os.path.join(jd, "output-%02d" % n)
        open(outputs_file, "w").close()
        full = {
            "PATH": self.shims + os.pathsep + os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": home, "TMPDIR": temp, "LANG": os.environ.get("LANG", "C.UTF-8"),
            "CI": "true", "GITHUB_ACTIONS": "true",
            "GITHUB_OUTPUT": outputs_file, "GITHUB_STEP_SUMMARY": summary,
            "GITHUB_ENV": os.path.join(jd, "github-env"), "GITHUB_REF": run["ctx"]["github"]["ref"],
            "GITHUB_SHA": SHA, "GITHUB_REPOSITORY": REPO, "GITHUB_SERVER_URL": "https://github.com",
            "GITHUB_EVENT_NAME": run["ctx"]["github"]["event_name"], "GITHUB_WORKSPACE": workspace,
            "RUNNER_TEMP": temp,
            "FAKE_GH_STATE": run["state"], "FAKE_GH_FILES": run["gh_files"], "FAKE_GH_LOG": run["gh_log"],
            "FAKE_CURL_LOG": run["curl_log"], "HARNESS_SHIMS": self.shims, "HARNESS_STEP": script,
        }
        full.update(env)
        cwd = workspace
        wd = step.working_directory or job.working_directory
        if wd:
            cwd = os.path.join(workspace, wd)
        p = subprocess.run([self.bash, "--noprofile", "--norc", "-eo", "pipefail", wrapper], env=full, cwd=cwd,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT, universal_newlines=True)
        res = StepResult(step.label, "success" if p.returncode == 0 else "failure", p.stdout, _annotations(p.stdout))
        if p.returncode == 97:
            raise SystemExit("harness: %s/%s: %s" % (job.id, step.label, p.stdout.strip()))
        return res, (_read_outputs(outputs_file) if p.returncode == 0 else {})

    def action(self, run, job, step, ctx, status, workspace, temp):
        with_ = dict((k, interpolate(v, dict(ctx, runner={"temp": temp}), status)) for k, v in step.with_.items())
        name = step.uses.split("@")[0]
        if name == "actions/checkout":
            sparse = [s.strip().lstrip("/") for s in with_.get("sparse-checkout", "").split("\n") if s.strip()]
            for path, text in run["repo_files"].items():
                if sparse and path not in sparse:
                    continue
                full = os.path.join(workspace, path)
                os.makedirs(os.path.dirname(full) or workspace, exist_ok=True)
                with open(full, "w") as f:
                    f.write(text)
            return StepResult(step.label, "success", "checked out " + (", ".join(sparse) or "everything")), {}
        if name == "actions/download-artifact":
            src = os.path.join(run["artifacts"], with_.get("name", ""))
            if not os.path.isdir(src):
                return StepResult(step.label, "failure", "no artifact %r" % with_.get("name"),
                                  [("error", "Artifact not found: %s" % with_.get("name"))]), {}
            dest = with_.get("path") or workspace
            os.makedirs(dest, exist_ok=True)
            for f in os.listdir(src):
                shutil.copy(os.path.join(src, f), os.path.join(dest, f))
            return StepResult(step.label, "success", "downloaded %s" % with_["name"]), {}
        if name == "actions/upload-artifact":
            art = with_.get("name", "")
            paths = [p.strip() for p in with_.get("path", "").split("\n") if p.strip()]
            missing = [p for p in paths if not os.path.isfile(p)]
            if not paths or (missing and with_.get("if-no-files-found") == "error"):
                return StepResult(step.label, "failure", "missing %s" % missing,
                                  [("error", "No files were found with the provided path: %s" % " ".join(missing))]), {}
            dest = os.path.join(run["artifacts"], art)
            if os.path.exists(dest):
                return StepResult(step.label, "failure", "artifact %s exists" % art,
                                  [("error", "an artifact with this name already exists")]), {}
            os.makedirs(dest)
            for p in paths:
                if os.path.isfile(p):
                    shutil.copy(p, dest)
            run["uploads"].append((art, sorted(os.listdir(dest))))
            return StepResult(step.label, "success", "uploaded %s" % art), {}
        return StepResult(step.label, "failure", "the harness does not simulate %s" % step.uses,
                          [("error", "unknown action %s" % step.uses)]), {}


# --- expectations -------------------------------------------------------------------------

def _calls(path):
    with open(path) as f:
        return [" ".join(json.loads(line)) for line in f if line.strip()]


def _release_key(r):
    return (r["id"], r["tagName"], r.get("name"), r["isDraft"], json.dumps(r.get("assets"), sort_keys=True))


def invariants(run, state):
    """What no run may ever do, whatever the scenario: delete a published release, turn one
    back into a draft, or touch a release of another tag."""
    problems = []
    tag = "agentnotch-v" + run["version"]
    for r in state.get("deleted", []):
        if not r["isDraft"]:
            problems.append("published release %s (id %s) deleted" % (r["tagName"], r["id"]))
        if r["tagName"] != tag:
            problems.append("release %s (id %s) of another tag deleted" % (r["tagName"], r["id"]))
    now = dict((r["id"], r) for r in state["releases"])
    for r in run["initial"]["releases"]:
        if r["tagName"] != tag and (r["id"] not in now or _release_key(now[r["id"]]) != _release_key(r)):
            problems.append("release %s (id %s) of another tag changed" % (r["tagName"], r["id"]))
        if r["tagName"] == tag and not r["isDraft"] and (r["id"] not in now or now[r["id"]]["isDraft"]):
            problems.append("published release %s (id %s) no longer published" % (r["tagName"], r["id"]))
    return problems


def _check_calls(calls, want_calls, where):
    problems = []
    for want in want_calls.get("called", []):
        if not any(want in c for c in calls):
            problems.append("%sgh never called with %r" % (where, want))
    for unwanted in want_calls.get("notCalled", []):
        hit = [c for c in calls if unwanted in c]
        if hit:
            problems.append("%sgh called with %r: %s" % (where, unwanted, hit[0][:160]))
    order = want_calls.get("order")
    if order:
        idx = []
        for want in order:
            pos = [i for i, c in enumerate(calls) if want in c]
            idx.append(pos[0] if pos else -1)
        if -1 in idx or idx != sorted(idx):
            problems.append("%sgh calls not in the order %s" % (where, order))
    return problems


def check(run, expect):
    problems = []
    jobs = run["jobs"]
    first = expect.get("firstAttempt")
    if first is not None:
        if run["first"] is None:
            problems.append("firstAttempt expected, but the scenario has no rerun")
        else:
            for job_id, want in (first.get("jobs") or {}).items():
                if run["first"][job_id].result != want:
                    problems.append("first attempt: job %s: %s, expected %s" % (job_id, run["first"][job_id].result, want))
    if "rerunJobs" in expect and run.get("rerun_jobs") != expect["rerunJobs"]:
        problems.append("jobs run again: %s, expected %s" % (run.get("rerun_jobs"), expect["rerunJobs"]))
    for job_id, want in (expect.get("jobs") or {}).items():
        if jobs[job_id].result != want:
            problems.append("job %s: %s, expected %s" % (job_id, jobs[job_id].result, want))
    for job_id, outs in (expect.get("outputs") or {}).items():
        for k, v in outs.items():
            got = jobs[job_id].outputs.get(k)
            if got != v:
                problems.append("%s output %s: %r, expected %r" % (job_id, k, got, v))
    for job_id, steps in (expect.get("steps") or {}).items():
        got = dict((s.label, s.result) for s in jobs[job_id].steps)
        for label, want in steps.items():
            if got.get(label, "not run") != want:
                problems.append("%s step %r: %s, expected %s" % (job_id, label, got.get(label, "not run"), want))
    every = []
    for job_id in JOB_ORDER:
        every.extend((job_id, lvl, text) for lvl, text in jobs[job_id].annotations)
    # After a re-run, the annotations looked for may be the first attempt's too; the ones
    # that must not be there are the final attempt's.
    seen = list(every)
    if run["first"] is not None:
        for job_id in run["rerun_jobs"]:
            seen.extend((job_id, lvl, text) for lvl, text in run["first"][job_id].annotations)
    for a in expect.get("annotations") or []:
        if not any(lvl == a["level"] and a["contains"] in text and a.get("job", job_id) == job_id
                   for job_id, lvl, text in seen):
            problems.append("no %s%s containing %r" % (a["level"], " in " + a["job"] if "job" in a else "", a["contains"]))
    for level in expect.get("noAnnotations") or []:
        found = [text for _, lvl, text in every if lvl == level]
        if found:
            problems.append("unexpected %s: %s" % (level, found[0][:160]))
    calls = _calls(run["gh_log"])
    problems.extend(_check_calls(calls, expect.get("gh") or {}, ""))
    if "rerunGh" in expect:
        problems.extend(_check_calls(calls[run.get("rerun_calls_from", len(calls)):], expect["rerunGh"], "in the re-run: "))
    curl = _calls(run["curl_log"])
    if "curl" in expect and len(curl) != expect["curl"]:
        problems.append("curl called %d times, expected %d" % (len(curl), expect["curl"]))
    with open(run["state"]) as f:
        state = json.load(f)
    problems.extend(invariants(run, state))
    for tag, want in (expect.get("releasesFor") or {}).items():
        found = [(r["id"], r.get("name"), "draft" if r["isDraft"] else "published")
                 for r in state["releases"] if r["tagName"] == tag]
        if len(found) != want:
            problems.append("%d release(s) for %s, expected %d: %s" % (len(found), tag, want, found))
    for rid in expect.get("deletedIds") or []:
        if not any(r["id"] == rid for r in state.get("deleted", [])):
            problems.append("release id %s not deleted" % rid)
    if "deletedCount" in expect and len(state.get("deleted", [])) != expect["deletedCount"]:
        problems.append("%d release(s) deleted, expected %d" % (len(state.get("deleted", [])), expect["deletedCount"]))
    rel = expect.get("release")
    if rel:
        found = [r for r in state["releases"] if r["tagName"] == rel["tag"]]
        if len(found) != 1:
            problems.append("%d releases %s, expected exactly one" % (len(found), rel["tag"]))
        else:
            r = found[0]
            for k in ("isDraft", "isLatest", "notesStartTag", "name", "targetCommitish", "generateNotes"):
                if k in rel and r.get(k) != rel[k]:
                    problems.append("release %s %s: %r, expected %r" % (rel["tag"], k, r.get(k), rel[k]))
            if "assets" in rel:
                names = sorted(a["name"] for a in r.get("assets", []))
                if names != sorted(rel["assets"]):
                    problems.append("release %s assets %s, expected %s" % (rel["tag"], names, sorted(rel["assets"])))
            for text in rel.get("notesContain", []):
                if text not in r.get("notes", ""):
                    problems.append("release notes lack %r" % text)
            for text in rel.get("notesLack", []):
                if text in r.get("notes", ""):
                    problems.append("release notes contain %r" % text)
    for tag in expect.get("noRelease", []):
        if any(r["tagName"] == tag for r in state["releases"]):
            problems.append("release %s exists" % tag)
    uploads = dict(run["uploads"])
    for art, names in (expect.get("artifacts") or {}).items():
        if names is None:
            if art in uploads:
                problems.append("artifact %s uploaded" % art)
        elif sorted(uploads.get(art, [])) != sorted(names):
            problems.append("artifact %s holds %s, expected %s" % (art, uploads.get(art), sorted(names)))
    if "website" in expect:
        runs = jobs["website"].result == "would run"
        if runs != expect["website"]:
            problems.append("website job %s, expected it %s" % (
                "would run" if runs else "skipped", "to run" if expect["website"] else "skipped"))
    return problems


def describe(run):
    lines = []
    attempts = [("", run["jobs"])] if run["first"] is None else \
        [("  first attempt:", run["first"]), ("  re-run of the failed jobs:", dict((j, run["jobs"][j]) for j in run["rerun_jobs"]))]
    for title, jobs in attempts:
        if title:
            lines.append("  " + title)
        for job_id in JOB_ORDER:
            if job_id not in jobs:
                continue
            j = jobs[job_id]
            lines.append("    %s: %s%s" % (job_id, j.result, (" (" + j.note + ")") if j.note else ""))
            for s in j.steps:
                lines.append("      - %s: %s" % (s.label, s.result))
                if s.output.strip():
                    for out in s.output.rstrip().split("\n")[-25:]:
                        lines.append("          | " + out)
    lines.append("    gh calls:")
    lines.extend("      " + c for c in _calls(run["gh_log"]))
    lines.append("    curl calls:")
    lines.extend("      " + c for c in _calls(run["curl_log"]))
    return "\n".join(lines)


# --- structure -----------------------------------------------------------------------------

SEED_STEPS = {("mac", "Check the update signing key"), ("mac", "Build the release"),
              ("keys", "Derive the Windows update key"), ("sign-windows", "Sign")}
TOOLING = re.compile(r"\b(cargo|rustup|rustc|npm|npx|node|pip|pip3|make|swift)\b")


def _dump(v):
    return json.dumps(v, sort_keys=True)


def structure(wf):
    """[(ok, message)]"""
    out = []

    def add(ok, message):
        out.append((bool(ok), message))

    add(wf.permissions == {"contents": "read"}, "workflow permissions are contents: read")
    add(wf.concurrency == {"group": "release", "queue": "max", "cancel-in-progress": False},
        "concurrency: one group, queued, never cancelled")
    inputs = ((wf.on or {}).get("workflow_dispatch") or {}).get("inputs") or {}
    add(set(inputs) == {"dry_run", "rotate_update_key", "skip_windows"} and
        all(i.get("type") == "boolean" and i.get("default") is False for i in inputs.values()),
        "manual inputs: dry_run, rotate_update_key, skip_windows (booleans, off)")
    add(wf.job_order == JOB_ORDER, "jobs: " + ", ".join(wf.job_order))

    writers = [j for j in wf.job_order
               if any(v == "write" and k != "id-token" for k, v in (wf.jobs[j].permissions or {}).items())]
    add(writers == ["publish"], "only publish may write (%s)" % ", ".join(writers))
    for job_id in wf.job_order:
        job = wf.jobs[job_id]
        if job_id in ("checks",):
            continue
        add(job.permissions is not None, "%s declares its permissions" % job_id)

    envs = dict((j, wf.jobs[j].environment) for j in wf.job_order)
    add(envs == {"checks": None, "plan": None, "release-tool": None, "keys": "release", "mac": "release",
                 "windows": None, "sign-windows": "release", "publish": None, "website": "release"},
        "environments: release only for keys, mac, sign-windows, website")

    publish = wf.jobs["publish"]
    add(publish.permissions == {"contents": "write"} and publish.environment is None,
        "publish: contents: write, no environment")
    add(not any(s.uses and s.uses.startswith("actions/checkout") for s in publish.steps),
        "publish checks nothing out (it runs none of the repository's code)")
    # gh release view/edit/delete <tag> pick one of several drafts that share the tag; publish
    # addresses the release it made, and the leftovers it deletes, by id.
    by_tag = [s.label for s in publish.steps if s.run and re.search(r"\bgh release (view|edit|delete)\b", s.run)]
    add(not by_tag, "publish addresses releases by id, never gh release view/edit/delete <tag>%s" % (
        (" (" + ", ".join(by_tag) + ")") if by_tag else ""))

    # Every artifact a job of the run downloads is kept as long as the others: "Re-run failed
    # jobs" downloads them again, and one that expired first fails that re-run. Uploaded in
    # release.yml or, for windows-unsigned, in the Windows workflow it calls.
    retention = {}
    for job_id in wf.job_order:
        for s in wf.jobs[job_id].steps:
            if s.uses and s.uses.startswith("actions/upload-artifact"):
                retention.setdefault(s.with_.get("name"), []).append(s.with_.get("retention-days"))
    windows_yml = os.path.join(os.path.dirname(RELEASE_YML), "agentnotch-windows.yml")
    if not os.path.exists(windows_yml):  # --workflow with a lone copy of release.yml
        windows_yml = os.path.join(ROOT, ".github", "workflows", "agentnotch-windows.yml")
    with open(windows_yml) as f:
        for wjob in (extract.parse(f.read()).get("jobs") or {}).values():
            for s in wjob.get("steps") or []:
                if str(s.get("uses", "")).startswith("actions/upload-artifact") and \
                        (s.get("with") or {}).get("name") == "windows-unsigned":
                    retention.setdefault("windows-unsigned", []).append(s["with"].get("retention-days"))
    handed = sorted(set(s.with_.get("name") for j in wf.job_order for s in wf.jobs[j].steps
                        if s.uses and s.uses.startswith("actions/download-artifact")))
    kept = dict((name, retention.get(name) or [None]) for name in handed)
    days = set(d for ds in kept.values() for d in ds)
    add(handed and len(days) == 1 and all(isinstance(d, int) and d >= 3 for d in days),
        "artifacts handed between jobs are all kept the same days, at least 3 (%s)" % ", ".join(
            "%s %s" % (n, "/".join(str(d) for d in ds)) for n, ds in sorted(kept.items())))

    tool = wf.jobs["release-tool"]
    add(tool.environment is None and "secrets." not in _dump(tool.data) and tool.permissions == {"contents": "read"},
        "release-tool: no environment, no secret, contents: read")
    add(not any(s.uses and "cache" in s.uses for s in tool.steps), "release-tool: no cache")

    win = wf.jobs["windows"]
    add(win.environment is None and win.secrets is None and "secrets." not in _dump(win.data),
        "windows: no environment, no secrets passed (not even inherit)")
    add(win.uses == "./.github/workflows/agentnotch-windows.yml" and win.with_.get("release") is True,
        "windows: agentnotch-windows.yml with release: true")

    # Every secret, where it may be: only the seed-holding steps' env (and the Mac build's).
    for job_id in wf.job_order:
        job = wf.jobs[job_id]
        shell = dict((k, v) for k, v in job.data.items() if k != "steps")
        if "secrets." in _dump(shell):
            add(False, "%s: a secret outside its steps" % job_id)
        for step in job.steps:
            allowed = (job_id, step.name) in SEED_STEPS
            in_env = "secrets." in _dump(step.env)
            elsewhere = "secrets." in _dump(dict((k, v) for k, v in step.data.items() if k != "env"))
            if elsewhere:
                add(False, "%s/%s: a secret outside env:" % (job_id, step.label))
            if in_env and not allowed:
                add(False, "%s/%s: a secret in a step that may hold none" % (job_id, step.label))
            mentions = "SPARKLE_ED_PRIVATE_KEY" in _dump(step.data)
            if mentions and not allowed:
                add(False, "%s/%s names SPARKLE_ED_PRIVATE_KEY" % (job_id, step.label))
    seed_steps = [(j, s.name) for j in wf.job_order for s in wf.jobs[j].steps
                  if "secrets.SPARKLE_ED_PRIVATE_KEY" in _dump(s.env)]
    add(sorted(seed_steps) == sorted(SEED_STEPS),
        "SPARKLE_ED_PRIVATE_KEY only in env: of " + "; ".join("%s/%s" % p for p in sorted(seed_steps)))

    for job_id, step_name in (("keys", "Derive the Windows update key"), ("sign-windows", "Sign")):
        step = wf.jobs[job_id].step(step_name)
        env_ok = step.env.get("SEED") == "${{ secrets.SPARKLE_ED_PRIVATE_KEY }}"
        uses = re.findall(r'[^\n]*\$\{?SEED\b[^\n]*', step.run or "")
        only_stdin = all(re.search(r'-z "\$SEED"', u) or re.search(r"printf '%s' \"\$SEED\" \| \"\$RUNNER_TEMP/release-tool/agentnotch-release\"", u)
                         for u in uses) and any("printf '%s' \"$SEED\" |" in u for u in uses)
        add(env_ok and only_stdin, "%s/%s: the seed in env: only, piped to the tool's stdin" % (job_id, step_name))

    for job_id in ("keys", "sign-windows"):
        job = wf.jobs[job_id]
        labels = [s.label for s in job.steps]
        check_at = labels.index("Check the release tool") if "Check the release tool" in labels else -1
        check = job.steps[check_at] if check_at >= 0 else None
        hashes = check is not None and "sha256sum" in (check.run or "") and \
            check.env.get("SHA256") == "${{ needs.release-tool.outputs.sha256 }}" and "chmod +x" in check.run
        first_use = min([i for i, s in enumerate(job.steps) if s.run and "agentnotch-release" in s.run and i != check_at] or [99])
        downloads = [i for i, s in enumerate(job.steps) if s.uses and s.uses.startswith("actions/download-artifact")
                     and s.with_.get("name") == "release-tool"]
        add(hashes and downloads and downloads[0] < check_at < first_use,
            "%s: downloads release-tool, checks its SHA-256 before it runs it" % job_id)
        tooling = [s.label for s in job.steps if (s.run and TOOLING.search(s.run)) or
                   (s.uses and re.search(r"setup-|rust-toolchain|rust-cache|actions/cache", s.uses))]
        add(not tooling, "%s: no cargo, npm or other build tooling%s" % (job_id, (" (" + ", ".join(tooling) + ")") if tooling else ""))
        checkouts = [s for s in job.steps if s.uses and s.uses.startswith("actions/checkout")]
        sparse_ok = all(s.with_.get("sparse-checkout") and s.with_.get("sparse-checkout-cone-mode") is False and
                        all(re.match(r"^/Scripts/[a-z-]+-key\.txt$", p.strip())
                            for p in s.with_["sparse-checkout"].split("\n") if p.strip())
                        for s in checkouts)
        add(sparse_ok, "%s: checks out %s" % (job_id, "only the key files" if checkouts else "nothing"))
        add(job.needs and "release-tool" in job.needs, "%s needs release-tool" % job_id)

    plan = wf.jobs["plan"]
    add(plan.environment is None and plan.permissions == {"contents": "read"} and "secrets." not in _dump(plan.data),
        "plan: no environment, no secret")
    add(set(plan.outputs) == {"version", "tag", "skip", "previous_tag", "previous_has_feed", "windows"},
        "plan outputs: " + ", ".join(sorted(plan.outputs)))

    site = wf.jobs["website"]
    add("publish" in site.needs and site.permissions == {"contents": "read", "id-token": "write"},
        "website: after publish, id-token: write")
    return out


# --- main ---------------------------------------------------------------------------------

def find_tool():
    if os.environ.get("AGENTNOTCH_RELEASE_TOOL"):
        return os.environ["AGENTNOTCH_RELEASE_TOOL"]
    target = os.environ.get("CARGO_TARGET_DIR") or os.path.join(WINDOWS, "target")
    if not os.path.isabs(target):
        target = os.path.join(WINDOWS, target)
    path = os.path.join(target, "debug", "agentnotch-release")
    if not os.path.exists(path):
        print("Building agentnotch-release (cargo build --locked -p agentnotch-release)...")
        subprocess.check_call(["cargo", "build", "--locked", "-p", "agentnotch-release"], cwd=WINDOWS)
    return path


def load_scenarios():
    d = os.path.join(HERE, "scenarios")
    out = []
    for name in sorted(os.listdir(d)):
        if name.endswith(".json"):
            with open(os.path.join(d, name)) as f:
                sc = json.load(f)
            sc["id"] = name[:-5]
            out.append(sc)
    return out


def main(argv):
    global RELEASE_YML
    verbose = "-v" in argv
    if "--workflow" in argv:
        # Another copy of release.yml, e.g. a deliberately broken one to see a scenario fail.
        at = argv.index("--workflow")
        RELEASE_YML = os.path.abspath(argv[at + 1])
        argv = argv[:at] + argv[at + 2:]
    filters = [a for a in argv if a != "-v"]
    for tool in ("bash", "jq"):
        if not shutil.which(tool):
            print("run.py needs %s on PATH" % tool)
            return 1
    with open(RELEASE_YML) as f:
        wf = extract.load(f.read())
    tool = find_tool()
    base = tempfile.mkdtemp(prefix="release-harness-")
    failed = 0
    try:
        harness = Harness(wf, tool, base, verbose)
        scenarios = [s for s in load_scenarios() if not filters or any(f in s["id"] for f in filters)]
        if not scenarios:
            print("no scenario matches %s" % filters)
            return 1
        print("Release workflow scenarios (%s, fake gh, release tool %s):" % (os.path.relpath(RELEASE_YML, ROOT), TEST_KEY_ID))
        for sc in scenarios:
            run = harness.run(sc)
            problems = check(run, sc.get("expect", {}))
            print("%s %s: %s" % ("ok  " if not problems else "FAIL", sc["id"], sc.get("name", "")))
            if problems:
                failed += 1
                for p in problems:
                    print("       - " + p)
            if problems or verbose:
                print(describe(run))
        print("Release workflow structure:")
        for ok, message in structure(wf):
            print("%s %s" % ("ok  " if ok else "FAIL", message))
            failed += 0 if ok else 1
    finally:
        shutil.rmtree(base, ignore_errors=True)
    if failed:
        print("%d failed." % failed)
        return 1
    print("All passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
