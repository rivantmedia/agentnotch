#!/usr/bin/env bash
# Set the app's version in the two places that must agree: VERSION (the Mac app's only version
# source, and the release workflow's trigger) and the "version" line of
# windows/codenotch/tauri.conf.json (the Windows installer's version). The release workflow
# refuses a Windows build whose two disagree, so a release bumps both in one commit.
#
#   Scripts/bump-version.sh 1.2.0            set both to 1.2.0
#   Scripts/bump-version.sh --sync           copy VERSION into tauri.conf.json
#   Scripts/bump-version.sh 1.2.0 --root DIR work in the tree at DIR (default: this checkout)
#
# --sync is for after merging main into a branch that carries the Windows files: main's
# VERSION moves on without knowing about tauri.conf.json.
#
# Only the one "version" line is rewritten; nothing else in tauri.conf.json changes. Exit 2 on a
# bad version, a missing file or a missing (or ambiguous) line, with nothing written.
set -euo pipefail

usage() {
    echo "usage: Scripts/bump-version.sh <major.minor.patch> [--root DIR]" >&2
    echo "       Scripts/bump-version.sh --sync [--root DIR]" >&2
    exit 2
}
fail() { echo "bump-version: $*" >&2; exit 2; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION_ARG=""
SYNC=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --sync) SYNC=1 ;;
        --root)
            [[ $# -ge 2 ]] || usage
            [[ -d "$2" ]] || fail "--root $2 is not a folder"
            ROOT="$(cd "$2" && pwd)"
            shift ;;
        -h|--help) usage ;;
        -*) usage ;;
        *)
            [[ -z "$VERSION_ARG" ]] || usage
            VERSION_ARG="$1" ;;
    esac
    shift
done

VERSION_FILE="$ROOT/VERSION"
TAURI_CONF="$ROOT/windows/codenotch/tauri.conf.json"
# No leading zeros and no suffix: Sparkle compares CFBundleVersion, and the installer wants digits.
SEMVER='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
LINE_PATTERN='^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"[^"]*"[[:space:]]*,?[[:space:]]*$'

if [[ $SYNC -eq 1 ]]; then
    [[ -z "$VERSION_ARG" ]] || usage
    [[ -f "$VERSION_FILE" ]] || fail "$VERSION_FILE is missing"
    NEW="$(tr -d '[:space:]' < "$VERSION_FILE")"
else
    [[ -n "$VERSION_ARG" ]] || usage
    NEW="$VERSION_ARG"
fi
[[ "$NEW" =~ $SEMVER ]] || fail "'$NEW' is not major.minor.patch (digits only, no leading zeros)"

[[ -f "$TAURI_CONF" ]] || fail "$TAURI_CONF is missing"
COUNT="$(grep -Ec "$LINE_PATTERN" "$TAURI_CONF" || true)"
[[ "$COUNT" -eq 1 ]] || fail "expected exactly one \"version\" line in $TAURI_CONF, found $COUNT"

OLD_VERSION="(none)"
if [[ -f "$VERSION_FILE" ]]; then
    OLD_VERSION="$(tr -d '[:space:]' < "$VERSION_FILE")"
fi
OLD_TAURI="$(grep -E "$LINE_PATTERN" "$TAURI_CONF" | sed -E 's/^[^:]*:[[:space:]]*"([^"]*)".*$/\1/')"

# Each file is written through a copy beside it (the copy keeps the mode), then moved into
# place, so a failure leaves both as they were.
TMP_TAURI="$(mktemp "$TAURI_CONF.XXXXXX")"
TMP_VERSION="$(mktemp "$VERSION_FILE.XXXXXX")"
trap 'rm -f "$TMP_TAURI" "$TMP_VERSION"' EXIT
cp -p "$TAURI_CONF" "$TMP_TAURI"
if [[ -f "$VERSION_FILE" ]]; then
    cp -p "$VERSION_FILE" "$TMP_VERSION"
else
    chmod 644 "$TMP_VERSION"
fi
# The line keeps its indentation and trailing comma; only the value changes.
sed -E "s/^([[:space:]]*\"version\"[[:space:]]*:[[:space:]]*\")[^\"]*(\".*)$/\1$NEW\2/" "$TAURI_CONF" > "$TMP_TAURI"
printf '%s\n' "$NEW" > "$TMP_VERSION"
mv "$TMP_TAURI" "$TAURI_CONF"
mv "$TMP_VERSION" "$VERSION_FILE"

echo "VERSION: $OLD_VERSION -> $NEW"
echo "windows/codenotch/tauri.conf.json: $OLD_TAURI -> $NEW"
