#!/usr/bin/env python3
"""
Drive a running Agent Notch with fake Claude Code sessions.
Ported from Superpowered Vibe Notch's scripts/dev/simulate-sessions.py.

Talks to the app's socket exactly like the real integration does:
- hook events are piped through the real hook script
  (Packages/ClaudeControl/Scripts/agentnotch-hook.py) with the
  environment Claude Code gives hooks (CLAUDE_PID, CLAUDE_CONFIG_DIR, ...),
- status line updates are sent in the status line wrapper's message format.

Fake sessions live in two fake accounts, <root>/.claude and
<root>/.claude-work, with small JSONL transcripts written there. <root> is a
fresh temporary folder (removed on exit unless --keep-root), so parallel runs
never collide. Each session gets its own stand-in "Claude process" (a shell
that exits as soon as the simulator does, even if it is killed), so the
app's liveness check keeps it exactly as long as the simulator runs, and
keeps its session registry entry (<configDir>/sessions/<pid>.json) current
the way Claude Code does (busy / waiting / idle; --no-registry turns that
off). Restarting the app while the simulator lingers exercises rediscovery
and the restored review queue.

Nothing here touches ~/.claude or any real Claude config, and nothing here
starts the app: run it yourself against a private socket and support folder.

Typical dev loop (the root is printed at start):

    AGENTNOTCH_SOCKET=/tmp/agentnotch-dev.sock AGENTNOTCH_SUPPORT_DIR=/tmp/agentnotch-dev-support \\
    AGENTNOTCH_NO_INSTALL=1 AGENTNOTCH_NO_NOTIFICATIONS=1 \\
    AGENTNOTCH_EXTRA_CONFIG_DIRS=<root>/.claude:<root>/.claude-work \\
      "<Agent Notch.app>/Contents/MacOS/<executable>" --dump-state --dev-console
    AGENTNOTCH_SOCKET=/tmp/agentnotch-dev.sock Packages/ClaudeControl/DevTools/simulate-sessions.py \\
      --root <root> --scenario all

(--root lets the app be started first with the matching AGENTNOTCH_EXTRA_CONFIG_DIRS;
without it the simulator makes one and prints the line to use.)

Scenarios: permission, question, tasks, review, ratelimit, statusline,
registry, goal, bgagent, burst (or all; burst is not in all). Each prints what
the app's `--dump-state` output should show ("EXPECT ..."). PermissionRequest
and AskUserQuestion hooks block until answered from the app (UI, or
`approve <id>` / `answer <id> <question>=<label>` in --dev-console); their hook
output is printed when they return.
"""
import argparse
import json
import os
import random
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_HOOK = os.path.join(HERE, "..", "Scripts", "agentnotch-hook.py")
MAX_SOCKET_PATH_BYTES = 103


def default_socket():
    """The app's socket: AGENTNOTCH_SOCKET, else <support>/hook.sock, else the /tmp
    fallback when that path is too long for a Unix socket (as the app does)."""
    if os.environ.get("AGENTNOTCH_SOCKET"):
        return os.environ["AGENTNOTCH_SOCKET"]
    support = os.environ.get("AGENTNOTCH_SUPPORT_DIR") or os.path.expanduser(
        "~/Library/Application Support/Agent Notch/Claude"
    )
    path = os.path.join(support, "hook.sock")
    if len(path.encode("utf-8")) > MAX_SOCKET_PATH_BYTES:
        path = "/tmp/agentnotch-%d/hook.sock" % os.getuid()
    return path


SCENARIOS = ["permission", "question", "tasks", "review", "ratelimit", "statusline", "registry", "goal", "bgagent", "bgworkflow"]
EXTRA_SCENARIOS = ["burst"]
PREFIXES = {
    "permission": "a111",
    "question": "b222",
    "tasks": "c333",
    "review": "d444",
    "ratelimit": "e555",
    "statusline": "f666",
    "registry": "9777",
    "goal": "6888",
    "bgagent": "5999",
    "bgworkflow": "3bbb",
    "burst": "4000",
}

print_lock = threading.Lock()
ARGS = None  # parsed command line, set in main()
ACCOUNTS = {}  # account name -> config dir, set in main()
LIVE_SESSIONS = []  # every FakeSession ever made, cleaned up on exit
BLOCKING = []  # blocking hook processes


def say(tag, text):
    with print_lock:
        print("[sim] %-10s %s" % (tag, text), flush=True)


def slug(cwd):
    """Claude Code's project folder name: every non-alphanumeric char -> '-'."""
    return "".join(c if (c.isascii() and c.isalnum()) else "-" for c in cwd)


