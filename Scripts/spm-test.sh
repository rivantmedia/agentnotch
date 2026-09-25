#!/usr/bin/env bash
# Run the Swift Testing suites of the local packages without Xcode.
#
#   Scripts/spm-test.sh                    # every package under Packages/, then the app's
#   Scripts/spm-test.sh ClaudeControl      # one package
#   Scripts/spm-test.sh ClaudeControl --filter SealedModeTests
#   Scripts/spm-test.sh app                # the app's own Swift Testing suites
#                                          # (Tests/ForkSPM, @testable import Codenotch)
#
# With only the Command Line Tools installed, SwiftPM neither finds the Testing
# macro plugin nor adds rpaths for Testing.framework, so both are passed here.
# The SDK is pinned to the 26.x SDK: the 27 SDK's SwiftUI @State macro plugin
# ships only with Xcode. With Xcode selected, this is plain `swift test`.
#
# Upstream's XCTest suite (Tests/) does not run here; it needs `make test`
# under Xcode.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLT=/Library/Developer/CommandLineTools

EXTRA=()
if [[ "$(xcode-select -p 2>/dev/null)" == "$CLT"* ]]; then
    if [[ -z "${SDKROOT:-}" ]]; then
        SDK=$(ls -d "$CLT"/SDKs/MacOSX26.*.sdk 2>/dev/null | sort -V | tail -1 || true)
        [[ -n "$SDK" ]] && export SDKROOT="$SDK"
    fi
    EXTRA=(
        -Xswiftc -plugin-path -Xswiftc "$CLT/usr/lib/swift/host/plugins/testing"
        -Xlinker -rpath -Xlinker "$CLT/Library/Developer/Frameworks"
        -Xlinker -rpath -Xlinker "$CLT/Library/Developer/usr/lib"
    )
fi

packages=()
app=1
if [[ $# -gt 0 && "$1" == "app" ]]; then
    shift
elif [[ $# -gt 0 && -d "$ROOT/Packages/$1" ]]; then
    packages=("$1"); shift; app=0
else
    for dir in "$ROOT"/Packages/*/; do
        [[ -f "$dir/Package.swift" ]] && packages+=("$(basename "$dir")")
    done
fi

echo "SDKROOT=${SDKROOT:-<default>}"
for name in ${packages[@]+"${packages[@]}"}; do
    echo "== swift test: Packages/$name"
    swift test --package-path "$ROOT/Packages/$name" ${EXTRA[@]+"${EXTRA[@]}"} "$@"
done
if [[ $app -eq 1 ]]; then
    # Package.resolved is also the Xcode project's pin file; keep it as
    # committed if SwiftPM only rewrites its originHash (see spm-build-app.sh).
    RESOLVED_BACKUP="$(mktemp)"
    cp "$ROOT/Package.resolved" "$RESOLVED_BACKUP"
    echo "== swift test: the app (Tests/ForkSPM)"
    status=0
    # Sparkle.framework sits beside the test bundle (Products/<config>/).
    swift test --package-path "$ROOT" ${EXTRA[@]+"${EXTRA[@]}"} \
        -Xlinker -rpath -Xlinker @loader_path/../../.. "$@" || status=$?
    if ! cmp -s "$ROOT/Package.resolved" "$RESOLVED_BACKUP" && python3 - "$RESOLVED_BACKUP" "$ROOT/Package.resolved" <<'PY'
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
sys.exit(0 if a.get("pins") == b.get("pins") else 1)
PY
    then
        cp "$RESOLVED_BACKUP" "$ROOT/Package.resolved"
    fi
    rm -f "$RESOLVED_BACKUP"
    exit $status
fi
