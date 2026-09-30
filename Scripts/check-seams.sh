#!/usr/bin/env bash
# Check that the fork's edits to upstream Codenotch are all there, and that
# nothing else of upstream's has been touched (design §3, DESIGN-WIN §6.7).
#
#   Scripts/check-seams.sh                 # against upstream/main
#   UPSTREAM_REF=642d329 Scripts/check-seams.sh
#
# Three checks, all against Scripts/fork-seams.txt:
#  1. every SEAM line is present, verbatim, in its file (on at least as many
#     lines as it names), and no FORBID text is on a line that isn't a comment
#     (what counts as a comment depends on the file's type, see comment_re);
#  2. every file under Sources/, Tests/ and windows/, and upstream's own
#     workflows (ci, package, windows, windows-package: never edited), that
#     differs from upstream (committed, staged, unstaged or untracked) is on the
#     ALLOW list. "Upstream" is the merge-base of HEAD and the ref, so commits
#     fetched from upstream but not merged yet do not count as fork edits;
#  3. Packages/ClaudeControl/Sources/ClaudeControl/Engine imports no SwiftUI.
# Plus: the embedded hook scripts match Packages/ClaudeControl/Scripts/*.py,
# and no string literal under Sources/ names "Codenotch" outside L10n.t (R1
# puts this app's name into everything L10n.t returns; a literal that skips
# it, such as a SwiftUI Text("…") key a merge brings in, would show upstream's
# name on screen). Known literals that aren't on-screen copy are listed below.
#
# Windows (upstream's Tauri port in windows/, DESIGN-WIN §2.3, §6.7):
#  a. the engine and the protocol crate stay pure: their manifests name no
#     Tauri, windows*, winapi, libc or C-building crate, and their sources have
#     no `use tauri`, no `#[cfg(windows)]` and no `std::os::windows`;
#  b. tauri.conf.json's "version" is VERSION;
#  c. no fork-owned .rs/.js/.html file has a string naming "Codenotch" outside
#     the rebrand itself, tests and NAME_OK, and no fork line added to an
#     upstream file under windows/ has one;
#  d. (the page seams are SEAM lines);
#  e. agentnotch-hook never exits 2 by accident: its manifest names no argument
#     parser, and its sources (before any #[cfg(test)]) hold no printing macro,
#     no .unwrap() and no .expect(;
#  f. every CreateFileW in agentnotch-hook/src and agentnotch-win/src opens
#     with SECURITY_SQOS_PRESENT (a client of the hook pipe must let the server
#     identify it and never act as it), unless the line or the two before it say
#     `not a pipe` (a plain file);
#  g. upstream's own hook plumbing stays off whatever a merge brings: no file
#     under windows/codenotch/src but its own names server:: (the TCP hook
#     server on upstream's port), and hooks_install:: (it writes settings.json
#     with no consent) is used only by main's "install-hooks" and
#     "uninstall-hooks" arms, which WCLI claims first. (watcher:: is
#     verify-token-free.sh's.)
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

# The start of a comment line in `grep -n` output ("<n>:<text>"), by file type.
# A lone `*` counts only as a doc-comment continuation (`* …`, `*/`), never a
# Rust dereference (`*x = …`). JSON has no comments: every line is content.
comment_re() {
    case "$1" in
        *.rs|*.js|*.mjs|*.cjs) echo '^[0-9]+:[[:space:]]*(//|/\*|\*([[:space:]/]|$))' ;;
        *.html)                echo '^[0-9]+:[[:space:]]*(//|/\*|\*([[:space:]/]|$)|<!--)' ;;
        *.toml|*.yml|*.yaml|*.sh|*.ps1) echo '^[0-9]+:[[:space:]]*#' ;;
        *.nsh|*.nsi)           echo '^[0-9]+:[[:space:]]*(;|#)' ;;
        *.json)                echo '' ;;
        *)                     echo '^[0-9]+:[[:space:]]*(#|//|@#)' ;;  # Swift, Makefile, plists
    esac
}

# `grep -n` lines of $2 that aren't comments of file $1's type.
without_comments() {
    local re
    re=$(comment_re "$1")
    if [[ -n "$re" ]]; then grep -vE "$re" <<< "$2" || true; else printf '%s\n' "$2"; fi
}

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
    hits=$(grep -nF -- "$text" "$file" || true)
    [[ -n "$hits" ]] || continue
    hits=$(without_comments "$file" "$hits")
    if [[ -n "$hits" ]]; then
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

