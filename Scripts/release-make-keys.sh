#!/usr/bin/env bash
# Make the keys the Release workflow needs. The maintainer runs this once, on
# their own Mac; it prints the commands that make the repository's `release`
# environment (deployable from main only) and set the keys as its secrets,
# and runs them only when asked to (--set-secrets).
#
#   Scripts/release-make-keys.sh --update-key ~/agentnotch-keys
#   Scripts/release-make-keys.sh --signing-cert ~/agentnotch-keys
#   Scripts/release-make-keys.sh --update-key ~/agentnotch-keys --signing-cert ~/agentnotch-keys --set-secrets
#
# --update-key <dir> [--rotate]
#     The Ed25519 key pair Sparkle checks every update with (CryptoKit, never
#     the Keychain). The private key goes into <dir>/sparkle-ed-private-key.txt
#     (0600, never printed) and becomes the SPARKLE_ED_PRIVATE_KEY secret; the
#     public key goes into Scripts/sparkle-public-ed-key.txt, to be committed.
#     Keep a copy of the private key somewhere safe: installs accept only
#     updates signed with it, so losing it strands every one of them.
#     An existing public key is replaced only with --rotate, and rotating
#     strands EVERY install, however it is signed (ad hoc, self-signed or
#     Developer ID). Release builds set SUVerifyUpdateBeforeExtraction, so
#     Sparkle checks the downloaded zip's EdDSA signature against the key
#     built into the installed app before it unpacks anything; its only
#     fallback needs the archive itself to carry a Developer ID signature of
#     the installed app's team, and a zip never does. Every installed copy
#     stops updating and has to be reinstalled by hand, and the Release
#     workflow refuses to publish with a key other than the latest release's
#     unless a manual run says so. Back up the private key; never lose it.
#     The same key signs the Windows updates (the Windows key is derived from
#     it), so a rotation strands every Windows copy too, unless one bridge
#     release (carrying the new key, signed with the old key's derivative)
#     goes out first. Release can't make one yet: keep the old key (as
#     SPARKLE_ED_PRIVATE_KEY_PREVIOUS) and add that signing path before rotating.
#
# --signing-cert <dir> [--cert-name <name>]
#     Optional, for releases without a Developer ID: a self-signed code signing
#     certificate (RSA, extended key usage codeSigning) and its key, as
#     <dir>/agentnotch-signing.p12 with a random password, for the
#     MACOS_SIGNING_P12_BASE64 and MACOS_SIGNING_P12_PASSWORD secrets. Every
#     release is then signed by the same identity, so macOS keeps the app's
#     Automation permission (terminal tabs) and keychain "Always Allow" grants
#     across updates, which an ad-hoc signature loses each time. Gatekeeper
#     still asks on the first open: only a notarized Developer ID build skips that.
#
# --set-secrets
#     Also run the printed commands (needs gh, signed in with admin access to
#     the repository): make the `release` environment of rivantmedia/agentnotch
#     with main as its only deployment branch, unless it has that already, then
#     `gh secret set … --env release`. The secrets are the environment's, not
#     the repository's: a repository secret reaches a workflow run from any
#     branch. A repository-level copy of one is reported, never deleted here.
#
# <dir> must be outside this repository, so a key can never be committed by
# accident. Nothing is ever overwritten: a file that exists is a refusal.
#
# Environment:
#   AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE  the public key file to write (default
#                                      Scripts/sparkle-public-ed-key.txt); tests
#                                      point it at a scratch file
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="rivantmedia/agentnotch"
UPDATE_DIR=""
CERT_DIR=""
CERT_NAME="Agent Notch Release Signing"
ROTATE=0
SET_SECRETS=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --update-key)   UPDATE_DIR="$2"; shift ;;
        --signing-cert) CERT_DIR="$2"; shift ;;
        --cert-name)    CERT_NAME="$2"; shift ;;
        --rotate)       ROTATE=1 ;;
        --set-secrets)  SET_SECRETS=1 ;;
        -h|--help)      sed -n '/^set -euo pipefail/q;2,$p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done
fail() { echo "release-make-keys: $*" >&2; exit 1; }
[[ -n "$UPDATE_DIR$CERT_DIR" ]] || fail "nothing to make: pass --update-key <dir> and/or --signing-cert <dir> (see --help)"
KEY_FILE="${AGENTNOTCH_UPDATE_PUBLIC_KEY_FILE:-$ROOT/Scripts/sparkle-public-ed-key.txt}"

