#!/usr/bin/env bash
# Render ClaudeControl's panel, chat and settings sheets to PNGs from fixtures,
# without Xcode:
#
#   Scripts/spm-snapshots.sh <output dir>
#
# The package's ClaudeControlSnapshots tool, with the SDK pinned the way
# Scripts/spm-test.sh pins it: with only the Command Line Tools, the 27 SDK's
# SwiftUI @State macro plugin is missing, so a plain `swift run` fails there.
# The app's own sheets (notch cells, badges, resting marks, panel chrome) come
# from a sealed launch instead: Scripts/spm-run-sealed.sh --snapshot-claude <dir>.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[[ $# -eq 1 ]] || { echo "usage: $0 <output dir>" >&2; exit 2; }
OUT="$1"
[[ "$OUT" = /* ]] || OUT="$PWD/$OUT"

CLT=/Library/Developer/CommandLineTools
if [[ -z "${SDKROOT:-}" && "$(xcode-select -p 2>/dev/null)" == "$CLT"* ]]; then
    SDK=$(ls -d "$CLT"/SDKs/MacOSX26.*.sdk 2>/dev/null | sort -V | tail -1 || true)
    [[ -n "$SDK" ]] && export SDKROOT="$SDK"
fi

swift run --package-path "$ROOT/Packages/ClaudeControl" ClaudeControlSnapshots "$OUT"
