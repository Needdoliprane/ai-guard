# Local workstation

[Français](README.fr.md) · [Setup index](../README.md) · [Trusted broker](../broker/README.md) · [AI providers](../providers/README.md)

Use this guide to run the control services in Docker or Podman, execute a first command in a worker, inspect its audit and export its changes. The trusted Rust CLI controls the engine on your workstation. The gateway and dashboard run in separate containers; workers receive a filtered copy of the project, never the original project mount.

## Prepare the workstation

You need Git, the Rust toolchain selected by `rust-toolchain.toml`, and either Docker with Compose or rootless Podman with a Compose provider. Building images requires access to their pinned dependencies; prepare images before disconnecting an offline installation. A local image build is supported; no public ai-guard image is required.

Run the following from the repository root. Replace the project path with a real, existing directory. Keep configuration outside that project and outside this checkout.

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_CONFIG="$HOME/.ai-guard"
AIGUARD_PROJECT="/absolute/path/to/your/project"
AIGUARD_GATEWAY="http://127.0.0.1:8787"
AIGUARD_DASHBOARD="http://127.0.0.1:4000"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
```

`init` creates distinct runner and viewer credentials and preserves existing credentials. The configuration directory must be private (`0700`), with private source credentials (`0600`). It also prepares the read-only projections used by containers; do not loosen the source files to make a container read them. Keep configuration, databases, session plans and credentials out of Git, screenshots and exported workspaces.

Choose one engine for this installation:

```sh
"$AIGUARD_BIN" doctor --runtime docker
"$AIGUARD_BIN" build --runtime docker --source .
```

For Podman, use:

```sh
"$AIGUARD_BIN" doctor --runtime podman
"$AIGUARD_BIN" build --runtime podman --source .
```

The normal requirement is a rootless engine. If you deliberately use Docker Desktop or another daemon that does not report rootless operation, add `--allow-rootful-runtime` to `doctor`, `build` and each `run`. This option explicitly permits a daemon that does not report rootless operation; a VM does not establish equivalence to rootless isolation. The [runtime diagnostic](../../../src/runtime.rs) reads that reported mode, but this path currently emits no `runtime.security_downgrade` event. Its dedicated durable audit evidence remains to be implemented and qualified. Do not grant the application containers an engine socket or privileged mode.

The default worker image supports ordinary commands. For Codex or Claude Code, build the client versions described in the [provider guide](../providers/README.md) before starting an AI session.

`build --source` selects the ai-guard source distribution, not the project an agent will edit. Before contacting the engine, the CLI checks the required Dockerfiles, `.dockerignore`, manifests/locks, Rust/frontend entry files and policy, including their parent directories. A missing, linked or incorrectly typed required entry is reported by its relative path. Restore that entry in the selected source distribution and retry; this refusal does not require deleting images, caches or data. An explicitly selected source root is still canonicalized normally.

That check is structural. `build` then copies its inputs before the first engine command and points the engine CLI at that copy instead of the checkout, as described in [prepared build and Compose sources](#prepared-source): a change made to the checkout after the copy does not reach the two builds. Neither step validates every import or lockfile content; `.dockerignore` is compared byte for byte with the file this CLI mirrors and is not interpreted; and the two image builds are not atomic: the first image can still build successfully before a later build failure. Retain the actual engine results, of which the CLI [prints only a part](#engine-output); a completed check and a private copy are not a reproducible-build or production qualification.

<a id="prepared-source"></a>

### Prepared build and Compose sources

Before any engine build or Compose command, the CLI `build`, `up` and `down` commands make a private, bounded copy of what they hand to the engine:

- `build` copies a fixed list of public build inputs: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `LICENSE`, `.dockerignore`, `src/**/*.rs`, `policies/*.json`, five `front/` files (`package.json`, `package-lock.json`, `tsconfig.json`, `vite.config.ts`, `index.html`), `front/src/**/*.ts` and `*.css`, `front/public/favicon.svg`, and `images/*.Dockerfile`. Both image builds use the same copy, and the structural check above runs again on it.
- `up` and `down` copy exactly `deploy/<engine>/compose.yaml` and add an empty env file.

The list of build inputs is fixed in the CLI binary and mirrors the `.dockerignore` of the distribution that CLI was built from; the file is copied, not interpreted. `build` refuses, before any engine command, a selected distribution whose `.dockerignore` has other bytes (a checkout with other line endings included) or that holds a per-Dockerfile ignore file `images/<name>.Dockerfile.dockerignore`; both messages are in the table of refused inputs below. A distribution adapted to send another path to the engine has to change its `.dockerignore`, and is then refused by a CLI whose list was not changed with it: no input is left out of the copy silently. Build such a distribution with the CLI built from it, after changing the list and its pinned SHA-256 together with the file (`Scope::selects_file` and `BUILD_IGNORE_SHA256` in `src/operations/source_snapshot/capture.rs`; a unit test fails while they differ), or run the engine build directly, which reads the checkout and not a prepared copy.

The engine CLI then receives only paths inside that copy as arguments; with Docker they follow `--host <endpoint>`:

```text
build --file <copy>/context/images/<name>.Dockerfile --tag <tag> [--build-arg <NAME>=<version>]… <copy>/context
compose --project-name agentsandbox --env-file <copy>/compose-empty.env --file <copy>/context/deploy/<engine>/compose.yaml up --detach --force-recreate
compose --project-name agentsandbox --env-file <copy>/compose-empty.env --file <copy>/context/deploy/<engine>/compose.yaml down
```

The selected source is read without following links, is never written and is never named in these arguments. Each preparation prints one line on standard error:

```text
Prepared public distribution (build): sha256=<hex> files=<n> bytes=<n> shared_writable=<n> device=<n> inode=<n> source=<path>
```

`up` and `down` print the same line with `(compose)`. `sha256` covers the relative path, length and SHA-256 of every prepared file, so it identifies the prepared bytes; it is not an image digest. `shared_writable` counts the originals that group or others may write: this is reported, not refused. `device`, `inode` and `source` identify the selected directory. The copy is checked file by file against the recorded lengths and hashes when the preparation ends, immediately before each engine command starts and again after it ended.

The copy is a new directory under the temporary directory: `TMPDIR` when it is set, otherwise the platform default, which is `/tmp` on Linux. Its directories have mode `0700` and its files `0600`:

```text
<TMPDIR>/ai-guard-distribution-<random>/
  context/                engine-visible tree: the build context, or deploy/<engine>/compose.yaml
  compose-empty.env       `up` and `down` only
  manifest.json           source path, device and inode; path, bytes and SHA-256 of each file; the digest
  RETAINED-UNKNOWN.json   only when the termination of an engine command is unknown
```

It is removed once the engine commands that use it have ended, whether the command succeeded, was refused, failed or was interrupted; `up` removes it before it waits for the health URLs. The one exception is an [unknown termination](#prepared-source-retained). The removal is by name, and only while that name still designates the directory that was created (same device and inode). Two warnings on standard error report a copy that was not removed:

```text
warning: prepared source at <path> could not be removed: <error>
warning: prepared source at <path> was not removed: the path no longer names the directory that was prepared (<another entry has that name | system error>); nothing was removed
```

The first leaves the directory, or what remains of it, in place. The second means that the copy was moved, or that something else took its name, while the command ran: neither the copy, wherever it now is, nor the entry at that path is touched, and the command has usually failed with `the prepared copy changed`. In both cases follow [a directory left by a killed CLI](#prepared-source-retained) before removing anything.

**Temporary directory.** `build`, `up` and `down` therefore need a temporary directory that passes the same private-directory policy as the configuration directory. `TMPDIR` must be an absolute path outside the selected source: both are checked before anything is created, the second by device and inode along the directories that lead to it, so that no spelling of the path hides the source. Every directory from `/` down to it must be a real directory, belong to root or to the account running the CLI, sit on a supported local filesystem (ext2/3/4, XFS, Btrfs, tmpfs, overlayfs, ramfs or F2FS on Linux; local APFS or HFS+ with ownership enabled on macOS) and not be writable by group or others, except a root-owned directory with the sticky bit, as `/tmp` usually is. A symbolic link anywhere in that path is refused, except the macOS system aliases `/tmp` and `/var`, which are mapped to `/private` (the default per-user macOS temporary directory is under `/var/folders`); an ACL that lets another principal modify one of these directories is refused too. The usual root-owned sticky `/tmp`, on one of those disk filesystems or on tmpfs, therefore passes. Three messages refuse a temporary directory, each before any engine build or Compose command:

```text
Error: the temporary directory must be an absolute path, but TMPDIR is "<value>"; set TMPDIR to an absolute directory outside the selected source
Error: the temporary directory <TMPDIR> (TMPDIR) is the selected source <source> or lies inside it; set TMPDIR to a directory outside the source
Error: cannot create the private prepared-source directory under <directory> (TMPDIR). Set TMPDIR to a directory owned by you or root, not writable by group or others, on a supported local filesystem, with no symbolic link in its path
```

The first refuses a relative or empty value (`TMPDIR=` sets an empty value, it does not unset the variable): such a value would be resolved against the working directory, which is the selected source in the commands of this guide. The second refuses a `TMPDIR` that is the source or any directory below it, whatever its spelling. Both come before anything is created. The third refuses a temporary directory on ZFS, NFS or a FUSE mount, one reached through another symbolic link, or one that group or others can write to without the root sticky bit; the policy's own reason follows under `Caused by:` and names the rule that failed, not the directory that failed it, and an empty private directory created for that check is removed at once. A `TMPDIR` whose path is not UTF-8 cannot be used either: the paths given to the engine must be UTF-8 (`the prepared-source path must be UTF-8; choose a UTF-8 TMPDIR`).

For `build`, the engine was not contacted when these refusals, or those of the next table, are raised. For `up` and `down` they come after the engine probes: with Docker, its context (unless `--runtime-endpoint` is given) and its identity; for `up`, with either engine, its information as `doctor` reads it; a Podman `down` makes none. `up` has also already created or refreshed the configuration directory and its credential projections, without recreating the services: rerun `up` once the cause is fixed.

Nothing is repaired or re-moded for you. Choose an existing directory that passes, with room for the inputs (256 MiB at most), and name it for that command:

```sh
TMPDIR="/absolute/path/to/a/conforming/directory" "$AIGUARD_BIN" build --runtime docker --source .
```

To find the directory that fails the policy, look at every directory from `/` down to the refused one: its owner, its mode and sticky bit, whether it is a link, and its filesystem type. On Linux:

```sh
AIGUARD_TMP="/absolute/path/of/the/refused/temporary/directory"
namei -l "$AIGUARD_TMP"
stat -f -c %T "$AIGUARD_TMP"
```

On macOS, run `ls -lde` on each directory in turn (it also prints ACLs) and read the filesystem type from `mount`. These are standard tools and were not run in this validation. If `/` itself, or another directory that every path shares, fails the policy, no `TMPDIR` can pass and the CLI `build`, `up` and `down` cannot be used on that host; the same policy already guards the configuration directory.

**Refused inputs.** Each refusal below comes before the engine build or Compose command and leaves the source untouched; existing files are never modified to pass a check. The cause reported by the system, when there is one, follows under `Caused by:`.

| What is refused | Message |
|---|---|
| A symbolic link at a path the command selects. For `build` that is **any** symbolic link under `src/` or `front/src/`, whatever its name, including a name the list does not select, such as `src/notes`, or an editor lock link such as `src/.#main.rs`, and a linked input elsewhere, such as `policies/extra.json` or `images/extra.Dockerfile`. For `up` and `down` it is `deploy`, `deploy/<engine>` or `compose.yaml` itself. Links at paths the command does not select are ignored. A linked required entry is reported earlier, by the structural check and with its own message. | `source input must not be a symbolic link: <path>` |
| A FIFO, device or socket where an input file is expected | `source input must be a regular file: <path>` |
| For `build`, a `.dockerignore` whose bytes differ from those this CLI mirrors | `the build input list of this CLI mirrors a .dockerignore whose SHA-256 is <pinned>; the .dockerignore of the selected distribution has SHA-256 <actual>. Build with the CLI built from that distribution, or run the engine build directly` |
| For `build`, a per-Dockerfile ignore file `images/<name>.Dockerfile.dockerignore` | `the build input list of this CLI mirrors .dockerignore only: the per-Dockerfile ignore file <path> of the selected distribution is not reconciled with it, and a builder may apply that file instead. Remove it, or run the engine build directly` |
| More than 16,384 entries looked at. For `build`: the entries of the source root, everything under `src/` and `front/src/`, and the direct entries of `policies/`, `front/`, `front/public/` and `images/`. For `up` and `down`: the entries of the source root, of `deploy/` and of `deploy/<engine>/`. A directory the command does not enter, such as `target/` or `.git/`, counts as one entry. | `source exceeds the bound of 16384 entries in its selected directories` |
| An input over 32 MiB, or a Compose file over 1 MiB | `source input exceeds the per-file bound of <n> bytes: <path>` |
| More than 256 MiB of inputs in total; the named path is where the bound was passed, not necessarily the largest file | `source inputs exceed the total bound of 268435456 bytes of the prepared copy at: <path>` |
| A selected path deeper than 64 components or longer than 4,096 bytes, not UTF-8, or holding a control character | `source path exceeds the bound of 64 components or 4096 bytes: <path>`, `source path must be UTF-8: <path>` or `invalid source path: "<path>"` |
| An input or directory that changed, disappeared, became a link or could not be read while it was being copied or confirmed | `source file changed while it was being read: <path>`, `source changed while it was being prepared: <path>`, `source directory changed while it was being prepared: <path>`, `source input is unavailable or is a symbolic link: <path>`, `source input is unavailable: <path>`, `source input cannot be read: <path>` or `source enumeration failed: <path>` |
| The selected source directory, or a directory above it, that cannot be opened without following a link; on macOS, also one that the account running the CLI cannot read | `source directory is unavailable or reached through a symbolic link: <path>` |
| The open-file limit of the process or of the system reached during the preparation | ``the open-file limit of this process or of the system was reached while the source was being prepared; raise the descriptor limit (`ulimit -n`) and run the command again`` |

In these messages a source name that was not validated is shown with its control characters and backslashes escaped and is cut after 1,024 characters; `invalid source path:` shows the name quoted and escaped. Format characters that are not control characters (bidirectional overrides, for example) are neither refused nor escaped.

Remove the link or close the editor that holds the lock, reduce the input, or wait for the tool that is writing the source; then retry. For an ignore-file refusal, use the CLI built from that distribution or remove the per-Dockerfile ignore file, as the message says. For the open-file limit, raise it in that shell (`ulimit -n <number>`, within the hard limit) and rerun: a preparation holds about two descriptors per level of depth of the deepest selected directory, one per component of the source path and about twenty more, whatever the number of selected directories. For an unavailable source directory, read the cause: on Linux the directories above the selected source need only search permission for the account running the CLI, while the source directory and the directories the command enters must be readable; on macOS every directory from `/` down to the selected source must also be readable, and `Permission denied` follows under `Caused by:` otherwise.

**A copy that changed.** If anything under `context/` was created, modified or removed while the copy was in use, including by the engine CLI itself writing into its build context or next to the Compose file, the command fails. Before the engine command: `the prepared copy changed before docker build started (<detail>); docker build was not started`. After it: `the prepared copy changed while docker build was running (<detail>); the CLI process exited (<status>) but its result is not bound to the reported digest`. When that command was also stopped by a signal or at its bound, the stop message comes first and ends with `The prepared copy also changed while it was running (<detail>)`. `<detail>` names what was found: `unexpected entry in the prepared copy: <path>`, `prepared copy content changed: <path>`, `prepared copy entry disappeared: <path>`, `prepared copy directory changed: <path>`, `prepared Compose environment file changed` or, when the copy itself was moved or replaced, `control directory path identity changed; reopen the intended private configuration`; the reason given by the private-file policy may follow it, for example `private file exceeds the configured size limit` for a file that grew. In the last two cases the engine command already ran and an image or a stack change may exist. Unless a warning says that the copy was not removed, it no longer exists when you read the message and cannot be inspected: look for what writes under `<TMPDIR>/ai-guard-distribution-*` while the command runs, check the engine's result, then rerun.

**A command that was not started.** Any other refusal between the preparation and the start of the engine command reads `docker build was not started` (or `podman compose was not started`, …), with its reason under `Caused by:`: for example an engine program that changed since the CLI approved it, or one that cannot be executed (`docker is unavailable; install and start a container runtime. No local fallback exists`). The prepared copy is removed and nothing was started; fix the cause and rerun.

**What this does and does not establish.**

- The copy is a sequence of stable per-file reads followed by a confirmation pass, not an atomic snapshot of the tree. An original is compared by device, inode, length, mode and its two timestamps: a rewrite that keeps the length of a file and lands within the precision of those timestamps is not seen, and a change made before the preparation is simply what gets copied.
- The owner, modes, link count and filesystem of the source are not judged. `shared_writable` only counts the originals that group or others may write; nothing else about them is reported. A copy does not make an untrusted source trusted. The account running the CLI, root, and the storage and engine administrators remain trusted.
- The engine CLI is given these paths; it is not confined to them. Its own configuration and plugins are read as before.
- It applies to the CLI `build`, `up` and `down` only. A direct `docker build`, `podman build` or Compose command reads the paths you give it.
- The build list knows `.dockerignore` only. `.containerignore` and `images/<name>.Dockerfile.containerignore` are not in it: they are neither copied nor compared, so the engine does not receive them through the CLI `build`. How a builder treats those names, and whether it applies a per-Dockerfile ignore file instead of `.dockerignore`, was not observed on a real engine.
- The removal of the copy is by name after an identity check. This is a check, not a lock: between the comparison and the removal, a process running as the CLI account or as root can still swap the directory; other accounts cannot under the temporary-directory policy. The identity recorded is what the path named right after creation, not that of a descriptor.
- Compose metadata after `up` (`NOT_RUN` on a real provider). `up` hands Compose a file and an env file under `<TMPDIR>/ai-guard-distribution-<random>/` and removes that directory when the command ends. A provider that records its config files, working directory and env file in project labels then names paths that no longer exist, and under the default Linux `/tmp` another local account can later create a directory of that name. ai-guard never reads those labels: `down` prepares a new copy and addresses the project by name, and the shipped Compose files contain no relative path. A third-party tool or a manual command that re-reads the label path would read whatever is there; none was run. Address the stack by `--project-name agentsandbox` or by the checkout's Compose file, as in [follow a stack started by the CLI `up`](#cli-up-stack), never by the label path, and on a host shared with other local accounts run `up` with a private `TMPDIR` (owned by the account, mode `0700`).
- Compose provider acceptance of `--env-file` (`NOT_RUN`). The CLI `up` and `down` add `--env-file <empty prepared file>` to the Compose command; with Podman the whole command is passed to the external provider. No provider was run, and the fixture that stands for it accepts any argument. The direct Podman commands of this guide already use `podman compose --env-file`: it is the same provider requirement, now also needed by the CLI. If a provider rejects the option, `up` and `down` fail with only `podman command failed (<status>)`; the provider's standard error is captured but [not printed](#engine-output).
- Descriptors. Executed on Linux: 300 selected directories build under a limit of 128 descriptors, and a selected chain 40 levels deep is refused under a limit of 64 with the message that names the limit. Computed, not measured: about 170 descriptors for the deepest accepted source (64 levels), below the usual macOS soft limit of 256.
- Directories above the source. Executed on Linux: a source below a directory that the account can only traverse is prepared, with a directory of that same account. `NOT_RUN`: a directory owned by another account, and macOS, where such a source is still refused. On every platform the temporary directory and, for `up` and `down`, the configuration directory need readable ancestors (private-directory policy).
- It was exercised with Rust fixture processes standing in for the engine CLI, on Linux ARM64, including a command run without `TMPDIR` under the root-owned sticky `/tmp` of that system. A real Docker or Podman engine, a real Compose provider, macOS and a second local account writing to the source or the temporary directory are `NOT_RUN`. In particular, no real engine CLI was observed reading a `0700`/`0600` context, and no real Compose provider was observed with an empty `--env-file` and no `--project-directory`.
- Documented from the code, without an executed test: the temporary-directory refusals for an unsupported filesystem (ZFS, NFS, FUSE), another owner, an ACL and a path that is not UTF-8; a link on `deploy` or `deploy/<engine>` (the `compose.yaml` case ran); and the messages `source file changed while it was being read`, `source input is unavailable:`, `source input cannot be read:` and `source enumeration failed`. Through the CLI binary, `<engine> <verb> was not started` ran for a copy altered before the start only; its other causes ran at the level of the retained consumer and on constructed ends.

## Create the stack configuration

The [local example](../../../.env.exemple.local) controls Compose interpolation. Create a **new** private copy; the subshell below refuses to overwrite an existing file.

```sh
(
  umask 077
  set -C
  cat .env.exemple.local > "$AIGUARD_CONFIG/local.env"
)
```

Open that new file in your editor. Keep `AGENTSANDBOX_CONFIG_DIR` identical to the directory passed to `init`. `COMPOSE_PROJECT_NAME` identifies this stack and its named audit volume. `AI_GUARD_CONTROL_IMAGE` selects the control image; the build above creates `agentsandbox-control:local`. A different image name requires an image you have built or obtained through your own trusted distribution.

`AI_GUARD_GATEWAY_PORT` and `AI_GUARD_DASHBOARD_PORT` change the host ports, still bound to `127.0.0.1`. Published container ports remain `8787` and `4000`; control port `8788` stays internal as described below. If you change host ports, update `AIGUARD_GATEWAY` and `AIGUARD_DASHBOARD` in your shell too. The Rust CLI does not read this dotenv file. Never `source` it; pass it explicitly to Compose. Shell environment values can override Compose interpolation, so check for an inherited setting when a value appears wrong.

Start Docker services:

```sh
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml up -d
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml ps
"$AIGUARD_BIN" healthcheck --url "$AIGUARD_GATEWAY/healthz" --url "$AIGUARD_DASHBOARD/healthz"
```

Or start the Podman variant:

```sh
podman compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/podman/compose.yaml up -d
podman compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/podman/compose.yaml ps
"$AIGUARD_BIN" healthcheck --url "$AIGUARD_GATEWAY/healthz" --url "$AIGUARD_DASHBOARD/healthz"
```

Use the same engine account, context/endpoint, env file and Compose file throughout. Keep to this direct Compose workflow when customizing ports or project names; the CLI `up/down` workflow has its own fixed project and default readiness URLs. The CLI `up` and `down` hand Compose a [private copy](#prepared-source) of `deploy/<engine>/compose.yaml` and an empty `--env-file`, without `--project-directory`, in an environment that carries `AGENTSANDBOX_CONFIG_DIR` from `--config-dir` and none of your shell's Compose variables: the directory of that copy holds no `.env`, so a `.env` placed beside `deploy/<engine>/compose.yaml` is not read by these two commands and the Compose file's defaults apply. The direct `docker compose --env-file …` and `podman compose --env-file …` commands above are unchanged. How a real Compose provider treats the CLI's arguments was not verified.

<a id="cli-up-stack"></a>

### Follow a stack started by the CLI `up`

The CLI `up` creates its stack under the fixed Compose project name `agentsandbox`, from a temporary copy of `deploy/<engine>/compose.yaml` and an empty env file that are both removed once the Compose command has ended, before the wait for the health URLs. The only variable it gives Compose is `AGENTSANDBOX_CONFIG_DIR`, the absolute path of `--config-dir`. For this stack, "the same configuration" therefore means that project name, that engine and that variable, not a file that still exists. What follows is written from the arguments and the environment the CLI builds; none of these commands was run against Docker Compose or a Podman Compose provider.

- **Project.** Address the project by its name, `--project-name agentsandbox`. A Compose provider that records the Compose file, the working directory and the env file of a project in the labels of its containers names, for this stack, paths under the removed `<TMPDIR>/ai-guard-distribution-<random>/` directory: never point a command or a tool at such a path (see the Compose metadata limit under [prepared build and Compose sources](#prepared-source)). The direct commands above, run with `--env-file "$AIGUARD_CONFIG/local.env"`, address the project named by that file's `COMPOSE_PROJECT_NAME` (`ai-guard-local` in the example), not this one.
- **Engine.** With Docker the CLI targets one endpoint, `--host <endpoint>`: the value of `--runtime-endpoint` when it was given, otherwise the Docker host of the account's current context, which the CLI reads with `docker context inspect --format '{{json .Endpoints.docker.Host}}'` in a rebuilt environment that carries the account's home directory and `DOCKER_CONFIG=<home>/.docker`, and neither `DOCKER_HOST` nor `DOCKER_CONTEXT` from your shell. `doctor` does not print that endpoint. A direct `docker` command run in a shell where these variables are set can address another engine: pass the endpoint explicitly. With Podman the CLI adds no engine-global option, and `--runtime-endpoint` is refused for these commands.
- **Variable.** A direct command that is given the checkout's Compose file needs `AGENTSANDBOX_CONFIG_DIR` set to the same configuration directory: the file requires it (`${AGENTSANDBOX_CONFIG_DIR:?Run agentsandbox init first}`).

For Docker, read the endpoint the way the CLI does when no `--runtime-endpoint` was given. The first command prints a JSON string such as `"unix:///var/run/docker.sock"`; use it without its quotes:

```sh
env -u DOCKER_HOST -u DOCKER_CONTEXT -u DOCKER_CONFIG docker context inspect --format '{{json .Endpoints.docker.Host}}'
AIGUARD_ENDPOINT="unix:///the/endpoint/printed/above"
docker --host "$AIGUARD_ENDPOINT" compose --project-name agentsandbox ps
docker --host "$AIGUARD_ENDPOINT" compose --project-name agentsandbox logs --tail 100 gateway dashboard ingress
```

For Podman:

```sh
podman compose --project-name agentsandbox ps
podman compose --project-name agentsandbox logs --tail 100 gateway dashboard ingress
```

Where a provider needs a Compose file to find the project, add the checkout's file and the variable; `$AIGUARD_CONFIG` stands for the directory given to `up` as `--config-dir`:

```sh
AGENTSANDBOX_CONFIG_DIR="$AIGUARD_CONFIG" docker --host "$AIGUARD_ENDPOINT" compose --project-name agentsandbox --file deploy/docker/compose.yaml ps
AGENTSANDBOX_CONFIG_DIR="$AIGUARD_CONFIG" podman compose --project-name agentsandbox --file deploy/podman/compose.yaml ps
```

To stop or recreate that stack, use the CLI `down` and `up` with the same `--runtime`, `--runtime-program`, `--runtime-endpoint` and `--config-dir`: each prepares a new copy and addresses the project by name. Review logs for secrets before sharing them.

### HTTP ingress limits

The public Rust ingress has independent budgets for **16 request bodies being read** and **64 upstream exchanges**. Each body is limited to **256 KiB**, **5 seconds in total** and **2 seconds without a new chunk**, starting when body reading begins. The complete body is read and its upload permit released before an exchange is acquired. A body known to be empty from its stream state bypasses the upload budget; the HTTP method or declared `Content-Length` does not decide this. Public routes retain their fixed destinations and upstream authentication/authorization. These bounds are fixed in the binary; the dotenv file and CLI expose no setting for them.

At most **48 SSE responses** can retain exchange slots at once, counted within the total of 64. The ingress recognizes the actual upstream `Content-Type` before hop-by-hop headers are removed, ignoring media-type case and allowing parameters. A request path, `Accept` header or claimed tenant does not select this budget. When it is full, a new SSE response returns **429** with `Trop de flux simultanés.`; downstream body consumption or drop, and pump failure, release its permits. The limit is fixed in the binary.

A full upload budget can return **429** for a request with a body. SSE retention alone leaves 16 total slots outside that retention budget; pending upstream headers and other responses still share the total of 64, and can exhaust it. These bounds provide neither per-tenant stream fairness nor general flood protection, and do not qualify runtime isolation. After an acknowledged start, the runner's read-only monitor permits at most 60 seconds for classified transient failures, including the first failing request's timeout; the original execution deadline and an observed STOP take precedence. Authentication, identity, audit and unclassified 503 failures still request a targeted stop. Preserve the recorded session and reconcile it after an interruption rather than launching another instance to hide an unknown result. See the [monitor contract](../../engineering/GATEWAY_TRANSIENT_FAILURE_PROPOSAL.md) and its qualification limits.

### Explicit control transport

The updated Compose files configure `gateway --control-listen 0.0.0.0:8788` and `ingress --control-upstream http://gateway:8788`. The gateway flag is optional; the ingress flag is required and has no public-upstream fallback. Port 8788 stays on the existing internal network, with no new host port or credential owned by the relay. The base Docker/Podman files, their SSO profiles and both Docker local-model TLS overrides retain this connection. Preserve the existing data, credentials and host loopback bindings when adapting a deployment.

The runner and typed control clients explicitly select `/control/`, using the dedicated destination outside the public 16/64 pools. The gateway authenticates the existing identity before reading the body or reserving one of the tenant's **16 runner** or **4 broker-presence** operation slots. The [transport contract](../../engineering/INGRESS_CONTROL_TRANSPORT.md) lists the closed routes, body/response bounds and errors. Keep `AIGUARD_GATEWAY`, `--gateway` and broker registry values as origins without a path; the clients add the prefix. Do not append `/control` or point workstation clients at the container-only port 8788. No field is added to strict plans 1/2/3.

Runner control identity, admission, decisions, typed control reads/writes and trusted control events use that channel. Ordinary ingestion, public queries and the authenticated protocol diagnostic remain public. Selecting runner control is explicit; its refusal does not fall back to that diagnostic. The broker-presence client uses its three scoped routes; it gains no runner authority. The mixed broker outbox still needs durable provenance and cannot be promoted from event kind/source. Update CLI, gateway and ingress together: a missing control route on an older gateway is refused without public fallback. The frozen SDK v2 passes [22 targeted software checks and all-target compilation](../../engineering/evidence/2026-11-07/control-clients/README.md), with warnings retained. The [combined control/recovery checks](../../engineering/evidence/2026-11-07/control-recovery-composition/README.md) pass 52 software functions and all-target compilation. Runtime qualification remains open.

`control_operation_capacity_exhausted` is distinct from public-pool 429 and audit quotas. A closed `control_transport_failure` response reports an **unknown** result, not rollback. Preserve the operation identity and reconcile its existing receipt. Only eligible post-ACK monitor GETs recognize a validated transport DTO with coherent 502/504; a protocol error, malformed response or audit 503 remains an error. This adds no retry for mutations, starts, claims or mandatory audit; the start callback retains one attempt.

## Open the control view

Open `AIGUARD_DASHBOARD` in your browser. The gateway URL is for CLI/API access; `/healthz` reports service availability, not permission to run a worker or proof that an AI provider is authenticated.

The `init` and base Compose commands above prepare technical runner/viewer access, not a named account. For a personal view without an IdP, follow [named local access](../identity/README.md): migrate the existing tenant store explicitly, provision an account and grant its real resources before signing in. Select an offered administrative context only when authorized. Personal resources require an explicit owner or share grant; tenant membership and a matching session name give no access or authorship. [Keycloak/OIDC](../sso/README.md) is a separate identity choice and does not select your AI provider.

Keep technical credentials separate from that identity: `viewer` grants its configured read scope and `runner` belongs to trusted control. For a viewer-key connection, obtain the viewer value from the private source file in a private editor and enter it only in the trusted connection form. Never paste the runner credential into a browser, worker or chat. Older events identified only by a shared key remain shared-key history; a later login does not establish their human author.

The dashboard presents session provenance, decisions, runtime observations and changes. A stale connection, partial coverage, denied request and empty result are different states. Use the displayed last observation and scope when deciding whether to refresh, reconnect or request access.

Open an agent or sandbox to inspect its explicitly linked workers, then open a worker to return to its related agents and sandboxes. Links use exact session or runtime identifiers from the authenticated tenant's data; their provenance is displayed, and a matching session does not prove execution or delegation. The worker registry is paginated: navigate its pages before concluding that no matching record was received. A failed read retains a stale result with a retry action; it is not an empty inventory. The links combine audit projections and registry records, rather than querying an engine or discovering remote brokers.

In the technical viewer's worker table and details, **Autorité déclarée du lancement** identifies the declared launch authority; **Autorité de la dernière inspection enregistrée** identifies the last recorded inspection's authority, alongside its runtime and observation date. An absent or null authority remains unknown, with no fallback from the launch to an inspection. Supplied IDs must be nonempty strings, preserved exactly and limited to 256 UTF-8 bytes without control characters; invalid values produce a contract error. These fields establish neither an associated host or broker, current availability nor a complete inventory. See the [worker view contract](../../ui.md#workers-quotas-et-allocations).

A 401/403 refusal on an active read closes the viewer session and removes its data, including open details. Reconnect with an authorized credential; repeated refreshes cannot restore a refused credential. A service error is shown explicitly. In the worker, quota and allocation views, the last successful data remains marked stale within the same session and scope until a successful refresh.

## Execute a first command

The commands below use the technical runner credential retained in `AIGUARD_CONFIG`, independently of a human dashboard login. They do not create authenticated human initiation or delegation; the [human-to-broker execution contract](../../engineering/HUMAN_EXECUTION.md) remains proposed. Start with a small offline command using the strict policy and disabled worker networking:

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile strict --network none --timeout 30 -- /usr/bin/id
```

For Podman, replace `--runtime docker` with `--runtime podman` in every `run`. Record the returned session UUID. Inspect its decision and observed completion in the dashboard, then read the registry and verify the local audit chain:

```sh
AIGUARD_SESSION="replace-with-the-returned-session-uuid"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" verify-audit --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

`verify-audit` checks the gateway's local hash chain. Keep that result distinct from independent signed checkpoints and external anchoring. A successful admission is also distinct from a runtime start or a completed command.

To make a deliberate change only in the copied workspace:

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network none --timeout 30 -- /usr/bin/touch /workspace/ai-guard-example.txt
```

This is a separate session with a fresh project copy. `observe` does not mean that every transitive effect is mediated or understood. Select `review` when a request requires human approval: inspect the exact request and use its full approval hash with `--approve-hash` when resubmitting that request. Changed inputs need a new decision; never approve an unrelated hash to get past a refusal.

## Verify the audit for a SetupV2 tenant

In the prepared installation's `tenants.json`, find `viewer_token_file` for the authorized tenant. Use the corresponding private file as seen from the machine running the CLI. The gateway selects the tenant from that credential.

```sh
AIGUARD_VIEWER_FILE="/absolute/private/installation/tenants/<tenant>/viewer"
"$AIGUARD_BIN" verify-audit --gateway "$AIGUARD_GATEWAY" --viewer-token-file "$AIGUARD_VIEWER_FILE"
```

Omit `--viewer-token-file` to read `<config-dir>/secrets/viewer`. An unreadable explicit file fails the command. To add the offline signed report, use the [checkpoint guide](../audit-signatures/README.md) and select the database, checkpoints and anchor for the same tenant.

## Inspect and export changes

Select the session that created the file, then use its UUID. The export parent must exist and the destination must be **new**, outside the original project. Do not use an existing working tree as the export destination.

```sh
AIGUARD_SESSION="replace-with-the-file-session-uuid"
AIGUARD_EXPORT="$HOME/ai-guard-export-new"
"$AIGUARD_BIN" export-workspace --session "$AIGUARD_SESSION" --output "$AIGUARD_EXPORT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

Review the exported files and diff before applying selected changes to your project. ai-guard does not rewrite the original automatically. A failed export is not an empty diff; if a destination was published before a later error, inspect and preserve it before choosing a different new destination.

Exports omit known sensitive/cache names excluded on import and tool metadata including `.husky`, `.envrc`, `.idea`, `.vscode`, `.devcontainer`, `.direnv`, `.pre-commit-config.yaml`, `.gitattributes`, `.gitmodules`, `.mise.toml` and `.tool-versions`. Matching includes case variants and specific Unicode aliases of reserved names; ordinary Unicode filenames remain available. A Git-directory-shaped tree (`HEAD` with `objects`/`refs`, or `HEAD` with `commondir`) is refused before publication, including nested directories and implicit parents. Worker-authored `.git` configuration and hooks are not restored. Regular files are written as `0644`, including files the worker marked executable. Ignored entries still undergo path, type, duplicate and size validation: filtering does not make an invalid archive acceptable.

The export manifest records published files and modes; the workspace scan records the original observed modes. New observations can therefore distinguish a permissions-only change from a content change. Historical manifests without a mode retain unknown mode coverage; unchanged content does not prove unchanged permissions. `workspace.export.completed` reports `omitted_entries`, counting explicit archive entries (files and directories), not implicit parent directories. A historical event without this field does not report zero omissions.

The control emits a generic warning with the omission count, without exposing names. If this warning cannot be written, publication is refused. **Exported content remains untrusted.** Some tool files, including `.cargo/config.toml`, `Makefile` and `package.json`, remain present. The policy does not neutralize every tool's configuration or detect arbitrary secrets. Inspect files as text before opening the project in a tool that can run configuration, installing dependencies or deliberately restoring executable permissions.

The copy excludes known sensitive names and dependency/cache directories, but it is not a universal secret scanner. Review the source project's contents before import. Keep keys outside it. Bounds include 20,000 entries, 128 MiB total and 32 MiB per file; links and unsafe paths are rejected. A bound violation requires reducing the input or changing an approved limit, not silently treating a partial copy as complete.

## Diagnose a refusal or interruption

| Symptom | Check and next action |
|---|---|
| Engine missing, unsafe executable or wrong endpoint | Run `doctor` under the same account and repeat explicit `--runtime-program`/`--runtime-endpoint` choices on the commands that accept them. Do not fall back to running the task on the host. |
| Non-rootless daemon refused | Configure rootless operation or deliberately apply the per-command exception described above. |
| `build`, `up` or `down` refuses a source input, an ignore file or the temporary directory | Read the named path and cause against [prepared build and Compose sources](#prepared-source): a link, a special file, a bound, a change during the preparation, a `.dockerignore` that differs from the one the CLI mirrors, an unreadable directory above the source, the open-file limit or the `TMPDIR` rules. Fix the checkout or choose a conforming `TMPDIR`; nothing is repaired automatically and the source is never modified. |
| `build`, `up` or `down` reports `the prepared copy changed` | Something wrote into the private copy while it was in use, possibly the engine CLI itself. The engine command may already have run: check its result, find the writer and rerun. See [prepared build and Compose sources](#prepared-source). |
| `build`, `up` or `down` ends with `docker command failed (<status>)` or `podman command failed (<status>)` | The engine command failed by itself. The CLI printed none of its output and the prepared copy is removed. Read [what the CLI shows of the engine's output](#engine-output) to obtain the engine's own diagnostics, then rerun. |
| `build`, `up` or `down` refuses to start with `SIGCHLD is ignored in this process` | The program that started the CLI ignores `SIGCHLD`; nothing was prepared and no engine command was started. Start the command with the default disposition: see [interrupt or time out a build or Compose command](#interrupt-build-or-compose). |
| `build`, `up` or `down` interrupted or past its bound | The message states what was observed about the local engine CLI process; see [interrupt or time out a build or Compose command](#interrupt-build-or-compose). Check the engine's own state, then rerun the same command. An interrupted `up` may leave a partial stack: rerun `up` or run `down`. |
| `termination is unknown`, a `was not removed` or `could not be removed` warning, or an `ai-guard-distribution-*` directory left in the temporary directory | Follow [recover a retained prepared source](#prepared-source-retained) before removing anything. ai-guard does not remove that directory. |
| Stack unavailable, or `stack started but readiness did not pass within 45 seconds; inspect docker compose logs` | Check Compose `ps` and bounded `logs --tail 100 gateway dashboard ingress` with the same configuration: the same env file and Compose file for the direct workflow, or the project `agentsandbox` addressed by name for a stack started by the CLI `up`, as in [follow a stack started by the CLI `up`](#cli-up-stack). Check host-port collisions and private credential projections. Never publish raw logs without reviewing them for secrets. |
| HTTP 429 through ingress | Distinguish the [upload/exchange budgets](#http-ingress-limits) from upstream quotas using the returned message. Reduce parallel requests and retry a read after capacity is available. Check the recorded session before repeating a launch. |
| HTTP 408 or 413 while sending a body | Check the 5-second total/2-second idle deadlines and 256-KiB body limit. Reduce the payload or fix a stalled sender; changing the dotenv file does not extend these bounds. |
| HTTP 503 with audit descriptor exhaustion (`EMFILE`/`ENFILE`) | Preserve the session and operation identity. Once descriptors are available, retry an authorized read that reaches Store validation; see [audit descriptor recovery](#audit-descriptor-recovery) before replaying a mutation. |
| Unauthenticated or forbidden | Reconnect with the appropriate identity and authorized tenant/scope. Do not substitute a runner key for a missing user permission. |
| Policy deny or review | Read the decision reason and requested scope. Adjust the request or obtain approval for that exact request. |
| Project copy or export refused | Check size, links, destination freshness and directory permissions. Preserve the original and any already published export. |
| Runtime unavailable or status unknown | Preserve the session, plans and volume. Query status and reconcile the recorded runtime authority; do not create another start to hide uncertainty. |
| Provider login or client unavailable | Follow the [provider diagnosis](../providers/README.md#diagnose-provider-problems); health checks do not validate provider access. |

<a id="interrupt-build-or-compose"></a>

### Interrupt or time out a build or Compose command

During `build`, `up` and `down`, a `SIGINT` (Ctrl-C), `SIGTERM` or `SIGHUP` received by the CLI process is a stop request, not the immediate end of the CLI. The engine build or Compose command runs in its own process group, so the CLI forwards the signal to that group once, waits up to 10 seconds (less if the command's own bound elapses first), then sends `SIGKILL` to the group. A further signal at least one second after the first ends that wait at once; signals within the first second count as the same request. The CLI reports a successful command only after collecting the real exit status of the engine CLI process **and** reaching end of file on both of its output pipes. When a stop was requested and those facts are collected, it removes the [prepared copy](#prepared-source), prints what it observed and exits with status **1**, not `128 + signal`. If either fact remains unknown after the bounded collection following `SIGKILL`, it instead returns an error saying `termination is unknown` and keeps the copy for [recovery](#prepared-source-retained), without claiming that the process or its consumers have ended. No further engine command of that invocation is started: an interrupted first image build is not followed by the second.

| When the signal arrived | Message, after `Error: ` | Prepared copy |
|---|---|---|
| No engine build or Compose command was running, for example during an engine probe | `interrupted by SIGINT: no engine build or Compose command was running` | Removed if one existed |
| While the source was being copied | `interrupted by SIGINT while the source was being prepared; no engine build or Compose command was started` | The partial copy is removed |
| Just before the engine command was started | `interrupted by SIGINT: docker build was not started` | Removed |
| While the engine command ran, and it ended within the wait | `interrupted by SIGINT: SIGINT was forwarded to the process group of the CLI process for docker build; that process exited (<status>) and both of its output pipes reached end of file` | Removed |
| Same, but `SIGKILL` was needed | `interrupted by SIGINT: SIGINT was forwarded to the process group of the CLI process for docker build, then SIGKILL was sent; that process exited (signal: 9 (SIGKILL)) and both of its output pipes reached end of file` | Removed |
| The end could not be established within 2 seconds after `SIGKILL` | `docker build termination is unknown: …` | **Kept**: see [recover a retained prepared source](#prepared-source-retained) |

The messages name the signal received and the engine command (`docker build`, `podman compose`, …). A signal that was already ignored when the CLI started, under `nohup` for example, stays ignored: the CLI keeps running and forwards nothing. When the preparation itself ends without a result, which no input is known to produce, the message is `the source preparation ended without a result; no engine build or Compose command was started`: look for a leftover copy as for [a directory left by a killed CLI](#prepared-source-retained), then rerun.

**A process that cannot wait for its children.** `build`, `up` and `down` refuse to start in a process whose `SIGCHLD` is ignored (inherited from a supervisor, script or CI wrapper that ignores it) or carries `SA_NOCLDWAIT`: the wait status of an engine CLI process could not be obtained, so the end of an engine command could not be established.

```text
Error: SIGCHLD is ignored in this process, inherited from the program that started this CLI: the wait status of an engine CLI process could not be obtained, so the end of an engine command could not be established. Start this CLI with the default SIGCHLD disposition. Nothing was prepared and no engine command was started
```

The exit status is 1. Nothing is copied, `up` does not create or refresh the configuration directory and no engine command is started; the disposition is read, never changed. The variant for the flag begins with `SIGCHLD has SA_NOCLDWAIT set in this process`. Start the command from a program that leaves `SIGCHLD` at its default, an interactive shell for example, or correct the wrapper. GNU coreutils `env` can reset it for one command, `env --default-signal=CHLD "$AIGUARD_BIN" build --runtime docker --source .`; that example was not run in this validation. The other commands of the CLI do not read this disposition.

**Bounds.** Each image build is bounded to 30 minutes and each Compose command to 180 seconds. Both bounds are fixed in the binary: no option, variable or dotenv setting changes them. At the bound, `SIGKILL` is sent to the process group and the result is reported after the same collection:

```text
Error: docker compose exceeded its 180 s bound: SIGKILL was sent to the process group of the CLI process for docker compose; that process exited (signal: 9 (SIGKILL)) and both of its output pipes reached end of file
```

When the engine CLI had already exited and a process it started kept an output pipe open until the bound, the status in parentheses is the CLI's own, for example `exit status: 0`, and the message adds `The CLI process had already exited when SIGKILL was sent; the command was not collected earlier, usually because a process it started kept an output pipe open`. That process belongs to the signalled group and is killed with it. The command is still reported as failed, and a `build` does not go on to its second image.

<a id="engine-output"></a>

**What the CLI shows of the engine's output.** While an engine build or Compose command runs, the CLI displays nothing of its output: both streams are captured. After a command that succeeded, it prints the command's standard output, at most 256 KiB; a longer build log is cut there and announced by `Build log was truncated to 256 KiB; runtime reported successful completion.`, and a longer Compose output is cut without a notice. The engine's standard error is captured, at most 64 KiB, and never printed. After a failure, a bound or an interruption, neither stream is printed. A command that failed by itself is reported by its exit status only:

```text
Error: docker command failed (exit status: 1)
```

These sizes are fixed in the binary too. To read the engine's own diagnostics, run the engine command directly from the checkout, with `podman` in place of `docker` for Podman, and with the `--host <endpoint>` and the `--build-arg` pairs the CLI would have used:

```sh
docker build --file images/control.Dockerfile --tag agentsandbox-control:local .
docker build --file images/worker.Dockerfile --tag agentsandbox-worker:local .
```

A direct command reads the live checkout, not a prepared copy, applies none of the checks of [prepared build and Compose sources](#prepared-source) and was not run in this validation. A build that needs more than 30 minutes can meet the same bound again when the CLI command is rerun unchanged; a direct engine build has no such bound.

**What to do next.** The exit status and the message describe the local engine CLI process only. Check the engine's own state under the same account and endpoint (`docker image ls` and `docker ps --all`, or `podman image ls` and `podman ps --all`; for a stack started by `up`, the commands of [follow a stack started by the CLI `up`](#cli-up-stack)), then rerun the same command with the same options. `build` builds both images again. An interrupted or timed-out `up` may leave the stack partly or fully started: rerun `up`, which recreates the services, or run `down`. Rerun an interrupted `down`. Do not delete images, volumes or configuration to recover.

**Limits.**

- These are local facts: the exit status of the engine CLI process, end of file on its two output pipes and the signals the CLI sent. Nothing is established about work done by an engine daemon, or about a process started by the engine command that no longer holds either of its two output pipes, whether or not it stayed in the command's process group: when the command ends by itself or on the forwarded signal, no further signal is sent and the group is not inspected, so such a process can still be running, and reading the copy, when the CLI removes it. After `SIGKILL`, the same holds for a process that left the group or that your account may not signal. Executed with fixture processes on Linux ARM64, at the level of the retained consumer: a helper started with its output on `/dev/null`, in the process group of the command, is alive after the command was collected and its input released, both when the command exits by itself and when it ends on a forwarded `SIGINT` that the helper ignores. That such a process reads the copy when the CLI removes it is the consequence read from the code.
- A CLI that is itself killed (`SIGKILL`, a crash, power loss; `SIGQUIT` keeps its default action) can neither forward a signal nor clean up. The engine command, in its own process group, is not signalled by ai-guard, and the copy stays without a marker: see [recover a retained prepared source](#prepared-source-retained).
- A job-control stop (Ctrl-Z) suspends only the CLI; the engine command continues. While it is stopped, the CLI reads none of the engine command's output, so that command can block once a pipe is full, and the 30-minute or 180-second bound keeps running; when the CLI resumes, a command past its bound is killed at once and reported as `exceeded its … bound`. Derived from the code, not executed.
- The preparation has entry and byte bounds but no time bound. A stop request (`SIGINT`, `SIGTERM`, `SIGHUP`) received during the preparation takes effect between two entries of the copy or two checks of the confirmation; a repeated signal does not escalate there. If a read of the source or a write under `TMPDIR` does not return (unresponsive network, FUSE or removable filesystem), the CLI does not end until that system call returns. Only `SIGKILL` or `SIGQUIT` ends it, leaving a partial `ai-guard-distribution-*` directory without manifest or marker: handle it as [a directory left by a killed CLI](#prepared-source-retained). Derived by reading; `NOT_RUN`.
- At the bound of a command, `SIGKILL` is sent to the whole process group of the engine CLI, also when that CLI process had already exited with status 0 and only a process it started still holds an output pipe; the command is then reported as failed. An operator signal received in that state is forwarded to that same group: with fixture processes, the holder ends on it and the command is collected with exit status 0, without `SIGKILL`. Whether a real engine leaves such a process after `up --detach` (first candidate: rootless Podman with an external Compose provider) is `NOT_RUN`; if one does, `up` is reported as failed at the 180-second bound and that process is killed with the group.
- The engine command is not in the terminal's foreground process group and its standard input is `/dev/null`. A helper that prompts on the terminal, a credential helper for example, is not supported by these commands and, by the job-control rules, would stay stopped until the bound: unlock or log in to the engine beforehand. This was derived, not executed.
- If the signal arrives while `up` waits for the health URLs, the Compose command has already ended: the message is the first one of the table and the services Compose started are left as they are. This follows from the code path and was not exercised by a test.
- The `SIGCHLD` refusal was executed on Linux ARM64 for an inherited ignored `SIGCHLD` (`down` with a Podman-kind fixture engine, `build`, `up`). Not executed: `SA_NOCLDWAIT`; macOS, where whether an ignored `SIGCHLD` survives `exec` was not determined.
- macOS `NOT_RUN`: whether `kill(2)` on a process group whose only member is the exited, unreaped engine CLI process is accepted was not established on Darwin (Linux accepts it). If Darwin refuses it, an unknown termination reads `<SIGNAL> was forwarded to process group N, SIGKILL was not sent` (or `no signal was sent to process group N` when the forwarded signal was refused too), without `2 s later`, and `RETAINED-UNKNOWN.json` records `kill_sent: false` although `kill(2)` was called. A command that is collected reports only the signals that were accepted (`no signal was accepted by the process group of the CLI process for …` when none was). Retention is unaffected: nothing in the owner of the command depends on the return value of `kill`.
- Exercised with signals sent to the CLI's process ID and Rust fixture processes standing in for the engine CLI, on Linux ARM64, including the real 10-second wait, a command that failed by itself and a build log beyond 256 KiB. The real 180-second Compose bound is exercised by one test that is ignored by default: it is `NOT_RUN` in `cargo test` and in every default selection, and runs by name in about three minutes:

  ```sh
  cargo test --test source_lifecycle -- --ignored --exact paths::r15_compose_command_whose_output_is_held_past_its_real_bound_is_killed_and_reported --nocapture --test-threads=1
  ```

  `NOT_RUN`: a Ctrl-C delivered by a real terminal to a foreground process group, a real Docker or Podman engine (how its CLI reacts to the forwarded signal or to `SIGKILL`, whether daemon-side work stops), macOS and the 30-minute build bound.
- Documented from the code, without a test executed through the CLI binary: `<engine> <verb> was not started` after an interruption, the `SA_NOCLDWAIT` variant, the message of a preparation that ends without a result, and the message of a command killed at its bound while its CLI process still ran, shown in the block above (the test of the real bound runs the variant in which the CLI process had already exited). The first and the last ran at the level of the retained consumer and on constructed ends.

<a id="prepared-source-retained"></a>

### Recover a retained prepared source

When the end of an engine command could not be established, the command exits with status 1 and names the directory it keeps. The process group and path below are example values:

Paths in these messages escape control characters and backslashes. A path longer than 1,024 characters is abbreviated with `…`; these are presentation rules, not changes to the directory name or the source recorded in `manifest.json`. Do not use an escaped or abbreviated message as a filesystem path: identify the actual directory under the effective `TMPDIR` before setting `AIGUARD_RETAINED` below. For subsequent commands, a short temporary-directory path with printable names makes this recovery easier.

```text
Error: docker build termination is unknown: SIGTERM was forwarded to process group 4242, then SIGKILL was sent and, 2 s later, stdout was still open and stderr was still open. The prepared copy is kept at /tmp/ai-guard-distribution-AbC123 and is not removed by ai-guard; see docs/setup/local/README.md#prepared-source-retained
```

It means that the CLI could not collect the engine CLI process within its observation time: the process was not reaped, its exit status could not be obtained, or one of its output pipes was still open, usually because a process it started is still running outside the signalled process group. A process may therefore still be reading the copy. ai-guard keeps the directory, sends no further signal and removes nothing automatically in this state, neither when the CLI exits nor in a later command. A system temporary-file cleaner, if one is configured, acts independently of ai-guard.

The same procedure applies to `<engine> <verb> ended without a collected result. The prepared copy is kept at <path> and is not removed by ai-guard`, an internal failure of the CLI for which the presence of a marker is not established: skip step 2 when there is none.

1. Read why it was kept. `manifest.json` gives the scope (`build` or `compose`), the source and the digest; the marker gives the facts of the stop: `version`, `written_at` (UTC), `control_pid`, `runtime`, `verb`, `leader_pid` and `process_group` (the same number), `stop_reason`, `forwarded_signal`, `kill_sent`, `leader_reaped`, `wait_uncertain`, `stdout_open`, `stderr_open`, `scope` and `sha256`. It holds no argument and no environment.

   ```sh
   AIGUARD_RETAINED="/absolute/path/of/the/identified/directory"
   cat "$AIGUARD_RETAINED/RETAINED-UNKNOWN.json"
   cat "$AIGUARD_RETAINED/manifest.json"
   ```

   If the marker itself could not be written, the CLI printed `warning: RETAINED-UNKNOWN.json could not be written in <path>: <error>`; the directory is kept all the same and the message carries the process group.

2. List the processes still in that process group. Replace `4242` with the recorded `process_group`. The result is often empty, because the usual holder left the group.

   ```sh
   ps -A -o pid,pgid,stat,command | awk -v group=4242 '$2 == group'
   ```

3. List the processes whose command line names the copy, or that hold a file or a working directory under it. Ignore the line of `grep` itself. `lsof` may have to be installed, and without privileges it shows the processes of your account only. Where it is not installed, on Linux, the links `/proc/<pid>/cwd` and `/proc/<pid>/fd/*` of a process you may inspect show its working directory and its open files.

   ```sh
   ps -A -o pid,pgid,stat,command | grep -F -- "$AIGUARD_RETAINED"
   lsof +D "$AIGUARD_RETAINED"
   ```

4. Check the engine's own state under the same account and endpoint: `docker image ls` and `docker ps --all`, or `podman image ls` and `podman ps --all`; for Docker, name the endpoint the CLI targeted, as in [follow a stack started by the CLI `up`](#cli-up-stack). ai-guard observed nothing about it; the image or the stack change may or may not exist.

5. If step 2 or 3 lists a process, something may still be reading the copy. `ps -A` lists the processes of every account, so it is not necessarily one of yours. Let it finish or, once you have identified it as a process you may stop, stop it with your usual tools, then repeat both steps. Do not signal a process group number taken from an old marker without checking it first: identifiers are reused.

6. When both steps list nothing, remove that exact directory and nothing else. These checks show the current users only; none proves that nothing will read the directory later. Never remove images, volumes, the source or the configuration to repair this state.

   ```sh
   rm -r -- "$AIGUARD_RETAINED"
   ```

7. Rerun the original command with the same options.

**A directory left by a killed CLI.** A CLI that was killed during `build`, `up` or `down` leaves its copy with no marker and no message; a copy named by a `was not removed` or `could not be removed` warning is in the same state. First make sure that no `build`, `up` or `down` of your account is still running: a running command owns such a directory. Look for a line of your account whose command is `agentsandbox` with `build`, `up` or `down`, and ignore the line of `grep` itself:

```sh
ps -A -o pid,user,stat,command | grep -F -- agentsandbox
```

Then list what is left in the temporary directory that command used. Name that directory explicitly: a `TMPDIR=… <command>` prefix sets it for that command only, so the `TMPDIR` of your shell may designate another directory. When the command ran without `TMPDIR`, the directory is `/tmp` on Linux. On macOS a session normally sets `TMPDIR` to a per-user directory under `/var/folders`, which `getconf DARWIN_USER_TEMP_DIR` prints; what the CLI uses there when `TMPDIR` is unset was not verified, and macOS was not run.

```sh
AIGUARD_TMP="/absolute/path/of/the/temporary/directory/that/command/used"
ls -ld "$AIGUARD_TMP"/ai-guard-distribution-*
```

Consider only directories owned by your account: under a shared `/tmp` the listing also shows the copies of other accounts, which you can neither read nor remove. Read the `manifest.json` of each directory if its preparation had finished, and apply steps 3 to 6 to it; there is no recorded process group to look up.

The retention and its message were exercised with Rust fixture processes on Linux ARM64. The inspection commands above are standard tools and were not run as part of that validation; no real engine, and not macOS, was involved.

### Audit descriptor recovery

When backing-file validation retains a typed OS error `EMFILE` or `ENFILE`, the Store refuses the operation without a successful acknowledgement or execution permission for it. Gateway routes receiving this error map it to HTTP **503**; descriptor exhaustion may also prevent accepting a connection or transmitting the response. The live supervision origin and sibling connections sharing it remain suspended. Once descriptors are available, the next authorized access reaching Store validation automatically checks a complete, consistent SQLite snapshot: file identity, schema, tenant binding, metadata/quota, event chain, control projections and the live rollback witness. Only successful validation restores that shared origin; a sibling connection can perform the recovery. `/healthz` alone does not trigger it.

SQLite may already have committed before the final file check fails. Preserve the event or operation identity and its exact content. Use that operation's existing idempotent replay path where available: the durable result is reused without another event or quota debit. A new identity or session is a new operation, and a consumed start claim stays consumed. Do not infer that an error response means nothing was committed.

`EIO`, substituted file identity, corruption and a poisoned supervision lock keep the origin permanently invalidated. Apparent file repair, refreshes and `worker-resume` cannot reactivate it; there is no operator override. Preserve data and session references for diagnosis. Descriptor recovery, HTTP status, health checks and a valid audit chain do not prove engine availability or a worker's stop. Continue to use the recorded runtime observations; consult the [dated validation evidence](../../engineering/VALIDATION.md) for the scope actually qualified.

### Reconcile the recorded worker

The normal recovery commands use the session's recorded engine authority:

```sh
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-resume --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --wait-seconds 30
```

Resume reconciles an interrupted session; it does not grant a second use of a consumed start claim. Interactive admission and terminal attachment have their own continuity requirements. When a retained authority needs an explicit executable, use the approved `--runtime-program` path; `--adopt-runtime-program` is a deliberate legacy-authority binding, not a generic retry switch.

For direct `run`/`worker-resume`, trusted file scanning precedes worker-declared telemetry. Inspect `workspace.diff.completed` and `observer.coverage`: the file prefix is bounded by 200 events and the 3 MiB private journal, so omitted changes remain explicit. Equal masked display paths keep distinct opaque change IDs and are marked ambiguous within that prefix. Coverage distinguishes prepared file events from durable acknowledgements; it does not promise exhaustive provider activity.

Keep `sessions/<session>.postprocessing-v1.json` inside the original private `<config-dir>`, together with its plan and worker volume. Resume replays retained events with their original IDs and content. If no batch was prepared, a terminal session with unknown `changed_files` can be scanned again through its recorded runtime authority; that is a new recovery observation, not reconstructed historical output. Missing declared capture stays unknown, even if file recovery succeeds. The CLI remains unsuccessful for unresolved postprocessing or an external execution diagnostic; repeated resume does not turn missing capture into empty activity. Preserve the journal on an audit or storage failure; do not edit or remove it to force success.

The new CLI requires the runner-only `POST /control/api/v1/control/events` route on the published origin before launching a session. Update CLI and gateway together if this route is unavailable. Event ingestion and control each have a separate one-second, 200-write window per tenant; this does not reserve durable audit capacity. Replayable event writes retry only a typed write-rate refusal, while the mandatory start-audit callback remains one attempt per POST. See the [API contract](../../api.md#ingestion-et-faits-du-contrôle--ingestion-and-control-facts) for the exact limits. These direct-CLI journals do not migrate the permanent broker's V4 outbox or change strict plan formats 1/2/3. Their presence is not a runtime, crash-recovery or production qualification result.

## Stop, preserve data and restart

Request a stop for each active worker before stopping its control stack:

```sh
"$AIGUARD_BIN" worker-stop --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml stop
```

A durable stop request is not proof of terminal runtime state. Confirm the observation or retain the unknown state for reconciliation. For Podman, use its matching Compose command and file. Restart the preserved stack with the same `up -d` command, then run health and session checks again.

`stop` preserves containers and named volumes. `down` without `-v` removes stack containers/networks while retaining named audit data; neither operation substitutes for stopping workers. Do not use `down -v`, `prune`, a new project name or database deletion to repair an error. Preserve `<config-dir>` (credentials, plans, bindings and session state), the stack's audit volume and retained worker volumes together. Back up SQLite coherently while stopped, including relevant WAL state; do not copy a live database as an ordinary file. Review [enterprise operations](../enterprise/README.md#operate-and-recover) before migration or restoration.