def stand_in_process():
    """A stand-in "Claude process" whose pid the app can check. It waits on
    its stdin, so it exits when the simulator does, however that happens."""
    return subprocess.Popen(["/bin/sh", "-c", "read _"], stdin=subprocess.PIPE)


class FakeSession:
    def __init__(self, scenario, account, run_id, cwd, title):
        self.scenario = scenario
        self.account = account
        self.config_dir = ACCOUNTS[account]
        self.session_id = "%s%s-0000-4000-8000-%012x" % (PREFIXES[scenario], run_id, random.getrandbits(48))
        self.cwd = cwd
        self.title = title
        self.process = None
        self.registry_path = None
        # Registered before anything that can fail, so it's always cleaned up.
        LIVE_SESSIONS.append(self)
        project_dir = os.path.join(self.config_dir, "projects", slug(cwd))
        os.makedirs(project_dir, exist_ok=True)
        os.makedirs(os.path.join(self.config_dir, "sessions"), exist_ok=True)
        self.transcript_path = os.path.join(project_dir, self.session_id + ".jsonl")
        self.process = stand_in_process()
        self.pid = self.process.pid
        self.tool_counter = 0
        self.registry_path = os.path.join(self.config_dir, "sessions", "%d.json" % self.pid)
        self.registry_status = None
        self.started_ms = int(time.time() * 1000)
        self.write_transcript([
            {"type": "ai-title", "aiTitle": title, "sessionId": self.session_id},
            {
                "type": "user",
                "uuid": self.uuid(),
                "sessionId": self.session_id,
                "timestamp": now_iso(),
                "message": {"role": "user", "content": "Please work on: " + title},
            },
        ])

    @property
    def short(self):
        return self.session_id[:8]

    def uuid(self):
        return "%08x-0000-4000-8000-%012x" % (random.getrandbits(32), random.getrandbits(48))

    def next_tool_id(self):
        self.tool_counter += 1
        return "toolu_%s_%02d" % (self.session_id[:8], self.tool_counter)

    def write_transcript(self, entries):
        with open(self.transcript_path, "a") as handle:
            for entry in entries:
                handle.write(json.dumps(entry) + "\n")

    def assistant_turn(self, text, context_tokens, message_id=None, tool_uses=None):
        """Append an assistant response (with usage) to the transcript. Repeated
        lines share the message id, like Claude Code's streamed blocks."""
        message_id = message_id or "msg_%x" % random.getrandbits(40)
        blocks = [{"type": "text", "text": text}] + (tool_uses or [])
        usage = {
            "input_tokens": 12,
            "output_tokens": 180,
            "cache_read_input_tokens": context_tokens - 2012,
            "cache_creation_input_tokens": 2000,
        }
        lines = []
        for block in blocks:
            lines.append({
                "type": "assistant",
                "uuid": self.uuid(),
                "sessionId": self.session_id,
                "requestId": "req_" + message_id,
                "isSidechain": False,
                "timestamp": now_iso(),
                "message": {
                    "id": message_id,
                    "role": "assistant",
                    "model": "claude-opus-4-5",
                    "content": [block],
                    "usage": usage,
                },
            })
        self.write_transcript(lines)

    def tool_result(self, tool_use_id, text, structured=None):
        """Append the user line carrying a tool result, as Claude Code does."""
        entry = {
            "type": "user",
            "uuid": self.uuid(),
            "sessionId": self.session_id,
            "isSidechain": False,
            "timestamp": now_iso(),
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tool_use_id, "content": text},
            ]},
        }
        if structured is not None:
            entry["toolUseResult"] = structured
        self.write_transcript([entry])

    def hook_env(self):
        env = dict(os.environ)
        env.update({
            "AGENTNOTCH_SOCKET": ARGS.socket,
            "AGENTNOTCH_DEV": "1",
            "CLAUDE_PID": str(self.pid),
            "CLAUDE_CONFIG_DIR": self.config_dir,
            "CLAUDE_CODE_SESSION_ATTENDED": "1",
            "CLAUDE_CODE_ENTRYPOINT": "cli",
            "CLAUDE_CODE_SESSION_ID": self.session_id,
        })
        return env

    def payload(self, event, **fields):
        data = {
            "hook_event_name": event,
            "session_id": self.session_id,
            "transcript_path": self.transcript_path,
            "cwd": self.cwd,
            "permission_mode": "default",
        }
        data.update(fields)
        return data

    def hook(self, event, mirror=True, **fields):
        """Run the real hook script for one event (fire and forget)."""
        result = subprocess.run(
            [sys.executable, ARGS.hook],
            input=json.dumps(self.payload(event, **fields)).encode(),
            env=self.hook_env(),
            capture_output=True,
            timeout=10,
        )
        if not ARGS.quiet:
            say(self.short, "hook %s" % event)
        if mirror:
            self.mirror_registry(event, fields)
        return result

    def mirror_registry(self, event, fields):
        """Claude Code updates its session registry after the hooks of a
        status change have run; do the same so the app's registry
        reconciliation sees realistic data (--no-registry turns it off)."""
        if ARGS.no_registry or fields.get("agent_id"):
            return
        status = {
            "SessionStart": "idle",
            "UserPromptSubmit": "busy",
            "Stop": self.status_after_stop(fields.get("background_tasks") or []),
            "StopFailure": "idle",
        }.get(event)
        if status and status != self.registry_status:
            self.write_registry(status, quiet=True)

    @staticmethod
    def status_after_stop(background_tasks):
        """Claude Code 2.1.x stays "busy" while agents, workflows, cloud
        sessions or teammates of the session run, and says "shell" when only
        shells or monitors are left."""
        types = {task.get("type") for task in background_tasks if isinstance(task, dict)}
        if types & {"subagent", "workflow", "teammate", "cloud session"}:
            return "busy"
        if types & {"shell", "monitor"}:
            return "shell"
        return "idle"

    def blocking_hook(self, event, on_done, **fields):
        """Start a hook that waits for the app's decision; `on_done(stdout)` runs
        in a thread when it returns. Returns the Popen handle."""
        process = subprocess.Popen(
            [sys.executable, ARGS.hook],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            env=self.hook_env(),
        )
        BLOCKING.append(process)
        process.stdin.write(json.dumps(self.payload(event, **fields)).encode())
        process.stdin.close()
        say(self.short, "hook %s (blocking until answered)" % event)
        # The terminal shows its permission dialog at the same time.
        if not ARGS.no_registry and not fields.get("agent_id"):
            time.sleep(0.2)
            self.write_registry("waiting", waiting_for="permission prompt", quiet=True)

        def wait():
            output = process.stdout.read()
            process.wait()
            if not ARGS.no_registry and not fields.get("agent_id") and self.registry_status == "waiting":
                self.write_registry("busy", quiet=True)
            on_done(output.decode("utf-8", "replace").strip())

        threading.Thread(target=wait, daemon=True).start()
        return process

    def status_line(self, used_5h, used_7d, context_pct, cost):
        """Send a status line update the way the status line wrapper does."""
        now = int(time.time())
        message = {
            "event": "StatusLine",
            "session_id": self.session_id,
            "transcript_path": self.transcript_path,
            "cwd": self.cwd,
            "config_dir_env": self.config_dir,
            "status_line": {
                "rate_limits": {
                    "five_hour": {"used_percentage": used_5h, "resets_at": now + 2 * 3600 + 13 * 60},
                    "seven_day": {"used_percentage": used_7d, "resets_at": now + 3 * 86400},
                },
                "context_window": {"used_percentage": context_pct, "context_window_size": 200000},
                "model": {"id": "claude-opus-4-5", "display_name": "Opus 4.5"},
                "cost": {"total_cost_usd": cost},
                "session_name": self.title,
                "version": "2.1.280",
            },
        }
        send_raw(message)
        say(self.short, "status line: 5h %s%% · 7d %s%% · context %s%%" % (used_5h, used_7d, context_pct))

    def write_registry(self, status, waiting_for=None, kind="interactive", entrypoint="cli", name=None,
                       name_source="derived", quiet=False):
        """Write <configDir>/sessions/<pid>.json like Claude Code's session registry
        (atomically, as a reader may poll it at any moment)."""
        now_ms = int(time.time() * 1000)
        entry = {
            "pid": self.pid,
            "sessionId": self.session_id,
            "cwd": self.cwd,
            "startedAt": self.started_ms,
            "procStart": proc_start_utc(self.pid),
            "version": "2.1.280",
            "kind": kind,
            "entrypoint": entrypoint,
            # Claude Code derives "<folder>-<n>" unless the session was named.
            "name": name or "%s-%d" % (os.path.basename(self.cwd), self.pid % 97),
            "nameSource": name_source if name else "derived",
            "status": status,
            "updatedAt": now_ms,
            "statusUpdatedAt": now_ms,
        }
        if waiting_for:
            entry["waitingFor"] = waiting_for
        temporary = self.registry_path + ".tmp"
        with open(temporary, "w") as handle:
            json.dump(entry, handle)
        os.replace(temporary, self.registry_path)
        self.registry_status = status
        if not quiet:
            say(self.short, "registry %s%s (%s)" % (status, " / " + waiting_for if waiting_for else "", kind))

    def expect(self, text):
        say("EXPECT", "%s acct=%s %s" % (self.short, os.path.basename(self.config_dir), text))

    def close(self):
        if self.process is not None and self.process.poll() is None:
            try:
                self.process.stdin.close()
            except Exception:
                pass
            self.process.terminate()
            try:
                self.process.wait(timeout=2)
            except Exception:
                self.process.kill()
        if self.registry_path:
            try:
                os.remove(self.registry_path)
            except OSError:
                pass


