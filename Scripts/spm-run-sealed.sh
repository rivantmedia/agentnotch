#!/usr/bin/env bash
# Launch a sealed copy of the app for a few seconds, report what it did, kill it.
#
#   Scripts/spm-run-sealed.sh                        # 8 seconds (10 at most)
#   Scripts/spm-run-sealed.sh 5 --skip-build
#   Scripts/spm-run-sealed.sh 8 --skip-build --edge top --open-panel sessions
#   Scripts/spm-run-sealed.sh --skip-build --snapshot-claude /tmp/shots
#
# Options:
#   --skip-build, --release   passed to Scripts/spm-build-app.sh
#   --edge right|left|top|bottom
#                             the notch's edge, written only into the sealed
#                             copy's own preferences domain (deleted after)
#   --open-panel <route>      open the Claude sessions panel ~2 s after launch
#                             (sessions | sessions:<ring> | session:<id> | setup,
#                             `auto:` first to open it as an auto-open does), check
#                             it, then close it again before the run ends. Needs
#                             at least 7 seconds.
#   --panel-self-test         open the panel on a session's chat and drive it
#                             in-process (⌘V fallback, Esc back then close,
#                             ring click open/switch/close, outside click with
#                             and without "Keep open", size and edge changes
#                             re-anchoring); each step is checked. Needs at
#                             least 9 seconds.
#   --snapshot-claude <dir>   render the fork's notch and panel sheets to PNGs
#                             in <dir> and exit (no window is shown); also
#                             SPCN_SNAPSHOT_CLAUDE=<dir> in the environment
#
# Safe beside a real setup, including the official Codenotch:
# - a separate bundle, "build/sealed/Superpowered Codenotch Sealed.app", with
#   bundle id com.paraswtf.superpowered-codenotch.sealed, so its preferences,
#   single-instance check and login item are its own (never com.vinz.codenotch,
#   never the real fork's id);
# - SPCN_SAFE_MODE=1: fixture rings only; no keychain, session/transcript
#   reads, provider network calls or subprocesses (see Fork.isSealed);
# - only the pid this script started is ever signalled, and the sealed
#   preferences domain is deleted afterwards.
# Other agents may launch the same sealed bundle id; hold the machine-wide
# lock (mkdir /tmp/spcn-sealed-run.lock) around a run.
#
# The report lists inet sockets (expect none), files it holds open under
# .claude/.codex or a keychain (expect none), and its on-screen windows. With
# --open-panel it also asserts, from the window server's own bounds, that the
# panel window sits on the inner side of the notch window and inside the
# screen's visible frame, and, from the app's log, that hover cards were off
# and the notch pinned while the panel was open and restored after; any
# failure makes the exit status 1.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SEALED_ID="com.paraswtf.superpowered-codenotch.sealed"
NAME="Superpowered Codenotch Sealed"
OUT="$ROOT/build/sealed"

SECONDS_ALIVE=8
BUILD_ARGS=()
EDGE=""
OPEN_PANEL=""
SNAPSHOT_DIR=""
SELF_TEST=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        [0-9]*)           SECONDS_ALIVE="$1" ;;
        --skip-build|--release|--debug) BUILD_ARGS+=("$1") ;;
        --edge)           EDGE="${2:-}"; shift ;;
        --open-panel)     OPEN_PANEL="${2:-}"; shift ;;
        --snapshot-claude) SNAPSHOT_DIR="${2:-}"; shift ;;
        --panel-self-test) SELF_TEST=1 ;;
        -h|--help)        sed -n '2,40p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done
# SPCN_SNAPSHOT_CLAUDE=<dir> in the environment is the same as
# --snapshot-claude <dir> (the app reads it in a sealed run and exits when
# done, which a timed run would report as a failure).
if [[ -z "$SNAPSHOT_DIR" && -n "${SPCN_SNAPSHOT_CLAUDE:-}" ]]; then
    SNAPSHOT_DIR="$SPCN_SNAPSHOT_CLAUDE"
fi
case "$EDGE" in ""|right|left|top|bottom) ;; *) echo "--edge must be right, left, top or bottom" >&2; exit 2 ;; esac
# A sealed run lasts 10 seconds at most (the rule every agent and script
# launching it keeps): it shares the Mac with whoever is using it.
[[ "$SECONDS_ALIVE" -le 10 ]] || { echo "a sealed run lasts at most 10 seconds" >&2; exit 2; }
if [[ $SELF_TEST -eq 1 ]]; then
    OPEN_PANEL="${OPEN_PANEL:-session:needs-question}"
    [[ "$SECONDS_ALIVE" -ge 9 ]] || { echo "--panel-self-test needs at least 9 seconds" >&2; exit 2; }
