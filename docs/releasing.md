# Releasing

How to cut a release that the auto-updater will accept.

## How updating works

On startup NXTPDF fetches an update manifest, compares its `version` against
the running build, and if it is newer downloads the installer **immediately in
the background**. From there:

- Ignore the banner → the installer runs silently when the app closes, so the
  next launch is the new version.
- Press **Update now** → it installs at once and reopens.

The download is verified against a signature before anything is executed. An
unsigned or wrongly-signed artifact is rejected, so a compromised or corrupted
download cannot be installed.

## One-time setup

### 1. The signing keypair

Already generated. The private key lives at:

```
%USERPROFILE%\.tauri\nxtpdf-updater.key
```

It is **outside the repo on purpose** and must never be committed. The matching
public key is embedded in `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`.

> **If you lose this key you cannot ship updates to anyone already running the
> app.** They would have to uninstall and reinstall manually. Back it up
> somewhere durable — a password manager entry is ideal.

The key was generated without a password so local builds sign unattended. To
add one later, regenerate with `pnpm tauri signer generate` and note that
existing installs will stop accepting updates.

### 2. The update endpoint

Set to this repository:

```json
"endpoints": ["https://github.com/0xt1m/nxtpdf/releases/latest/download/latest.json"]
```

The URL is baked into the binary at build time, so **changing it requires a
rebuild** — editing the config alone does nothing to installers already made.

Any host serving two static files over HTTPS would work; GitHub Releases is
free and needs no infrastructure. Tauri expands `{{target}}`, `{{arch}}` and
`{{current_version}}` inside the URL if you ever want per-platform manifests. A
Windows-only build does not need them.

### 3. Repository secrets

The release workflow signs on the runner, so add these under
**Settings → Secrets and variables → Actions**:

| Secret | Value |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | The entire contents of `%USERPROFILE%\.tauri\nxtpdf-updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Empty — the key has no password |

## Cutting a release

### The short version

```bash
pnpm version:set 1.1.0        # updates all three version files at once
git commit -am "release: 1.1.0"
git tag v1.1.0
git push origin main --tags
```

**The first release is the exception:** the version is already 1.0.0, so just
tag and push.

The tag and the three version files must agree. The updater compares the
version in `latest.json` — taken from `tauri.conf.json` — against the running
build, so a release tagged `v1.1.0` while the config still says `1.0.0` looks
published but is never offered to anyone. The workflow checks this and fails
the build rather than letting it through.

The workflow in `.github/workflows/release.yml` builds, signs, generates
`latest.json`, and opens a **draft** release. Review the assets, then publish
it — the updater only sees releases marked latest.

### Building locally instead

Only needed to test an installer before tagging.

```powershell
# tauri build reads the key itself, not a path to it. Setting
# TAURI_SIGNING_PRIVATE_KEY_PATH instead looks like it works - the installers
# still build - but the .sig files are silently missing and the release is
# useless to the updater.
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content "$env:USERPROFILE\.tauri\nxtpdf-updater.key" -Raw
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
pnpm app:build
```

A password-less key **still prompts** for a password, so a shell with no stdin
hangs at "Decrypting updater signing key". Run it from a normal terminal, or
pipe a newline in.

Output lands in `src-tauri/target/release/bundle/`:

| File | Purpose |
|---|---|
| `nsis/NXTPDF_<version>_x64-setup.exe` | The installer, and the update payload |
| `nsis/NXTPDF_<version>_x64-setup.exe.sig` | Signature the updater verifies |
| `msi/NXTPDF_<version>_x64_en-US.msi` | Alternative installer for first-time installs |

Check the signatures exist before publishing anything by hand:

```powershell
Get-ChildItem src-tauri	arget
eleaseundle
sis\*.sig
```

**Use the NSIS build as the update payload, not the MSI.** MSI updates need
elevation, which cannot be granted silently while the app is closing.

### 3. Write `latest.json`

```json
{
  "version": "0.2.0",
  "notes": "What changed in this release.",
  "pub_date": "2026-08-21T00:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<paste the entire contents of the .sig file>",
      "url": "https://github.com/OWNER/REPO/releases/download/v0.2.0/NXTPDF_0.2.0_x64-setup.exe"
    }
  }
}
```

- `version` has no `v` prefix.
- `signature` is the whole `.sig` file contents, one long base64 line.
- `notes` is shown to the user, so write it for them.

### 4. Publish

Upload the setup `.exe` and `latest.json` to the release. With GitHub Releases,
tag it `v0.2.0` and mark it **latest** so the
`/releases/latest/download/latest.json` URL resolves.

### 5. Verify before announcing

Install the *previous* version, launch it, and confirm the banner appears and
the update applies. Testing this on a machine that is not your dev box is worth
the five minutes — a broken update is not something you can fix remotely.

## Testing the pipeline locally

You do not need to publish anything to test the mechanism:

```bash
# 1. Sign any file as if it were the installer
# The signer CLI accepts a path too; `tauri build` only accepts the key itself.
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/nxtpdf-updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
npx tauri signer sign path/to/payload.exe

# 2. Write a latest.json with a high version number, the printed signature,
#    and a http://localhost URL
# 3. Serve that directory over HTTP
# 4. Point plugins.updater.endpoints at http://localhost:PORT/latest.json
# 5. pnpm app:dev
```

The banner should appear, the payload should download, and the stage should
reach "ready to install". Remember to restore the real endpoint afterwards.

## Code signing (separate concern)

The update signature above is Tauri's own and is unrelated to **Authenticode**,
which is what stops Windows SmartScreen warning users that the installer is
untrusted. Both are worth having:

| | Update signature | Authenticode |
|---|---|---|
| Stops | Tampered updates | SmartScreen warnings |
| Cost | Free | ~$10/mo (Azure Trusted Signing) to ~$600/yr |
| Needed for | Auto-update to work at all | A trustworthy first install |

Without Authenticode the auto-update still works — the silent NSIS install does
not trigger SmartScreen, only the initial download does.
