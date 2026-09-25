#!/usr/bin/env bash
# Build "Agent Notch.app" with only the Command Line Tools (no Xcode).
#
#   Scripts/spm-build-app.sh                 # debug build -> build/Agent Notch.app
#   Scripts/spm-build-app.sh --release       # optimized build
#   Scripts/spm-build-app.sh --skip-build    # re-assemble the bundle from the last build
#   Scripts/spm-build-app.sh --bundle-id com.rivantmedia.agentnotch.dev \
#                            --name "Agent Notch Dev" --out build/dev
#
# Environment:
#   SDKROOT        SDK to build against. Defaults to the newest MacOSX26.x SDK of
#                  the Command Line Tools: the 27 SDK's SwiftUI @State macro
#                  plugin ships only with Xcode, so it fails with 190+ errors.
#   SIGN_IDENTITY  codesign identity (default "-", ad hoc). A stable identity such
#                  as "Apple Development: …" keeps keychain "Always Allow" grants
#                  across rebuilds; an ad-hoc signature is new on every build.
#
# What the Command Line Tools cannot compile is converted here instead:
#   Localizable.xcstrings -> <lang>.lproj/Localizable.strings (no xcstringstool)
#   Assets.xcassets       -> loose images + AppIcon.icns via iconutil (no actool)
#
# The bundle id must stay one that `Fork.ownsBundleIdentifier` accepts
# (com.rivantmedia.agentnotch or a suffix of it); the official
# Codenotch (com.vinz.codenotch) is refused.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BASE_ID="com.rivantmedia.agentnotch"
CONFIG=debug
SKIP_BUILD=0
BUNDLE_ID="$BASE_ID"
NAME="Agent Notch"
OUT="$ROOT/build"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)    CONFIG=release ;;
        --debug)      CONFIG=debug ;;
        --skip-build) SKIP_BUILD=1 ;;
        --bundle-id)  BUNDLE_ID="$2"; shift ;;
        --name)       NAME="$2"; shift ;;
        --out)        OUT="$2"; shift ;;
        -h|--help)    sed -n '2,26p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done
