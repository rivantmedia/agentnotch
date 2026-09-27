#!/usr/bin/env bash
# Build "Agent Notch.app" with only the Command Line Tools (no Xcode).
#
#   Scripts/spm-build-app.sh                 # debug build -> build/Agent Notch.app
#   Scripts/spm-build-app.sh --release       # optimized build
#   Scripts/spm-build-app.sh --skip-build    # re-assemble the bundle from the last build
#   Scripts/spm-build-app.sh --bundle-id com.rivantmedia.agentnotch.dev \
#                            --name "Agent Notch Dev" --out build/dev
#
# Release options (Scripts/release-build.sh passes them; day-to-day builds don't):
#   --universal         arm64 and x86_64, each built on its own and joined with lipo
#   --with-updates      the Sparkle feed and public key in the Info.plist, so the
#                       app updates itself from this fork's GitHub releases.
#                       Refused unless the bundle id is exactly
#                       com.rivantmedia.agentnotch and the public key file holds
#                       a key. Every other build carries no feed or key: a copy
#                       built from source never replaces itself with a release.
#   --hardened-runtime  hardened runtime, a secure timestamp and
#                       Scripts/AgentNotch.entitlements, for a Developer ID
#                       identity that is going to be notarized. Not for ad hoc
#                       or self-signed identities: library validation then
#                       refuses to load Sparkle ("different Team IDs").
#
# Environment:
#   SDKROOT        SDK to build against. Defaults to the newest MacOSX26.x SDK of
#                  the Command Line Tools: the 27 SDK's SwiftUI @State macro
#                  plugin ships only with Xcode, so it fails with 190+ errors.
#   SIGN_IDENTITY  codesign identity (default "-", ad hoc). A stable identity such
#                  as "Apple Development: …" keeps keychain "Always Allow" grants
#                  across rebuilds; an ad-hoc signature is new on every build.
#   SIGN_KEYCHAIN  the keychain holding SIGN_IDENTITY, when it is not on the
#                  search list (release-build.sh's temporary one)
#   AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE
#                  the public key file --with-updates reads (default
#                  Scripts/sparkle-public-ed-key.txt); tests use a throwaway key
#
# The version is the repo root's VERSION file, for CFBundleShortVersionString
# and CFBundleVersion alike: Sparkle compares CFBundleVersion, and project.yml's
# numbers are upstream's (they only move on a merge).
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
# The one feed a release build reads (Fork.updateFeedURL checks it is exactly this).
FEED_URL="https://github.com/rivantmedia/agentnotch/releases/latest/download/appcast.xml"
CONFIG=debug
SKIP_BUILD=0
UNIVERSAL=0
WITH_UPDATES=0
HARDENED=0
BUNDLE_ID="$BASE_ID"
NAME="Agent Notch"
OUT="$ROOT/build"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)    CONFIG=release ;;
        --debug)      CONFIG=debug ;;
        --skip-build) SKIP_BUILD=1 ;;
        --universal)  UNIVERSAL=1 ;;
        --with-updates) WITH_UPDATES=1 ;;
        --hardened-runtime) HARDENED=1 ;;
        --bundle-id)  BUNDLE_ID="$2"; shift ;;
        --name)       NAME="$2"; shift ;;
        --out)        OUT="$2"; shift ;;
        -h|--help)    sed -n '/^set -euo pipefail/q;2,$p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done
