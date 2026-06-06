#!/usr/bin/env bash
# Acquire a Unity *Personal* (free) license file (.ulf) entirely on your local machine, so your
# Unity account credentials never touch a CI runner. Only the resulting .ulf (an expiring, lower-
# value file) gets uploaded as the UNITY_LICENSE secret for the `Unity acceptance` workflow.
#
# Why this exists: Unity retired web-based manual activation for Personal licenses, and there is no
# arm64 Linux Unity Hub, so the usual "generate a .ulf in Unity Hub" route is unavailable here.
# game-ci/unity-license-activate logs into your account headlessly (Chromium has arm64 builds) and
# mints the .ulf from an .alf activation request.
#
# Usage:
#   scripts/acquire-unity-license.sh [path/to/Unity_vX.alf]
#
# Credentials are read from the environment, or prompted for (password is never echoed):
#   UNITY_EMAIL      your Unity account email
#   UNITY_PASSWORD   your Unity account password
#   UNITY_TOTP_KEY   (optional) base32 TOTP secret, if your account has 2FA enabled
#   UNITY_VERSION    (optional) editor version for .alf generation (default: 2022.3.22f1)
#
# The .alf:
#   * Pass one as $1 if you already have it (recommended: download it from an anonymous CI run of
#     `unity-editor -createManualActivationFile` — no secrets are involved in generating an .alf).
#   * Omit it and this script tries to generate one via the amd64 GameCI Docker image under
#     emulation. That needs `docker` plus amd64 binfmt (e.g. `docker run --privileged --rm
#     tonistiigi/binfmt --install amd64`); if unavailable it stops with instructions.
#
# Output: ./unity-personal.ulf (git-ignored). Paste its contents into the UNITY_LICENSE repo secret.

set -euo pipefail

UNITY_VERSION="${UNITY_VERSION:-2022.3.22f1}"
ALF="${1:-}"
OUT="unity-personal.ulf"

command -v npx >/dev/null || { echo "error: npx (Node.js) is required." >&2; exit 1; }

# --- credentials (prompt if absent; never echo the password) ----------------------------------
: "${UNITY_EMAIL:=}"
if [ -z "$UNITY_EMAIL" ]; then read -r -p "Unity account email: " UNITY_EMAIL; fi
: "${UNITY_PASSWORD:=}"
if [ -z "$UNITY_PASSWORD" ]; then read -r -s -p "Unity account password: " UNITY_PASSWORD; echo; fi
: "${UNITY_TOTP_KEY:=}"

# --- obtain an .alf ---------------------------------------------------------------------------
if [ -z "$ALF" ]; then
  echo "No .alf given; attempting to generate one with the amd64 GameCI image under emulation..."
  command -v docker >/dev/null || {
    echo "error: no .alf provided and docker is unavailable." >&2
    echo "Provide one: download a Unity_v*.alf from an anonymous CI run of" >&2
    echo "  unity-editor -batchmode -nographics -quit -createManualActivationFile" >&2
    echo "then re-run: scripts/acquire-unity-license.sh path/to/Unity_v${UNITY_VERSION}.alf" >&2
    exit 1
  }
  work="$(mktemp -d)"
  if ! docker run --rm --platform linux/amd64 -v "$work:/work" -w /work \
        "unityci/editor:${UNITY_VERSION}-base-3" \
        unity-editor -batchmode -nographics -quit -logFile /dev/stdout \
        -createManualActivationFile 2>&1 | tail -n 20; then
    echo "error: amd64 emulation failed (register binfmt: docker run --privileged --rm tonistiigi/binfmt --install amd64)." >&2
    echo "Or supply an .alf from an anonymous CI run and re-run with it as the first argument." >&2
    exit 1
  fi
  ALF="$(find "$work" -name 'Unity_v*.alf' | head -n1)"
  [ -n "$ALF" ] || { echo "error: editor did not produce an .alf." >&2; exit 1; }
  echo "Generated $ALF"
fi

[ -f "$ALF" ] || { echo "error: .alf not found: $ALF" >&2; exit 1; }

# --- mint the .ulf locally (credentials stay on this machine) ---------------------------------
echo "Minting .ulf via unity-license-activate (your credentials are used only locally)..."
args=("$UNITY_EMAIL" "$UNITY_PASSWORD" "$ALF")
[ -n "$UNITY_TOTP_KEY" ] && args+=(--authenticator-key "$UNITY_TOTP_KEY")
# Pin nothing: --yes fetches the latest published tool. Run `npx unity-license-activate --help`
# if a future version changes the argument order.
npx --yes unity-license-activate "${args[@]}"

ulf="$(find . -maxdepth 2 -name '*.ulf' -newer "$ALF" 2>/dev/null | head -n1)"
[ -n "$ulf" ] || ulf="$(find . -maxdepth 2 -name 'Unity_*.ulf' | head -n1)"
[ -n "$ulf" ] || { echo "error: no .ulf was produced (check the login output above)." >&2; exit 1; }

cp "$ulf" "$OUT"
echo
echo "Wrote $OUT (git-ignored). Next:"
echo "  1. Add a repo secret UNITY_LICENSE = the full contents of $OUT"
echo "  2. Add secrets UNITY_EMAIL and UNITY_PASSWORD (required for Personal activation in CI)"
echo "  3. Run the 'Unity acceptance' workflow."
echo "Your email/password never left this machine; only the .ulf does."