def proc_start_utc(pid):
    """`ps -o lstart` in UTC, as Claude Code records it."""
    try:
        env = dict(os.environ, TZ="UTC", LC_ALL="C")
        out = subprocess.run(["ps", "-o", "lstart=", "-p", str(pid)], capture_output=True, text=True, env=env, timeout=5)
        return out.stdout.strip() or None
    except Exception:
        return None


def now_iso():
    return time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime())


def send_raw(message):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(1.0)
    try:
        sock.connect(ARGS.socket)
        sock.sendall(json.dumps(message).encode())
        sock.shutdown(socket.SHUT_WR)
    finally:
        sock.close()


# MARK: - Scenarios

def scenario_permission(run_id):
    s = FakeSession("permission", "personal", run_id, "/Users/dev/@org/api-server", "Clean build artifacts")
    s.hook("SessionStart", source="startup", model="claude-opus-4-5")
    s.hook("UserPromptSubmit", prompt="clean the build folder", source="user")
    tool_id = s.next_tool_id()
    tool_input = {"command": "rm -rf build", "description": "Remove build output"}
    s.assistant_turn("I'll remove the build folder.", 42000, tool_uses=[
        {"type": "tool_use", "id": tool_id, "name": "Bash", "input": tool_input},
    ])
    s.hook("PreToolUse", tool_name="Bash", tool_input=tool_input, tool_use_id=tool_id)
    suggestion = {
        "type": "addRules",
        "rules": [{"toolName": "Bash", "ruleContent": "rm -rf build"}],
        "behavior": "allow",
        "destination": "session",
    }

    def done(output):
        say(s.short, "permission hook returned: %s" % (output or "<no output: Claude Code's own prompt decides>"))
        if '"allow"' in output:
            s.hook("PostToolUse", tool_name="Bash", tool_input=tool_input, tool_use_id=tool_id,
                   tool_response={"stdout": "", "stderr": ""})
            s.assistant_turn("Build folder removed.", 43000)
            s.hook("Stop", last_assistant_message="Removed ./build. The next build will start clean.", background_tasks=[])
            s.expect("attn=readyForReview (after the allow)")

    s.blocking_hook(
        "PermissionRequest", done,
        tool_name="Bash", tool_input=tool_input, permission_suggestions=[suggestion],
    )
    s.expect("attn=needsInput(permission:Bash) phase=waitingForApproval(Bash)")
    say("HINT", "dev console: 'approve %s' or 'approve %s always' or 'deny %s Not now'" % (s.short, s.short, s.short))
    return s


