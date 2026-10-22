#!/usr/bin/env bash
# Create the signed updater artifacts for a GitHub release.
#
# Output (in `dist/` by default, all attached to the GitHub Release):
#   Agent-Monitor-macos-arm64.app.tar.gz       updater artifact (the .app, gzipped tar)
#   Agent-Monitor-macos-arm64.app.tar.gz.sig   minisign signature (Tauri CLI)
#   latest.json                                Tauri updater manifest
#
# The tarball must contain `Agent Monitor.app` at its root (the updater strips the
# first path component and renames the extracted directory into place), and the
# signature is mandatory: the app refuses to install an unsigned artifact.
#
# The private key never lives in this repository. It is read from
# TAURI_SIGNING_PRIVATE_KEY (either the key content, like the CI secret, or a
# path to the key file) or from --key/--private-key/--pubkey-file. Signing needs
# `npx @tauri-apps/cli@2`, which is the only network access (first fetch only).
#
# Usage:
#   scripts/make-updater-artifact.sh [VERSION] [TAG] [options]
#   scripts/make-updater-artifact.sh 0.7.1 v0.7.1 --key ~/.tauri/agent-monitor.key
#   TAURI_SIGNING_PRIVATE_KEY=... GITHUB_REF_NAME=v0.7.1 scripts/make-updater-artifact.sh
#
# Options:
#   --version V        release version (default: src-tauri/tauri.conf.json)
#   --tag T            release tag (default: $GITHUB_REF_NAME, else vVERSION)
#   --repo O/R         OWNER/REPO for the download URL (default: $GITHUB_REPOSITORY
#                      or Hoemr/agent-monitor)
#   --url-base URL     base URL for the tarball in latest.json
#                      (default: https://github.com/REPO/releases/download/TAG;
#                      pass e.g. http://127.0.0.1:8766 for local verification)
#   --key K, --private-key K, --pubkey-file K
#                      private key content or path (default: $TAURI_SIGNING_PRIVATE_KEY)
#   --password P       key password (default: $TAURI_SIGNING_PRIVATE_KEY_PASSWORD)
#   --sign             explicit: signing is required (the default; kept for clarity)
#   --skip-sign        build tarball + manifest without a signature (dry runs only)
#   --notes TEXT       release notes in latest.json (default: $RELEASE_NOTES, else
#                      --notes-file/$RELEASE_NOTES_FILE, else the tag)
#   --notes-file F     read the release notes from a file
#   --app PATH         .app to package (default: dist/Agent Monitor.app)
#   --out DIR          output directory (default: dist)
#   --dry-run          alias of --skip-sign (no signature; dry runs only)
#   -h, --help         this help
#
# CHANGELOG/notes are plain text; the manifest JSON is written by python3 so
# quotes/newlines can never produce invalid JSON.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_DIR="${APP_DIR:-dist/Agent Monitor.app}"
OUT_DIR="${DIST_DIR:-dist}"
TARBALL_NAME="${TARBALL_NAME:-Agent-Monitor-macos-arm64.app.tar.gz}"
REPO="${GITHUB_REPOSITORY:-Hoemr/agent-monitor}"
TAURI_CLI_VERSION="${TAURI_CLI_VERSION:-2}"

VERSION="${APP_VERSION:-}"
TAG="${GITHUB_REF_NAME:-}"
KEY="${TAURI_SIGNING_PRIVATE_KEY:-}"
KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
NOTES="${RELEASE_NOTES:-}"
NOTES_FILE="${RELEASE_NOTES_FILE:-}"
URL_BASE="${URL_BASE:-}"
SIGN=1
POSITIONAL=()

