#!/usr/bin/env bash
# Build a release of Agent Notch: everything the Release workflow does
# (.github/workflows/release.yml) except publishing it, so the whole pipeline
# can be tried on a Mac with a throwaway key.
#
#   Scripts/release-build.sh --ed-key-file <file>          # -> build/release/
#   Scripts/release-build.sh --out <dir> --ed-key-file <file> [--skip-build] [--host-arch]
#
# Writes into --out (default build/release):
#   AgentNotch-<VERSION>.dmg  the download: the app beside an Applications link
#   AgentNotch-<VERSION>.zip  the update Sparkle installs (made with ditto, which
#                             keeps Sparkle.framework's symlinks; zip -r does not)
#   appcast.xml               the feed: one item, that zip, with its EdDSA signature
#   release-info.env          what the workflow's release notes need to know
#   app/Agent Notch.app       the app. Never open it: it has the real bundle id
#                             and the feed, so it would update itself.
#
# Options:
#   --ed-key-file <file>  the private key (base64 of the 32-byte seed), or "-"
#                         for stdin; without it, SPARKLE_ED_PRIVATE_KEY
#   --skip-build          re-assemble from the last build (spm-build-app.sh)
#   --host-arch           this Mac's architecture only, for a toolchain that
#                         cannot build x86_64 (the Command Line Tools for
#                         Swift 6.4). The feed then says arm64 only. Refused on
#                         GitHub Actions: a release is universal.
#
# Environment (the workflow's secrets; all optional but the key):
#   SPARKLE_ED_PRIVATE_KEY           the private key, when --ed-key-file isn't given
#   MACOS_SIGNING_P12_BASE64, MACOS_SIGNING_P12_PASSWORD
#                                    a code signing identity (Developer ID, or the
#                                    self-signed one release-make-keys.sh makes),
#                                    imported into a temporary keychain that is
#                                    deleted afterwards; without it, ad hoc
#   APPLE_NOTARY_API_KEY_P8_BASE64, APPLE_NOTARY_API_KEY_ID, APPLE_NOTARY_API_ISSUER_ID
#                                    notarization (App Store Connect API key), for a
#                                    Developer ID identity only: the app is signed
#                                    for the hardened runtime, notarized and stapled,
#                                    then the dmg
#   AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE  the public key (default Scripts/sparkle-public-ed-key.txt)
#
# Nothing is written for a feed unless the private key belongs to the
# committed public key and the built app carries exactly the release's
# Info.plist: a feed signed with another key, or an app that reads another
# feed, would strand every install. The private key is only ever passed on
# stdin, never on a command line or to the build.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BASE_ID="com.rivantmedia.agentnotch"
REPO_URL="https://github.com/rivantmedia/agentnotch"
FEED_URL="$REPO_URL/releases/latest/download/appcast.xml"
MIN_MACOS="15.0"
OUT="$ROOT/build/release"
ED_KEY_FILE=""
SKIP_BUILD=0
HOST_ARCH=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out)         OUT="$2"; shift ;;
        --ed-key-file) ED_KEY_FILE="$2"; shift ;;
        --skip-build)  SKIP_BUILD=1 ;;
        --host-arch)   HOST_ARCH=1 ;;
        -h|--help)     sed -n '/^set -euo pipefail/q;2,$p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done