def scenario_question(run_id):
    s = FakeSession("question", "work", run_id, "/Users/dev/work/web-app", "Choose a database")
    s.hook("UserPromptSubmit", prompt="set up persistence", source="user")
    tool_id = s.next_tool_id()
    question = "Which database should we use?"
    tool_input = {"questions": [{
        "question": question,
        "header": "Database",
        "multiSelect": False,
        "options": [
            {"label": "Postgres", "description": "Relational, runs in Docker"},
            {"label": "SQLite", "description": "Single file, zero setup"},
        ],
    }]}
    s.assistant_turn("I need one decision first.", 31000, tool_uses=[
        {"type": "tool_use", "id": tool_id, "name": "AskUserQuestion", "input": tool_input},
    ])
    s.hook("PreToolUse", tool_name="AskUserQuestion", tool_input=tool_input, tool_use_id=tool_id)

    def done(output):
        say(s.short, "question hook returned: %s" % (output or "<no output>"))
        if '"answers"' in output:
            s.hook("PostToolUse", tool_name="AskUserQuestion", tool_input=tool_input, tool_use_id=tool_id,
                   tool_response={"answers": json.loads(output)["hookSpecificOutput"]["decision"]["updatedInput"]["answers"]})
            s.expect("attn=working (answered)")

    s.blocking_hook("PermissionRequest", done, tool_name="AskUserQuestion", tool_input=tool_input)
    s.expect("attn=needsInput(question)")
    say("HINT", "dev console: answer %s %s=Postgres" % (s.short, question))
    return s