die() { echo "make-updater-artifact: $*" >&2; exit 1; }
note() { echo "make-updater-artifact: $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
    --tag) TAG="${2:?--tag needs a value}"; shift 2 ;;
    --repo) REPO="${2:?--repo needs a value}"; shift 2 ;;
    --url-base) URL_BASE="${2:?--url-base needs a value}"; shift 2 ;;
    --key|--private-key|--pubkey-file)
      KEY="${2:?$1 needs a value}"; shift 2 ;;
    --password) KEY_PASSWORD="${2:?--password needs a value}"; shift 2 ;;
    --sign) SIGN=1; shift ;;
    --skip-sign|--dry-run) SIGN=0; shift ;;
    --notes) NOTES="${2:?--notes needs a value}"; shift 2 ;;
    --notes-file) NOTES_FILE="${2:?--notes-file needs a value}"; shift 2 ;;
    --app) APP_DIR="${2:?--app needs a path}"; shift 2 ;;
    --out|--dist) OUT_DIR="${2:?$1 needs a dir}"; shift 2 ;;
    -h|--help) sed -n '2,45p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) die "unknown option: $1 (see --help)" ;;
    *) POSITIONAL+=("$1"); shift ;;
  esac
done

# Positional form: VERSION [TAG]. Written as if-statements: a trailing `[ ] &&`
# list would trip `set -e` when the test is false.
if [ ${#POSITIONAL[@]} -ge 1 ] && [ -z "$VERSION" ]; then VERSION="${POSITIONAL[0]}"; fi
if [ ${#POSITIONAL[@]} -ge 2 ] && [ -z "$TAG" ]; then TAG="${POSITIONAL[1]}"; fi
if [ ${#POSITIONAL[@]} -gt 2 ]; then die "too many positional arguments (expected [VERSION] [TAG])"; fi

# ── Resolve version / tag / notes ───────────────────────────────
CONF="src-tauri/tauri.conf.json"
if [ -z "$VERSION" ]; then
  [ -f "$CONF" ] || die "cannot read $CONF to determine the version (pass --version)"
  VERSION="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$CONF")"
fi
case "$VERSION" in
  [0-9]*) : ;;
  *) die "version must start with a digit (got: $VERSION)" ;;
esac
if [ -z "$TAG" ] || [ "$TAG" = "main" ] || [ "$TAG" = "HEAD" ]; then
  # workflow_dispatch runs on a branch: derive the tag from the version.
  TAG="v$VERSION"
fi
case "$TAG" in
  v*) : ;;
  *) TAG="v$TAG" ;;
esac
if [ -z "$NOTES" ] && [ -n "$NOTES_FILE" ]; then
  [ -f "$NOTES_FILE" ] || die "notes file not found: $NOTES_FILE"
  NOTES="$(cat "$NOTES_FILE")"
fi
[ -n "$NOTES" ] || NOTES="Agent Monitor $VERSION"

TARBALL="$OUT_DIR/$TARBALL_NAME"
SIG_FILE="$TARBALL.sig"
MANIFEST="$OUT_DIR/latest.json"
URL_BASE="${URL_BASE:-https://github.com/$REPO/releases/download/$TAG}"
URL_BASE="${URL_BASE%/}"
ASSET_URL="$URL_BASE/$TARBALL_NAME"

# ── Package the .app ────────────────────────────────────────────
[ -d "$APP_DIR" ] || die "app bundle not found: $APP_DIR (run scripts/bundle-mac.sh first)"
APP_PARENT="$(cd "$(dirname "$APP_DIR")" && pwd)"
APP_BASE="$(basename "$APP_DIR")"
mkdir -p "$OUT_DIR"
# COPYFILE_DISABLE keeps macOS extended attributes out of the archive as
# AppleDouble `._*` files; the updater would otherwise extract them as garbage.
note "packaging $APP_BASE -> $TARBALL"
rm -f "$TARBALL" "$SIG_FILE"
COPYFILE_DISABLE=1 tar -czf "$TARBALL" -C "$APP_PARENT" "$APP_BASE"
[ -s "$TARBALL" ] || die "tarball is empty: $TARBALL"
# The updater strips one path component: the archive must have exactly the .app
# at its root, nothing else.
FIRST_ENTRY="$(tar -tzf "$TARBALL" | head -n 1)"
case "$FIRST_ENTRY" in
  "$APP_BASE/"|"$APP_BASE") : ;;
  *) die "tarball root must be '$APP_BASE', found '$FIRST_ENTRY'" ;;
esac
if tar -tzf "$TARBALL" | grep -qE '(^|/)\._'; then
  die "tarball contains AppleDouble ._ files (COPYFILE_DISABLE failed)"
fi

