#!/usr/bin/env bash
# Check that no Claude usage path can read a token (design D5, §9).
#
#   Scripts/verify-token-free.sh
#   UPSTREAM_REF=642d329 Scripts/verify-token-free.sh
#
# The fork reads Claude usage only through ClaudeControl (Claude Code's own
# get_usage, .claude.json, the status line, Claude Desktop's cache). None of
# these may appear in the fork's code:
#   ClaudeOAuthProvider(  ClaudeTokenRefresher(  ClaudeProfile.discover(
#   ClaudeUsageCLI.locate(  SecItemCopyMatching  kSecReturnData
#   .credentials.json  sessions/*.key
# nor upstream's own wrappers around Claude's keychain item, which a token
# read through would use without either Security call on the fork's line:
#   ClaudeCredentials.  Claude Code-credentials  KeychainPrompt.
#   hasKeychainCredential  find-generic-password  KeychainItem.read(
#
# Where it looks:
#  - Sources/ClaudeBridge/** and Packages/ClaudeControl/** (not .build): nowhere.
#  - Sources/** and Tests/**: nowhere in a line the fork added or changed
#    (compared with upstream, committed or not, untracked files included),
#    except the lines in ALLOWED_LINES; `ClaudeProfile.discover(` nowhere at
#    all in Sources/App.
#    Upstream's own AppDelegate still builds `ClaudeOAuthProvider(` and
#    `ClaudeTokenRefresher(` from `claudeProfiles`; with U1 that list is
#    empty, so neither is ever constructed. This script requires U1 to be in
#    place and those upstream sites to stay where they are (AppDelegate only,
#    at most one of each); a new one anywhere is a failure.
#
# Exit 0 when clean, 1 with the offending lines otherwise. Reads only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
REF="${UPSTREAM_REF:-upstream/main}"
PATTERNS=(
    'ClaudeOAuthProvider('
    'ClaudeTokenRefresher('
    'ClaudeProfile.discover('
    'ClaudeUsageCLI.locate('
    'SecItemCopyMatching'
    'kSecReturnData'
    '.credentials.json'
    'sessions/*.key'
    'ClaudeCredentials.'
    'Claude Code-credentials'
    'KeychainPrompt.'
    'hasKeychainCredential'
    'find-generic-password'
    'KeychainItem.read('
)
# Fork lines that name a pattern legitimately, verbatim (leading space trimmed):
# the XCTest that pins the keychain remap to leave Claude's item alone.
ALLOWED_LINES=(
    'XCTAssertEqual(Fork.keychainService("Claude Code-credentials"), "Claude Code-credentials")'
)
problems=0
fail() { echo "FAIL: $*"; problems=$((problems + 1)); }

grep_args=()
for pattern in "${PATTERNS[@]}"; do grep_args+=(-e "$pattern"); done

# --- The fork's own code: nothing at all ---------------------------------------
for dir in Sources/ClaudeBridge Packages/ClaudeControl; do
    [[ -d "$dir" ]] || continue
    if hits=$(grep -rnF --exclude-dir=.build --exclude-dir=.swiftpm "${grep_args[@]}" "$dir"); then
        while IFS= read -r hit; do fail "$hit"; done <<< "$hits"
    fi
done

# --- Sources/App: U1 holds, nothing new ----------------------------------------
U1='private let claudeProfiles: [ClaudeProfile] = []'
grep -qF -- "$U1" Sources/App/AppDelegate.swift \
    || fail "U1 missing: AppDelegate must declare '$U1' (it keeps upstream's token paths dormant)"

if hits=$(grep -rnF -e 'ClaudeProfile.discover(' Sources/App); then
    while IFS= read -r hit; do fail "$hit"; done <<< "$hits"
fi

# Upstream's dormant sites: allowed only in AppDelegate, at most once each.
for pattern in 'ClaudeOAuthProvider(' 'ClaudeTokenRefresher('; do
    elsewhere=$(grep -rlF -e "$pattern" Sources/App | grep -v '^Sources/App/AppDelegate.swift$' || true)
    [[ -z "$elsewhere" ]] || fail "$pattern in $(echo $elsewhere)"
    n=$(grep -cF -e "$pattern" Sources/App/AppDelegate.swift || true)
    [[ "$n" -le 1 ]] || fail "$pattern appears $n times in AppDelegate.swift (upstream has 1)"
done

# Lines the fork added anywhere in Sources/ and Tests/ (committed or not,
# untracked files too), compared with the upstream commit the fork is based on
# (the merge-base: a fetched but unmerged upstream would otherwise make
# upstream's own removals look like fork lines). An edit inside an upstream
# file that check-seams allows is still checked here, line by line.
is_allowed_line() {
    local line="$1" allowed
    line="${line#"${line%%[![:space:]]*}"}"
    for allowed in "${ALLOWED_LINES[@]}"; do [[ "$line" == "$allowed" ]] && return 0; done
    return 1
}
if git rev-parse --verify --quiet "$REF^{commit}" >/dev/null && BASE=$(git merge-base HEAD "$REF"); then
    added=$( { git diff "$BASE" -- Sources Tests
               while IFS= read -r f; do [[ -n "$f" ]] && sed 's/^/+/' "$f"; done \
                   < <(git ls-files --others --exclude-standard -- Sources Tests)
             } | grep -E '^\+' | grep -vE '^\+\+\+ ' || true)
    if hits=$(grep -F "${grep_args[@]}" <<< "$added"); then
        while IFS= read -r hit; do
            is_allowed_line "${hit#+}" || fail "fork line in Sources/ or Tests/: ${hit#+}"
        done <<< "$hits"
    fi
else
    fail "no '$REF' to compare Sources/ and Tests/ against (git fetch upstream, or set UPSTREAM_REF)"
fi

if [[ $problems -gt 0 ]]; then
    echo "verify-token-free: $problems problem(s)"
    exit 1
fi
echo "verify-token-free: OK (bridge, package and the fork's lines in Sources/ and Tests/ are token-free; U1 keeps upstream's Claude token paths dormant)"
