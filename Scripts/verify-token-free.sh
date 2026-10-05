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
# Windows (upstream's Tauri port in windows/, DESIGN-WIN §1.8, §6.7). The
# Windows patterns, added to the Mac's for the Windows fork's code:
#   claudeAiOauth  api/oauth/usage  oauth-2025-04-20  CredReadW  CredEnumerateW
#   read_credentials(  probe_credentials(  run_renewal(  maybe_renew(
#   start_login(  auth login  setup-token  sessions\*.key  usage::start(
#   claude_auth::  doctor::run(  watcher::start(  usage::profile_dirs
#   usage::request_refresh  usage::find_cli
#  - The fork's own Windows code (windows/agentnotch-*, the glue and its pages,
#    the NSIS hooks, the smoke and build scripts, windows/tools; not target,
#    gen or node_modules): none of them, comments included, except
#    ALLOWED_LINES.
#  - Lines the fork added to upstream files under windows/: none of them.
#  - Upstream's Claude token path stays dormant: its Claude-only markers
#    (claudeAiOauth, api/oauth/usage, oauth-2025-04-20) only in usage.rs and
#    claude_auth.rs; usage::read_credentials( and usage::probe_credentials(
#    called only from usage.rs and doctor.rs (the doctor is unreachable: WD and
#    WCLI); claude_auth:: used outside usage.rs and claude_auth.rs only for the
#    Sign-in card's busy flag in main.rs (claude_auth::state(),
#    claude_auth::AuthState: no credential); start_login only in
#    claude_auth.rs; seams WU1a-e, WD and WCLI in place; usage::start( and
#    usage::request_refresh() on no code line of main.rs. (Upstream's other
#    providers read their own credentials with functions of the same names, in
#    grok.rs, cursor.rs and antigravity.rs; those are not Claude's.)
#    Upstream merges keep bringing new code, so three rules go by name rather
#    than by pattern, over upstream's files and the fork's glue alike (the glue
#    is where a new call to upstream's code would be written): code outside the
#    dormant files (usage.rs, claude_auth.rs, doctor.rs, watcher.rs) reaches
#    into usage.rs only for the names in
#    USAGE_REVIEWED (its types and the saved snapshot: a poller start, a
#    refresh or a new credential helper called from any file fails until
#    someone reviews it); doctor::run() (it reads the credential) is called
#    once, from main's "doctor" arm, which WCLI makes unreachable; and no file
#    but the dormant doctor names watcher:: (upstream's transcript watcher,
#    off by WH).
#  - GLM's one read of a key from Claude Code's settings.json (glm.rs
#    claude_code_key: an API key for Z.ai, not a Claude login, kept as upstream
#    ships it) stays exactly that: ANTHROPIC_AUTH_TOKEN / ANTHROPIC_API_KEY
#    appear in windows/codenotch/src only in glm.rs and in claude_auth.rs (which
#    removes them from its child's environment), and the function's text hashes
#    to GLM_CLAUDE_KEY_PIN. An upstream merge that changes it fails here until
#    someone reviews the change and records the new hash.
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
    # The Windows smoke script's assertion that the doctor's report never names a credential
    # (DESIGN-WIN §7.5 phase 3). The one line of the script that may name them.
    "foreach (\$secret in '.credentials.json', 'claudeAiOauth', 'accessToken') {"
)
# The Windows fork's code: the Mac's patterns plus these (DESIGN-WIN §6.7).
WIN_PATTERNS=(
    'claudeAiOauth'
    'api/oauth/usage'
    'oauth-2025-04-20'
    'CredReadW'
    'CredEnumerateW'
    'read_credentials('
    'probe_credentials('
    'run_renewal('
    'maybe_renew('
    'start_login('
    'auth login'
    'setup-token'
    'sessions\*.key'
    'usage::start('
    'claude_auth::'
    # Upstream's indirect routes to the credential: its doctor (probe_credentials), its
    # transcript watcher and usage.rs's profile walk, refresh and CLI lookup.
    'doctor::run('
    'watcher::start('
    'usage::profile_dirs'
    'usage::request_refresh'
    'usage::find_cli'
)
# sha256 of glm.rs's `fn claude_code_key` (its line through the closing `}` at
# the start of a line), as reviewed at upstream 642d329.
GLM_CLAUDE_KEY_PIN=2bb3f6781cc33352d259fbaf95e25c3ebffd7e73b7759a2a76ed1489edd3ee37
# What upstream's other files may use of usage.rs (its Claude token path), as
# reviewed at upstream 642d329: the snapshot types every provider shares, and
# load_persisted (upstream's saved Claude snapshot from its own config folder;
# no credential). Add a name only after reading what it reaches.
USAGE_REVIEWED=(UsageSnapshot LimitWindow load_persisted)
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
    # .ps1 files are checked out with CRLF (.gitattributes): the line end is not the line.
    line="${line%$'\r'}"
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

# --- Windows ------------------------------------------------------------------
UP=windows/codenotch/src
win_args=("${grep_args[@]}")
for pattern in "${WIN_PATTERNS[@]}"; do win_args+=(-e "$pattern"); done