# ── Sign ────────────────────────────────────────────────────────
SIGNATURE=""
if [ "$SIGN" = "1" ]; then
  [ -n "$KEY" ] || die "no signing key: set TAURI_SIGNING_PRIVATE_KEY (content or path) or pass --key/--pubkey-file; signing is required so updates can be verified"
  KEY_IS_FILE=0
  if [ -f "$KEY" ]; then KEY_IS_FILE=1; fi
  if [ "$KEY_IS_FILE" = "1" ]; then
    note "signing with key file: $KEY"
    KEY_CONTENT="$(cat "$KEY")"
  else
    note "signing with key from environment/argument"
    KEY_CONTENT="$KEY"
  fi
  command -v npx >/dev/null 2>&1 || die "npx not found (Node 20+ is required to sign with @tauri-apps/cli)"
  # The CLI reads TAURI_SIGNING_PRIVATE_KEY[_PATH] from the environment too and
  # clap rejects passing both at once, so hide the unused one for this call.
  if [ "$KEY_IS_FILE" = "1" ]; then
    SIGN_ARGS=(signer sign --private-key-path "$KEY")
    SIGN_ENV=(env -u TAURI_SIGNING_PRIVATE_KEY)
  else
    SIGN_ARGS=(signer sign --private-key "$KEY_CONTENT")
    SIGN_ENV=(env -u TAURI_SIGNING_PRIVATE_KEY_PATH)
  fi
  # Embed the version in the signature's trusted comment (required if the app
  # ever enables plugins.updater.requireSignedVersion; harmless otherwise).
  SIGN_ARGS+=(--app-version "$VERSION")
  if [ -n "$KEY_PASSWORD" ]; then SIGN_ARGS+=(--password "$KEY_PASSWORD"); fi
  note "signing $TARBALL_NAME via @tauri-apps/cli@$TAURI_CLI_VERSION"
  "${SIGN_ENV[@]}" npx --yes "@tauri-apps/cli@$TAURI_CLI_VERSION" "${SIGN_ARGS[@]}" "$TARBALL" \
    || die "signing failed (wrong key/password? npx/network unavailable?)"
  [ -s "$SIG_FILE" ] || die "signature file was not created: $SIG_FILE"
  # latest.json carries the signature as ONE base64 string: the updater decodes
  # it with a strict base64 decoder, so whitespace/newlines must be stripped.
  SIGNATURE="$(tr -d '\r\n' < "$SIG_FILE")"
  [ -n "$SIGNATURE" ] || die "signature file is empty: $SIG_FILE"
else
  note "WARNING: --skip-sign given; latest.json will NOT be installable (dry run)"
fi

# ── latest.json (Tauri updater manifest) ────────────────────────
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
note "writing $MANIFEST"
python3 - "$MANIFEST" "$VERSION" "$NOTES" "$PUB_DATE" "$ASSET_URL" "$SIGNATURE" <<'PY'
import json, sys

path, version, notes, pub_date, url, signature = sys.argv[1:7]
platform = {"signature": signature, "url": url}
manifest = {
    "version": version,
    "notes": notes,
    "pub_date": pub_date,
    "platforms": {"darwin-aarch64": platform},
}
with open(path, "w", encoding="utf-8") as fh:
    json.dump(manifest, fh, ensure_ascii=False, indent=2)
    fh.write("\n")
PY
python3 -c 'import json,sys;json.load(open(sys.argv[1]))' "$MANIFEST" \
  || die "generated latest.json is invalid JSON"

# Sanity: the manifest must describe a darwin-aarch64 artifact (macOS arm64 only).
python3 - "$MANIFEST" "$SIGNATURE" <<'PY' || die "latest.json platforms.darwin-aarch64 is incomplete"
import json, sys
m = json.load(open(sys.argv[1]))
assert m["platforms"]["darwin-aarch64"]["url"], "missing darwin-aarch64 url"
if sys.argv[2]:
    assert m["platforms"]["darwin-aarch64"]["signature"], "missing darwin-aarch64 signature"
PY

note "done: $VERSION ($TAG)"
note "  $TARBALL"
if [ -n "$SIGNATURE" ]; then note "  $SIG_FILE"; fi
note "  $MANIFEST"
note "  asset url: $ASSET_URL"