CLT=/Library/Developer/CommandLineTools
if [[ -z "${SDKROOT:-}" && "$(xcode-select -p 2>/dev/null)" == "$CLT"* ]]; then
    SDK=$(ls -d "$CLT"/SDKs/MacOSX26.*.sdk 2>/dev/null | sort -V | tail -1 || true)
    [[ -n "$SDK" ]] && export SDKROOT="$SDK"
fi

# A folder for secrets: outside the repository (checked on the real path, so
# a symlink into it counts). make_folder creates it 0700 once every check
# has passed.
key_folder() {
    local dir root
    dir="$(python3 -c 'import os, sys; print(os.path.realpath(os.path.expanduser(sys.argv[1])))' "$1")"
    root="$(cd "$ROOT" && pwd -P)"
    if [[ "$dir" == "$root" || "$dir" == "$root/"* ]]; then
        fail "$1 is inside the repository; keep keys outside it"
    fi
    echo "$dir"
}
make_folder() {
    if [[ ! -d "$1" ]]; then
        (umask 077; mkdir -p "$1")
    fi
}
# The key in the public key file, or nothing (comments and blank lines skipped).
committed_key() {
    [[ -f "$KEY_FILE" ]] || return 0
    awk '!/^[[:space:]]*(#|$)/ { gsub(/[[:space:]]/, ""); print; exit }' "$KEY_FILE"
}

SECRETS=()   # "<name> <file>"

# Every refusal comes before anything is made, so a refused run leaves no
# half of its work behind (a new update key without its certificate, say).
if [[ -n "$UPDATE_DIR" ]]; then
    UPDATE_DIR="$(key_folder "$UPDATE_DIR")"
    SEED="$UPDATE_DIR/sparkle-ed-private-key.txt"
    [[ ! -e "$SEED" ]] || fail "$SEED exists already; it is never overwritten"
    OLD="$(committed_key)"
    if [[ -n "$OLD" && $ROTATE -eq 0 ]]; then
        fail "$KEY_FILE holds a key already ($OLD). Replacing it strands every installed copy, however it is signed (release builds verify the update with the installed app's key before unpacking it); they would have to be reinstalled by hand. The same key also signs the Windows updates (a key derived from it), so every installed Windows copy would be stranded too, unless a bridge release signed with the old key's derivative goes out first, which the Release workflow can't make yet. Pass --rotate if that is really meant."
    fi
fi
if [[ -n "$CERT_DIR" ]]; then
    CERT_DIR="$(key_folder "$CERT_DIR")"
    P12="$CERT_DIR/agentnotch-signing.p12"
    P12_BASE64="$CERT_DIR/agentnotch-signing.p12.base64.txt"
    P12_PASSWORD="$CERT_DIR/agentnotch-signing.p12.password.txt"
    CERT_PEM="$CERT_DIR/agentnotch-signing-certificate.pem"
    for file in "$P12" "$P12_BASE64" "$P12_PASSWORD" "$CERT_PEM"; do
        [[ ! -e "$file" ]] || fail "$file exists already; it is never overwritten"
    done
fi

# --- Update signing key --------------------------------------------------------
if [[ -n "$UPDATE_DIR" ]]; then
    make_folder "$UPDATE_DIR"
    PUBLIC="$(swift "$ROOT/Scripts/release-ed25519.swift" generate "$SEED")"
    python3 - "$KEY_FILE" "$PUBLIC" <<'PY'
import os, sys
path, key = sys.argv[1:3]
try:
    lines = open(path, encoding="utf-8").read().splitlines()
except FileNotFoundError:
    lines = ["# Agent Notch's update signing key (SUPublicEDKey): the base64 of the",
             "# 32-byte Ed25519 public key. Lines starting with # are ignored."]
# The header stays; the old key, if any, is the first line that isn't one.
kept, replaced = [], False
for line in lines:
    stripped = line.strip()
    if stripped and not stripped.startswith("#") and not replaced:
        replaced = True
        continue
    kept.append(line)
while kept and not kept[-1].strip():
    kept.pop()