[[ "$OUT" = /* ]] || OUT="$ROOT/$OUT"

if [[ "$BUNDLE_ID" != "$BASE_ID" && "$BUNDLE_ID" != "$BASE_ID".* ]]; then
    echo "refusing bundle id '$BUNDLE_ID': must be $BASE_ID or $BASE_ID.<suffix>" >&2
    exit 2
fi

VERSION_FILE="$ROOT/VERSION"
if [[ ! -f "$VERSION_FILE" ]]; then
    echo "missing $VERSION_FILE: it holds the app's version, one line such as 1.0.0" >&2
    exit 1
fi
VERSION="$(<"$VERSION_FILE")"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "$VERSION_FILE must be one line of major.minor.patch numbers (such as 1.0.0), not '$VERSION'" >&2
    exit 1
fi

PUBLIC_KEY=""
if [[ $WITH_UPDATES -eq 1 ]]; then
    # A development or sealed copy with the feed would replace itself with the
    # release (Sparkle's installer matches by name before bundle id).
    if [[ "$BUNDLE_ID" != "$BASE_ID" ]]; then
        echo "refusing --with-updates for bundle id '$BUNDLE_ID': only $BASE_ID updates itself" >&2
        exit 2
    fi
    KEY_FILE="${AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE:-$ROOT/Scripts/sparkle-public-ed-key.txt}"
    # Sparkle reads the key from the bundle on disk and nowhere else; one it
    # cannot decode makes every check fail with an alert, so refuse it here.
    PUBLIC_KEY="$(python3 - "$KEY_FILE" <<'PY'
import base64, binascii, sys
path = sys.argv[1]
try:
    lines = open(path, encoding="utf-8").read().splitlines()
except OSError as error:
    sys.exit(f"--with-updates: cannot read the public key file {path}: {error.strerror}")
key = next((l.strip() for l in lines if l.strip() and not l.strip().startswith("#")), None)
if key is None:
    sys.exit(f"--with-updates: {path} holds no key yet; run Scripts/release-make-keys.sh --update-key <dir>")
try:
    raw = base64.b64decode(key, validate=True)
except (binascii.Error, ValueError):
    raw = b""
if len(raw) != 32:
    sys.exit(f"--with-updates: the key in {path} is not the base64 of a 32-byte Ed25519 public key")
print(key)
PY
)" || exit 2
fi
if [[ $HARDENED -eq 1 && "${SIGN_IDENTITY:--}" == "-" ]]; then
    echo "refusing --hardened-runtime with an ad-hoc signature: library validation would refuse Sparkle; set SIGN_IDENTITY to a Developer ID" >&2
    exit 2
fi

CLT=/Library/Developer/CommandLineTools
if [[ -z "${SDKROOT:-}" && "$(xcode-select -p 2>/dev/null)" == "$CLT"* ]]; then
    SDK=$(ls -d "$CLT"/SDKs/MacOSX26.*.sdk 2>/dev/null | sort -V | tail -1 || true)
    [[ -n "$SDK" ]] && export SDKROOT="$SDK"
fi
echo "SDKROOT=${SDKROOT:-<default>}  config=$CONFIG  bundle id=$BUNDLE_ID  version=$VERSION$([[ $UNIVERSAL -eq 1 ]] && echo "  universal")$([[ $WITH_UPDATES -eq 1 ]] && echo "  with updates")"

# The toolchain's Swift compatibility libraries: static ones the linker adds
# for code that targets an older macOS than the stdlib feature it uses, and
# back-deployment dylibs (swift-<version>/macosx) the app has to carry.
TOOLCHAIN_LIB="$(dirname "$(dirname "$(xcrun -f swiftc)")")/lib"
if [[ $UNIVERSAL -eq 1 ]]; then ARCHS=(arm64 x86_64); else ARCHS=(""); fi

# A universal build needs them in x86_64 too, and the Command Line Tools for
# Swift 6.4 ship them for Apple silicon only: the x86_64 link then fails on
# libswiftCompatibility56.a (swift-nio and swift-collections target macOS
# 10.15), and even a linked x86_64 slice could not launch on an Intel Mac with
# macOS 15, having no x86_64 libswiftCompatibilitySpan to carry. Say so before
# spending two builds on it. (Only x86_64 is looked for: the swift-5.0
# folder is x86_64 alone and never needed by Apple silicon.)
if [[ $UNIVERSAL -eq 1 ]]; then
    missing=()
    for lib in "$TOOLCHAIN_LIB"/swift/macosx/libswiftCompatibility*.a "$TOOLCHAIN_LIB"/swift-*/macosx/*.dylib; do
        [[ -f "$lib" ]] || continue
        if [[ " $(lipo -archs "$lib") " != *" x86_64 "* ]]; then
            missing+=("$lib ($(lipo -archs "$lib"))")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        echo "--universal: this toolchain ($(xcrun -f swiftc)) has no x86_64 slice of:" >&2
        printf '  %s\n' "${missing[@]}" >&2
        echo "The x86_64 link needs the static ones (swift-nio targets macOS 10.15), and an Intel Mac on macOS 15 needs libswiftCompatibilitySpan." >&2
        echo "Build the universal app with Xcode 26 (sudo xcode-select -s /Applications/Xcode_26.x.app), as the release workflow does." >&2
        exit 1
    fi
fi

# --- Build ---------------------------------------------------------------------
# A universal app is one build per architecture, joined with lipo below. One
# `swift build` with two --arch flags would hand the build to XCBuild on some
# toolchains (Swift 6.3's native build system), with its own products folder
# and its own warnings. The bin path is always asked for with the same flags
# as the build, because it moves with both the toolchain and the flags.
# Each architecture's executable is kept here: swiftbuild writes both into
# the same products folder, so the second build would replace the first.
SLICES="$ROOT/.build/agentnotch-slices/$CONFIG"
build_flags() {
    FLAGS=(-c "$CONFIG")
    if [[ -n "$1" ]]; then FLAGS+=(--arch "$1"); fi
}

if [[ $SKIP_BUILD -eq 0 ]]; then
    # Package.resolved is also the Xcode project's pin file (`make gen` copies
    # it, `make verify-deps` diffs it). SwiftPM keeps its pins; if it ever
    # rewrites only the originHash, put the committed file back.
    RESOLVED_BACKUP="$(mktemp)"
    cp Package.resolved "$RESOLVED_BACKUP"
    for arch in "${ARCHS[@]}"; do
        build_flags "$arch"
        swift build "${FLAGS[@]}" --product Codenotch
        if [[ -n "$arch" ]]; then
            ARCH_BIN="$(swift build "${FLAGS[@]}" --show-bin-path)"
            mkdir -p "$SLICES/$arch"
            cp "$ARCH_BIN/Codenotch" "$SLICES/$arch/Codenotch"
        fi
    done
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
build_flags "${ARCHS[0]}"
BIN="$(swift build "${FLAGS[@]}" --show-bin-path)"
if [[ $UNIVERSAL -eq 1 ]]; then
    EXECUTABLES=()
    for arch in "${ARCHS[@]}"; do
        exe="$SLICES/$arch/Codenotch"
        [[ -x "$exe" ]] || { echo "no $arch binary at $exe; build without --skip-build first" >&2; exit 1; }
        [[ "$(lipo -archs "$exe")" == "$arch" ]] || { echo "$exe is $(lipo -archs "$exe"), not $arch" >&2; exit 1; }
        EXECUTABLES+=("$exe")
    done
else
    [[ -x "$BIN/Codenotch" ]] || { echo "no binary at $BIN/Codenotch; build first" >&2; exit 1; }
    EXECUTABLES=("$BIN/Codenotch")
fi

# --- Assemble ------------------------------------------------------------------
SRC="$ROOT/Sources"
APP="$OUT/$NAME.app"
C="$APP/Contents"
EXE="$C/MacOS/$NAME"
rm -rf "$APP"
# A bundle that stops halfway is removed rather than left looking finished.
BUILT=0
SLICE_DIR=""
trap 'if [[ $BUILT -eq 0 ]]; then rm -rf "$APP"; fi; if [[ -n "$SLICE_DIR" ]]; then rm -rf "$SLICE_DIR"; fi' EXIT
mkdir -p "$C/MacOS" "$C/Resources" "$C/Frameworks"
# Spotlight would otherwise list every build as an installed app.
touch "$OUT/.metadata_never_index"

# The executable finds its @rpath libraries in the bundle's Frameworks folder
# and in /usr/lib/swift, never in the build machine's folders: SwiftPM adds the
# build tree's PackageFrameworks and the toolchain's back-deployment folder by
# absolute path, which on another Mac is missing or holds something else.
# Those folders are remembered first, as where the linker found the libraries
# embedded below.
rpaths() {
    otool -l "$1" | awk '/cmd LC_RPATH/ { r = 1; next }
        r && $1 == "path" { sub(/^ *path /, ""); sub(/ \(offset [0-9]+\)$/, ""); print; r = 0 }'
}
LIB_DIRS=()
SLICE_DIR="$(mktemp -d)"
THIN=()
for exe in "${EXECUTABLES[@]}"; do
    thin="$SLICE_DIR/$(lipo -archs "$exe" | tr ' ' '-')"
    cp "$exe" "$thin"
    found_rpaths="$(rpaths "$thin")"
    while IFS= read -r rpath; do
        [[ "$rpath" == /* && "$rpath" != /usr/lib/* && "$rpath" != /System/* ]] || continue
        LIB_DIRS+=("$rpath")
        # Its "will invalidate the code signature" warning is expected: signed below.
        install_name_tool -delete_rpath "$rpath" "$thin" 2>/dev/null
    done <<< "$found_rpaths"
    if ! grep -qx "@executable_path/../Frameworks" <<< "$(rpaths "$thin")"; then
        install_name_tool -add_rpath @executable_path/../Frameworks "$thin" 2>/dev/null
    fi
    THIN+=("$thin")
done
if [[ ${#THIN[@]} -gt 1 ]]; then
    lipo -create -output "$EXE" "${THIN[@]}"
else
    cp "${THIN[0]}" "$EXE"
fi
rm -rf "$SLICE_DIR"
SLICE_DIR=""
EXE_ARCHS="$(lipo -archs "$EXE")"
if [[ $UNIVERSAL -eq 1 ]]; then
    for arch in "${ARCHS[@]}"; do
        [[ " $EXE_ARCHS " == *" $arch "* ]] || { echo "the universal executable is '$EXE_ARCHS', with no $arch" >&2; exit 1; }
    done
fi
ditto "$BIN/Sparkle.framework" "$C/Frameworks/Sparkle.framework"
for arch in $EXE_ARCHS; do
    if [[ " $(lipo -archs "$C/Frameworks/Sparkle.framework/Sparkle") " != *" $arch "* ]]; then
        echo "Sparkle.framework has no $arch slice" >&2
        exit 1
    fi
done

# Swift libraries the executable loads through @rpath are back-deployment
# libraries (libswiftCompatibilitySpan for a macOS 15 target): macOS 26 has
# them in /usr/lib/swift, which the executable searches first, and macOS 15
# does not, so without a copy here the app cannot launch there. Every one
# has to cover every architecture of the executable, or that architecture
# cannot launch on macOS 15.
while IFS= read -r dir; do
    if [[ -d "$dir" ]]; then LIB_DIRS+=("$dir"); fi
done < <(printf '%s\n' "$TOOLCHAIN_LIB"/swift-*/macosx | sort -rV)
LIB_DIRS+=("$TOOLCHAIN_LIB/swift/macosx")
while IFS= read -r lib; do
    [[ -n "$lib" ]] || continue
    found=""
    for dir in "${LIB_DIRS[@]}"; do
        if [[ -f "$dir/$lib" ]]; then found="$dir/$lib"; break; fi
    done
    if [[ -z "$found" ]]; then
        echo "the app loads @rpath/$lib, which is in none of: ${LIB_DIRS[*]}" >&2
        exit 1
    fi
    for arch in $EXE_ARCHS; do
        if [[ " $(lipo -archs "$found") " != *" $arch "* ]]; then
            echo "$found has no $arch slice ($(lipo -archs "$found")), so the $arch app could not launch on macOS 15." >&2
            echo "Build it with a toolchain that ships the library for every architecture: Xcode 26, as the release workflow does." >&2
            exit 1
        fi
    done
    ditto "$found" "$C/Frameworks/$lib"
done < <(otool -arch all -L "$EXE" | awk '$1 ~ /^@rpath\/libswift[^\/]*\.dylib$/ { print substr($1, 8) }' | sort -u)
# Every @rpath library the executable loads is in the bundle, or it would
# launch only on a Mac that happens to have the build machine's folders.
while IFS= read -r dep; do
    [[ -n "$dep" ]] || continue
    [[ -e "$C/Frameworks/$dep" ]] || { echo "the app loads @rpath/$dep, which is not in Contents/Frameworks" >&2; exit 1; }
done < <(otool -arch all -L "$EXE" | awk '$1 ~ /^@rpath\// { print substr($1, 8) }' | sort -u)
printf 'APPL????' > "$C/PkgInfo"

# Info.plist: Sources/Info.plist with Xcode's build-setting placeholders filled in.
python3 - "$SRC/Info.plist" "$C/Info.plist" "$NAME" "$BUNDLE_ID" "$VERSION" "$WITH_UPDATES" "$FEED_URL" "$PUBLIC_KEY" <<'PY'
import plistlib, sys
src, dst, name, bundle_id, version, with_updates, feed_url, public_key = sys.argv[1:9]
d = plistlib.load(open(src, "rb"))
d.update(CFBundleExecutable=name, CFBundleIdentifier=bundle_id,
         CFBundleName=name, CFBundleDisplayName=name,
         CFBundleShortVersionString=version, CFBundleVersion=version,
         CFBundleIconFile="AppIcon", CFBundleSupportedPlatforms=["MacOSX"])
for key in [k for k in d if k.startswith("SU")]:
    del d[key]
if with_updates == "1":
    # A release: this fork's own feed and key, checked daily and installed
    # without asking; the archive is verified before it is even unpacked.
    d.update(SUFeedURL=feed_url, SUPublicEDKey=public_key,
             SUEnableAutomaticChecks=True, SUAutomaticallyUpdate=True,
             SUScheduledCheckInterval=86400, SUVerifyUpdateBeforeExtraction=True)
else:
    # A copy built from source never updates itself: no Sparkle feed or key
    # may reach the bundle.
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
# Inside out, one piece at a time, never `codesign --deep`: that signs every
# nested piece the same way, which for a Developer ID build would leave
# Sparkle's helpers without their hardened runtime and entitlements (Sparkle's
# installer logs "do not sign your app by passing --deep"). Sparkle's XPC
# services keep their own entitlements; Autoupdate's application identifier
# is dropped on purpose, since it must not survive into another team's
# signature.
SIGN=(codesign --force --sign "${SIGN_IDENTITY:--}")
if [[ -n "${SIGN_KEYCHAIN:-}" ]]; then SIGN+=(--keychain "$SIGN_KEYCHAIN"); fi
if [[ $HARDENED -eq 1 ]]; then SIGN+=(--options runtime --timestamp); fi
# Every piece arrives signed (Sparkle's by its build, the Swift library by
# Apple), so codesign would say "replacing existing signature" for each.
sign() {
    local log status=0
    log="$("${SIGN[@]}" "$@" 2>&1)" || status=$?
    if [[ -n "$log" ]]; then grep -v ': replacing existing signature$' <<< "$log" >&2 || true; fi
    return $status
}
SPARKLE="$(cd "$C/Frameworks/Sparkle.framework/Versions/Current" && pwd -P)"
for xpc in "$SPARKLE"/XPCServices/*.xpc; do
    if [[ -e "$xpc" ]]; then sign --preserve-metadata=entitlements "$xpc"; fi
done
for helper in "$SPARKLE/Autoupdate" "$SPARKLE/Updater.app"; do
    if [[ -e "$helper" ]]; then sign "$helper"; fi
done
sign "$C/Frameworks/Sparkle.framework"
for lib in "$C"/Frameworks/*.dylib; do
    if [[ -e "$lib" ]]; then sign "$lib"; fi
done
# The app drives terminals with Apple Events (tab focus, typed replies); under
# the hardened runtime that needs its entitlement, or every event is refused.
if [[ $HARDENED -eq 1 ]]; then
    sign --entitlements "$ROOT/Scripts/AgentNotch.entitlements" "$APP"
else
    sign "$APP"
fi
codesign --verify --deep --strict "$APP"
BUILT=1
echo "built: $APP ($VERSION, $EXE_ARCHS)"