# Upstream's workflows only run in vinzdg/codenotch; the fork's own live beside
# them. An edit would conflict on every merge, so none is allowed.
UPSTREAM_WORKFLOWS=(.github/workflows/ci.yml .github/workflows/package.yml
                    .github/workflows/windows.yml .github/workflows/windows-package.yml)
changed=$( { git diff --name-only "$BASE" -- Sources Tests windows "${UPSTREAM_WORKFLOWS[@]}"
             git ls-files --others --exclude-standard -- Sources Tests windows "${UPSTREAM_WORKFLOWS[@]}"; } | sort -u )
count=0
while IFS= read -r file; do
    [[ -n "$file" ]] || continue
    count=$((count + 1))
    if ! is_allowed "$file"; then
        case "$file" in
            windows/*) fail "$file differs from upstream ($REF, merge-base ${BASE:0:7}) but is not on the ALLOW list (Windows fork code belongs in windows/agentnotch-* or windows/codenotch/src/agentnotch; an upstream file changes only at a listed seam)" ;;
            .github/*) fail "$file is upstream's workflow and differs from upstream ($REF, merge-base ${BASE:0:7}): the fork's workflows are fork.yml, release.yml and agentnotch-windows.yml" ;;
            *) fail "$file differs from upstream ($REF, merge-base ${BASE:0:7}) but is not on the ALLOW list (fork code belongs in Sources/ClaudeBridge or Packages/ClaudeControl)" ;;
        esac
    fi
done <<< "$changed"
echo "files differing from upstream ($REF, merge-base ${BASE:0:7}) under Sources/, Tests/ and windows/: $count"

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
    # Windows: the glue's stand-in rebrand until the engine's core::rebrand (WP7).
    'windows/codenotch/src/agentnotch/engine.rs:const UPSTREAM_NAME: &str = "Codenotch";'
    # Windows: the panel's note about the official app's own hooks (they stay; Settings removes them).
    "windows/codenotch/ui/agentnotch/panel.js:var OFFICIAL_APP = 'Codenotch';"
)
name_ok() {
    local file="$1" text="$2" ok okFile okText
    for ok in "${NAME_OK[@]}"; do
        okFile="${ok%%:*}"; okText="${ok#*:}"
        [[ "$file" == "$okFile" && "$text" == *"$okText"* ]] && return 0
    done
    return 1
}
names=0
while IFS= read -r hit; do
    [[ -z "$hit" ]] && continue
    file="${hit%%:*}"; rest="${hit#*:}"; text="${rest#*:}"
    [[ "$text" =~ ^[[:space:]]*// ]] && continue
    [[ "$text" == *'L10n.t('* ]] && continue
    name_ok "$file" "$text" && continue
    fail "upstream's name in a literal that skips L10n.t, so it isn't rebranded (see Fork.rebranded): $hit"
    names=$((names + 1))
done < <(grep -rnE '"[^"]*Codenotch[^"]*"' Sources --include='*.swift' || true)
echo "upstream's name outside L10n.t: $names"