tmp = path + ".tmp"
with open(tmp, "w", encoding="utf-8") as f:
    f.write("\n".join(kept + [key]) + "\n")
os.replace(tmp, path)
PY
    [[ "$(committed_key)" == "$PUBLIC" ]] || fail "could not write the public key into $KEY_FILE"
    echo "update key:"
    echo "  private  $SEED (keep a copy somewhere safe; never commit it)"
    echo "  public   $PUBLIC -> $KEY_FILE (commit this file)"
    if [[ -n "$OLD" ]]; then
        echo "  ROTATED: every installed copy keeps the old key ($OLD) and will not take updates signed with the new one; each has to be reinstalled by hand. Windows copies verify their updates with a key derived from the old one: keep the OLD private key (as SPARKLE_ED_PRIVATE_KEY_PREVIOUS); only a bridge release signed with its derivative and carrying the new key reaches them, and the Release workflow can't make one yet. Without it every installed Windows copy is stranded as well."
    fi
    SECRETS+=("SPARKLE_ED_PRIVATE_KEY $SEED")
fi

# --- Self-signed code signing identity ------------------------------------------
if [[ -n "$CERT_DIR" ]]; then
    make_folder "$CERT_DIR"
    # The system's LibreSSL, on purpose: its PKCS#12 uses the algorithms
    # `security import` has always read (OpenSSL 3's defaults need -legacy).
    OPENSSL=/usr/bin/openssl
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
    cat > "$WORK/certificate.cnf" <<EOF
[ req ]
distinguished_name = subject
x509_extensions = codesigning
prompt = no
[ subject ]
CN = $CERT_NAME
[ codesigning ]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF
    (
        umask 077
        "$OPENSSL" req -new -x509 -newkey rsa:3072 -nodes -sha256 -days 3650 \
            -config "$WORK/certificate.cnf" -keyout "$WORK/key.pem" -out "$CERT_PEM" 2>/dev/null
        "$OPENSSL" rand -base64 24 | tr -d '\n' > "$P12_PASSWORD"
        "$OPENSSL" pkcs12 -export -name "$CERT_NAME" -inkey "$WORK/key.pem" -in "$CERT_PEM" \
            -out "$P12" -passout "file:$P12_PASSWORD"
        base64 -i "$P12" | tr -d '\n' > "$P12_BASE64"
    )
    rm -f "$WORK/key.pem"
    "$OPENSSL" pkcs12 -in "$P12" -passin "file:$P12_PASSWORD" -noout >/dev/null 2>&1 \
        || fail "the new $P12 does not open with its password"
    grep -q 'Code Signing' <<< "$("$OPENSSL" x509 -in "$CERT_PEM" -noout -text)" \
        || fail "the new certificate has no code signing usage"
    echo "signing identity \"$CERT_NAME\" (self-signed, 10 years):"
    echo "  $P12 (password in $P12_PASSWORD)"
    echo "  certificate $CERT_PEM"
    SECRETS+=("MACOS_SIGNING_P12_BASE64 $P12_BASE64" "MACOS_SIGNING_P12_PASSWORD $P12_PASSWORD")
fi

# --- Secrets ---------------------------------------------------------------------
# They are secrets of the `release` environment, whose only deployment branch
# is main. GitHub hands those only to a job that names the environment and runs
# from a branch it allows; a repository secret reaches a run from any branch,
# and so the code of any branch that is pushed.
ENVIRONMENT=release
ENVIRONMENT_JSON='{"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}'
ALL_SECRETS=(SPARKLE_ED_PRIVATE_KEY MACOS_SIGNING_P12_BASE64 MACOS_SIGNING_P12_PASSWORD
             APPLE_NOTARY_API_KEY_P8_BASE64 APPLE_NOTARY_API_KEY_ID APPLE_NOTARY_API_ISSUER_ID)
echo
echo "GitHub Actions secrets for $REPO, in its \"$ENVIRONMENT\" environment (deployable from main only):"
echo "  # the first two once, for an environment that doesn't exist yet or has no branch rule:"
printf "  gh api -X PUT repos/%s/environments/%s --input - <<< '%s'\n" "$REPO" "$ENVIRONMENT" "$ENVIRONMENT_JSON"
printf '  gh api -X POST repos/%s/environments/%s/deployment-branch-policies -f name=main -f type=branch\n' "$REPO" "$ENVIRONMENT"
for entry in "${SECRETS[@]}"; do
    printf '  gh secret set %s --env %s -R %s < %q\n' "${entry%% *}" "$ENVIRONMENT" "$REPO" "${entry#* }"
