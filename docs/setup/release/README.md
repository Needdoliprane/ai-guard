# Release metadata

[Français](README.fr.md) · [Setup index](../README.md) · [Local workstation](../local/README.md) · [AI providers](../providers/README.md)

Use this guide to describe an ai-guard build source in a release manifest, a CycloneDX inventory and a `SHA256SUMS` file, to bind the files you built to that description by name, size and SHA-256, and to check all of it again later, offline. Two operator commands of the Rust CLI do this: `agentsandbox release manifest` and `agentsandbox release verify`. They start no engine, open no network connection, never write into the source and never replace or remove an existing directory.

**Status (13 November 2026, validated composition).** W1–W3 manifest/verify commands are delivered and compiled: 123 selected release parents PASS, alongside 144 Kubernetes and 5 broker parents in the same campaign. Compilation and all-targets check PASS. The global harness returns RC99: the `it-manifest` other-UID parent0700 subcase remains NOT_RUN because no second account was available; two ignored helpers remain NOT_RUN. See [current evidence](../../engineering/evidence/2026-11-13/claude-composition/README.md); the [original record](../../engineering/evidence/2026-11-12/release-integrity/README.md) retains its dated scope. No release build or product control/worker image was built or inspected. Only the QA tool image was inspected. PW1/PW2/PW3/PW6/PW8 remain proposed and unapplied. No qualified release procedure or product release was established by this campaign.

<a id="scope"></a>

## What the commands establish, and what they do not

