#!/usr/bin/env bash
# Tests Scripts/bump-version.sh in a temporary copy of the two files it touches, never in this
# checkout: set, --sync, a bad version, a missing line, and that nothing else in
# tauri.conf.json changes. Run by fork.yml's Tool tests, or by hand: bash windows/tools/tests/bump-version.test.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
SCRIPT="$REPO/Scripts/bump-version.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0
ok() { PASSED=$((PASSED + 1)); echo "ok   $1"; }
bad() { FAILED=$((FAILED + 1)); echo "FAIL $1" >&2; }

# A fresh tree: the real tauri.conf.json, its version set to 1.0.0 whatever the checkout's is
# (so the test holds after every release), and a VERSION that disagrees with it.
fresh() {
    local tree="$WORK/$1"
    rm -rf "$tree"
    mkdir -p "$tree/windows/codenotch"
    sed -E 's/^([[:space:]]*"version"[[:space:]]*:[[:space:]]*")[^"]*(".*)$/\11.0.0\2/' \
        "$REPO/windows/codenotch/tauri.conf.json" > "$tree/windows/codenotch/tauri.conf.json"
    chmod "$(mode "$REPO/windows/codenotch/tauri.conf.json")" "$tree/windows/codenotch/tauri.conf.json"
    echo "1.0.1" > "$tree/VERSION"
    echo "$tree"
}
mode() { stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"; }
conf_version() { sed -n -E 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"([^"]*)".*$/\1/p' "$1/windows/codenotch/tauri.conf.json"; }

# set: both files carry the new version and only that one line differs.
tree="$(fresh set)"
cp "$tree/windows/codenotch/tauri.conf.json" "$WORK/set.before"
out="$("$SCRIPT" 1.2.3 --root "$tree")"
if [[ "$(cat "$tree/VERSION")" == "1.2.3" && "$(conf_version "$tree")" == "1.2.3" ]]; then ok "set writes both"; else bad "set writes both"; fi
changed="$(diff "$WORK/set.before" "$tree/windows/codenotch/tauri.conf.json" | grep -c '^>' || true)"
if [[ "$changed" == "1" ]]; then ok "set changes one line of tauri.conf.json"; else bad "set changes one line (changed: $changed)"; fi
if grep -q "1.0.1 -> 1.2.3" <<<"$out" && grep -q "1.0.0 -> 1.2.3" <<<"$out"; then ok "set prints what changed"; else bad "set prints what changed: $out"; fi
if [[ "$(mode "$tree/windows/codenotch/tauri.conf.json")" == "$(mode "$REPO/windows/codenotch/tauri.conf.json")" ]]; then ok "set keeps the file mode"; else bad "set keeps the file mode"; fi

# sync: tauri.conf.json follows VERSION, VERSION stays as it is.
tree="$(fresh sync)"
"$SCRIPT" --sync --root "$tree" >/dev/null
if [[ "$(cat "$tree/VERSION")" == "1.0.1" && "$(conf_version "$tree")" == "1.0.1" ]]; then ok "sync copies VERSION into tauri.conf.json"; else bad "sync copies VERSION"; fi

# a bad version: exit 2 and nothing written.
for version in 1.2 1.2.3.4 v1.2.3 1.2.x 01.2.3 1.2.3-beta "" " "; do
    tree="$(fresh bad)"
    set +e
    "$SCRIPT" "$version" --root "$tree" >/dev/null 2>&1
    code=$?
    set -e
    if [[ $code -eq 2 && "$(cat "$tree/VERSION")" == "1.0.1" && "$(conf_version "$tree")" == "1.0.0" ]]; then ok "refuses '$version'"; else bad "refuses '$version' (exit $code)"; fi
done

# sync with a malformed VERSION: exit 2 and nothing written.
tree="$(fresh badsync)"
echo "garbage" > "$tree/VERSION"
set +e
"$SCRIPT" --sync --root "$tree" >/dev/null 2>&1
code=$?
set -e
if [[ $code -eq 2 && "$(conf_version "$tree")" == "1.0.0" ]]; then ok "sync refuses a bad VERSION"; else bad "sync refuses a bad VERSION (exit $code)"; fi

# a missing line, a doubled line, a missing file: exit 2 and nothing written.
tree="$(fresh noline)"
grep -v '^[[:space:]]*"version"' "$tree/windows/codenotch/tauri.conf.json" > "$tree/conf.tmp"
mv "$tree/conf.tmp" "$tree/windows/codenotch/tauri.conf.json"
set +e
"$SCRIPT" 1.2.3 --root "$tree" >/dev/null 2>&1
code=$?
set -e
if [[ $code -eq 2 && "$(cat "$tree/VERSION")" == "1.0.1" ]]; then ok "refuses a missing version line"; else bad "refuses a missing version line (exit $code)"; fi

tree="$(fresh twolines)"
sed -E 's/^([[:space:]]*)"version"(.*)$/&\
\1"version"\2/' "$tree/windows/codenotch/tauri.conf.json" > "$tree/conf.tmp"
mv "$tree/conf.tmp" "$tree/windows/codenotch/tauri.conf.json"
set +e
"$SCRIPT" 1.2.3 --root "$tree" >/dev/null 2>&1
code=$?
set -e
if [[ $code -eq 2 && "$(cat "$tree/VERSION")" == "1.0.1" ]]; then ok "refuses two version lines"; else bad "refuses two version lines (exit $code)"; fi

tree="$(fresh nofile)"
rm "$tree/windows/codenotch/tauri.conf.json"
set +e
"$SCRIPT" 1.2.3 --root "$tree" >/dev/null 2>&1
code=$?
set -e
if [[ $code -eq 2 && "$(cat "$tree/VERSION")" == "1.0.1" ]]; then ok "refuses a missing tauri.conf.json"; else bad "refuses a missing tauri.conf.json (exit $code)"; fi

echo "$PASSED passed, $FAILED failed"
[[ $FAILED -eq 0 ]]