[[ "$OUT" = /* ]] || OUT="$ROOT/$OUT"

if [[ "$BUNDLE_ID" != "$BASE_ID" && "$BUNDLE_ID" != "$BASE_ID".* ]]; then
    echo "refusing bundle id '$BUNDLE_ID': must be $BASE_ID or $BASE_ID.<suffix>" >&2
    exit 2
fi

CLT=/Library/Developer/CommandLineTools
if [[ -z "${SDKROOT:-}" && "$(xcode-select -p 2>/dev/null)" == "$CLT"* ]]; then
    SDK=$(ls -d "$CLT"/SDKs/MacOSX26.*.sdk 2>/dev/null | sort -V | tail -1 || true)
    [[ -n "$SDK" ]] && export SDKROOT="$SDK"
fi
echo "SDKROOT=${SDKROOT:-<default>}  config=$CONFIG  bundle id=$BUNDLE_ID"

# --- Build ---------------------------------------------------------------------
if [[ $SKIP_BUILD -eq 0 ]]; then
    # Package.resolved is also the Xcode project's pin file (`make gen` copies
    # it, `make verify-deps` diffs it). SwiftPM keeps its pins; if it ever
    # rewrites only the originHash, put the committed file back.
    RESOLVED_BACKUP="$(mktemp)"
    cp Package.resolved "$RESOLVED_BACKUP"
    swift build -c "$CONFIG" --product Codenotch
    if ! cmp -s Package.resolved "$RESOLVED_BACKUP"; then
        if python3 - "$RESOLVED_BACKUP" Package.resolved <<'PY'
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
sys.exit(0 if a.get("pins") == b.get("pins") else 1)
PY
        then
            cp "$RESOLVED_BACKUP" Package.resolved
            echo "note: restored Package.resolved (SwiftPM changed only its originHash)"
        else
            echo "WARNING: SwiftPM changed the pins in Package.resolved; review 'git diff Package.resolved'" >&2
        fi
    fi
    rm -f "$RESOLVED_BACKUP"
fi
BIN="$(swift build -c "$CONFIG" --show-bin-path)"
[[ -x "$BIN/Codenotch" ]] || { echo "no binary at $BIN/Codenotch; build first" >&2; exit 1; }

# --- Assemble ------------------------------------------------------------------
SRC="$ROOT/Sources"
APP="$OUT/$NAME.app"
C="$APP/Contents"
rm -rf "$APP"
mkdir -p "$C/MacOS" "$C/Resources" "$C/Frameworks"
# Spotlight would otherwise list every build as an installed app.
touch "$OUT/.metadata_never_index"

cp "$BIN/Codenotch" "$C/MacOS/$NAME"
if ! otool -l "$C/MacOS/$NAME" | grep -q "@executable_path/../Frameworks"; then
    # Its "will invalidate the code signature" warning is expected: signed below.
    install_name_tool -add_rpath @executable_path/../Frameworks "$C/MacOS/$NAME" 2>/dev/null
fi
ditto "$BIN/Sparkle.framework" "$C/Frameworks/Sparkle.framework"
printf 'APPL????' > "$C/PkgInfo"

# Info.plist: Sources/Info.plist with Xcode's build-setting placeholders filled in.
MARKETING_VERSION=$(awk -F'"' '/MARKETING_VERSION:/ {print $2; exit}' project.yml)
CURRENT_PROJECT_VERSION=$(awk -F'"' '/CURRENT_PROJECT_VERSION:/ {print $2; exit}' project.yml)
python3 - "$SRC/Info.plist" "$C/Info.plist" "$NAME" "$BUNDLE_ID" "$MARKETING_VERSION" "$CURRENT_PROJECT_VERSION" <<'PY'
import plistlib, sys
src, dst, name, bundle_id, version, build = sys.argv[1:7]
d = plistlib.load(open(src, "rb"))
d.update(CFBundleExecutable=name, CFBundleIdentifier=bundle_id,
         CFBundleName=name, CFBundleDisplayName=name,
         CFBundleShortVersionString=version, CFBundleVersion=build,
         CFBundleIconFile="AppIcon", CFBundleSupportedPlatforms=["MacOSX"])
# This fork never updates itself: no Sparkle feed or key may reach the bundle.
for key in [k for k in d if k.startswith("SU")]:
    del d[key]
d["SUEnableAutomaticChecks"] = False
unresolved = [k for k, v in d.items() if isinstance(v, str) and "$(" in v]
if unresolved:
    sys.exit(f"Info.plist: unresolved build settings in {unresolved}")
plistlib.dump(d, open(dst, "wb"))
PY

# String catalog -> <lang>.lproj/Localizable.strings. Plural variations would
# need .stringsdict; the catalog has none today, and any that appear are
# reported and fall back to the English key.
python3 - "$SRC/Localizable.xcstrings" "$C/Resources" <<'PY'
import collections, json, os, plistlib, sys
catalog = json.load(open(sys.argv[1]))
tables, skipped = collections.defaultdict(dict), 0
for key, entry in catalog["strings"].items():
    for lang, loc in entry.get("localizations", {}).items():
        unit = loc.get("stringUnit")
        if unit and "value" in unit:
            tables[lang][key] = unit["value"]
        elif "variations" in loc:
            skipped += 1
tables.setdefault(catalog.get("sourceLanguage", "en"), {})
for lang, table in tables.items():
    folder = os.path.join(sys.argv[2], lang + ".lproj")
    os.makedirs(folder, exist_ok=True)
    with open(os.path.join(folder, "Localizable.strings"), "wb") as f:
        plistlib.dump(table, f, fmt=plistlib.FMT_BINARY)
print("localizations:", ", ".join(f"{k} {len(v)}" for k, v in sorted(tables.items())),
      f"(skipped {skipped} plural entries)" if skipped else "")
PY

# Asset catalog -> loose images NSImage(named:) finds; 2x-only PNGs keep their
# point size through the @2x suffix. Template rendering is applied in code.
python3 - "$SRC/Assets.xcassets" "$C/Resources" <<'PY'
import glob, json, os, shutil, sys
catalog, out = sys.argv[1:3]
count = 0
for imageset in sorted(glob.glob(os.path.join(catalog, "*.imageset"))):
    name = os.path.basename(imageset)[: -len(".imageset")]
    for image in json.load(open(os.path.join(imageset, "Contents.json"))).get("images", []):
        filename = image.get("filename")
        if not filename:
            continue
        ext = os.path.splitext(filename)[1]
        scale = image.get("scale", "1x")
        suffix = "" if scale == "1x" or ext == ".svg" else "@" + scale
        shutil.copy(os.path.join(imageset, filename), os.path.join(out, name + suffix + ext))
        count += 1
print(f"images: {count}")
PY
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
cp "$SRC"/Assets.xcassets/AppIcon.appiconset/icon_*.png "$ICONSET/"
iconutil -c icns "$ICONSET" -o "$C/Resources/AppIcon.icns"
rm -rf "$(dirname "$ICONSET")"

# Resources, and the notices the licences ask to travel with the binary.
cp -R "$SRC/Resources/." "$C/Resources/"
cp "$ROOT/LICENSE" "$C/Resources/Codenotch-LICENSE.txt"
cp "$SRC/Vendor/zstd/LICENSE" "$C/Resources/zstd-LICENSE.txt"
# Fork: ClaudeControl is Apache-2.0 (derived from Superpowered Vibe Notch and
# Vibe Notch), and its NOTICE has to go wherever the binary goes (§4(d)); so do
# the licences of what it links statically, swift-markdown (Apache-2.0, with a
# NOTICE) and swift-cmark (BSD-2-Clause).
cp "$ROOT/Packages/ClaudeControl/LICENSE" "$C/Resources/ClaudeControl-LICENSE.txt"
cp "$ROOT/Packages/ClaudeControl/NOTICE" "$C/Resources/ClaudeControl-NOTICE.txt"
CHECKOUTS="$ROOT/.build/checkouts"
for notice in swift-markdown/LICENSE.txt swift-markdown/NOTICE.txt swift-cmark/COPYING; do
    if [[ ! -f "$CHECKOUTS/$notice" ]]; then
        echo "missing $CHECKOUTS/$notice: build without --skip-build first" >&2
        exit 1
    fi
    name="${notice%%/*}-$(basename "$notice")"
    cp "$CHECKOUTS/$notice" "$C/Resources/${name%.txt}.txt"
done

# --- Sign ----------------------------------------------------------------------
codesign --force --deep --sign "${SIGN_IDENTITY:--}" "$APP"
codesign --verify --deep --strict "$APP"
echo "built: $APP"
