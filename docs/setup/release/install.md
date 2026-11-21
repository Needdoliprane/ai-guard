# Install from a GitHub Release

[Français](install.fr.md) · [Release metadata and updates](README.md) · [Setup index](../README.md) · [Security policy](../../../SECURITY.md)

Use this guide to download an ai-guard release, check what you downloaded, install it next to a previous version and go back. Replace `vX.Y.Z` with the tag you chose. Release archives are built by the [release workflow](../../../.github/workflows/release.yaml) and are published as a draft first; a maintainer publishes it.

**Status.** Nothing here has been run against a published release yet. The procedure describes the delivered workflow; the first release run is the first proof. A release archive is not a qualified production build: see [the production-readiness gates](../../engineering/PRODUCTION_READINESS.md).

## What a release contains

| File | Content |
|---|---|
| `ai-guard-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` | Native binaries (`bin/`), `LICENSE`, `THIRD-PARTY-NOTICES`, `BUILD-INFO`. Built on Ubuntu 22.04. |
| `ai-guard-vX.Y.Z-aarch64-apple-darwin.tar.gz` | The same for macOS on Apple silicon. Built on macOS 15. |
| `ai-guard-vX.Y.Z-frontend.tar.gz` | The compiled dashboard bundle, with the Vite licence. |
| `ai-guard-vX.Y.Z-release-metadata.tar.gz` | `manifest/release-manifest.json`, `manifest/sbom.cdx.json`, the tool's own `manifest/SHA256SUMS`, `BUILD-INFO`, `LICENSE`. |
| `ai-guard-vX.Y.Z-sbom.cdx.json` | The same CycloneDX 1.6 inventory, standalone. |
| `SHA256SUMS` | SHA-256 of every file above. |

Other platforms are not built. Windows and Linux on ARM have no archive.

## Download and check

```sh
TAG=vX.Y.Z
mkdir -p "$HOME/ai-guard-download/$TAG" && cd "$HOME/ai-guard-download/$TAG"
gh release download "$TAG" --repo Needdoliprane/ai-guard
```

A draft release is visible only to users with write access; a browser download works too. Then check each file you keep against `SHA256SUMS`:

```sh
FILE=ai-guard-$TAG-x86_64-unknown-linux-gnu.tar.gz   # or the aarch64-apple-darwin archive
grep -F "  $FILE" SHA256SUMS | shasum -a 256 -c -      # Linux: sha256sum -c -
```

`shasum -a 256 -c SHA256SUMS` alone also works, but reports every file you did not download as missing.

### What these checks prove, and what they do not

**Nothing is cryptographically signed.** There is no signature, build attestation or SLSA provenance. `SHA256SUMS` is published beside the archives it describes, so it detects corruption and mix-ups but not a compromised release page: whoever can edit the release can edit both. If that matters to you, obtain the expected digest of `SHA256SUMS` (or of `release-manifest.json`) from a maintainer through a separate trusted channel and compare it, or build from source and compare with [`release verify`](README.md#verify).

Each archive carries a `BUILD-INFO` file. After extraction, check that it names the tag, the commit shown in the release notes and the run of the release workflow:

```sh
tar -xzf "$FILE"
cat "${FILE%.tar.gz}/BUILD-INFO"
```

Fields: `tag`, `commit`, `target` (native archives only), `run`, `repository`. The release notes list the same commit and the CI run that passed on it.

Check the binary reports the same identity:

```sh
"${FILE%.tar.gz}/bin/agentsandbox" version --json
```

`build.revision` must equal `commit`, and `version` must equal the tag without the `v`. This is a declared identity embedded at build time, not an attestation.

### Inventory and notices

- `ai-guard-vX.Y.Z-sbom.cdx.json` is a CycloneDX 1.6 inventory of what `Cargo.lock`, `front/package-lock.json` and the image recipes declare (lifecycle `pre-build`). It is not an inventory of the compiled binary.
- `THIRD-PARTY-NOTICES` inside each native archive lists the Rust crates linked into the binaries with their licence texts. It covers every crate reachable on any platform and with all features, so it can list more than one binary uses. It does not cover the C library of your system or the Rust standard library.
- To re-derive the manifest and the SBOM from the tagged source, check out the tag, build the CLI from it and run `agentsandbox release verify --source . --manifest <extracted>/manifest --source-only`. A modified CLI can report anything: this check is only as trustworthy as the CLI you built.

## Platform notes

**Linux x86_64.** The binaries are dynamically linked against glibc and were built on Ubuntu 22.04, so they need a glibc at least as recent as that system's (2.35). That floor has not been measured in this repository; check your system with `ldd --version`, and the real requirement of a binary with `objdump -T bin/agentsandbox | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1`. Distributions with an older glibc must build from source.

**macOS (Apple silicon).** The binaries are neither signed nor notarised. A file downloaded with a browser carries a quarantine flag and Gatekeeper will refuse to run it. After you have checked the digest, remove the flag from the extracted directory:

```sh
xattr -dr com.apple.quarantine "ai-guard-$TAG-aarch64-apple-darwin"
```

Files fetched with `gh` or `curl` normally carry no flag. Do not disable Gatekeeper globally.

**Container engine.** The archives contain the control programs only. The control and worker images are built by you with `agentsandbox build`; see [the local guide](../local/README.md).

## Install and keep the previous version

Extract each version into its own directory and point a link at the one in use, so that going back is a link change:

```sh
sudo install -d /opt/ai-guard
sudo tar -xzf "$FILE" -C /opt/ai-guard
sudo ln -sfn "/opt/ai-guard/${FILE%.tar.gz}" /opt/ai-guard/current
/opt/ai-guard/current/bin/agentsandbox version --json
```

Use a directory you own if you do not use `sudo`. Nothing is installed as a service and no updater is included.

## Upgrade

1. Read the release notes and [Product version, build identity and manual updates](README.md#versions-and-updates).
2. Stop the services and automatic restarts that use the data, then take a coherent private backup of all stores, configuration, credentials references and audit anchors, following the [enterprise recovery procedure](../enterprise/README.md#operate-and-recover).
3. Install the new version beside the old one, check it as above and switch the link.
4. Rebuild your images with the new CLI, apply them with your usual deployment procedure, then verify component versions, health, refusals and audit continuity before reopening admissions.

Migrations are explicit commands, one step at a time; version 0.0.1 applies none automatically.

## Roll back

Switch the link back to the previous directory and restore the backup you took before the upgrade into a new destination. **Never downgrade a database that has been migrated**, edit its `user_version` or reuse it with an older binary: restore the coherent backup instead. If no backup exists, do not guess; keep the data and ask through the channels in [the contribution guide](../../../CONTRIBUTING.md).

## Report a problem with a release

For a suspected vulnerability, use the private channel of [the security policy](../../../SECURITY.md). For anything else, open an issue and include `agentsandbox version --json` and the contents of `BUILD-INFO`.