[[ "$OUT" = /* ]] || OUT="$ROOT/$OUT"
fail() { echo "release-build: $*" >&2; exit 1; }

VERSION="$(<"$ROOT/VERSION")" || fail "no VERSION file"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "VERSION must be one line like 1.0.0, not '$VERSION'"
TAG="agentnotch-v$VERSION"
DMG="$OUT/AgentNotch-$VERSION.dmg"
ZIP="$OUT/AgentNotch-$VERSION.zip"
APPCAST="$OUT/appcast.xml"
APP="$OUT/app/Agent Notch.app"
# A run that stops early leaves no feed behind, not even a previous run's.
rm -f "$DMG" "$ZIP" "$APPCAST" "$OUT/release-info.env"

if [[ $HOST_ARCH -eq 1 && -n "${GITHUB_ACTIONS:-}" ]]; then
    fail "--host-arch is for trying the pipeline locally; a release is universal"
fi

CLT=/Library/Developer/CommandLineTools
if [[ -z "${SDKROOT:-}" && "$(xcode-select -p 2>/dev/null)" == "$CLT"* ]]; then
    SDK=$(ls -d "$CLT"/SDKs/MacOSX26.*.sdk 2>/dev/null | sort -V | tail -1 || true)
    [[ -n "$SDK" ]] && export SDKROOT="$SDK"
fi
ed25519() { swift "$ROOT/Scripts/release-ed25519.swift" "$@"; }

WORK="$(mktemp -d)"
KEYCHAIN=""
ORIGINAL_KEYCHAINS=()
cleanup() {
    if [[ -n "$KEYCHAIN" ]]; then
        if [[ ${#ORIGINAL_KEYCHAINS[@]} -gt 0 ]]; then
            security list-keychains -d user -s "${ORIGINAL_KEYCHAINS[@]}" || true
        fi
        security delete-keychain "$KEYCHAIN" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# --- 1. The update key ---------------------------------------------------------
# Read once, then gone from the environment, so nothing started below (the
# build above all) inherits it.
if [[ -n "$ED_KEY_FILE" ]]; then
    [[ "$ED_KEY_FILE" == "-" || -r "$ED_KEY_FILE" ]] || fail "cannot read the key file $ED_KEY_FILE"
    if [[ "$ED_KEY_FILE" == "-" ]]; then KEY="$(cat)"; else KEY="$(<"$ED_KEY_FILE")"; fi
else
    KEY="${SPARKLE_ED_PRIVATE_KEY:-}"
fi
unset SPARKLE_ED_PRIVATE_KEY
KEY="${KEY//[[:space:]]/}"
[[ -n "$KEY" ]] || fail "no update signing key: pass --ed-key-file, or set SPARKLE_ED_PRIVATE_KEY (Scripts/release-make-keys.sh --update-key makes one)"

KEY_FILE="${AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE:-$ROOT/Scripts/sparkle-public-ed-key.txt}"
export AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE="$KEY_FILE"
PUBLIC_KEY="$(python3 - "$KEY_FILE" <<'PY'
import base64, binascii, sys
path = sys.argv[1]
try:
    lines = open(path, encoding="utf-8").read().splitlines()
except OSError as error:
    sys.exit(f"release-build: cannot read the public key file {path}: {error.strerror}")
key = next((l.strip() for l in lines if l.strip() and not l.strip().startswith("#")), None)
if key is None:
    sys.exit(f"release-build: {path} holds no public key; run Scripts/release-make-keys.sh --update-key <dir> and commit it")
try:
    raw = base64.b64decode(key, validate=True)
except (binascii.Error, ValueError):
    raw = b""
if len(raw) != 32:
    sys.exit(f"release-build: the key in {path} is not the base64 of a 32-byte Ed25519 public key")
print(key)
PY
)" || exit 1
DERIVED="$(printf '%s\n' "$KEY" | ed25519 public)" || fail "the update signing key is not a base64 Ed25519 seed (32 bytes)"
if [[ "$DERIVED" != "$PUBLIC_KEY" ]]; then
    fail "the update signing key's public key ($DERIVED) is not the one in $KEY_FILE ($PUBLIC_KEY): installed copies would refuse every update signed with it"
fi
echo "update key: matches $KEY_FILE"

# --- 2. Code signing identity --------------------------------------------------
NOTARIZE=0
if [[ -n "${APPLE_NOTARY_API_KEY_P8_BASE64:-}${APPLE_NOTARY_API_KEY_ID:-}${APPLE_NOTARY_API_ISSUER_ID:-}" ]]; then
    [[ -n "${APPLE_NOTARY_API_KEY_P8_BASE64:-}" && -n "${APPLE_NOTARY_API_KEY_ID:-}" && -n "${APPLE_NOTARY_API_ISSUER_ID:-}" ]] \
        || fail "notarization needs all three of APPLE_NOTARY_API_KEY_P8_BASE64, APPLE_NOTARY_API_KEY_ID and APPLE_NOTARY_API_ISSUER_ID"
    NOTARIZE=1
    P8="$WORK/notary-key.p8"
    (umask 077; printf '%s' "$APPLE_NOTARY_API_KEY_P8_BASE64" | base64 --decode > "$P8")
    NOTARY=(--key "$P8" --key-id "$APPLE_NOTARY_API_KEY_ID" --issuer "$APPLE_NOTARY_API_ISSUER_ID")
fi
unset APPLE_NOTARY_API_KEY_P8_BASE64

IDENTITY="${SIGN_IDENTITY:--}"
IDENTITY_NAME="ad hoc"
if [[ "$IDENTITY" != "-" ]]; then IDENTITY_NAME="$IDENTITY"; fi
if [[ -n "${MACOS_SIGNING_P12_BASE64:-}" ]]; then
    # A keychain of its own, unlocked for this run only: codesign may use the
    # key without a prompt (it is pointed at the keychain with --keychain),
    # and nothing is left in the login keychain.
    KEYCHAIN="$WORK/agentnotch-release.keychain-db"
    KEYCHAIN_PASSWORD="$(openssl rand -hex 24)"
    P12="$WORK/signing.p12"
    (umask 077; printf '%s' "$MACOS_SIGNING_P12_BASE64" | base64 --decode > "$P12")
    unset MACOS_SIGNING_P12_BASE64
    security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
    security set-keychain-settings -lut 21600 "$KEYCHAIN"
    security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
    security import "$P12" -k "$KEYCHAIN" -f pkcs12 -P "${MACOS_SIGNING_P12_PASSWORD:-}" -T /usr/bin/codesign >/dev/null \
        || fail "cannot import the signing identity (MACOS_SIGNING_P12_BASE64 / MACOS_SIGNING_P12_PASSWORD)"
    rm -f "$P12"
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null
    # On a runner the keychain also joins the search list, which codesign
    # builds the certificate chain from (a Developer ID p12 carries Apple's
    # intermediate certificate), and the list is put back afterwards. On a
    # Mac of yours the list is left alone: its login keychain already has the
    # intermediates a Developer ID needs, and a self-signed identity has none.
    if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
        while IFS= read -r line; do
            line="${line#"${line%%[![:space:]]*}"}"; line="${line#\"}"; line="${line%\"}"
            if [[ -n "$line" ]]; then ORIGINAL_KEYCHAINS+=("$line"); fi
        done < <(security list-keychains -d user)
        security list-keychains -d user -s "$KEYCHAIN" ${ORIGINAL_KEYCHAINS[@]+"${ORIGINAL_KEYCHAINS[@]}"}
    fi
    # Any code signing identity, trusted or not: a self-signed one is never
    # "valid" to find-identity -v, and codesign signs with it all the same.
    FOUND="$(security find-identity -p codesigning "$KEYCHAIN" | awk '$1 == "1)" && !found { print; found = 1 }')"
    [[ -n "$FOUND" ]] || fail "the signing identity holds no code signing certificate and key"
    IDENTITY="$(awk '{ print $2 }' <<< "$FOUND")"
    IDENTITY_NAME="$(sed -E 's/^[^"]*"([^"]*)".*/\1/' <<< "$FOUND")"
    export SIGN_KEYCHAIN="$KEYCHAIN"
fi
unset MACOS_SIGNING_P12_PASSWORD
if [[ $NOTARIZE -eq 1 && "$IDENTITY_NAME" != "Developer ID Application:"* ]]; then
    fail "notarization needs a Developer ID Application identity, not '$IDENTITY_NAME'"
fi
echo "signing: $IDENTITY_NAME$([[ $NOTARIZE -eq 1 ]] && echo ", notarized")"

# --- 3. Build ----------------------------------------------------------------------
rm -rf "$OUT/app"
mkdir -p "$OUT"
touch "$OUT/.metadata_never_index"
BUILD=(--release --with-updates --out "$OUT/app")
if [[ $HOST_ARCH -eq 0 ]]; then BUILD+=(--universal); fi
if [[ $SKIP_BUILD -eq 1 ]]; then BUILD+=(--skip-build); fi
if [[ $NOTARIZE -eq 1 ]]; then BUILD+=(--hardened-runtime); fi
SIGN_IDENTITY="$IDENTITY" "$ROOT/Scripts/spm-build-app.sh" "${BUILD[@]}"

# --- 4. Check what was built -------------------------------------------------------
C="$APP/Contents"
EXE="$C/MacOS/Agent Notch"
python3 - "$C/Info.plist" "$BASE_ID" "$VERSION" "$MIN_MACOS" "$FEED_URL" "$PUBLIC_KEY" <<'PY' || exit 1
import plistlib, sys
path, bundle_id, version, min_macos, feed, key = sys.argv[1:7]
d = plistlib.load(open(path, "rb"))
want = {
    "CFBundleIdentifier": bundle_id, "CFBundleExecutable": "Agent Notch",
    "CFBundleShortVersionString": version, "CFBundleVersion": version,
    "LSMinimumSystemVersion": min_macos,
    "SUFeedURL": feed, "SUPublicEDKey": key,
    "SUEnableAutomaticChecks": True, "SUAutomaticallyUpdate": True,
    "SUScheduledCheckInterval": 86400, "SUVerifyUpdateBeforeExtraction": True,
}
wrong = [f"{k}={d.get(k)!r} (want {v!r})" for k, v in want.items()
         if d.get(k) != v or type(d.get(k)) is not type(v)]
# Anything else of Sparkle's (an installer service, a signed-feed demand) would
# change how every install updates; only what is asked for above may be there.
extra = sorted(k for k in d if k.startswith("SU") and k not in want)
if wrong or extra:
    sys.exit("release-build: the built Info.plist is not a release's: " + "; ".join(wrong + [f"unexpected {k}" for k in extra]))
print("Info.plist: release keys as expected")
PY

ARCHS="$(lipo -archs "$EXE")"
if [[ $HOST_ARCH -eq 0 ]]; then
    for arch in arm64 x86_64; do
        [[ " $ARCHS " == *" $arch "* ]] || fail "the executable is '$ARCHS', with no $arch"
    done
fi

codesign --verify --deep --strict --verbose=2 "$APP" 2>&1 | tail -2
# get-task-allow lets any process read the app's memory; only a debugging
# signature may carry it.
if grep -q get-task-allow <<< "$(codesign -d --entitlements - --xml "$APP" 2>/dev/null)"; then
    fail "the app's signature carries get-task-allow"
fi
# One signer for everything inside, or library validation (hardened runtime)
# and Sparkle's installer connection (same team) refuse the pieces. (Output
# is read whole here and below: a reader that stops early would end the
# writer with SIGPIPE, which pipefail turns into a failure.)
signer() { codesign -dvv "$1" 2>&1 | awk -F= '!s && /^Authority=/ { s = $2 } !s && /^Signature=adhoc/ { s = "adhoc" } END { print s }'; }
APP_SIGNER="$(signer "$APP")"
SPARKLE="$(cd "$C/Frameworks/Sparkle.framework/Versions/Current" && pwd -P)"
for piece in "$SPARKLE"/XPCServices/*.xpc "$SPARKLE/Autoupdate" "$SPARKLE/Updater.app" "$C/Frameworks/Sparkle.framework" "$C"/Frameworks/*.dylib; do
    [[ -e "$piece" ]] || continue
    [[ "$(signer "$piece")" == "$APP_SIGNER" ]] || fail "${piece#"$C/"} is signed by '$(signer "$piece")', the app by '$APP_SIGNER'"
    if [[ $NOTARIZE -eq 1 ]] && ! grep -q 'flags=.*runtime' <<< "$(codesign -dv "$piece" 2>&1)"; then
        fail "${piece#"$C/"} is not signed for the hardened runtime"
    fi
done

# Loads on the oldest macOS it claims: every architecture targets it, every
# @rpath library is in the bundle for every architecture and targets it too,
# and nothing points into the build machine. A binary that targets a macOS
# before 10.14 says so with LC_VERSION_MIN_MACOSX instead of
# LC_BUILD_VERSION, as the toolchain's older back-deployment libraries do.
macos_minos() { otool -arch "$1" -l "$2" | awk '
    /cmd LC_BUILD_VERSION/ { b = 1; next }
    /cmd LC_VERSION_MIN_MACOSX/ { v = 1; next }
    b && $1 == "platform" { p = $2 }
    b && $1 == "minos" { if ((p == "1" || p == "MACOS") && !m) m = $2; b = 0 }
    v && $1 == "version" { if (!m) m = $2; v = 0 }
    END { print m }'; }
older_or_same() { [[ -n "$1" && "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" == "$2" ]]; }
for arch in $ARCHS; do
    minos="$(macos_minos "$arch" "$EXE")"
    older_or_same "$minos" "$MIN_MACOS" || fail "the $arch executable needs macOS ${minos:-(unknown)}, not $MIN_MACOS"
done
if otool -arch all -l "$EXE" | awk '/cmd LC_RPATH/ { r = 1; next } r && $1 == "path" { print $2; r = 0 }' | grep -E '^/' | grep -vE '^/(usr/lib|System)/'; then
    fail "the executable searches the build machine's folders for libraries (above)"
fi
while IFS= read -r dep; do
    [[ -n "$dep" ]] || continue
    [[ -e "$C/Frameworks/$dep" ]] || fail "the app loads @rpath/$dep, which is not in the bundle"
    if [[ "$dep" == *.dylib ]]; then
        for arch in $ARCHS; do
            [[ " $(lipo -archs "$C/Frameworks/$dep") " == *" $arch "* ]] || fail "$dep has no $arch slice"
            minos="$(macos_minos "$arch" "$C/Frameworks/$dep")"
            older_or_same "$minos" "$MIN_MACOS" || fail "$dep ($arch) needs macOS ${minos:-(unknown)}"
            # otool -L lists the library's own install name first, which
            # may be @rpath/<itself>; only what it loads has to be the system's.
            own="$(otool -arch "$arch" -D "$C/Frameworks/$dep" | tail -n +2)"
            if otool -arch "$arch" -L "$C/Frameworks/$dep" | awk -v own="$own" '/^\t/ && $1 != own { print $1 }' | grep -vE '^/(usr/lib|System)/'; then
                fail "$dep ($arch) loads something outside the system (above)"
            fi
        done
    fi
done < <(otool -arch all -L "$EXE" | awk '$1 ~ /^@rpath\// { print substr($1, 8) }' | sort -u)
echo "app: $VERSION, $ARCHS, macOS $MIN_MACOS+, signed $APP_SIGNER"

# --- 5. Notarize the app -------------------------------------------------------------
notarize() {
    local result status id
    result="$(xcrun notarytool submit "$1" "${NOTARY[@]}" --wait --timeout 1h --output-format json)" || true
    status="$(python3 -c 'import json, sys; print(json.load(sys.stdin).get("status", ""))' <<< "$result" 2>/dev/null || true)"
    if [[ "$status" != "Accepted" ]]; then
        id="$(python3 -c 'import json, sys; print(json.load(sys.stdin).get("id", ""))' <<< "$result" 2>/dev/null || true)"
        echo "$result" >&2
        [[ -z "$id" ]] || xcrun notarytool log "$id" "${NOTARY[@]}" >&2 || true
        fail "notarization of $(basename "$1") ended '${status:-without an answer}'"
    fi
    echo "notarized: $(basename "$1")"
}
if [[ $NOTARIZE -eq 1 ]]; then
    ditto -c -k --keepParent "$APP" "$WORK/notarize-app.zip"
    notarize "$WORK/notarize-app.zip"
    xcrun stapler staple "$APP"
    xcrun stapler validate "$APP"
fi

# --- 6. Disk image -------------------------------------------------------------------
# Runners intermittently fail hdiutil with "Resource busy"; a retry clears it.
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/Agent Notch.app"
ln -s /Applications "$STAGE/Applications"
for attempt in 1 2 3; do
    if hdiutil create -quiet -volname "Agent Notch" -srcfolder "$STAGE" -ov -format UDZO "$DMG"; then break; fi
    [[ $attempt -lt 3 ]] || fail "hdiutil could not create the disk image"
    sleep 3
done
hdiutil verify -quiet "$DMG"
# A Developer ID signs the disk image too (Gatekeeper checks it on open); an
# ad hoc or self-signed signature would only make it look broken.
if [[ "$IDENTITY_NAME" == "Developer ID Application:"* ]]; then
    # --keychain only for the imported identity: a SIGN_IDENTITY of yours is
    # found on the search list.
    DMG_SIGN=(codesign --force --sign "$IDENTITY" --timestamp)
    if [[ -n "$KEYCHAIN" ]]; then DMG_SIGN+=(--keychain "$KEYCHAIN"); fi
    "${DMG_SIGN[@]}" "$DMG"
fi
if [[ $NOTARIZE -eq 1 ]]; then
    notarize "$DMG"
    xcrun stapler staple "$DMG"
fi
echo "disk image: $DMG"

# --- 7. The update archive and its signature -----------------------------------------
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
# What Sparkle will unpack has to be the same, intact app.
ditto -x -k "$ZIP" "$WORK/unzipped"
codesign --verify --deep --strict "$WORK/unzipped/Agent Notch.app" \
    || fail "the app unpacked from $ZIP does not verify"
SIGN_UPDATE="$ROOT/.build/artifacts/sparkle/Sparkle/bin/sign_update"
if [[ ! -x "$SIGN_UPDATE" ]]; then
    SIGN_UPDATE="$(find "$ROOT/.build/artifacts" -type f -name sign_update -path '*/bin/*' -print -quit 2>/dev/null || true)"
fi
[[ -n "$SIGN_UPDATE" && -x "$SIGN_UPDATE" ]] || fail "no Sparkle sign_update under .build/artifacts (built without SwiftPM?)"
SIGNATURE="$(printf '%s\n' "$KEY" | "$SIGN_UPDATE" --ed-key-file - -p "$ZIP")" || fail "sign_update failed"
unset KEY
ed25519 verify "$PUBLIC_KEY" "$ZIP" "$SIGNATURE" >/dev/null \
    || fail "the update's signature does not verify with the committed public key"
LENGTH="$(stat -f%z "$ZIP")"
echo "update: $ZIP ($LENGTH bytes), signature verified with $KEY_FILE"

# --- 8. The feed ---------------------------------------------------------------------
# One item: installs only ever need the newest version, and the enclosure is
# the zip in this tag's own release (latest/download would redirect to
# whatever release is newest when the download starts, which a signature
# made for this zip might not match).
HARDWARE=""
[[ " $ARCHS " == *" x86_64 "* ]] || HARDWARE="arm64"
python3 - "$APPCAST" "$VERSION" "$MIN_MACOS" "$REPO_URL" "$TAG" "$(basename "$ZIP")" "$LENGTH" "$SIGNATURE" "$HARDWARE" <<'PY'
import email.utils, sys
import xml.etree.ElementTree as ET
path, version, min_macos, repo, tag, zip_name, length, signature, hardware = sys.argv[1:10]
SPARKLE = "http://www.andymatuschak.org/xml-namespaces/sparkle"
ET.register_namespace("sparkle", SPARKLE)
def s(name): return f"{{{SPARKLE}}}{name}"
rss = ET.Element("rss", {"version": "2.0"})
channel = ET.SubElement(rss, "channel")
ET.SubElement(channel, "title").text = "Agent Notch"
ET.SubElement(channel, "link").text = f"{repo}/releases"
ET.SubElement(channel, "description").text = "Agent Notch updates"
ET.SubElement(channel, "language").text = "en"
item = ET.SubElement(channel, "item")
ET.SubElement(item, "title").text = f"Agent Notch {version}"
ET.SubElement(item, "pubDate").text = email.utils.formatdate(usegmt=True)
ET.SubElement(item, s("version")).text = version
ET.SubElement(item, s("shortVersionString")).text = version
ET.SubElement(item, s("minimumSystemVersion")).text = min_macos
if hardware:
    ET.SubElement(item, s("hardwareRequirements")).text = hardware
ET.SubElement(item, s("releaseNotesLink")).text = f"{repo}/releases/tag/{tag}"
ET.SubElement(item, "enclosure", {
    "url": f"{repo}/releases/download/{tag}/{zip_name}",
    "length": length,
    "type": "application/octet-stream",
    s("edSignature"): signature,
})
ET.indent(rss, space="    ")
ET.ElementTree(rss).write(path, encoding="utf-8", xml_declaration=True)

# Read back as Sparkle will.
item = ET.parse(path).getroot().find("channel/item")
enclosure = item.find("enclosure")
checks = {
    "sparkle:version": item.findtext(s("version")) == version,
    "enclosure url": enclosure.get("url") == f"{repo}/releases/download/{tag}/{zip_name}",
    "enclosure length": enclosure.get("length") == length,
    "edSignature": enclosure.get(s("edSignature")) == signature,
}
bad = [k for k, ok in checks.items() if not ok]
if bad:
    sys.exit(f"release-build: appcast.xml does not read back: {bad}")
PY
echo "feed: $APPCAST"

SIGNED_WITH="adhoc"
if [[ "$IDENTITY_NAME" == "Developer ID Application:"* ]]; then SIGNED_WITH="developer-id"
elif [[ "$IDENTITY" != "-" ]]; then SIGNED_WITH="identity"; fi
cat > "$OUT/release-info.env" <<EOF
VERSION=$VERSION
TAG=$TAG
DMG=$(basename "$DMG")
ZIP=$(basename "$ZIP")
ARCHS=${ARCHS// /,}
SIGNED_WITH=$SIGNED_WITH
NOTARIZED=$NOTARIZE
EOF
echo "release $VERSION ready in $OUT$([[ -n "$HARDWARE" ]] && echo " (arm64 only: for trying the pipeline, not for publishing)")"