def scenario_tasks(run_id):
    s = FakeSession("tasks", "work", run_id, "/Users/dev/work/web-app", "Add user settings page")
    s.hook("UserPromptSubmit", prompt="build the settings page", source="user")
    subjects = [
        ("Design the settings schema", "Designing the settings schema"),
        ("Build the settings form", "Building the settings form"),
        ("Write tests", "Writing tests"),
    ]
    for index, (subject, active) in enumerate(subjects, start=1):
        tool_id = s.next_tool_id()
        tool_input = {"subject": subject, "description": subject + " for the app", "activeForm": active}
        s.assistant_turn("Planning step %d." % index, 52000 + index * 1000, tool_uses=[
            {"type": "tool_use", "id": tool_id, "name": "TaskCreate", "input": tool_input},
        ])
        s.hook("PreToolUse", tool_name="TaskCreate", tool_input=tool_input, tool_use_id=tool_id)
        s.hook("TaskCreated", task_id=str(index), task_subject=subject, task_description=tool_input["description"])
        s.tool_result(tool_id, "Task #%d created successfully: %s" % (index, subject),
                      {"task": {"id": str(index), "subject": subject}})
        s.hook("PostToolUse", tool_name="TaskCreate", tool_input=tool_input, tool_use_id=tool_id,
               tool_response={"task": {"id": str(index), "subject": subject}})
    s.expect("tasks=0/3")

    def update(task_id, status):
        tool_id = s.next_tool_id()
        tool_input = {"taskId": task_id, "status": status}
        s.assistant_turn("Updating task %s." % task_id, 56000, tool_uses=[
            {"type": "tool_use", "id": tool_id, "name": "TaskUpdate", "input": tool_input},
        ])
        s.hook("PreToolUse", tool_name="TaskUpdate", tool_input=tool_input, tool_use_id=tool_id)
        s.tool_result(tool_id, "Updated task #%s status" % task_id)
        s.hook("PostToolUse", tool_name="TaskUpdate", tool_input=tool_input, tool_use_id=tool_id,
               tool_response={"success": True, "taskId": task_id})
        if status == "completed":
            s.hook("TaskCompleted", task_id=task_id)

    update("1", "in_progress")
    time.sleep(ARGS.step)
    update("1", "completed")
    update("2", "in_progress")
    # A subagent's task calls must not count.
    s.hook("PreToolUse", tool_name="TaskUpdate", tool_input={"taskId": "3", "status": "completed"},
           tool_use_id=s.next_tool_id(), agent_id="agent-sub-1", agent_type="general-purpose")
    s.expect('attn=working tasks=1/3 active="Building the settings form"')
    return s


def scenario_review(run_id):
    s = FakeSession("review", "personal", run_id, "/Users/dev/@org/api-server", "Fix flaky login test")
    s.hook("UserPromptSubmit", prompt="the login test is flaky, fix it", source="user")
    tool_id = s.next_tool_id()
    tool_input = {"file_path": "/Users/dev/@org/api-server/tests/login.test.ts", "old_string": "sleep(100)", "new_string": "await ready()"}
    s.assistant_turn("Replacing the sleep with an explicit wait.", 64000, tool_uses=[
        {"type": "tool_use", "id": tool_id, "name": "Edit", "input": tool_input},
    ])
    s.hook("PreToolUse", tool_name="Edit", tool_input=tool_input, tool_use_id=tool_id)
    s.hook("PostToolUse", tool_name="Edit", tool_input=tool_input, tool_use_id=tool_id, tool_response={"filePath": tool_input["file_path"]})
    s.assistant_turn("The login test now waits for readiness instead of sleeping.", 66000)
    s.hook("Stop", last_assistant_message="Fixed the flaky login test: it now awaits ready() instead of sleeping 100 ms. 3/3 runs green.",
           background_tasks=[], stop_hook_active=False)
    # Late events after Stop must not bring it back to "working".
    s.hook("PostToolUse", tool_name="Bash", tool_input={"command": "npm test"}, tool_use_id="toolu_late_bg", tool_response={})
    s.hook("Notification", notification_type="idle_prompt", message="Claude is waiting for your input")
    s.expect('attn=readyForReview review="Fixed the flaky login test..."')
    say("HINT", "dev console: 'review %s' clears it" % s.short)
    return s