fi
if [[ -n "$OPEN_PANEL" && "$SECONDS_ALIVE" -lt 7 ]]; then
    echo "--open-panel needs at least 7 seconds" >&2; exit 2
fi

"$ROOT/Scripts/spm-build-app.sh" --bundle-id "$SEALED_ID" --name "$NAME" --out "$OUT" ${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"} \
    | grep -v -E "warning:|^\s*[0-9]+ \||^\s+\||^\s*$" | tail -5

EXE="$OUT/$NAME.app/Contents/MacOS/$NAME"
LOG="$OUT/run.log"
: > "$LOG"
SDK_FOR_SWIFT="${SDKROOT:-$(ls -d /Library/Developer/CommandLineTools/SDKs/MacOSX26.*.sdk 2>/dev/null | sort -V | tail -1)}"

forget_sealed_domain() {
    defaults delete "$SEALED_ID" >/dev/null 2>&1 || true
    rm -f "$HOME/Library/Preferences/$SEALED_ID.plist"
    rm -rf "$HOME/Library/Saved Application State/$SEALED_ID.savedState" \
           "$HOME/Library/Caches/$SEALED_ID" "$HOME/Library/HTTPStorages/$SEALED_ID"
}

report() { if [[ -n "$1" ]]; then sed 's/^/  /' <<< "$1"; else echo "  (none)"; fi; }

# --- Snapshots: render and exit ------------------------------------------------
if [[ -n "$SNAPSHOT_DIR" ]]; then
    [[ "$SNAPSHOT_DIR" = /* ]] || SNAPSHOT_DIR="$PWD/$SNAPSHOT_DIR"
    trap forget_sealed_domain EXIT
    status=0
    SPCN_SAFE_MODE=1 CODENOTCH_DEMO=1 "$EXE" --snapshot-claude "$SNAPSHOT_DIR" > "$LOG" 2>&1 &
    PID=$!
    # Within the same 10 seconds as any sealed run.
    for ((t = 0; t < 10; t++)); do kill -0 "$PID" 2>/dev/null || break; sleep 1; done
    if kill -0 "$PID" 2>/dev/null; then
        kill -9 "$PID" 2>/dev/null || true
        echo "snapshot run did not exit within 10 s" >&2
        status=1
    else
        wait "$PID" || status=$?
    fi
    grep -v '^\[spcn-' "$LOG" || true
    [[ $status -eq 0 ]] || { echo "snapshot run failed (status $status); log: $LOG" >&2; exit 1; }
    exit 0
fi

# --- Window probe ----------------------------------------------------------------
# Every window of a pid and every screen, in the window server's coordinates
# (origin top-left of the main screen, y down), as JSON. Compiled once, so a
# sample is taken the moment it is asked for (`swift -e` takes seconds).
PROBE="$OUT/sealed-window-probe"
if [[ ! -x "$PROBE" || "$0" -nt "$PROBE" ]]; then
    PROBE_SRC="$(mktemp -d)/probe.swift"
    cat > "$PROBE_SRC" <<'SWIFT'
import AppKit
let pid = Int32(CommandLine.arguments[1])!
let mainHeight = NSScreen.screens.first?.frame.height ?? 0
func flip(_ r: CGRect) -> [String: Double] {
    ["x": r.minX, "y": mainHeight - r.maxY, "w": r.width, "h": r.height]
}
var windows: [[String: Any]] = []
for w in CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
    where (w["kCGWindowOwnerPID"] as? Int32) == pid {
    let b = w["kCGWindowBounds"] as? [String: Double] ?? [:]
    windows.append(["layer": w["kCGWindowLayer"] as? Int ?? -1,
                    "onscreen": (w["kCGWindowIsOnscreen"] as? Bool) ?? false,
                    "x": b["X"] ?? 0, "y": b["Y"] ?? 0, "w": b["Width"] ?? 0, "h": b["Height"] ?? 0])
}
let screens = NSScreen.screens.map { ["frame": flip($0.frame), "visible": flip($0.visibleFrame)] }
let data = try! JSONSerialization.data(withJSONObject: ["windows": windows, "screens": screens])
print(String(data: data, encoding: .utf8)!)
SWIFT
    SDKROOT="$SDK_FOR_SWIFT" swiftc -O -o "$PROBE" "$PROBE_SRC" 2>/dev/null || rm -f "$PROBE"
    rm -rf "$(dirname "$PROBE_SRC")"
fi
windows_json() { [[ -x "$PROBE" ]] && "$PROBE" "$1" 2>/dev/null; }

# --- Run -----------------------------------------------------------------------
# Only the sealed copy's own domain is written, and it is deleted on exit.
forget_sealed_domain
[[ -n "$EDGE" ]] && defaults write "$SEALED_ID" notchEdge -string "$EDGE"

ENV=(SPCN_SAFE_MODE=1 CODENOTCH_DEMO=1)
SAMPLE_AT=$SECONDS_ALIVE
if [[ $SELF_TEST -eq 1 ]]; then
    # Opens ~2 s after launch; the steps take ~5 s more and check themselves.
    ENV+=(SPCN_OPEN_PANEL_ON_LAUNCH="$OPEN_PANEL" SPCN_PANEL_SELF_TEST=1)
elif [[ -n "$OPEN_PANEL" ]]; then
    # Opens ~2 s after launch; sampled with it open; closed before the end.
    SAMPLE_AT=$((SECONDS_ALIVE - 3))
    ENV+=(SPCN_OPEN_PANEL_ON_LAUNCH="$OPEN_PANEL" SPCN_PANEL_CLOSE_AFTER=$((SECONDS_ALIVE - 4)))
fi

env "${ENV[@]}" "$EXE" >> "$LOG" 2>&1 &
PID=$!
echo "launched pid $PID (sealed, $SEALED_ID${EDGE:+, edge $EDGE}${OPEN_PANEL:+, panel $OPEN_PANEL}) for ${SECONDS_ALIVE}s"
cleanup() {
    if kill -0 "$PID" 2>/dev/null; then
        kill "$PID" 2>/dev/null || true
        sleep 1
        kill -9 "$PID" 2>/dev/null || true
    fi
    wait "$PID" 2>/dev/null || true
    forget_sealed_domain
}
trap cleanup EXIT

alive=1
SNAPSHOT=""
for ((t = 1; t <= SECONDS_ALIVE; t++)); do
    sleep 1
    if ! kill -0 "$PID" 2>/dev/null; then
        alive=0
        wait "$PID" 2>/dev/null && status=0 || status=$?
        echo "EXITED after ${t}s (status $status); output:"
        cat "$LOG"
        exit 1
    fi
    if [[ $t -eq $SAMPLE_AT ]]; then
        SNAPSHOT="$(windows_json "$PID" || true)"
    fi
done

failures=0
if [[ $alive -eq 1 ]]; then
    echo "alive after ${SECONDS_ALIVE}s"
    echo "inet sockets:"
    report "$(lsof -nP -a -p "$PID" -i 2>/dev/null || true)"
    echo "open files under .claude/.codex or a keychain:"
    report "$(lsof -nP -p "$PID" 2>/dev/null | grep -E '/\.claude|/\.codex|[Kk]eychain' || true)"
    [[ -n "$SNAPSHOT" ]] || SNAPSHOT="$(windows_json "$PID" || true)"
    echo "windows at ${SAMPLE_AT}s (layer, bounds with y down, on screen):"
    if [[ -n "$SNAPSHOT" ]]; then
        python3 - "$SNAPSHOT" <<'PY'
import json, sys
for w in json.loads(sys.argv[1])["windows"]:
    print(f"  {w['layer']}  x={w['x']:g} y={w['y']:g} w={w['w']:g} h={w['h']:g}  {int(w['onscreen'])}")
PY
    else
        echo "  (window list unavailable)"
    fi
fi

if [[ $SELF_TEST -eq 1 && $alive -eq 1 ]]; then
    echo "panel log:"
    report "$(grep '^\[spcn-panel\]' "$LOG" || true)"
    echo "panel self-test:"
    steps=$(grep -c '^\[spcn-panel\] self-test ' "$LOG" || true)
    failed=$(grep -c '^\[spcn-panel\] self-test .*: FAIL' "$LOG" || true)
    echo "  $steps steps, $failed failed"
    [[ "$steps" -ge 13 && "$failed" -eq 0 ]] || failures=$((failures + 1))
elif [[ -n "$OPEN_PANEL" && $alive -eq 1 ]]; then
    echo "panel log:"
    report "$(grep '^\[spcn-panel\]' "$LOG" || true)"
    echo "panel checks:"
    python3 - "${SNAPSHOT:-}" "${EDGE:-right}" "$LOG" <<'PY' || failures=$((failures + 1))
import json, re, sys

snapshot, edge, log_path = json.loads(sys.argv[1] or "{}"), sys.argv[2], sys.argv[3]
log = open(log_path, encoding="utf-8", errors="replace").read()
failed = False

def check(ok, what):
    global failed
    print(f"  {'ok  ' if ok else 'FAIL'} {what}")
    failed |= not ok

# NotchPanel is at the status bar level (25); the sessions panel one above.
windows = [w for w in snapshot.get("windows", []) if w["onscreen"] and w["w"] > 1 and w["h"] > 1]
notches = [w for w in windows if w["layer"] == 25]
panels = [w for w in windows if w["layer"] == 26]
floating = re.search(r"\[spcn-panel\] open .* floating ", log) is not None
check(len(panels) == 1, f"one panel window on screen while open ({len(panels)})")
if panels and floating:
    print("  (the panel floats: no notch or ring to hang off; adjacency not checked)")
if panels and notches and not floating:
    p = panels[0]
    def box(w): return w["x"], w["y"], w["x"] + w["w"], w["y"] + w["h"]
    px0, py0, px1, py1 = box(p)
    # The notch on the panel's screen: the one it overlaps or is nearest.
    def gap(n):
        nx0, ny0, nx1, ny1 = box(n)
        return max(nx0 - px1, px0 - nx1, 0) + max(ny0 - py1, py0 - ny1, 0)
    n = min(notches, key=gap)
    nx0, ny0, nx1, ny1 = box(n)
    overlap_x = min(px1, nx1) - max(px0, nx0)
    overlap_y = min(py1, ny1) - max(py0, ny0)
    tol = 1
    if edge == "right":
        ok = px1 <= nx1 - tol and px1 >= nx0 and overlap_y > 0
        detail = f"panel right {px1:g} inside notch x {nx0:g}..{nx1:g}, rows overlap {overlap_y:g}"
    elif edge == "left":
        ok = px0 >= nx0 + tol and px0 <= nx1 and overlap_y > 0
        detail = f"panel left {px0:g} inside notch x {nx0:g}..{nx1:g}, rows overlap {overlap_y:g}"
    elif edge == "top":
        ok = py0 >= ny0 + tol and py0 <= ny1 and overlap_x > 0
        detail = f"panel top {py0:g} inside notch y {ny0:g}..{ny1:g}, columns overlap {overlap_x:g}"
    else:
        ok = py1 <= ny1 - tol and py1 >= ny0 and overlap_x > 0
        detail = f"panel bottom {py1:g} inside notch y {ny0:g}..{ny1:g}, columns overlap {overlap_x:g}"
    check(ok, f"panel adjacent to the notch on its inner side ({edge}): {detail}")
    # Inside the visible frame of the screen it is on.
    screens = snapshot.get("screens", [])
    def contains(outer, x0, y0, x1, y1):
        return (x0 >= outer["x"] - tol and y0 >= outer["y"] - tol
                and x1 <= outer["x"] + outer["w"] + tol and y1 <= outer["y"] + outer["h"] + tol)
    home = [s for s in screens if contains(s["frame"], (px0 + px1) / 2, (py0 + py1) / 2, (px0 + px1) / 2, (py0 + py1) / 2)]
    visible = home[0]["visible"] if home else None
    check(visible is not None and contains(visible, px0, py0, px1, py1),
          f"panel inside the visible frame {visible}")

opened = re.search(r"\[spcn-panel\] open .*", log)
closed = re.search(r"\[spcn-panel\] close .*", log)
check(opened is not None, "the panel opened")
if opened:
    line = opened.group(0)
    counts = re.search(r"hoverCardsSuppressed=(\d+)/(\d+)", line)
    check(counts is not None and counts.group(1) == counts.group(2) and counts.group(2) != "0",
          "hover cards suppressed on every notch while open")
    if "floating" not in line:
        check("anchorPinned=true" in line, "the anchor notch pinned while open")
    if "(auto)" in line:
        check("wantsKey=false" in line and "key=false" in line, "an auto-open does not take key")
    else:
        # Key needs a user event (a click, the hot key) unless the window
        # server hands it over anyway; a launch hook has none, so both happen.
        check("wantsKey=true" in line, "opened as a key panel")
        key = re.search(r" key=(\w+)", line)
        print(f"  info key={key.group(1) if key else '?'}")
    before = re.search(r"frontmostBefore=(\S+)", line)
    after = re.search(r" frontmost=(\S+)", line)
    ours = "com.paraswtf.superpowered-codenotch.sealed"
    if before and before.group(1) == ours:
        print("  info the app was already frontmost when the panel opened")
    else:
        check(after is not None and after.group(1) != ours,
              f"the app did not come to the front (frontmost {before.group(1) if before else '?'} -> {after.group(1) if after else '?'})")
check(closed is not None, "the panel closed")
if closed:
    line = closed.group(0)
    check(re.search(r"hoverCardsSuppressed=0/\d+", line) is not None, "hover cards back after close")
    check("pin restored to false (the panel had pinned it: true)" in line or "floating" in (opened.group(0) if opened else ""),
          "the notch's own pin state (unpinned) restored after close")
sys.exit(1 if failed else 0)
PY
fi
# cleanup runs on exit: kill the pid, delete the sealed preferences domain.
[[ $failures -eq 0 ]] || exit 1
