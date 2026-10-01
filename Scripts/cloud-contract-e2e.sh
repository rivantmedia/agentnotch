#!/usr/bin/env bash
# The app <-> website contract (web/contract) end to end, each side running its real code: an
# app's own sync request goes through the website's own sync route into Postgres, and the
# route's own answer comes back through the app's client. No network and no Supabase: the
# website's test signs its own token and serves its own key set; the app tests never leave the
# process.
#
#   Scripts/cloud-contract-e2e.sh
#   AGENTNOTCH_CONTRACT_APP=swift|rust|both Scripts/cloud-contract-e2e.sh
#
# Which app runs: the Mac app's Swift engine (`swift`), the Windows port's Rust engine (`rust`,
# agentnotch-engine, which builds on any host) or `both`, one after the other, each with its own
# request and answer. Unset, it is `swift` when `swift` is on the PATH, else `rust` when `cargo`
# is; anything else is refused.
#
# 1. App: the Swift test CloudContractE2ETests/theAppsSyncRequestKeepsTheContract, or the Rust
#    test contract_e2e_request of `agentnotch-engine --test cloud_contract_e2e`, captures a
#    morning of sessions and usage through the sync service (temporary transcripts, ledger,
#    outbox) and writes the exact request body it sent (AGENTNOTCH_CONTRACT_OUT).
# 2. Web: tests/integration/contract-e2e.test.ts posts it twice through POST /api/app/v1/sync,
#    checks every stored row against it, and that every field the app sent was stored or is
#    listed as not stored, and writes the route's answer (AGENTNOTCH_CONTRACT_RESPONSE). It runs
#    in a throwaway Postgres container of its own, agentnotch-web-test-e2e on 127.0.0.1:55439,
#    which web/scripts/test-integration.mjs starts, migrates and always stops (stopping deletes
#    it). Should that runner die without stopping it (SIGKILL, out of memory), this script
#    removes the container on exit; it never touches any other.
# 3. App: theWebsitesSyncResponseIsRead (Swift) or contract_e2e_response (Rust) reads that
#    answer through the app's client and sync service.
#
# Needs Docker and web/node_modules (`npm ci` in web/), and `swift` or `cargo` for the app
# chosen. Exits non-zero when any step fails, and then keeps the requests, the answers and the
# logs in the folder it names.
#
# Sourced, it only defines its helpers (web/tests/unit/contract-e2e-script.test.ts tests them).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# The contract check's own container. Only this exact name is ever stopped or removed.
E2E_CONTAINER=agentnotch-web-test-e2e
E2E_PORT=55439
WORK=""

fail() {
    echo "cloud-contract-e2e: $*" >&2
    exit 1
}

# Removes the contract check's container if it is still there. The web runner stops it itself,
# including on failure, Ctrl-C and SIGTERM; this covers a runner killed before it could.
remove_e2e_container() {
    command -v docker >/dev/null 2>&1 || return 0
    local found
    found="$(docker ps -a --filter "name=^/${E2E_CONTAINER}\$" --format '{{.Names}}' 2>/dev/null)" || return 0
    [[ "$found" == "$E2E_CONTAINER" ]] || return 0
    if docker rm -f "$E2E_CONTAINER" >/dev/null 2>&1; then
        echo "cloud-contract-e2e: removed the leftover $E2E_CONTAINER" >&2
    fi
}

cleanup() {
    local status=$?
    remove_e2e_container
    if [[ $status -eq 0 ]]; then
        [[ -z "$WORK" ]] || rm -rf "$WORK"
    else
        echo "cloud-contract-e2e: FAILED (the request, answer and logs are in $WORK)" >&2
    fi
}

# Whether the Swift log `$2` says test `$1` ran and passed (colours stripped). grep reads the
# stripped log from a process substitution, not a pipe: `grep -q` stops at the first match, and
# under pipefail a sed still writing would die of SIGPIPE and fail a test that passed.
swift_log_passed() {
    local test="$1" log="$2"
    grep -Eq "Test $test\\(\\) passed" < <(sed $'s/\x1b\\[[0-9;]*m//g' "$log")
}

# Swift Testing passes a run whose filter matched nothing, so each Swift step also checks that
# its test ran and passed.
swift_step() {
    local test="$1"
    shift
    local log="$WORK/$test.log"
    env "$@" "$ROOT/Scripts/spm-test.sh" ClaudeControl --filter "CloudContractE2ETests/$test" 2>&1 | tee "$log"
    swift_log_passed "$test" "$log" || fail "$test didn't run and pass"
}