def scenario_ratelimit(run_id):
    s = FakeSession("ratelimit", "work", run_id, "/Users/dev/work/data-pipeline", "Backfill events table")
    s.hook("UserPromptSubmit", prompt="run the backfill", source="user")
    s.assistant_turn("Starting the backfill.", 88000)
    s.hook("StopFailure", error="rate_limit", error_details="429 Too Many Requests",
           last_assistant_message="API Error: rate limit reached")
    s.expect("attn=needsInput(error:Rate limited) (the preview keeps the last real reply)")
    return s


def scenario_statusline(run_id, others):
    # Status line for sessions that already exist (context % + usage bus)...
    for session, usage in zip(others, [(23, 41, 21, 0.42), (67, 55, 33, 1.10), (91, 80, 44, 2.35)]):
        session.status_line(*usage)
        session.expect("ctx=%d%% model=Opus 4.5" % usage[2])
    # ...and one only known from its status line: tracked as idle.
    s = FakeSession("statusline", "personal", run_id, "/Users/dev/notes", "Weekly notes")
    s.status_line(12, 40, 8, 0.05)
    s.expect("attn=idle phase=idle ctx=8%")
    # Its registry entry gives the app the pid (dropped when the process ends).
    s.write_registry("idle")
    return s


def scenario_registry(run_id):
    # A VS Code session the hooks never reported (e.g. started before the app),
    # blocked on a permission dialog...
    s = FakeSession("registry", "work", run_id, "/Users/dev/work/mobile", "Upgrade React Native")
    s.write_registry("waiting", waiting_for="permission prompt", entrypoint="claude-vscode",
                     name="Upgrade React Native", name_source="user")
    s.expect('attn=needsInput(dialog:permission prompt) phase=waitingForInput title="Upgrade React Native"')
    # ...and a background session, which must be ignored.
    bg = FakeSession("registry", "work", run_id, "/Users/dev/work/mobile", "Nightly job")
    bg.write_registry("busy", kind="bg")
    say("EXPECT", "%s is NOT tracked (kind=bg)" % bg.short)
    say("HINT", "the registry is polled every 3 s. The app must know the account: run after another "
        "work-account scenario, or launch it with AGENTNOTCH_EXTRA_CONFIG_DIRS=%s" % ":".join(ACCOUNTS.values()))
    return [s, bg]


def scenario_goal(run_id):
    """A blocking Stop hook (/goal) continues the turn after our Stop: the
    session must read "working" throughout and be done once, at the end."""
    s = FakeSession("goal", "personal", run_id, "/Users/dev/@org/api-server", "Make every test pass")
    s.hook("UserPromptSubmit", prompt="/goal all tests green", source="user")
    s.assistant_turn("First pass done; 2 tests still fail.", 70000)
    # Our hook runs with the other Stop hooks; the registry stays busy while
    # the /goal evaluator decides, and it decides to continue.
    s.hook("Stop", mirror=False, last_assistant_message="First pass done; 2 tests still fail.", background_tasks=[])
    s.expect("attn=working stop=pending (a Stop hook may still continue the turn)")
    time.sleep(max(ARGS.step, 1.5))
    tool_id = s.next_tool_id()
    tool_input = {"command": "npm test -- --only-failed"}
    s.assistant_turn("Fixing the remaining two.", 71000, tool_uses=[
        {"type": "tool_use", "id": tool_id, "name": "Bash", "input": tool_input},
    ])
    s.hook("PreToolUse", tool_name="Bash", tool_input=tool_input, tool_use_id=tool_id)
    s.hook("PostToolUse", tool_name="Bash", tool_input=tool_input, tool_use_id=tool_id, tool_response={"stdout": "all green"})
    s.assistant_turn("All tests pass.", 72000)
    s.hook("Stop", last_assistant_message="All tests pass.", background_tasks=[], stop_hook_active=True)
    s.expect('attn=readyForReview review="All tests pass." (one Done, not two)')
    return s