`release manifest` prepares `--source` exactly as [`build` prepares it](../local/README.md#prepared-source): the same structural check, the same bounded private copy under `TMPDIR`, the same confirmation and the same provenance line on standard error. It then reads `Cargo.lock`, `front/package-lock.json`, `Cargo.toml`, `rust-toolchain.toml`, `front/package.json` and every `images/*.Dockerfile` from that verified copy, which is checked again before and after this read. It parses both locks with strict readers, hashes the files given with `--artifact`, and publishes three files into a new private directory. `release verify` reads such a directory, prepares the source again, derives both documents again byte for byte, hashes the artefacts you present and reports one outcome.

A manifest binds, by SHA-256:

- the **prepared build inputs**: the relative path, length and SHA-256 of every file of the copy, and the source digest that `build` prints for the same bytes;
- the **lock files** and what they resolve: the Cargo and npm packages, their registry hashes and their dependency graphs;
- the **declared build facts**: package name, version and licence, the requested toolchain, the `[profile.release]` settings and, per recipe, its base images and `cargo build` invocations;
- the **declared artefacts**: the basename, length and SHA-256 of each file you name.

It does **not** establish:

- a signature, a build attestation, SLSA provenance or an anti-rollback measure: anyone who can write the manifest directory can rewrite the documents and `SHA256SUMS` together;
- that an artefact or an image was built from this source, nor the target platform or build profile of either;
- an image ID, an image digest or the content installed in an image;
- an approved commit, or trust in the selected distribution: a copy does not make an untrusted source trusted;
- a build identity. The manifest carries a [build-input identity](#build-input-identity), which identifies the inputs of a build, not a build.

Every value the documents call *declared* is what a file or the operator says; nothing authenticates it.

<a id="before-you-start"></a>

## Before you start

- **A CLI built from the distribution you describe.** The CLI compiles in its list of build inputs and the SHA-256 of the `.dockerignore` that list mirrors; like `build`, it refuses to prepare a distribution whose `.dockerignore` differs. A manifest records that SHA-256 as `producer.allowlist_sha256`, with the producer name and version, and `release verify` judges only manifests whose producer equals its own. The verifier is therefore a CLI built from the distribution it verifies: a modified CLI can report anything, and nothing in these files proves which CLI wrote them. The checks you can make without it are the [independent checks](#independent-checks): `SHA256SUMS` and the documented digest layouts.
- **Platform.** Linux or macOS, with the source, `TMPDIR`, the output parent and each artefact on a supported local filesystem: ext2/3/4, XFS, Btrfs, tmpfs, overlayfs, ramfs or F2FS on Linux; local APFS or HFS+ with ownership enabled on macOS. No engine and no network are needed.
- **`TMPDIR`.** The private copy follows the [temporary-directory rules of `build`](../local/README.md#prepared-source): an absolute directory outside the source that passes the private-directory policy, with room for the build inputs (256 MiB at most).
- **A private parent for the output.** `--output` names a directory that does not exist yet, in a parent that already exists, belongs to you and has mode exactly `0700`. Every directory above that parent must belong to root or to you and not be writable by group or others, except a root-owned directory with the sticky bit such as `/tmp`. No symbolic link may appear in that path, except the macOS aliases `/tmp`, `/var` and `/etc`, which are read as `/private/tmp`, `/private/var` and `/private/etc`. `mkdir -m 700` creates such a parent whatever your umask; if the directory already exists, `mkdir` fails: check its owner and mode yourself, since ai-guard never repairs a mode.
- **A default `SIGCHLD` disposition.** Like `build`, `up` and `down`, the release commands run inside the operator interruption scope and refuse to start in a process whose `SIGCHLD` is ignored or carries `SA_NOCLDWAIT`, with the [message of those commands](../local/README.md#interrupt-build-or-compose). That message speaks of engine commands, although `release` starts none.

Run the following from the root of the ai-guard distribution you describe:

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
RELEASE_PARENT="$HOME/ai-guard-releases"
mkdir -m 700 "$RELEASE_PARENT"
```

`$HOME` and every directory above it must pass the ancestor rule as well.

<a id="produce"></a>

## Produce a manifest

```sh
"$AIGUARD_BIN" release manifest --source . --output "$RELEASE_PARENT/inputs-1"
```

| Option | Rule |
|---|---|
| `--source DIST` | Required, without a default. The ai-guard distribution to describe, not a project an agent edits. |
| `--output NEW_DIR` | A new directory whose name has 1 to 96 characters of `[A-Za-z0-9._-]` and does not start with a dot, in a [private parent](#before-you-start), outside the source. A relative path is resolved from the current directory; `..` is refused. |
| `--artifact FILE` | Repeatable, at most 64. A built file to bind by basename, length and SHA-256: see [bind artefacts](#bind-artefacts). |
| `--codex-version V`, `--claude-code-version V` | The provider client versions you asked the worker image to install: see [declared provider versions](#provider-versions). |
| `--expect-source-digest HEX64` | The `sha256=` value of a `build` provenance line: see [relation to `build`](#relation-to-build). |

The command works in this order and stops at the first refusal, with exit status 1:

1. Before anything is prepared: the declared provider versions, the form of `--expect-source-digest`, the number, names and uniqueness of the artefacts, the existence of `--source`, and the output (parent policy, new name, no staging directory left by an earlier run, outside the source). A refusal here prints no provenance line and creates nothing under `TMPDIR`.
2. The preparation of the source, which prints `Prepared public distribution (build): sha256=<hex> …` on standard error and refuses what `build` refuses.
3. `--expect-source-digest`, compared with the digest of that copy.
4. The read of the retained declarations and the strict reading of both locks; the prepared copy is then released.
5. Each artefact, read and hashed.
6. The documents, then their publication: a staging directory `.<leaf>.release-staging` is written in the parent and synchronised, renamed to `<leaf>` without replacing anything, then listed and read back.

The exit status is 0 only once the directory is published and read back. It holds exactly three files of mode `0600`, in a directory of mode `0700`:

| File | Content |
|---|---|
| `release-manifest.json` | The manifest, format `ai-guard.release-manifest` version 1, in canonical JSON (the `serde_json` pretty form followed by one line feed). |
| `sbom.cdx.json` | The inventory, CycloneDX 1.6 JSON, lifecycle `pre-build`. |
| `SHA256SUMS` | `<sha256>  release-manifest.json` and `<sha256>  sbom.cdx.json`, in that order. |

Standard output then shows one block. Illustration of the format, not an observed output:

```text
release manifest: published <output as given>
manifest   sha256=<hex> format=ai-guard.release-manifest/1 producer=agentsandbox <version>
source     sha256=<hex> layout=ai-guard:public-distribution:v1 files=<n> bytes=<n>
locks      Cargo.lock sha256=<hex> packages=<n>; front/package-lock.json sha256=<hex> packages=<n>
sbom       sbom.cdx.json sha256=<hex> CycloneDX 1.6 pre-build
inputs identity <hex> (build inputs only; declared: CODEX_VERSION=<version|none>, CLAUDE_CODE_VERSION=<version|none>)
target     unknown (not observed)
artifacts  <k> bound by name, size and SHA-256 (declared, not attested)
limits     declared values are not authenticated; this is not a signature, a build attestation, SLSA provenance or anti-rollback
```

With `--expect-source-digest`, the line `expected source digest matched (declared by the operator)` comes before `limits`. No document holds a path or a time, so the same inputs are meant to give byte-identical documents whatever the source path, the umask or `TMPDIR`; the corresponding selected software parent passed in the [current composition](../../engineering/evidence/2026-11-13/claude-composition/README.md).

<a id="bind-artefacts"></a>

### Bind artefacts

```sh
"$AIGUARD_BIN" release manifest --source . --output "$RELEASE_PARENT/release-1" --artifact /absolute/path/to/a/built/file
```

An artefact must be:

- a regular file owned by root or by you, not writable by group or others, without a set-ID or sticky bit, and at most 4 GiB. Its type is read before it is opened: a device node, a FIFO, a socket or a directory is refused without being opened, and a name replaced between that read and the opening is refused, unless the replacing entry reuses the inode number of the entry that was read (a filesystem can reuse a freed number at once): a regular file is then bound, as if the swap had been made before the read, and another type is opened once, then refused. The replacing entry may already have been opened once by then, without blocking and never as a controlling terminal: whoever can write the directory can make that swap, including another user whose own entry you name in a root-owned sticky directory such as `/tmp`. Keep artefacts in a directory that only you can write;
- reached from `/` through real directories that belong to root or to you and are not writable by group or others, except a root-owned sticky directory, on a supported local filesystem and without a symbolic link (the macOS aliases above excepted);
- named, by its basename, with 1 to 128 characters of `[A-Za-z0-9._+-]`, not starting with a dot and not ending with `.env`, `.db`, `.sqlite`, `.sqlite3`, `-wal`, `-shm`, `.key`, `.pem` or `.p12`. This name rule guards against mistakes; it is not a security boundary. Two artefacts with the same basename are refused.

A file with several hard links is accepted: a second name gives nobody another way to write the file, whose owner and mode are checked. The bytes are bound to one observed state of the open file (device, inode, owner, group, mode, link count, length and both timestamps). After the read, that state is compared again on the open file and on the name opened again, and the directories are walked again from `/`. A file rewritten, renamed, linked or unlinked while it is hashed, or a directory above it that is replaced, is refused and nothing is bound. Only the basename, the length and the SHA-256 enter the manifest; the artefact's `profile` and `target` are recorded as `unknown`. On macOS the extended ACL of the artefact file itself is not inspected; those of its directories are.

<a id="bind-cli"></a>

### Bind the CLI you built

> **Observed in this QA composition.** The parent `the_cli_just_built_by_cargo_is_bound_as_an_artifact` passed using the built QA CLI. This does not qualify a release build or native macOS.

```sh
cargo build --release --locked
"$AIGUARD_BIN" release manifest --source . --output "$RELEASE_PARENT/cli-1" --artifact target/release/agentsandbox
```

According to Cargo's sources, on Linux `target/release/agentsandbox` is a hard link to a file under `target/release/deps/`; this was not checked here, and the reader accepts a linked file as well as a single one. The CLI may bind its own executable: it only reads it. Bind the file once Cargo has finished: a build that rewrites or relinks it during the hash makes the command refuse it. If the file is refused, read the rule named under `Caused by:` (for example a directory of the checkout that group or others may write, or an unsupported filesystem) and correct that condition where the file lives. Do not copy the binary elsewhere to have it accepted: the manifest is meant to describe the file you distribute, and the refused condition would remain where that file was built.

<a id="provider-versions"></a>

### Declared provider versions

`--codex-version` and `--claude-code-version` record the versions of the provider clients you asked the worker image to install: the `CODEX_VERSION` and `CLAUDE_CODE_VERSION` build arguments of `images/worker.Dockerfile`. Each is checked before anything is prepared, by two rules: the exact-version rule of `build` (`X.Y.Z` with an optional pre-release and build suffix, at most 100 bytes; no `latest`, range or shell syntax) **and** the pattern the worker recipe applies before `npm install`, `^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$`. A version that fails either rule is refused, so that no manifest names a version with which no worker image can be built: `1.2.3`, `1.2.3-rc.1` and `1.2.3+b.5` are accepted; `latest`, `1.2` and `1.2.3-rc.1+b.5` are refused. `build` itself still accepts `1.2.3-rc.1+b.5`, and the worker recipe then fails: use a version that both rules accept.

Pass the same versions to `build` and to `release manifest`: the manifest puts them into the build-input identity and the inventory, and `build` passes them to the worker image, whose [labels](#image-labels) record them. They are recorded in `build.provider_clients` as declared by the operator and not observed in an image. The package name of each client is recorded only if the worker recipe holds its exact install token (`"@openai/codex@$CODEX_VERSION"`, `"@anthropic-ai/claude-code@$CLAUDE_CODE_VERSION"`); otherwise it is `unknown`.

<a id="read-manifest"></a>

## Read the manifest

A fact that a file declares is written `{"status": "declared", "value": …, "from": "<file>"}`; a fact that format 1 does not know is written `{"status": "unknown", "reason": "…"}`. An unknown value is never replaced by a default. The fields, in their order:

| Field | Content | Evidence |
|---|---|---|
| `format`, `format_version` | `ai-guard.release-manifest`, `1` | Fixed. |
| `producer` | Name and version of the CLI package, and `allowlist_sha256`, the SHA-256 of the `.dockerignore` the CLI mirrors. | The CLI that wrote the file. The version is the package version (`0.1.0`) of every build and does not identify the CLI build. |
| `statement` | `lifecycle: pre-build`, what the manifest `binds` and what it `does_not_establish`. | Fixed text. |
| `source` | `scope: build`, `digest` (layout `ai-guard:public-distribution:v1`), `files`, `bytes`, and `entries` (path, bytes and SHA-256 of each file). | Observed on the verified copy. |
| `package` | Name and version of the `Cargo.lock` root package; `license` from `Cargo.toml`. | Declared. A `Cargo.toml` name or version that differs from the lock root is refused as a stale lock; this comparison is made only when the `[package]` table declares both as plain strings. A package written another way (a root `package` key, `[[package]]`) leaves `license` `unknown`, with its line. |
| `locks.cargo`, `locks.npm` | Path, length, SHA-256, lock version and package counts; for npm also `dev`, `optional` and `install_scripts`. | Bytes observed on the copy, then parsed. |
| `build.inputs_identity` | The [build-input identity](#build-input-identity), with what it `covers` and its `statement`. | Computed. |
| `build.target` | Always `unknown`. | The platform of a build is not observed. |
| `build.toolchain` | Channel, profile, components and targets of `rust-toolchain.toml`; an absent `components` or `targets` key is written as an empty list. | Declared: the requested toolchain, not the compiler that ran. |
| `build.release_profile` | Scalar settings of a `[profile.release]` table in `Cargo.toml`. | Declared. `unknown`, with its line, when the release profile is written another way: a `release` key under `[profile]`, a root `profile` key, an array of tables, a nested table such as `[profile.release.package.<name>]` or a value that is not a scalar. A construct outside the TOML subset of format 1, such as a dotted key, makes `package.license` and this field `unknown`. |
| `build.recipes` | Per `images/*.Dockerfile`: path, SHA-256, `base_images` (reference, stage alias, internal stage, pinned by digest) and `cargo_build_invocations` (argv, `profile`, `target`). | SHA-256 observed; the rest declared by the recipe text, read without shell interpretation. |
| `build.provider_clients` | Per declared build argument: `requested_version` and `package`. | Declared by the operator; not observed in an image. |
| `sbom` | Path, format, specification version, length and SHA-256 of `sbom.cdx.json`. | Computed. |
| `artifacts` | Per artefact, sorted by name: `name`, `bytes`, `sha256`; `profile` and `target` `unknown`. | Hashed when the manifest was made; provenance declared. |
| `uncovered` | Sixteen codes, each with its subjects and statement. | What format 1 does not cover: see [limits](#limits). |

A recipe is read on logical instructions: a line ending with `\` continues on the next. A parser directive before the first instruction (`# syntax=…`, `# escape=…`), a `<<` that can start a heredoc, or a comment or blank line inside a continued instruction makes the whole recipe `unknown`. A `RUN` that mentions `cargo` outside the plain form `RUN cargo build <arguments>` gives an `unknown` invocation. An invocation's `profile` is what its tokens say (`release` for `--release` or `-r`, `P` for `--profile P`, `dev` otherwise); its `target` is declared only by `--target T`. Cargo configuration in a base image, `ENV` and `ARG` values and the build environment are not interpreted. A recipe without `FROM` has `base_images` `unknown`, not an empty list, and a `FROM` reference holding `$` is one `unknown` image.

<a id="build-input-identity"></a>

### Build-input identity

```text
inputs_identity = SHA-256( "ai-guard:build-inputs:v1" ‖ 0x00
                           ‖ u64be(64) ‖ source digest (64 lowercase hex)
                           ‖ u8(p) ‖ u64be(length) ‖ CODEX_VERSION
                           ‖ u8(p) ‖ u64be(length) ‖ CLAUDE_CODE_VERSION )
```

`u64be` is an unsigned 64-bit big-endian integer and `length` counts bytes. `p` is 1 for a declared version and 0 for an absent one, which then contributes an empty string. The source digest already covers `Cargo.toml` (package version and release profile), `rust-toolchain.toml`, the locks and the recipes with their declared `cargo build --release`. The value therefore names these build inputs and these declared provider versions, and nothing more. It is **not a build identity**: an amd64 and an arm64 build, a debug and a release build on a host, or two image builds that resolve different images behind the same `FROM` tag all share it. The manifest states each of these facts as `unknown` (`build.target`, the artefacts' `profile` and `target`) or uncovered (`build-target`, `base-image-digest`, `cargo-configuration`). No binary or image reports this value today.

<a id="read-sbom"></a>

## Read the inventory

`sbom.cdx.json` is a CycloneDX 1.6 JSON document of lifecycle `pre-build`: it describes what the locks and recipes declare before any build. Its `metadata.properties` repeat the source digest, the build-input identity and their layouts.

| Components | Content |
|---|---|
| The described package (`metadata.component`) | The `Cargo.lock` root package, `bom-ref` `agentsandbox`, with its `Cargo.toml` licence when one is declared, marked declared: an SPDX expression when it is well formed, otherwise a licence name. |
| Rust crates | One `library` per crates.io package, `bom-ref` `cargo:<name>@<version>`, SHA-256 equal to the lock checksum of the registry archive, properties `ai-guard:scope=unresolved` and `ai-guard:cargo-source=crates.io`; no `scope`, no licence. |
| npm packages | One `library` per lock entry, `bom-ref` `npm:<name>@<version>`, the SHA-512 of its `integrity` written in hexadecimal, `scope: excluded` for development entries, the declared licence, the registry URL as a `distribution` reference, and `ai-guard:npm-*` properties for optional, platform and install-script entries. |
| The frontend package | The npm lock root, with `ai-guard:first-party=true`. |
| Base images | One `container` per external `FROM` reference, name and tag split, `ai-guard:pinning=digest`, `tag` or `none` with `ai-guard:version-evidence=declared:digest`, `declared:tag` or `declared:none` (a tag names no content; a SHA-256 hash is written only for an `@sha256:` digest of 64 lowercase hexadecimal characters), and `ai-guard:declared-by=<recipe>`. |
| Toolchain | `toolchain:rust@<channel>`, with `ai-guard:version-evidence=declared:rust-toolchain.toml`. |
| Provider clients | Only when declared: `provider:<package>@<version>`, with `ai-guard:version-evidence=declared:build-argument`. |

What the inventory leaves out, and why:

- No `serialNumber` and no `timestamp`: the same inputs give the same bytes, and a fixed serial number would contradict the uniqueness the specification asks of it.
- No `scope` and no licence for Rust crates: `Cargo.lock` resolves neither targets nor dependency kinds, and records no licence. A consumer that reads a missing `scope` as `required` (the CycloneDX default) overstates what the lock says; `ai-guard:scope=unresolved` states it.
- A licence is written as an SPDX `expression` only when it is a well-formed identifier or `AND`/`OR`/`WITH` expression, otherwise as a licence `name`; both carry `acknowledgement: declared`. Whether an identifier is on the SPDX list is not checked.
- Separate dependency graphs: the Rust package, the frontend package, each base image, the toolchain and each provider client are separate roots. Neither lock says that the Rust package depends on the frontend, an image or a provider client, and recipes only declare how images are assembled, so no edge is invented. `ai-guard:dependency-graph` and the `compositions` (`incomplete` for `agentsandbox`, `unknown` for images and provider clients) say so.
- No artefact: artefacts are bound in the manifest, so the same source and declared versions give the same inventory whatever you bind.
- **No package identifier** (no purl, no CPE): this inventory is **not ready for vulnerability scanners**, which usually match components by purl, and a supply-chain review cannot rely on it for automated matching yet.

Validating the file with an external CycloneDX tool is your choice; no such tool is part of ai-guard, and none was run.

<a id="verify"></a>

## Verify

```sh
"$AIGUARD_BIN" release verify --source . --manifest "$RELEASE_PARENT/cli-1" --artifact target/release/agentsandbox
"$AIGUARD_BIN" release verify --source . --manifest "$RELEASE_PARENT/inputs-1" --source-only
```

`release verify` takes the declared provider versions from the manifest; it has no version option. It works in this order and reports the first outcome that applies:

1. It reads `release-manifest.json`, `sbom.cdx.json` and `SHA256SUMS` from `--manifest`: each must be a regular file with a single link, owned by root or by you, not writable by group or others and at most 16 MiB, in a directory chain that follows the ancestor rule of [before you start](#before-you-start).
2. It reads the manifest header leniently. A file that is not an ai-guard release manifest, another `format_version` or another producer is `UNVERIFIED`, before any strict check and before the source is prepared.
3. It checks `SHA256SUMS`, the canonical form of the manifest and its declared provider versions.
4. Unless `--source-only` is given, it hashes every `--artifact` under the [artefact rules](#bind-artefacts).
5. It prepares `--source` (the provenance line appears on standard error) and compares the digest of that copy with `source.digest.value`: a difference is `DIVERGENT source`, decided before anything is read from the copy. Otherwise it derives both documents again.
6. It compares, in this order: source, locks, declared facts, build-input identity, inventory, artefacts, then the remaining bytes (`declared`, field `manifest`).

`MATCH` and `SOURCE MATCH` are the first line of standard output; `DIVERGENT` and `UNVERIFIED` are printed on standard error.

| First line | Exit | Meaning |
|---|---|---|
| `release verify: MATCH` | 0 | Every comparison held, and every artefact the manifest lists was presented and matched. |
| `release verify: SOURCE MATCH; artifacts NOT VERIFIED (<k> listed)` | 0 | Under `--source-only` only: source, locks, declared facts and inventory match; **no artefact was read or verified**. `--source-only` cannot be combined with `--artifact`. |
| `Error: DIVERGENT <category>: …` | 1 | This CLI judged a format-1 manifest of its own producer and found a difference, or the directory is not a well-formed format-1 publication. |
| `Error: UNVERIFIED: …` | 1 | This CLI cannot judge, or did not: an input could not be read under its policy; the source could not be prepared, or it has the digest the manifest binds but its locks or declarations could not be derived; the file is not an ai-guard release manifest, its `format_version` is not 1, or its producer differs from this CLI; `--source-only` was combined with `--artifact`; a stop request ended the command; or a step ended without a result. This is not evidence of divergence, nor of a match: treat it as a failed verification. A manifest edited to name another `format_version` or producer is reported `UNVERIFIED`, not `DIVERGENT`. |

The success block repeats the lines of `release manifest`, with `artifacts  <k>/<k> matched` or, under `--source-only`, `artifacts  NOT VERIFIED (<k> listed; --source-only)`. Exit status 2 is a usage error of the argument parser, for example a misspelled option, not a verification result. A script must read both the exit status and the first line of standard output: only `release verify: MATCH` means that the artefacts were verified.

| Category | Cause |
|---|---|
| `sums` | A document does not match its line in `SHA256SUMS`. |
| `format` | `SHA256SUMS` is not exactly its two lines; or the manifest has an unknown field, is not in canonical form, names a build argument outside format 1 or declares an inadmissible provider version. |
| `source` | The source digest differs; at most ten changed, added and removed paths are named. When no path is listed, the first differing field of `source` is named instead (`no entry differs, source.<field> differs`). This is decided right after the preparation, before the locks are read, so a change that format 1 cannot read, such as a lock line it refuses, is reported here. |
| `lock` | A field of `locks.cargo` or `locks.npm` differs; the field is named. A lock that format 1 cannot read is never `lock`: it is `source` when the source differs from the manifest, otherwise `UNVERIFIED` (`--source`). |
| `declared` | A declared fact differs; the field is named. |
| `inputs-identity` | `build.inputs_identity` differs. |
| `inventory` | `sbom.cdx.json` or its record in the manifest differs. |
| `artifact-changed` | A presented artefact has another length or SHA-256. |
| `artifact-missing` | A listed artefact was not presented. |
| `artifact-unlisted` | A presented artefact is not listed. |

| `UNVERIFIED` message | Next action |
|---|---|
| `<input> could not be read under its policy: <reason>`, where `<input>` is `--manifest`, one of the three files, `--artifact` or `--source` | Correct the named condition: a missing file, a directory writable by group or others, a hard-linked manifest file, a source that cannot be prepared, or a source with the manifest's digest whose locks format 1 refuses (malformed, incomplete or stale). |
| `--source-only verifies no artefact and cannot be combined with --artifact; nothing was compared` | Run again with either `--source-only` or the artefacts. |
| `interrupted by <signal> before a result; nothing was judged and nothing was published`, possibly followed by `; the interrupted step reported: <reason>` | Run again when ready. |
| `the <step> step ended without a result; <what may exist>; this CLI judged nothing` | Run again; this is not a verification result. |
| `not an ai-guard release manifest; this CLI judges nothing in it` | The file is not a manifest of this format. |
| `unsupported format_version <n>; this CLI judges format_version 1 only` | Use a CLI that knows that format version. |
| `produced by <name> <version> (allowlist <sha>); this CLI is <name> <version> (allowlist <sha>). Verify with the CLI built from the distribution the manifest describes` | Build the CLI from the distribution the manifest describes and verify with it. |

The producer version is the package version, `0.1.0` for every build: two different builds of the CLI carry the same producer, and the producer check does not tell them apart. A derivation rule may only change with a new `format_version`; a build that changed one without it would report divergences on the manifests of another build.

**Moving a manifest.** No document holds a path, so the source and the manifest directory can both be moved. Copy the manifest directory, for example with `cp -R`; a hard-linked copy is refused as `UNVERIFIED`, because each file must have a single link. The copied files and directories must stay unwritable by group and others.

<a id="independent-checks"></a>

**Independent checks.** Without the CLI, from the manifest directory, on Linux (GNU coreutils) or on macOS respectively:

```sh
sha256sum -c SHA256SUMS
shasum -a 256 -c SHA256SUMS
```

Neither was run in this validation. They show only that the two documents match `SHA256SUMS`: whoever changes a document can rewrite that file too. The source digest can also be recomputed from `source.entries`, in the listed order, with its layout:

```text
source digest = SHA-256( "ai-guard:public-distribution:v1" ‖ 0x00
                         ‖ for each entry: u64be(length of path) ‖ path ‖ u64be(bytes) ‖ sha256 (64 lowercase hex) )
```

It can then be compared with the `sha256=` of a `build` provenance line, and each entry with the SHA-256 of the file in your checkout.

<a id="relation-to-build"></a>

## Relation to `agentsandbox build` and to images

`release manifest` makes its own capture of the source. It does not reuse the copy that `build` handed to the engine, and nothing records which copy an image was built from; two captures of an unchanged source have the same digest. To tie a manifest to the source a `build` reported, copy the `sha256=` value of the line `Prepared public distribution (build): sha256=<hex> …` that `build` printed, and give it to the manifest:

```sh
"$AIGUARD_BIN" build --runtime docker --source . --codex-version <X.Y.Z> --claude-code-version <X.Y.Z>
"$AIGUARD_BIN" release manifest --source . --output "$RELEASE_PARENT/build-1" --codex-version <X.Y.Z> --claude-code-version <X.Y.Z> --expect-source-digest <hex>
```

The value must be 64 lowercase hexadecimal characters. It is compared with the digest of the new copy after the preparation, before any read or output, and it is never written into the documents. A difference means that the source changed since that `build`, or that the value comes from another one; nothing is then written. This checks that two captures hold the same bytes, as you declared; it proves neither that an image was built from either capture nor that the line came from that build. Without the option, compare that value with `source.digest.value` yourself.

<a id="image-labels"></a>

### Image labels

Each image recipe declares labels in its final stage:

| Image | Label | Value |
|---|---|---|
| Control and worker | `ai-guard.image.recipe` | `images/control.Dockerfile` or `images/worker.Dockerfile`: the entry of `build.recipes` the image comes from. |
| Worker | `ai-guard.provider.codex.requested-version` | The `CODEX_VERSION` build argument; empty when not requested. |
| Worker | `ai-guard.provider.claude-code.requested-version` | The `CLAUDE_CODE_VERSION` build argument; empty when not requested. |

Compare the requested versions with `build.provider_clients` in the manifest. To read the labels after a build, with Docker or Podman respectively (standard engine commands, not run in this validation):

```sh
docker image inspect --format '{{json .Config.Labels}}' agentsandbox-worker:local
podman image inspect --format '{{json .Config.Labels}}' agentsandbox-worker:local
```

Labels are declared by whoever builds the image: anyone can build these recipes, or others, with any label. A label names a recipe and a requested version; it does not establish that the image was built from a prepared copy or from this manifest's source, nor that a client of that version is installed. No image ID or digest is recorded (`image-identity`), and no label carries the build-input identity.

<a id="failures"></a>

## Failures and recovery

Every refusal exits with status 1 and its message starts with `Error: `; a cause reported by a policy or by the system follows under `Caused by:`. A refusal of `--output`, `--artifact` or a provider version starts with `<option> is refused; nothing was prepared`, and the table gives the cause that follows it. In no case does ai-guard remove or overwrite something that exists.

| Message | Next action |
|---|---|
| `the parent of --output must be an existing directory owned by you with mode exactly 0700, whose ancestors belong to root or to you and are not writable by group or others (create it with mkdir -m 700 <parent>): <parent>` | Create a private parent as in [before you start](#before-you-start) and use it. |
| `--output already exists or cannot be inspected: <output>; release manifest writes only into a new directory and never replaces one` | Choose a new name. |
| `--output must be outside the source directory: …` | Choose a parent outside the distribution. |
| `a staging directory of an earlier run exists or cannot be inspected: <parent>/.<leaf>.release-staging; inspect or remove it first, ai-guard never adopts or removes it` | An earlier run stopped before publishing. Inspect that directory; remove it yourself once you no longer need it, or choose another name. |
| `provider versions must be exact semantic versions; …` or `provider version <v> is refused: the worker recipe accepts only versions matching …` | Use a version that both rules accept: see [declared provider versions](#provider-versions). |
| `--expect-source-digest must be 64 lowercase hexadecimal characters, as printed after sha256= by build; nothing was prepared` | Copy the value exactly. |
| `the prepared source digest <x> differs from the expected <y> (declared by the operator); …` | The source changed since that `build`, or the value comes from another source. Find out which; rebuild the images if they must match this source. |
| `at most 64 artefacts can be bound; …`, `artefact name <name> is refused: …` or `two artefacts have the name <name>; …` | Correct the number of artefacts, a name or a repeated basename. |
| `artefact <path>`, caused by the artefact rule that failed; or `artefact <path> was replaced while it was opened …`, `artefact <path> changed while it was hashed …`, `… no longer names the file that was hashed …`, `the directories leading to … changed while it was hashed …` | Correct the file or directory condition, or wait until no process writes the file, then rerun. |
| `<file>:<line>: <reason>; release manifest format 1 accepts only a complete lock it can resolve` | Format 1 reads a `Cargo.lock` version 4 with crates.io packages and one path package, and a flat `package-lock.json` version 3 with `https` registry URLs and SHA-512 integrity, whose root entry and `front/package.json` declare only `dependencies` and `devDependencies`. Another source (a Git dependency, another registry), a missing hash, an unknown key, a field of another type (`a field has a type or value outside format 1 …`: the value found is not repeated), an entry marked `devOptional`, or root `optionalDependencies` or `peerDependencies` is refused; nothing is summarised. |
| `Cargo.toml declares package <name> <version> but the root package of Cargo.lock is <name> <version>: the lock is stale and is refused` | Bring `Cargo.lock` up to date with Cargo and review the change. |
| A refusal of the source or of `TMPDIR` | Same messages and actions as for `build`: [prepared build and Compose sources](../local/README.md#prepared-source). |
| `interrupted by <signal> before publication; nothing was published and no staging directory was created; …` | Rerun when ready. During the preparation itself the message is that of `build`: `interrupted by <signal> while the source was being prepared; …`. |
| `the derivation step ended without a result; nothing was published and no staging directory was created`, or the same for `the artefact hashing step` | Rerun. |
| `the staging directory <staging> could not be created or checked; if it exists, inspect it and remove it yourself, ai-guard never adopts or removes it; nothing was published` | Read the cause; if `<staging>` exists, inspect it and remove it yourself, then rerun. |
| `the release documents could not be staged; what was written is kept at <staging> and nothing was published` | Read the cause; inspect the staging directory and remove it yourself, then rerun. |
| `the staging directory was not renamed to <output> (the cause is not reported: …); ai-guard replaced and removed nothing and did not move the staging directory it created at <staging>` or `<output> was not confirmed absent before the rename, so the staging directory was not renamed to it; …` | For the first message, an entry may exist at `<output>`, the parent or the staging directory may have changed or could not be checked again, the staging directory may not have been synchronised, or the system may have refused the rename. For the second, an entry may exist at `<output>`, that name may not have been inspectable, or the parent may have changed or could not be checked again; the reason follows the message. ai-guard did not move the staging directory; it is no longer at `<staging>` if it or a directory above it was moved. Inspect `<output>` and look for the staging directory; nothing was replaced or removed. Remove the staging directory yourself once you no longer need it, then rerun. |
| `the rename was performed but its durability is unknown; ai-guard removes neither name. After a crash the directory may be found at <parent>/<leaf> or at <parent>/.<leaf>.release-staging; run release verify --manifest <that directory> before using it` | Verify the directory where you find it before using it. |
| `the publication step ended without a result; the output may exist at <parent>/<leaf> or at <parent>/.<leaf>.release-staging; ai-guard removes neither name; run release verify --manifest <that directory> before using it` | Look at both names; verify the directory you find before using it. |
| `the documents were published at <output> but not confirmed (<cause>); the directory is kept, verify it with release verify --manifest before using it` | Run `release verify` on it. |

**Interruption.** `SIGINT` (Ctrl-C), `SIGTERM` and `SIGHUP` are stop requests. `release manifest` looks at them during the preparation, between its steps, before each artefact and after each 8 MiB of an artefact, and a last time before the documents are staged. From then on it writes three small files, renames one directory and confirms them without looking at a stop request: an interruption there could only turn a complete publication into an unexplained partial one, so the result stands. `release verify` looks at them while it hashes artefacts, prepares the source and derives it, and then ends with `UNVERIFIED: interrupted by <signal> before a result; …`. It publishes nothing; its only write is the temporary prepared copy under `TMPDIR`, handled as for `build`. A signal already ignored when the CLI starts stays ignored.

**No time bound.** Like the preparation of `build`, both commands have size bounds (the capture limits, 8 MiB per declaration file read, 64 artefacts of at most 4 GiB) but no time bound. If a read of the source or of an artefact, or a write under `TMPDIR` or in the output parent, does not return (an unresponsive network, FUSE or removable filesystem), the command does not end until that system call returns; only `SIGKILL` ends it then. A killed CLI can leave a partial copy under `TMPDIR`, to be handled as [a directory left by a killed CLI](../local/README.md#prepared-source-retained), or a `.<leaf>.release-staging` directory, which ai-guard never adopts or removes. Derived from the code; `NOT_RUN`.

<a id="data-hygiene"></a>

## Data hygiene

- Only the [build inputs](../local/README.md#prepared-source) are copied: `.env` files, `.claude/`, databases, `docs/`, `deploy/` and `tests/` are not build inputs and never enter the copy or the documents. A secret written inside a build input is copied like its other bytes, and the documents repeat text of build inputs, for example recipe `cargo build` arguments and base-image references, npm `resolved` URLs, package names and versions, licences, `[profile.release]` string values and the toolchain request.
- No document holds a host path (source, `TMPDIR`, copy, artefact directory or output parent), a device or inode number, a link count, a user ID, a host name, a time, an environment value or the `--expect-source-digest` value.
- An artefact contributes its basename, length and SHA-256 only, never its content.
- An npm `resolved` URL with user information (`user:password@`), a query or a fragment makes the lock refused, so no such URL is written. A token placed in the URL path is not detected and would be written with the URL.
- When `front/package-lock.json` or `front/package.json` holds a field of another type, the refusal names its line and column, not the value found.
- Standard output repeats `--output` as you gave it. The provenance line on standard error names the source path, device and inode, as for `build`: review logs before you share them.

<a id="limits"></a>

## Limits and open items

The `uncovered` list of every manifest names what format 1 does not cover, with the subjects concerned:

| Code | Not covered |
|---|---|
| `build-target` | The platform a build targets. |
| `base-image-digest` | The digest a build resolves behind a tag. |
| `base-image-content` | The content of base images. |
| `system-packages` | Packages that recipes install, such as APT packages. |
| `build-time-downloads` | Packages an image build downloads outside the locks, such as provider clients. |
| `toolchain-content` | The compiler that ran and its components. |
| `cargo-configuration` | Cargo configuration of base images, `ENV` and `ARG` values and the build environment. |
| `rust-applicability` | Targets and dependency kinds of crates. |
| `npm-installed-subset` | Which optional platform packages are installed. |
| `native-code-in-crates` | Native code that crates build or link. |
| `rust-licenses` | Crate licences. |
| `install-scripts` | The code that npm install scripts run. |
| `frontend-bundle` | The built frontend bundle. |
| `package-identifiers` | purl or CPE identifiers. |
| `artifact-provenance` | That an artefact was built from this source; its profile and target. |
| `image-identity` | Image IDs and digests. |

Other limits:

- No signature, trust anchor, anti-rollback or SLSA provenance; declared values are not authenticated.
- A manifest comes from its own capture: nothing ties it to the copy an engine consumed, and images are not bound to it.
- Base images are referenced by tag, not by digest, as the [operations prerequisites](../../operations.md#prérequis) state; provider clients are installed without a lock or an integrity check.
- No running binary, health endpoint or image reports the build-input identity.
- `build` accepts provider versions that the worker recipe refuses.
- On macOS the extended ACL of an artefact file is not inspected.
- A name replaced between the type read and the opening of an artefact may be opened once, without blocking and never as a controlling terminal, before it is refused; in a root-owned sticky directory, another user can do this with an entry they own.
- A replacing entry that reuses the inode number of the name that was read is not seen as a replacement: a regular file is bound, as if the swap had been made before the read; another type is opened once, then refused.
- No continuous-integration workflow verifies releases: such a workflow would depend on third-party actions that must be pinned by a verified commit digest.
- NOT_RUN: the explicit other-UID subcase and ignored helpers, product image builds and label inspection, release build, CycloneDX schema validation, real Docker/Podman workloads, native macOS and reproducible builds. Selected software parents pass only within the [current evidence scope](../../engineering/evidence/2026-11-13/claude-composition/README.md).

The gates G01 and G12 stay `BLOCKED`, and the risks RC-P3-R11, RC-P7-R07 and RC-P7-R04 stay open ([production readiness](../../engineering/PRODUCTION_READINESS.md), [risk coverage](../../engineering/RISK_COVERAGE_20261112.md) and its [risk register](../../engineering/RISK_COVERAGE_20261112.json)). These commands close no gate and no risk.
