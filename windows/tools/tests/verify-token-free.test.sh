#!/usr/bin/env bash
# Tests Scripts/verify-token-free.sh's Windows rules in a temporary git repository, never in this
# checkout: a copy of upstream's app sources and the fork's glue (windows/codenotch/src) and of
# AppDelegate.swift (the Mac's U1), committed, with UPSTREAM_REF at that commit. The copy as it is
# must pass; each glue file below, added to it, must fail. The glue is where a new call into
# upstream's dormant Claude token path would be written, so it is checked by name as well as by
# pattern: upstream's doctor reads Claude's credential, its transcript watcher walks usage.rs's
# credential-checked profiles, and usage.rs's helpers are allowed only by review.
# Run by fork.yml's Tool tests, or by hand: bash windows/tools/tests/verify-token-free.test.sh
#
# The names are assembled from parts, so that this file, which the script itself scans
# (windows/tools is the fork's own Windows code), does not hold them.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
SCRIPT="$REPO/Scripts/verify-token-free.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0
ok() { PASSED=$((PASSED + 1)); echo "ok   $1"; }
bad() { FAILED=$((FAILED + 1)); echo "FAIL $1" >&2; }

D=doctor
W=watcher
U=usage

TREE="$WORK/tree"
mkdir -p "$TREE/Scripts" "$TREE/Sources/App" "$TREE/windows/codenotch"
cp "$SCRIPT" "$TREE/Scripts/verify-token-free.sh"
cp "$REPO/Sources/App/AppDelegate.swift" "$TREE/Sources/App/AppDelegate.swift"
cp -R "$REPO/windows/codenotch/src" "$TREE/windows/codenotch/src"
git -C "$TREE" init -q
git -C "$TREE" add -A
git -C "$TREE" -c user.name=test -c user.email=test@example.invalid -c commit.gpgsign=false \
    commit -q -m base
BASE="$(git -C "$TREE" rev-parse HEAD)"
GLUE="$TREE/windows/codenotch/src/agentnotch/zz_token_free_test.rs"

# Runs the copied script; sets CODE and OUT.
run() {
    set +e
    OUT="$(UPSTREAM_REF="$BASE" bash "$TREE/Scripts/verify-token-free.sh" 2>&1)"
    CODE=$?
    set -e
}

run
if [[ $CODE -eq 0 ]]; then ok "the copy as it is passes"; else bad "the copy as it is passes (exit $CODE): $OUT"; fi

# $1 = what, $2 = the glue file's text, $3 = text the failure must name.
refused() {
    printf '%s\n' "$2" > "$GLUE"
    run
    rm -f "$GLUE"
    if [[ $CODE -eq 1 && "$OUT" == *"$3"* ]]; then ok "refuses $1"; else bad "refuses $1 (exit $CODE): $OUT"; fi
}

# By pattern (WIN_PATTERNS): the routes, named outright.
refused "upstream's doctor run from the glue" \
    "pub fn report() { let _ = crate::${D}::run(); }" "Windows fork code:"
refused "upstream's watcher started from the glue" \
    "pub fn go(app: &tauri::AppHandle) { crate::${W}::start(app.clone()); }" "Windows fork code:"
refused "usage.rs's profile walk from the glue" \
    "pub fn dirs() -> usize { crate::${U}::profile_dirs().len() }" "Windows fork code:"
refused "usage.rs's refresh from the glue" \
    "pub fn again() { crate::${U}::request_refresh(); }" "Windows fork code:"
refused "usage.rs's CLI lookup from the glue" \
    "pub fn cli() -> bool { crate::${U}::find_cli().is_some() }" "Windows fork code:"

# By name: whatever else the glue reaches in the dormant files, under any name.
refused "any other use of upstream's doctor in the glue" \
    "use crate::${D};
pub fn lines() -> usize { ${D}::provider_lines().len() }" "upstream's doctor"
refused "any use of upstream's watcher in the glue" \
    "pub fn w() { let _ = crate::${W}::Watcher::default(); }" "upstream's transcript watcher"
refused "a usage.rs name nobody reviewed, from the glue" \
    "pub fn s() { let _ = crate::${U}::current_snapshot(); }" "usage::current_snapshot"
refused "a reviewed and an unreviewed usage.rs name together, from the glue" \
    "use crate::${U}::{UsageSnapshot, poll_once};" "usage::poll_once"

# What the glue may use of usage.rs (USAGE_REVIEWED) still passes, and comment lines are not code.
printf '%s\n' "// crate::${U}::poll_once is upstream's poller; the glue never starts it." \
    "pub fn empty() -> Option<crate::${U}::UsageSnapshot> { None }" > "$GLUE"
run
rm -f "$GLUE"
if [[ $CODE -eq 0 ]]; then ok "the reviewed snapshot type from the glue passes"; else bad "the reviewed snapshot type from the glue passes (exit $CODE): $OUT"; fi

echo "verify-token-free tests: $PASSED passed, $FAILED failed"
[[ $FAILED -eq 0 ]]