def scenario_bgagent(run_id):
    """A background agent asks for permission after the main turn stopped:
    its request must stay answerable, and until the agent is done the
    session keeps working (waiting on it), not ready for review."""
    s = FakeSession("bgagent", "work", run_id, "/Users/dev/work/web-app", "Audit dependencies")
    s.hook("UserPromptSubmit", prompt="audit the dependencies in the background", source="user")
    agent_tool = s.next_tool_id()
    s.hook("PreToolUse", tool_name="Agent", tool_input={"description": "dependency audit", "run_in_background": True},
           tool_use_id=agent_tool)
    s.hook("PostToolUse", tool_name="Agent", tool_input={"description": "dependency audit"}, tool_use_id=agent_tool,
           tool_response={"status": "async_launched"})
    s.hook("Stop", last_assistant_message="The audit runs in the background; I'll report when it's done.",
           background_tasks=[{"type": "subagent", "id": "agent-audit"}])
    s.expect("attn=working awaiting=1 (the main turn stopped; its agent runs on)")
    tool_id = s.next_tool_id()
    tool_input = {"command": "npm audit --json"}
    s.hook("PreToolUse", tool_name="Bash", tool_input=tool_input, tool_use_id=tool_id,
           agent_id="agent-audit", agent_type="general-purpose")

    def done(output):
        say(s.short, "agent permission hook returned: %s" % (output or "<no output>"))

    s.blocking_hook("PermissionRequest", done, tool_name="Bash", tool_input=tool_input, agent_id="agent-audit")
    s.expect("attn=needsInput(permission:Bash) (the agent's request survived the main Stop)")
    say("HINT", "dev console: 'approve %s', then the agent reports back" % s.short)
    return s


def scenario_bgworkflow(run_id):
    """A workflow keeps running after the turn that launched it stopped:
    the session keeps working ("Waiting on 1 workflow") until the
    workflow's result wakes Claude, whose last turn is the one announced."""
    s = FakeSession("bgworkflow", "work", run_id, "/Users/dev/work/web-app", "Sweep the repo")
    s.hook("UserPromptSubmit", prompt="sweep the repo for the old name", source="user")
    tool = s.next_tool_id()
    s.hook("PreToolUse", tool_name="Workflow", tool_input={"script": "…"}, tool_use_id=tool)
    s.hook("PostToolUse", tool_name="Workflow", tool_input={"script": "…"}, tool_use_id=tool,
           tool_response={"status": "async_launched", "taskId": "w1", "taskType": "local_workflow"})
    s.hook("Stop", last_assistant_message="The sweep is running; I'll report when it's back.",
           background_tasks=[{"type": "workflow", "id": "w1", "name": "sweep"}])
    s.expect("attn=working awaiting=1 (the workflow runs on after the Stop)")
    s.hook("Notification", notification_type="idle_prompt", message="Claude is waiting for your input")
    s.expect("attn=working awaiting=1 (idle_prompt doesn't end the wait)")
    # The workflow's result wakes Claude (2.1.x sends no prompt source).
    s.hook("UserPromptSubmit", prompt="<task-notification>\n<task-id>w1</task-id>\n<status>completed</status>\n"
                                      "<summary>Dynamic workflow \"sweep\" completed</summary>\n</task-notification>")
    s.hook("Stop", last_assistant_message="The sweep found 12 places; all fixed.", background_tasks=[])
    s.expect('attn=readyForReview review="The sweep found 12 places; all fixed." (announced once)')
    return s


def scenario_burst(run_id):
    """--burst-sessions sessions × --burst-turns turns, as fast as possible
    (throughput: every one must end ready for review)."""
    sessions = [FakeSession("burst", "personal" if i % 2 else "work", run_id, "/Users/dev/burst/p%d" % i, "Burst %d" % i)
                for i in range(ARGS.burst_sessions)]
    started = time.time()
    count = 0
    for turn in range(ARGS.burst_turns):
        for s in sessions:
            tool_id = s.next_tool_id()
            s.hook("UserPromptSubmit", prompt="turn %d" % turn, source="user")
            s.hook("PreToolUse", tool_name="Bash", tool_input={"command": "true"}, tool_use_id=tool_id)
            s.hook("PostToolUse", tool_name="Bash", tool_input={"command": "true"}, tool_use_id=tool_id, tool_response={})
            s.hook("Stop", last_assistant_message="turn %d done" % turn, background_tasks=[])
            count += 4
    say("burst", "%d events in %.1f s" % (count, time.time() - started))
    say("EXPECT", "%d sessions attn=readyForReview review=\"turn %d done\"" % (len(sessions), ARGS.burst_turns - 1))
    return sessions


def run_scenario(name, run_id, made):
    if name == "statusline":
        others = [x for x in made if x.scenario in ("permission", "question", "tasks")]
        return scenario_statusline(run_id, others)
    return globals()["scenario_" + name](run_id)


