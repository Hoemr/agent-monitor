# In-app updates (0.7.0)

## v0.8.0 manual download

The v0.8.0 GitHub Release uses the existing persistent local signing certificate
and provides a manual-download ZIP. It is not Apple Developer ID signed or
notarized. Quit the old app, extract the ZIP and replace the app in Applications;
macOS may require explicit approval to open it. A change of signing identity
requires granting the app's permissions again.

The `local-signing` marker is retained, so this build does not install online
updates. No `latest.json` or updater archive is published for v0.8.0. Existing
v0.7.1 users must download this version manually. The automated workflow below
still requires a distribution certificate for future releases with online updates.

## Updater mechanism

Agent Monitor updates itself from **GitHub Releases** using the official
[`tauri-plugin-updater`](https://tauri.app/plugin/updater/) — the same approach
as [cc-switch](https://github.com/farion1231/cc-switch): the app checks a signed
manifest (`latest.json`) at
`https://github.com/Hoemr/agent-monitor/releases/latest/download/latest.json`,
downloads the new `.app` tarball, verifies its **minisign signature**, installs it
in place and offers a restart. There is no installer and no manual re-download.

> "Incremental" here means *in-app update without reinstalling*: only the normal
> release artifact is downloaded (the macOS app is ~9 MB). It is not a binary
> delta between versions.

## User-facing behaviour

* **Settings → Updates** (`update` pane): current version, automatic check
  switch, **Check for Updates**, release notes, **Download & Install** with
  progress, and **Restart Now** once installed. The repo release URL is shown as
  read-only text.
* On launch (when `auto_check_updates = true`, the default) the app checks once
  in the background ~4 s after start. When a newer version exists, the pet shows
  a small "new version available" notice with an **Update** action that opens the
  Updates pane. Being up to date is silent; being offline is silent too.
* Tray menu → **检查更新… / Check for Updates…** opens Settings directly on the
  Updates pane and runs a check.
* Updates must preserve the code-signing identity. Locally signed builds reject
  online replacement and are updated through the local build script. Public
  releases require a persistent Developer ID identity in CI, separately from
  the Tauri updater signature.

## Settings key

| key | type | default | meaning |
|---|---|---|---|
| `auto_check_updates` | bool | `true` | check GitHub Releases once ~4 s after launch; silent when up to date/offline |

`reset_settings { "scope": "update" }` restores it.

## Commands / events

| Command | Returns |
|---|---|
| `check_for_update` | `{ current_version, available, version?, notes?, pub_date? }` |
| `install_update` | `()` — downloads, verifies, installs; emits progress events |
| `restart_app` | `()` — restarts the app |

| Event | Payload | When |
|---|---|---|
| `update-available` | same as `check_for_update` | startup auto-check only (never on manual check, which returns the value) |
| `update-progress` | `{ downloaded: number, total: number \| null }` | during download/install |
| `update-installed` | `{ version }` | after the new bundle is installed (restart still needed) |

## One-time release setup

1. Generate a minisign-compatible keypair (once, on a trusted machine):

   ```bash
   npx @tauri-apps/cli@2 signer generate -w ~/.tauri/agent-monitor-updater.key
   ```

   Put the **public** key into `src-tauri/tauri.conf.json` →
   `plugins.updater.pubkey` (already done for this repo). Never commit the
   private key.

2. Add repository secrets (Settings → Secrets and variables → Actions):

   | Secret | Value |
   |---|---|
   | `TAURI_SIGNING_PRIVATE_KEY` | the private key file content (or a path in CI) |
   | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the key password, if one was set |

3. Publish a normal release: push a `v<version>` tag. The release workflow builds
   the `.app`, packages `Agent-Monitor-macos-arm64.zip`, creates the updater
   tarball, signs it with the private key, writes `latest.json`, and attaches all
   of them to the GitHub Release.

## Release assets

| Asset | Purpose |
|---|---|
| `Agent-Monitor-macos-arm64.zip` | manual download (unchanged) |
| `Agent-Monitor-macos-arm64.app.tar.gz` | updater artifact (installed in place) |
| `Agent-Monitor-macos-arm64.app.tar.gz.sig` | minisign signature |
| `latest.json` | updater manifest (`version`, `notes`, `pub_date`, `platforms.darwin-aarch64.{url,signature}`) |

`/releases/latest/` only resolves **published stable** releases (drafts and
prereleases are ignored), so a release must be published before clients see it.

## Local verification (no real release needed)

Build two versions: the running one (e.g. 0.7.0) and a test artifact with a
higher version (e.g. 0.7.1), sign the artifact with the test key, serve the
manifest over local HTTP and point the app at it:

```bash
# 0) build the CURRENT version first (e.g. 0.7.0), then keep a copy as the "old" app.
#    Use /private/tmp, not /tmp: on macOS /tmp is a symlink and the updater's
#    install helper rejects a current_exe() path containing a symlink.
cargo build --release --manifest-path src-tauri/Cargo.toml
bash scripts/bundle-mac.sh
mkdir -p /private/tmp/am-update/old && cp -R "dist/Agent Monitor.app" "/private/tmp/am-update/old/"

# 1) temporarily bump the version (Cargo.toml AND tauri.conf.json) to 0.7.1,
#    build the "new" app, sign the updater artifact, and point latest.json at a
#    local server (--url-base). --key accepts the private key file path.
cargo build --release --manifest-path src-tauri/Cargo.toml
bash scripts/bundle-mac.sh
TAURI_SIGNING_PRIVATE_KEY=~/.tauri/agent-monitor-updater.key \
  bash scripts/make-updater-artifact.sh 0.7.1 v0.7.1 \
  --url-base http://127.0.0.1:8765 --key ~/.tauri/agent-monitor-updater.key \
  --out /private/tmp/am-update/serve --notes "local update test"
# → serve/latest.json, Agent-Monitor-macos-arm64.app.tar.gz, .sig

# 2) serve the manifest and run the OLD app copy with the test overrides
#    (quit the real Agent Monitor first: the single-instance socket would
#    redirect the copied app to the running one).
python3 -m http.server 8765 --directory /private/tmp/am-update/serve &
AGENT_MONITOR_UPDATE_ENDPOINT=http://127.0.0.1:8765/latest.json \
  "/private/tmp/am-update/old/Agent Monitor.app/Contents/MacOS/agent-monitor" --check-update
# → {"current_version":"0.7.0","available":true,"version":"0.7.1",...}

AGENT_MONITOR_DATA_DIR=/private/tmp/am-update/data \
AGENT_MONITOR_UPDATE_ENDPOINT=http://127.0.0.1:8765/latest.json \
AGENT_MONITOR_UPDATE_AUTO_INSTALL=1 \
  "/private/tmp/am-update/old/Agent Monitor.app/Contents/MacOS/agent-monitor"
# log: "update: installed 0.7.1; restarting" then "already at 0.7.1"

# 3) after it restarts, verify the swapped bundle
plutil -p "/private/tmp/am-update/old/Agent Monitor.app/Contents/Info.plist" \
  | grep ShortVersion   # → 0.7.1
# revert the temporary version bump and rebuild the real app afterwards
```

The same flow was executed for this release with the real repository key:
a copied 0.7.0 bundle was checked, downloaded, signature-verified, swapped in
place and restarted as 0.7.1; a second auto-check reported "already at 0.7.1".

`AGENT_MONITOR_UPDATE_ENDPOINT` (comma-separated URLs) and
`AGENT_MONITOR_UPDATE_AUTO_INSTALL=1` are test-only overrides used by the
acceptance run; normal users never set them. Local HTTP is possible because the
updater config allows an insecure transport for the override — signatures are
still mandatory, so the local endpoint cannot install unsigned code.

Headless check without installing:

```bash
AGENT_MONITOR_UPDATE_ENDPOINT=http://127.0.0.1:8765/latest.json \
  src-tauri/target/release/agent-monitor --check-update
```

## Troubleshooting

| Symptom | Check |
|---|---|
| "already up to date" although a release exists | version in `tauri.conf.json`/tag must be semver-higher; release must be published (not draft/prerelease); `latest.json` platform key must be `darwin-aarch64` |
| check fails offline / 404 | expected: silent at launch, readable error in Settings → Updates |
| signature error | artifact signed with a different private key than the configured public key, or the `.sig` file attached to the release is stale |
| app won't launch after update | the updater artifact must contain `Agent Monitor.app` at the tarball root; verify the macOS signature and preserve the original signing identity |
| update installed but version unchanged | the app must be restarted (`Restart Now`); the running bundle is only replaced after the helper swaps it |

## Persistent local signing

Run `python3 scripts/sign-mac.py --setup` once. This creates a code-signing
certificate and imports its private key into the login Keychain, restricted to
`/usr/bin/codesign`. Temporary private-key files are removed after import.
Only its fingerprint and keychain path are stored in
`~/Library/Application Support/agent-monitor/signing.json`.

`scripts/bundle-mac.sh` checks that identity before touching the bundle, then
signs with a requirement containing the fixed bundle identifier and certificate
fingerprint. It never falls back to ad-hoc signing. Preserve both the Keychain
identity and signing.json; do not regenerate them during builds. The certificate
is valid for ten years; losing or replacing it requires a new permission grant.

Moving from an old ad-hoc signature requires one final remove/re-add of the exact
App in macOS privacy settings. Verify permissions from the packaged GUI.
To verify a future update, authorize build A, install a differently built B signed
by the same identity at the same path, restart, and check that Accessibility and
Window Access are still granted without changing system settings.

The signed `Contents/Resources/local-signing` marker disables online replacement
for local builds, preventing an unrelated release signature from discarding their
permissions. Manual update checks explain that local builds must be rebuilt.
No system trust roots or TCC database records are modified by the signing scripts.

## Public release signing

CI imports an existing signing identity from `MACOS_SIGNING_CERTIFICATE_P12`
(base64 P12), `MACOS_SIGNING_CERTIFICATE_PASSWORD`, and `MACOS_SIGNING_IDENTITY`
(certificate name or fingerprint). Use a Developer ID identity for distribution;
do not upload the private local-development identity. The packaging script uses
the explicit `AGENT_MONITOR_SIGNING_IDENTITY` environment variable in CI.
Missing credentials stop the release; the workflow never creates a new identity
or silently publishes an ad-hoc bundle. These secrets are distinct from
`TAURI_SIGNING_PRIVATE_KEY`, which authenticates the update archive only.