# --- Windows -------------------------------------------------------------------
# Dependency names a Cargo.toml declares (every [*dependencies] table, the
# `[dependencies.<name>]` form and `package = "<name>"` renames included).
deps_of() {
    awk '
        /^\[/ {
            dep = ($0 ~ /^\[(target\..*\.)?(dev-|build-)?dependencies\]/)
            if ($0 ~ /^\[(target\..*\.)?(dev-|build-)?dependencies\.[A-Za-z0-9_-]+\]/) {
                name = $0; sub(/^.*dependencies\./, "", name); sub(/\].*$/, "", name); print name
            }
            next
        }
        dep && /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=/ {
            name = $0; sub(/^[[:space:]]*/, "", name); sub(/[[:space:]]*=.*$/, "", name); print name
            if (match($0, /package[[:space:]]*=[[:space:]]*"[^"]+"/)) {
                pkg = substr($0, RSTART, RLENGTH); sub(/^[^"]*"/, "", pkg); sub(/"$/, "", pkg); print pkg
            }
        }
    ' "$1"
}

# a. The engine and the protocol stay platform-independent and C-free.
for crate in windows/agentnotch-engine windows/agentnotch-proto; do
    [[ -f "$crate/Cargo.toml" ]] || continue
    while IFS= read -r dep; do
        case "$dep" in
            tauri|tauri-*|wry|tao|webview2-com|webview2-com-*|windows|windows-sys|windows-core|winapi|libc|cc|ring|openssl|openssl-*|libsqlite3-sys|zstd-sys)
                fail "$crate/Cargo.toml depends on $dep: the engine and the protocol use no UI toolkit, no Windows crate and nothing that builds C (DESIGN-WIN §2.3)" ;;
        esac
    done < <(deps_of "$crate/Cargo.toml")
    hits=$(grep -rnE --include='*.rs' --exclude-dir=target \
        '(^|[^A-Za-z0-9_])use[[:space:]]+tauri|(^|[^A-Za-z0-9_:])tauri::|#!?\[cfg(_attr)?\(.*windows|std::os::windows' \
        "$crate" | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)
    if [[ -n "$hits" ]]; then
        while IFS= read -r hit; do fail "platform code in $crate (it belongs in agentnotch-win): $hit"; done <<< "$hits"
    fi
done

# b. One version: VERSION, which the Tauri config must repeat.
CONF=windows/codenotch/tauri.conf.json
if [[ -f "$CONF" ]]; then
    want=$(tr -d '[:space:]' < VERSION)
    have=$(sed -nE 's/^  "version"[[:space:]]*:[[:space:]]*"([^"]*)".*/\1/p' "$CONF" | head -1)
    [[ "$have" == "$want" ]] \
        || fail "$CONF has \"version\": \"$have\" but VERSION is $want (they change together)"
fi

# c. Upstream's name in the fork's Windows code. Upstream copy is rebranded at
#    runtime (core::rebrand, rebrand.js); the fork's own copy says Agent Notch.
#    A Rust file is read up to its first #[cfg(test)] (the tests module).
fork_files=$(find windows/agentnotch-proto windows/agentnotch-engine windows/agentnotch-win \
                  windows/agentnotch-hook windows/agentnotch-release \
                  windows/codenotch/src/agentnotch windows/codenotch/ui/agentnotch \
                  \( -name target -o -name gen -o -name node_modules -o -name tests \) -prune -o \
                  -type f \( -name '*.rs' -o -name '*.js' -o -name '*.mjs' -o -name '*.cjs' -o -name '*.html' \) \
                  -print 2>/dev/null | grep -vE '(^|/)core/rebrand\.rs$|/ui/agentnotch/rebrand\.js$' | sort || true)
# Bash regexes kept in variables: quotes and `<` inside [[ =~ ]] parse differently
# across bash versions.
rs_literal='"[^"]*Codenotch'
js_literal="[\"'\`][^\"'\`]*Codenotch"
quoted_name='"[^"]*Codenotch[^"]*"'
comment_start='^[[:space:]]*(//|#|<!--)'
winnames=0
while IFS= read -r file; do
    [[ -n "$file" ]] || continue
    hits=$(awk '/#\[cfg\(test\)\]/ && FILENAME ~ /\.rs$/ { exit } /Codenotch/ { print FNR ":" $0 }' "$file")
    [[ -n "$hits" ]] || continue
    hits=$(without_comments "$file" "$hits")
    while IFS= read -r hit; do
        [[ -n "$hit" ]] || continue
        text="${hit#*:}"
        case "$file" in
            *.rs) [[ "$text" =~ $rs_literal ]] || continue ;;
            *.js|*.mjs|*.cjs) [[ "$text" =~ $js_literal ]] || continue ;;
        esac
        name_ok "$file" "$text" && continue
        fail "upstream's name in the fork's Windows code (say Agent Notch, or rebrand upstream copy): $file:$hit"
        winnames=$((winnames + 1))
    done <<< "$hits"
done <<< "$fork_files"
added=$(git diff -U0 "$BASE" -- windows ':!windows/agentnotch-*' ':!windows/codenotch/src/agentnotch' \
            ':!windows/codenotch/ui/agentnotch' ':!windows/Cargo.lock' | grep -E '^\+[^+]' || true)
while IFS= read -r line; do
    [[ -n "$line" ]] || continue
    text="${line#+}"
    [[ "$text" =~ $quoted_name ]] || continue
    [[ "$text" =~ $comment_start ]] && continue
    fail "a fork line in an upstream file under windows/ names Codenotch: $text"
    winnames=$((winnames + 1))
