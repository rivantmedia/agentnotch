#!/usr/bin/env bash
# Check that the fork's edits to upstream Codenotch are all there, and that
# nothing else of upstream's has been touched (design §3).
#
#   Scripts/check-seams.sh                 # against upstream/main
#   UPSTREAM_REF=642d329 Scripts/check-seams.sh
#
# Three checks, all against Scripts/fork-seams.txt:
#  1. every SEAM line is present, verbatim, in its file (on at least as many
#     lines as it names), and no FORBID text is on a non-comment line;
#  2. every file under Sources/ and Tests/ that differs from upstream (committed,
#     staged, unstaged or untracked) is on the ALLOW list. "Upstream" is the
#     merge-base of HEAD and the ref, so commits fetched from upstream but not
#     merged yet do not count as fork edits;
#  3. Packages/ClaudeControl/Sources/ClaudeControl/Engine imports no SwiftUI.
# Plus: the embedded hook scripts match Packages/ClaudeControl/Scripts/*.py,
# and no string literal under Sources/ names "Codenotch" outside L10n.t (R1
# puts this app's name into everything L10n.t returns; a literal that skips
# it, such as a SwiftUI Text("…") key a merge brings in, would show upstream's
# name on screen). Known literals that aren't on-screen copy are listed below.
#
# Exit 0 when everything holds, 1 with a list of problems otherwise. Reads only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
SEAMS="Scripts/fork-seams.txt"
REF="${UPSTREAM_REF:-upstream/main}"
ENGINE="Packages/ClaudeControl/Sources/ClaudeControl/Engine"
problems=0
fail() { echo "FAIL: $*"; problems=$((problems + 1)); }

if ! git rev-parse --verify --quiet "$REF^{commit}" >/dev/null; then
    echo "FAIL: no '$REF' to compare against (git fetch upstream, or set UPSTREAM_REF)"
    exit 1
fi
# The upstream commit the fork is based on: after a `git fetch upstream`, the
# ref can be ahead of the last merge, and its newer files are not ours.
if ! BASE=$(git merge-base HEAD "$REF"); then
    echo "FAIL: HEAD and '$REF' share no history"
    exit 1
fi

# --- 1. Seams ------------------------------------------------------------------
seams=0
while IFS=$'\t' read -r kind id file text times; do
    [[ "$kind" == "SEAM" ]] || continue
    seams=$((seams + 1))
    times="${times:-1}"
    if [[ ! -f "$file" ]]; then
        fail "$id: $file is missing"
    else
        # Lines holding the text; a seam on several call paths names how many.
        found=$(grep -cF -- "$text" "$file" || true)
        if [[ "$found" -lt "$times" ]]; then
            fail "$id: found $found of $times in $file: $text"
        fi
    fi
done < "$SEAMS"
echo "seams: $seams lines checked"

forbids=0
while IFS=$'\t' read -r kind id file text; do
    [[ "$kind" == "FORBID" ]] || continue
    forbids=$((forbids + 1))
    [[ -f "$file" ]] || continue
    # Comments may name what is forbidden (to say why); code may not.
    if hits=$(grep -nF -- "$text" "$file" | grep -vE '^[0-9]+:[[:space:]]*(#|//|@#)' ); then
        while IFS= read -r hit; do fail "$id: forbidden in $file: $hit"; done <<< "$hits"
    fi
done < "$SEAMS"
echo "forbidden: $forbids rules checked"

# --- 2. Only allowed upstream files differ ------------------------------------
allowed=()
while IFS=$'\t' read -r kind path _; do
    [[ "$kind" == "ALLOW" ]] && allowed+=("$path")
done < "$SEAMS"

is_allowed() {
    local file="$1" pattern
    for pattern in "${allowed[@]}"; do
        if [[ "$pattern" == *"/**" ]]; then
            [[ "$file" == "${pattern%/**}/"* ]] && return 0
        elif [[ "$file" == "$pattern" ]]; then
            return 0
        fi
    done
    return 1
}

changed=$( { git diff --name-only "$BASE" -- Sources Tests
             git ls-files --others --exclude-standard -- Sources Tests; } | sort -u )
count=0
while IFS= read -r file; do
    [[ -n "$file" ]] || continue
    count=$((count + 1))
    is_allowed "$file" || fail "$file differs from upstream ($REF, merge-base ${BASE:0:7}) but is not on the ALLOW list (fork code belongs in Sources/ClaudeBridge or Packages/ClaudeControl)"
done <<< "$changed"
echo "files differing from upstream ($REF, merge-base ${BASE:0:7}) under Sources/ and Tests/: $count"

# --- 3. No SwiftUI in the engine ---------------------------------------------
# Any form: `import SwiftUI`, `import struct SwiftUI.Color`, `@_exported`,
# `@preconcurrency`, and access-level imports (`public import SwiftUI`).
if hits=$(grep -rnE '^[[:space:]]*(@[A-Za-z_]+[[:space:]]+)*((public|package|internal|fileprivate|private)[[:space:]]+)?import[[:space:]]+([a-z]+[[:space:]]+)?SwiftUI([.;[:space:]]|$)' "$ENGINE" 2>/dev/null); then
    while IFS= read -r hit; do fail "SwiftUI imported in the engine: $hit"; done <<< "$hits"
fi
echo "engine: no SwiftUI import check done"

# --- Embedded scripts in step --------------------------------------------------
if ! Packages/ClaudeControl/Scripts/embed-scripts.sh --check >/dev/null 2>&1; then
    fail "Engine/Scripts/EmbeddedScripts.swift is stale: run Packages/ClaudeControl/Scripts/embed-scripts.sh"
fi

# --- Upstream's name outside L10n.t -------------------------------------------
# path:text pairs that may keep it: not copy anyone reads as this app's name.
NAME_OK=(
    'Sources/App/Fork.swift:'                                   # the rebrand itself
    'Sources/Providers/GitHubCopilotProvider.swift:"Codenotch", forHTTPHeaderField: "User-Agent"'
    'Sources/PhoneLink/PhoneLinkSnapshotBuilder.swift:source: "Codenotch"'  # read by upstream's phone app
)
names=0
while IFS= read -r hit; do
    [[ -z "$hit" ]] && continue
    file="${hit%%:*}"; rest="${hit#*:}"; text="${rest#*:}"
    [[ "$text" =~ ^[[:space:]]*// ]] && continue
    [[ "$text" == *'L10n.t('* ]] && continue
    allowed=0
    for ok in "${NAME_OK[@]}"; do
        okFile="${ok%%:*}"; okText="${ok#*:}"
        [[ "$file" == "$okFile" && "$text" == *"$okText"* ]] && allowed=1 && break
    done
    [[ $allowed -eq 1 ]] && continue
    fail "upstream's name in a literal that skips L10n.t, so it isn't rebranded (see Fork.rebranded): $hit"
    names=$((names + 1))
done < <(grep -rnE '"[^"]*Codenotch[^"]*"' Sources --include='*.swift' || true)
echo "upstream's name outside L10n.t: $names"

if [[ $problems -gt 0 ]]; then
    echo "check-seams: $problems problem(s)"
    exit 1
fi
echo "check-seams: OK"
