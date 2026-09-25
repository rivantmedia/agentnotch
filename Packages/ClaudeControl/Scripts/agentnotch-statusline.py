#!/usr/bin/env python3
"""
Agent Notch status line wrapper
Derived from Superpowered Vibe Notch's superpowered-notch-statusline.py (Apache-2.0).

Claude Code runs this as the account's `statusLine` command after each
assistant message, with the session's status line JSON on stdin. It:
- starts the status line command that was configured before the app
  installed this wrapper (saved next to this file in
  agentnotch-statusline.previous.json) with the same stdin;
- while that runs, forwards a small subset (rate limits, context window,
  model, cost, name) to Agent Notch.app over its Unix socket,
  fire-and-forget;
- then passes the previous command's output and exit code straight through.
  No previous command -> no output.

It must never break the user's status line: every failure is swallowed,
talking to the app costs at most a fraction of a second, and the previous
command gets as long as it needs (up to PREVIOUS_TIMEOUT_SECONDS). When
Claude Code cancels the run, SIGTERM or SIGHUP stops the previous command's
whole process group too, as it would have stopped the command unwrapped.

The app writes its socket path into SOCKET_PATH below when it installs this
file; AGENTNOTCH_SOCKET overrides it only with AGENTNOTCH_DEV=1 too (development and
tests). Python 3.9
compatible, no dependencies (runs with -S).
"""
import json
import os
import socket
import stat
import sys

SOCKET_PATH = (os.environ.get("AGENTNOTCH_SOCKET") if os.environ.get("AGENTNOTCH_DEV") == "1" else None) or "__AGENTNOTCH_SOCKET_PATH__"
PREVIOUS_FILE = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "agentnotch-statusline.previous.json",
)
SEND_TIMEOUT_SECONDS = 0.3
# Claude Code sets no limit of its own and cancels runs it no longer needs;
# this only stops a command that hangs outright.
PREVIOUS_TIMEOUT_SECONDS = 30

# The previous command while it runs, for the signal handlers.
_previous_process = None


def as_dict(value):
    return value if isinstance(value, dict) else {}


def build_message(raw):
    """The StatusLine event for the app, or None if stdin isn't a JSON object."""
    try:
        data = json.loads(raw.decode("utf-8"))
    except Exception:
        return None
    if not isinstance(data, dict):
        return None

    context_window = as_dict(data.get("context_window"))
    cost = as_dict(data.get("cost"))
    return {
        "event": "StatusLine",
        "session_id": data.get("session_id"),
        "transcript_path": data.get("transcript_path"),
        "cwd": data.get("cwd"),
        "config_dir_env": os.environ.get("CLAUDE_CONFIG_DIR"),
        "status_line": {
            "rate_limits": data.get("rate_limits"),
            "context_window": {
                "used_percentage": context_window.get("used_percentage"),
                "context_window_size": context_window.get("context_window_size"),
            },
            "model": data.get("model"),
            "cost": {"total_cost_usd": cost.get("total_cost_usd")},
            "session_name": data.get("session_name"),
            "version": data.get("version"),
        },
    }


def is_own_socket(path):
    """The socket at `path` is this user's (the /tmp fallback folder is
    shared; someone else's socket there never gets our status line)."""
    try:
        info = os.lstat(path)
    except OSError:
        return False
    return info.st_uid == os.getuid() and stat.S_ISSOCK(info.st_mode)


def send(message):
    """Fire-and-forget: connect, send, half-close, done."""
    if not is_own_socket(SOCKET_PATH):
        return
    sock = None
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(SEND_TIMEOUT_SECONDS)
        sock.connect(SOCKET_PATH)
        sock.sendall(json.dumps(message).encode("utf-8"))
        try:
            sock.shutdown(socket.SHUT_WR)
        except OSError:
            pass
    except Exception:
        pass
    finally:
        if sock is not None:
            try:
                sock.close()
            except Exception:
                pass


def previous_command():
    """The command of the status line this wrapper replaced, if any."""
    try:
        with open(PREVIOUS_FILE, "r", encoding="utf-8") as handle:
            previous = json.load(handle)
    except Exception:
        return None
    if not isinstance(previous, dict):
        return None
    command = previous.get("command")
    if not isinstance(command, str) or not command.strip():
        return None
    # Never chain to a notch app's wrapper: ourselves (under this name or our
    # former one, Superpowered Codenotch), or Superpowered Vibe Notch's, which
    # may chain back to us (a hand-edited file could loop forever).
    wrappers = ("agentnotch-statusline.py", "superpowered-codenotch-statusline.py", "superpowered-notch-statusline.py")
    if any(name in command for name in wrappers):
        return None
    return command


def stop_previous_and_exit(signum, _frame):
    """Claude Code cancelled this run: stop the previous command's process
    group too (it has its own), then exit as the signal would have."""
    process = _previous_process
    if process is not None:
        try:
            os.killpg(process.pid, signum)
        except Exception:
            pass
    os._exit(128 + signum)


def start_previous():
    """Start the previous status line command, or return None."""
    global _previous_process
    command = previous_command()
    if command is None:
        return None
    import signal
    import subprocess

    for signum in (signal.SIGTERM, signal.SIGHUP):
        try:
            signal.signal(signum, stop_previous_and_exit)
        except Exception:
            pass
    # Its own process group, so a timeout or a cancel can stop everything
    # the command started, not just the shell.
    _previous_process = subprocess.Popen(
        command,
        shell=True,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        start_new_session=True,
    )
    return _previous_process


def finish_previous(process, raw):
    """Feed the previous command stdin, pass its stdout through; its exit code."""
    import signal
    import subprocess

    try:
        output, _ = process.communicate(raw, timeout=PREVIOUS_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except Exception:
            pass
        try:
            process.wait(timeout=1)
        except Exception:
            pass
        return 0
    try:
        sys.stdout.buffer.write(output or b"")
        sys.stdout.buffer.flush()
    except Exception:
        pass
    code = process.returncode
    return code if isinstance(code, int) and code >= 0 else 1


def main():
    try:
        raw = sys.stdin.buffer.read()
    except Exception:
        raw = b""

    # The previous command first, so it runs while we talk to the app.
    try:
        process = start_previous()
    except Exception:
        process = None

    try:
        message = build_message(raw)
        if message is not None:
            send(message)
    except Exception:
        pass

    if process is None:
        return 0
    try:
        return finish_previous(process, raw)
    except Exception:
        return 0


if __name__ == "__main__":
    sys.exit(main())
