#!/usr/bin/env bash
# The app <-> website contract (web/contract) end to end, each side running its real code: the
# Mac app's own sync request goes through the website's own sync route into Postgres, and the
# route's own answer comes back through the app's client. No network and no Supabase: the
# website's test signs its own token and serves its own key set; the Swift tests never leave the
# process.
#
#   Scripts/cloud-contract-e2e.sh
#
# 1. Swift: CloudContractE2ETests/theAppsSyncRequestKeepsTheContract captures a morning of
#    sessions and usage through the sync service (temporary transcripts, ledger, outbox) and
#    writes the exact request body it sent (AGENTNOTCH_CONTRACT_OUT).
# 2. Web: tests/integration/contract-e2e.test.ts posts it twice through POST /api/app/v1/sync,
#    checks every stored row against it, and that every field the app sent was stored or is
#    listed as not stored, and writes the route's answer (AGENTNOTCH_CONTRACT_RESPONSE). It runs
#    in a throwaway Postgres container of its own, agentnotch-web-test-e2e on 127.0.0.1:55439,
#    which web/scripts/test-integration.mjs starts, migrates and always stops (stopping deletes
#    it). Should that runner die without stopping it (SIGKILL, out of memory), this script
#    removes the container on exit; it never touches any other.
# 3. Swift: CloudContractE2ETests/theWebsitesSyncResponseIsRead reads that answer through the
#    app's client and sync service.
#
# Needs Docker and web/node_modules (`npm install` in web/). Exits non-zero when any step fails,
# and then keeps the request, the answer and the logs in the folder it names.
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

main() {
    WORK="$(mktemp -d "${TMPDIR:-/tmp}/agentnotch-contract-e2e.XXXXXX")"
    trap cleanup EXIT
    local request="$WORK/sync-request.json"
    local response="$WORK/sync-response.json"

    docker info >/dev/null 2>&1 || fail "Docker is not running."
    [[ -d "$ROOT/web/node_modules" ]] || fail "web/node_modules is missing: run npm install in web/."

    echo "== 1/3 the app encodes a sync request (Swift)"
    swift_step theAppsSyncRequestKeepsTheContract AGENTNOTCH_CONTRACT_OUT="$request"
    [[ -s "$request" ]] || fail "the Swift test wrote no request"

    echo "== 2/3 the website's sync route stores it (web, $E2E_CONTAINER on 127.0.0.1:$E2E_PORT)"
    (
        cd "$ROOT/web"
        AGENTNOTCH_WEB_TEST_CONTAINER="$E2E_CONTAINER" AGENTNOTCH_WEB_TEST_PORT="$E2E_PORT" \
        AGENTNOTCH_CONTRACT_OUT="$request" AGENTNOTCH_CONTRACT_RESPONSE="$response" \
            node scripts/test-integration.mjs tests/integration/contract-e2e.test.ts
    ) 2>&1 | tee "$WORK/web.log"
    [[ -s "$response" ]] || fail "the website's test wrote no answer"

    echo "== 3/3 the app reads the website's answer (Swift)"
    swift_step theWebsitesSyncResponseIsRead AGENTNOTCH_CONTRACT_OUT="$request" AGENTNOTCH_CONTRACT_RESPONSE="$response"

    echo "cloud-contract-e2e: OK"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