# The fork's own Windows code: nothing at all, comments included.
win_paths=()
for path in windows/agentnotch-proto windows/agentnotch-engine windows/agentnotch-win \
            windows/agentnotch-hook windows/agentnotch-release windows/agentnotch-ui-tests \
            "$UP/agentnotch" windows/codenotch/ui/agentnotch windows/codenotch/nsis \
            windows/codenotch/capabilities/agentnotch.json windows/scripts/agentnotch-build.ps1 \
            windows/scripts/agentnotch-smoke.ps1 windows/scripts/smoke \
            windows/scripts/check-claude-code-facts.mjs windows/tools; do
    [[ -e "$path" ]] && win_paths+=("$path")
done
if [[ ${#win_paths[@]} -gt 0 ]]; then
    hits=$(grep -rnF --exclude-dir=target --exclude-dir=gen --exclude-dir=node_modules \
               "${win_args[@]}" "${win_paths[@]}" || true)
    while IFS= read -r hit; do
        [[ -n "$hit" ]] || continue
        rest="${hit#*:}"; text="${rest#*:}"
        is_allowed_line "$text" || fail "Windows fork code: $hit"
    done <<< "$hits"
fi

# Lines the fork added to upstream's files under windows/ (the fork's own
# files are covered above, where ALLOWED_LINES apply).
if [[ -n "${BASE:-}" ]]; then
    added=$(git diff "$BASE" -- windows ':!windows/agentnotch-*' ":!$UP/agentnotch" \
                ':!windows/codenotch/ui/agentnotch' ':!windows/codenotch/nsis' ':!windows/Cargo.lock' \
                ':!windows/codenotch/capabilities/agentnotch.json' ':!windows/scripts/agentnotch-*' \
                ':!windows/scripts/smoke' ':!windows/scripts/check-claude-code-facts.mjs' ':!windows/tools' \
            | grep -E '^\+' | grep -vE '^\+\+\+ ' || true)
    if hits=$(grep -F "${win_args[@]}" <<< "$added"); then
        while IFS= read -r hit; do
            is_allowed_line "${hit#+}" || fail "fork line in an upstream file under windows/: ${hit#+}"
        done <<< "$hits"
    fi
fi

# Upstream's Claude token path: compiled, never reachable.
if [[ -d "$UP" ]]; then
    for pattern in 'claudeAiOauth' 'api/oauth/usage' 'oauth-2025-04-20'; do
        elsewhere=$(grep -rlF --exclude-dir=agentnotch -e "$pattern" "$UP" \
                        | grep -vxE "$UP/(usage|claude_auth)\.rs" || true)
        [[ -z "$elsewhere" ]] || fail "$pattern outside usage.rs and claude_auth.rs: $(echo $elsewhere)"
    done
    for pattern in 'usage::read_credentials(' 'usage::probe_credentials('; do
        elsewhere=$(grep -rlF --exclude-dir=agentnotch -e "$pattern" "$UP" \
                        | grep -vxE "$UP/(usage|doctor)\.rs" || true)
        [[ -z "$elsewhere" ]] || fail "$pattern called outside usage.rs and doctor.rs: $(echo $elsewhere)"
    done
    while IFS= read -r hit; do
        [[ -n "$hit" ]] || continue
        file="${hit%%:*}"; rest="${hit#*:}"; text="${rest#*:}"
        stripped="${text//claude_auth::state()/}"
        stripped="${stripped//claude_auth::AuthState/}"
        if [[ "$file" != "$UP/main.rs" || "$stripped" == *'claude_auth::'* ]]; then
            fail "claude_auth used outside upstream's dormant token path: $hit"
        fi
    done < <(grep -rnF --exclude-dir=agentnotch -e 'claude_auth::' "$UP" \
                 | grep -vE "^$UP/(usage|claude_auth)\.rs:" || true)
    elsewhere=$(grep -rlF --exclude-dir=agentnotch -e 'start_login' "$UP" | grep -vxF "$UP/claude_auth.rs" || true)
    [[ -z "$elsewhere" ]] || fail "start_login outside claude_auth.rs: $(echo $elsewhere)"
    for seam in '#[allow(dead_code)] mod usage; // Fork: WU1' \
                '#[allow(dead_code)] mod claude_auth; // Fork: WU1' \
                'fn claude_sign_in() -> Result<(), String> { Err(agentnotch::SIGN_IN_REFUSED.into()) } // Fork: WU1' \
                'p if p == "claude" || p.starts_with("claude-") => return agentnotch::refresh_claude(app, p), // Fork: WU1' \
                '// Fork: WU1 usage::start never runs' \
                '#[allow(dead_code)] mod doctor; // Fork: WD' \
                'if let Some(code) = agentnotch::cli::run(&args) { std::process::exit(code); } // Fork: WCLI'; do
        grep -qF -- "$seam" "$UP/main.rs" || fail "seam missing from $UP/main.rs (it keeps upstream's Claude token path dormant): $seam"
    done
    code=$(grep -nF -e 'usage::start(' -e 'usage::request_refresh()' "$UP/main.rs" \
               | grep -vE '^[0-9]+:[[:space:]]*//' || true)
    [[ -z "$code" ]] || fail "upstream's Claude poller is started or refreshed in $UP/main.rs: $code"

    # Everything else upstream's code or the fork's glue uses of usage.rs, by
    # name (comment lines aside): `usage::X`, `usage::{X, Y}` and `usage::*` in
    # any file but the dormant ones, each kept unreachable by its own rule
    # here: claude_auth.rs (start_login, claude_auth::), doctor.rs (below) and
    # watcher.rs (upstream's transcript watcher, which walks usage.rs's
    # credential-checked profile list: never started, and named by no other
    # file).
    usage_ref='(^|[^A-Za-z0-9_])usage::(\{[^}]*\}|\*|[A-Za-z_][A-Za-z0-9_]*)'
    while IFS= read -r hit; do
        [[ -n "$hit" ]] || continue
        file="${hit%%:*}"; rest="${hit#*:}"; text="${rest#*:}"
        while IFS= read -r ref; do
            [[ -n "$ref" ]] || continue
            names="${ref#*usage::}"; names="${names#\{}"; names="${names%\}}"
            IFS=', ' read -ra list <<< "$names"
            for name in "${list[@]}"; do
                [[ -n "$name" ]] || continue
                reviewed=0
                for ok in "${USAGE_REVIEWED[@]}"; do [[ "$name" == "$ok" ]] && reviewed=1; done
                [[ $reviewed -eq 1 ]] || fail "upstream code reaches usage::$name (its Claude token path) from outside usage.rs; review what it reads, then add it to USAGE_REVIEWED: $hit"
            done
        done < <(grep -oE "$usage_ref" <<< "$text" || true)
    done < <(grep -rnE "$usage_ref" "$UP" \
                 | grep -vE "^$UP/(usage|claude_auth|doctor|watcher)\.rs:" \
                 | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)
    watcher=$(grep -rnE '(^|[^A-Za-z0-9_])watcher::' "$UP" \
                  | grep -vE "^$UP/(watcher|doctor)\.rs:" | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)
    [[ -z "$watcher" ]] || fail "upstream's transcript watcher (it walks usage.rs's profile list) is reached from: $watcher"

    # Upstream's doctor reads the credential (usage::probe_credentials): called
    # once, from main's "doctor" arm, which WCLI claims first. The glue is
    # searched too: its own doctor is cli.rs's, and a call to upstream's from
    # there would be reachable.
    doctor_calls=$(grep -rnE '(^|[^A-Za-z0-9_])doctor::' "$UP" \
                       | grep -vE "^$UP/doctor\.rs:" | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)
    doctor_ok=0
    if [[ -n "$doctor_calls" && $(grep -c . <<< "$doctor_calls") -eq 1 && "$doctor_calls" == "$UP/main.rs:"*'doctor::run()'* ]]; then
        n=$(cut -d: -f2 <<< "$doctor_calls")
        sed -n "$((n > 1 ? n - 1 : 1))p" "$UP/main.rs" | grep -qE '^[[:space:]]*"doctor"[[:space:]]*=>' && doctor_ok=1
    fi
    [[ $doctor_ok -eq 1 ]] || fail "upstream's doctor (it reads Claude's credential) must be called only from main's \"doctor\" arm, which WCLI makes unreachable: ${doctor_calls:-none found}"

    # GLM's key read, pinned.
    for name in ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY; do
        elsewhere=$(grep -rlF --exclude-dir=agentnotch -e "$name" "$UP" \
                        | grep -vxE "$UP/(glm|claude_auth)\.rs" || true)
        [[ -z "$elsewhere" ]] || fail "$name read outside glm.rs: $(echo $elsewhere)"
    done
    sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum; else shasum -a 256; fi | cut -d' ' -f1; }
    glm=$(awk '/^(pub(\([a-z]+\))? )?fn claude_code_key\(/ { on = 1 } on { print } on && /^}$/ { exit }' "$UP/glm.rs" 2>/dev/null || true)
    if [[ -z "$glm" ]]; then
        fail "glm.rs has no fn claude_code_key to check (review upstream's change, then update GLM_CLAUDE_KEY_PIN)"
    elif [[ "$(sha256 <<< "$glm")" != "$GLM_CLAUDE_KEY_PIN" ]]; then
        fail "glm.rs's claude_code_key changed (sha256 $(sha256 <<< "$glm")): review that it still reads only a Z.ai key and sends it nowhere else, then record the new hash in GLM_CLAUDE_KEY_PIN"
    fi
fi

if [[ $problems -gt 0 ]]; then
    echo "verify-token-free: $problems problem(s)"
    exit 1
fi
echo "verify-token-free: OK (bridge, package and the fork's lines in Sources/ and Tests/ are token-free; U1 keeps upstream's Claude token paths dormant; on Windows the fork's code and its lines in windows/ are token-free, WU1/WD/WCLI keep upstream's Claude token path dormant, and GLM's key read is pinned)"