def main():
    global ARGS
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--scenario", action="append",
                        help="scenario to run (repeatable or comma separated): %s, all"
                             % ", ".join(SCENARIOS + EXTRA_SCENARIOS))
    parser.add_argument("--socket", default=default_socket(), help="app socket (default: $AGENTNOTCH_SOCKET, else the app's)")
    parser.add_argument("--hook", default=os.path.normpath(DEFAULT_HOOK), help="hook script to run")
    parser.add_argument("--root", help="folder for the fake accounts (default: a new temporary one)")
    parser.add_argument("--keep-root", action="store_true", help="don't delete the fake accounts at the end")
    parser.add_argument("--step", type=float, default=0.5, help="pause between steps, seconds")
    parser.add_argument("--linger", type=float, default=60,
                        help="seconds to keep fake sessions alive at the end (0 = until Ctrl-C when blocking hooks are pending)")
    parser.add_argument("--answer-timeout", type=float, default=0,
                        help="kill still-blocked permission hooks after N seconds (exercises dead-hook detection)")
    parser.add_argument("--no-registry", action="store_true",
                        help="don't mirror session status into <configDir>/sessions/<pid>.json")
    parser.add_argument("--burst-sessions", type=int, default=30, help="sessions in the burst scenario")
    parser.add_argument("--burst-turns", type=int, default=4, help="turns per session in the burst scenario")
    parser.add_argument("--quiet", action="store_true", help="don't print every hook event")
    ARGS = parser.parse_args()

    wanted = []
    for item in ARGS.scenario or ["all"]:
        wanted += [part.strip() for part in item.split(",") if part.strip()]
    if "all" in wanted:
        wanted = [name for name in wanted if name != "all"] + [name for name in SCENARIOS if name not in wanted]
    unknown = [name for name in wanted if name not in SCENARIOS + EXTRA_SCENARIOS]
    if unknown:
        parser.error("unknown scenario(s): %s" % ", ".join(unknown))
    if not os.path.exists(ARGS.socket):
        parser.error("socket %s doesn't exist: is the app running with this AGENTNOTCH_SOCKET?" % ARGS.socket)
    if not os.path.exists(ARGS.hook):
        parser.error("hook script not found: %s" % ARGS.hook)

    made_root = ARGS.root is None
    root = ARGS.root or tempfile.mkdtemp(prefix="agentnotch-fake-")
    root = os.path.abspath(root)
    ACCOUNTS["personal"] = os.path.join(root, ".claude")
    ACCOUNTS["work"] = os.path.join(root, ".claude-work")
    for config_dir in ACCOUNTS.values():
        os.makedirs(config_dir, exist_ok=True)

    # Clean up the stand-in "Claude" processes on SIGTERM and SIGHUP too.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    signal.signal(signal.SIGHUP, lambda *_: sys.exit(0))

    run_id = "%04x" % random.getrandbits(16)
    say("run", "run %s · socket %s" % (run_id, ARGS.socket))
    say("run", "fake accounts: AGENTNOTCH_EXTRA_CONFIG_DIRS=%s" % ":".join(ACCOUNTS.values()))

    made = []
    try:
        for name in wanted:
            say("scenario", name)
            result = run_scenario(name, run_id, made)
            made += result if isinstance(result, list) else [result]
            time.sleep(ARGS.step)

        say("run", "all scenarios sent; sessions: %s" % ", ".join(
            "%s=%s(pid %d)" % (s.scenario, s.short, s.pid) for s in made[:12]))
        deadline = time.time() + ARGS.linger if ARGS.linger > 0 else None
        answer_deadline = time.time() + ARGS.answer_timeout if ARGS.answer_timeout > 0 else None
        while True:
            pending = [p for p in BLOCKING if p.poll() is None]
            if answer_deadline and time.time() >= answer_deadline and pending:
                for process in pending:
                    process.kill()
                say("run", "killed %d blocked hook(s): EXPECT the app to drop their approvals (dead hook)" % len(pending))
                answer_deadline = None
            if deadline and time.time() >= deadline:
                break
            if not deadline and not pending:
                break
            time.sleep(0.25)
    except (KeyboardInterrupt, SystemExit):
        pass
    finally:
        for process in BLOCKING:
            if process.poll() is None:
                process.kill()
        for session in LIVE_SESSIONS:
            session.close()
        if made_root and not ARGS.keep_root:
            shutil.rmtree(root, ignore_errors=True)
        say("run", "done; fake session pids ended (the app drops them within ~3 s)")


if __name__ == "__main__":
    main()