done
echo "  # and delete any repository-level copy (a run from any branch can read it): gh secret delete <NAME> -R $REPO"
if [[ $SET_SECRETS -eq 1 ]]; then
    command -v gh >/dev/null || fail "gh is not installed; run the commands above yourself"
    # The environment first (`gh secret set --env` needs it), and the secrets
    # only into one that main alone deploys from. One that exists keeps its
    # other rules (a required reviewer, say): it is changed only when it has
    # none, like the one the workflow's first run makes; otherwise the
    # maintainer changes it in Settings.
    state="$(gh api "repos/$REPO/environments" --paginate --jq ".environments[] | select(.name == \"$ENVIRONMENT\")
        | (.deployment_branch_policy | if . == null then \"any\" elif .custom_branch_policies then \"custom\" else \"protected\" end)
          + \" \" + ([.protection_rules[]? | select(.type != \"branch_policy\")] | length | tostring)")" \
        || fail "could not read the environments of $REPO"
    read -r rule others <<< "$state"
    if [[ -z "$state" || ( "$rule" == any && "$others" == 0 ) ]]; then
        gh api -X PUT "repos/$REPO/environments/$ENVIRONMENT" --input - <<< "$ENVIRONMENT_JSON" >/dev/null \
            || fail "could not make the $ENVIRONMENT environment of $REPO"
    elif [[ "$rule" != custom ]]; then
        fail "the $ENVIRONMENT environment of $REPO can be deployed from $([[ "$rule" == any ]] && echo "any branch" || echo "every protected branch"); set its deployment branches to main only in Settings > Environments > $ENVIRONMENT (not changed here, so its other rules stay), then run the gh secret set commands above"
    fi
    policies="$(gh api "repos/$REPO/environments/$ENVIRONMENT/deployment-branch-policies" --paginate \
        --jq '.branch_policies[] | (.type // "branch") + " " + .name')" \
        || fail "could not read the deployment branches of the $ENVIRONMENT environment"
    if [[ -z "$policies" ]]; then
        gh api -X POST "repos/$REPO/environments/$ENVIRONMENT/deployment-branch-policies" -f name=main -f type=branch >/dev/null \
            || fail "could not limit the $ENVIRONMENT environment to main"
    elif [[ "$policies" != "branch main" ]]; then
        fail "the $ENVIRONMENT environment of $REPO deploys from more than main ($(tr '\n' ',' <<< "$policies" | sed 's/,$//; s/,/, /g')); remove the others in Settings > Environments > $ENVIRONMENT, then run the gh secret set commands above"
    fi
    echo "environment \"$ENVIRONMENT\": deployable from main only."
    for entry in "${SECRETS[@]}"; do
        gh secret set "${entry%% *}" --env "$ENVIRONMENT" -R "$REPO" < "${entry#* }"
    done
    echo "secrets set."
    # Deleting a secret is the maintainer's call; this only points at copies
    # that would still reach every branch.
    if repo_secrets="$(gh secret list -R "$REPO" --json name --jq '.[].name')"; then
        for name in "${ALL_SECRETS[@]}"; do
            if grep -qx "$name" <<< "$repo_secrets"; then
                echo "WARNING: $REPO also has $name as a repository secret, which a run from any branch can read. Delete it: gh secret delete $name -R $REPO"
            fi
        done
    else
        echo "WARNING: could not list the repository secrets of $REPO; delete any repository-level copy of these yourself."
    fi
else
    echo "(not run: pass --set-secrets to run them, or run them yourself)"
fi
if [[ -n "$UPDATE_DIR" ]]; then
    echo
    if [[ -n "$OLD" ]]; then
        echo "Then commit $KEY_FILE and push it to main. The Release workflow publishes with a key other than the latest release's only from a manual run with 'Publish although the update key changed' ticked."
    else
        echo "Then, once the secret is set, commit $KEY_FILE and push it to main: that push starts the Release workflow, which publishes the current VERSION unless it is out already."
    fi
fi