done <<< "$added"
echo "upstream's name in the fork's Windows code: $winnames"

# e. The hook exe fails open: no argument parser, no panicking output or unwrap.
HOOK=windows/agentnotch-hook
if [[ -f "$HOOK/Cargo.toml" ]]; then
    while IFS= read -r dep; do
        case "$dep" in
            clap|clap_*|clap-*|argh|argh_*|pico-args|lexopt|gumdrop|structopt|docopt|getopts|bpaf)
                fail "$HOOK/Cargo.toml depends on $dep: an argument parser exits 2 on a usage error, and a hook exiting 2 blocks Claude Code (DESIGN-WIN §1.8)" ;;
        esac
    done < <(deps_of "$HOOK/Cargo.toml")
    while IFS= read -r file; do
        [[ -n "$file" ]] || continue
        hits=$(awk '/#\[cfg\(test\)\]/ { exit } { print FNR ":" $0 }' "$file" \
            | grep -E '(^|[^A-Za-z0-9_])e?print(ln)?!|\.unwrap\(\)|\.expect\(' || true)
        [[ -n "$hits" ]] || continue
        hits=$(without_comments "$file" "$hits")
        while IFS= read -r hit; do
            [[ -n "$hit" ]] && fail "the hook exe may panic or exit non-zero here (write_all and error handling instead): $file:$hit"
        done <<< "$hits"
    done < <(find "$HOOK/src" -name '*.rs' 2>/dev/null | sort)
fi

# f. Every open of the hook pipe lets the server identify the client and
#    never act as it.
sqos=0
while IFS= read -r file; do
    [[ -n "$file" ]] || continue
    while IFS=: read -r n _; do
        [[ -n "$n" ]] || continue
        sqos=$((sqos + 1))
        call=$(sed -n "${n},$((n + 12))p" "$file")
        before=$(sed -n "$(( n > 2 ? n - 2 : 1 )),${n}p" "$file")
        if ! grep -qF 'SECURITY_SQOS_PRESENT' <<< "$call" && ! grep -qF 'not a pipe' <<< "$before"; then
            fail "$file:$n opens with CreateFileW without SECURITY_SQOS_PRESENT (a pipe client must pass SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION; mark a plain file 'not a pipe')"
        fi
    done < <(without_comments "$file" "$(grep -n 'CreateFileW(' "$file" || true)")
done < <(find windows/agentnotch-hook/src windows/agentnotch-win/src -name '*.rs' 2>/dev/null | sort)
echo "CreateFileW calls checked for SECURITY_SQOS_PRESENT: $sqos"

# g. Upstream's hook server and hook installer stay unreachable. The seams
#    replace their call sites in main.rs; this catches a merge that calls them
#    from anywhere else (comment lines aside).
UPSRC=windows/codenotch/src
if [[ -d "$UPSRC" ]]; then
    code_refs() {  # $1 = module name: `grep -n` hits naming it in other files
        grep -rnE --exclude-dir=agentnotch "(^|[^A-Za-z0-9_])$1::" "$UPSRC" \
            | grep -vE "^$UPSRC/$1\.rs:" | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true
    }
    hits=$(code_refs server)
    if [[ -n "$hits" ]]; then
        while IFS= read -r hit; do fail "WH: upstream's TCP hook server is reached from $hit"; done <<< "$hits"
    fi
    install_arm='^[[:space:]]*"(un)?install-hooks"[[:space:]]*=>'
    while IFS= read -r hit; do
        [[ -n "$hit" ]] || continue
        file="${hit%%:*}"; rest="${hit#*:}"; n="${rest%%:*}"
        arm=""
        [[ "$file" == "$UPSRC/main.rs" ]] && arm=$(sed -n "$((n > 1 ? n - 1 : 1))p" "$file")
        [[ "$arm" =~ $install_arm ]] \
            || fail "WH: upstream's hook installer (it writes settings.json without consent) is reached outside main's install-hooks/uninstall-hooks arms: $hit"
    done <<< "$(code_refs hooks_install)"
fi

if [[ $problems -gt 0 ]]; then
    echo "check-seams: $problems problem(s)"
    exit 1
fi
echo "check-seams: OK"