# Whether the cargo log `$1` says a test ran and passed: exactly one test passed (colours
# stripped). A filter that matches nothing also ends "ok", with 0 passed, so "1 passed" is the
# proof that the named test ran. Read from a process substitution, for the reason above.
rust_log_passed() {
    local log="$1"
    grep -Eq "test result: ok\\. 1 passed" < <(sed $'s/\x1b\\[[0-9;]*m//g' "$log")
}

# One exact test of the Rust engine's contract check. The two tests take their files from the
# variables given after the name.
rust_step() {
    local test="$1"
    shift
    local log="$WORK/rust-$test.log"
    env "$@" cargo test --manifest-path "$ROOT/windows/Cargo.toml" --locked -p agentnotch-engine \
        --test cloud_contract_e2e "$test" -- --exact 2>&1 | tee "$log"
    rust_log_passed "$log" || fail "$test didn't run and pass"
}

# The apps to check, one per line, from AGENTNOTCH_CONTRACT_APP (swift, rust or both). Unset, the
# one this machine can run: swift, else rust.
selected_apps() {
    local choice="${AGENTNOTCH_CONTRACT_APP:-}"
    if [[ -z "$choice" ]]; then
        if command -v swift >/dev/null 2>&1; then
            choice=swift
        elif command -v cargo >/dev/null 2>&1; then
            choice=rust
        else
            echo "cloud-contract-e2e: neither swift nor cargo is on the PATH." >&2
            return 1
        fi
    fi
    case "$choice" in
    swift) echo swift ;;
    rust) echo rust ;;
    both)
        echo swift
        echo rust
        ;;
    *)
        echo "cloud-contract-e2e: AGENTNOTCH_CONTRACT_APP must be swift, rust or both, not '$choice'." >&2
        return 1
        ;;
    esac
}

# The app's request step: writes the request to `$1` ($2 names the app).
app_request() {
    local app="$1" request="$2"
    case "$app" in
    swift) swift_step theAppsSyncRequestKeepsTheContract AGENTNOTCH_CONTRACT_OUT="$request" ;;
    rust) rust_step contract_e2e_request AGENTNOTCH_CONTRACT_OUT="$request" ;;
    esac
}

# The app's response step: reads the request `$2` and the website's answer `$3`.
app_response() {
    local app="$1" request="$2" response="$3"
    case "$app" in
    swift) swift_step theWebsitesSyncResponseIsRead AGENTNOTCH_CONTRACT_OUT="$request" AGENTNOTCH_CONTRACT_RESPONSE="$response" ;;
    rust) rust_step contract_e2e_response AGENTNOTCH_CONTRACT_OUT="$request" AGENTNOTCH_CONTRACT_RESPONSE="$response" ;;
    esac
}

# One app, start to end: its request, the website's route, its answer read back.
check_app() {
    local app="$1"
    local request="$WORK/$app-sync-request.json"
    local response="$WORK/$app-sync-response.json"

    echo "== [$app] 1/3 the app encodes a sync request"
    app_request "$app" "$request"
    [[ -s "$request" ]] || fail "the $app test wrote no request"

    echo "== [$app] 2/3 the website's sync route stores it (web, $E2E_CONTAINER on 127.0.0.1:$E2E_PORT)"
    (
        cd "$ROOT/web"
        AGENTNOTCH_WEB_TEST_CONTAINER="$E2E_CONTAINER" AGENTNOTCH_WEB_TEST_PORT="$E2E_PORT" \
        AGENTNOTCH_CONTRACT_OUT="$request" AGENTNOTCH_CONTRACT_RESPONSE="$response" \
            node scripts/test-integration.mjs tests/integration/contract-e2e.test.ts
    ) 2>&1 | tee "$WORK/web-$app.log"
    [[ -s "$response" ]] || fail "the website's test wrote no answer"

    echo "== [$app] 3/3 the app reads the website's answer"
    app_response "$app" "$request" "$response"
}

main() {
    local apps app
    apps="$(selected_apps)" || exit 1
    WORK="$(mktemp -d "${TMPDIR:-/tmp}/agentnotch-contract-e2e.XXXXXX")"
    trap cleanup EXIT

    docker info >/dev/null 2>&1 || fail "Docker is not running."
    [[ -d "$ROOT/web/node_modules" ]] || fail "web/node_modules is missing: run npm ci in web/."
    # A word list, not a `read` loop: the steps below would read the rest of the list from stdin.
    for app in $apps; do
        case "$app" in
        swift) command -v swift >/dev/null 2>&1 || fail "swift is not on the PATH." ;;
        rust) command -v cargo >/dev/null 2>&1 || fail "cargo is not on the PATH." ;;
        esac
    done

    for app in $apps; do
        check_app "$app"
    done

    echo "cloud-contract-e2e: OK"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
