# Operator diagnostics

[Français](README.fr.md) · [Setup index](../README.md) · [Local workstation](../local/README.md) · [AI providers](../providers/README.md) · [ai-guard SSO](../sso/README.md)

`ai-guard-diagnose` explains an installation before anything is launched: which layout a directory holds, which tenants and capacity plan its files declare, which credential files are referenced and whether their reader would accept them, which AI client profiles are declared and which one a managed launch selects, and what this diagnostic does not examine. It reads the files you name and nothing else. It starts no agent, calls no provider and no model, contacts no identity provider, and creates or migrates nothing.

One option leaves this offline mode, and only when you give it: `--active oidc-discovery` asks the identity provider of an inspected OIDC file for its discovery document and its keys. Without that option nothing is contacted.

A report is a diagnosis aid. A report without error proves no runtime behaviour and closes no qualification gate: read its last two blocks, which list what was not examined.

## What the tool is, and what it is not

- **Offline by default.** No socket is opened, unless the option `--active` names an active check. A URL given on the command line is validated, never contacted.
- **Read-only.** Nothing is created, changed, migrated or removed: no store, no lock file, no credential, no directory.
- **No credential content.** No file that the inspected files name as a key file, a token file, a provider credential, a setup record or a store is opened. They are described from their file metadata only. A file given by option is opened as what its option says it is: the section on files never opened states what that leaves uncovered, and how the report says it.
- **Command line only.** No environment variable and no dotenv file is read, and no program is looked up in `PATH`. The current directory serves only to resolve relative arguments.
- **Named paths only.** The tool looks at the paths it is given and at the paths the inspected files reference. It lists no directory and searches no parent.
- **One short process.** It is not a service and not an authority: it grants nothing, and no other component consumes its result.

It does not replace the existing diagnostics. It tells which one to run next:

| Command | What it does that this tool does not |
|---|---|
| `agentsandbox doctor --runtime docker` or `--runtime podman` | Executes the engine to inspect it. |
| `agentsandbox providers status` | Shows the configured profiles through the product CLI, without private paths or credential values. |
| `agentsandbox providers check` | Resolves the planned provider and reports file metadata for its selected credential reference. [Provider diagnostics](../providers/README.md#diagnose-provider-problems) |
| `agentsandbox healthcheck --url URL` | Asks a running service for its health. |
| `agentsandbox verify-audit` | Asks the running gateway for its local hash-chain check. Online: the default reads the historical viewer file; `--viewer-token-file FILE --gateway URL` explicitly selects the viewer file of a prepared tenant. It does not establish independent rollback continuity. |
| `ai-guard-audit-checkpoint verify` | Explicit offline verification of signed tenant checkpoints with an independently held anchor; optional database comparison is documented in the [signed-checkpoint guide](../audit-signatures/README.md). This is a separate command, never invoked by this diagnostic. |
| `agentsandbox kubernetes doctor` | Inspects an explicit Kubernetes authority over HTTPS. It admits no worker. |
| `agentsandbox broker recovery-status` | Reads the confirmed recovery metadata of the local broker. |

## Quick Start

### Build the tool with the product

The binary is built from the same source, by the same command, as `agentsandbox`. Run this from the repository root:

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_DIAGNOSE="$PWD/target/release/ai-guard-diagnose"
"$AIGUARD_DIAGNOSE" --version
"$AIGUARD_DIAGNOSE" --help
```

Use a tool built from the same source as the gateway whose files you inspect: an older tool may refuse a newer file. Every report prints the package version of the tool. In the examples of this guide, `PACKAGE_VERSION` stands for that version.

For an installation on the host, run the tool on the host, under the account that owns the installation. A control image contains the tool beside `agentsandbox` only when it is built from a source whose [control Dockerfile](../../../images/control.Dockerfile) builds and copies `ai-guard-diagnose`. An installation that lives in a container volume is then inspected from that image, as "Inspect a prepared installation inside its volume" describes below; the commands of that section have not been run.

### First run, without an installation

Without any argument, the tool inspects `.agentsandbox` in the current directory, the default of `init`, `run` and `providers`:

```sh
"$AIGUARD_DIAGNOSE"
```

Where no such directory exists, the report says so, says how to give an installation that exists elsewhere, names both ways to create one, and the process exits with code 3:

```text
ai-guard-diagnose PACKAGE_VERSION - report version 1
mode       offline; nothing written; no file named as a credential was read; no network
read       no file content
inspected  config directory "<config>" (default; no target was given)
result     ERROR - 1 error, 0 unknown, 0 warning; 7 not checked, 12 not available

LAYOUT
  config directory "<config>"  absent
  [error] layout.target_absent
          No target was given and the default directory .agentsandbox does not exist in the current
          directory.
          scope    input config_dir
          basis    observed
          action   If an installation exists elsewhere, pass it with --config-dir DIR or
                   --installation-dir DIR. To create one here: for a local installation run
                   agentsandbox init --config-dir "<config>"; for an SSO installation run
                   agentsandbox setup prepare --input MANIFEST --output-dir NEW_DIR. If a
                   preparation was interrupted, pass the retained staging directory that setup
                   prepare printed as --installation-dir DIR.
```

`"<config>"` is a placeholder for your directory. Add `--show-paths` to print absolute paths, in the facts and in the actions. The complete output is [report.first-run.txt](examples/report.first-run.txt).

### A historical config directory

This is the directory created by `agentsandbox init`, `$AIGUARD_CONFIG` in the [local guide](../local/README.md):

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG"
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --json
```

On a directory that `init` has just created, the report has no error and the process exits with code 0:

```text
ai-guard-diagnose PACKAGE_VERSION - report version 1
mode       offline; nothing written; no file named as a credential was read; no network
read       no file content
inspected  config directory "<config>" (from --config-dir)
inspected  provider profiles "<config>/providers.json" (default; in the config directory)
result     OK - 0 error, 0 unknown, 0 warning; 9 not checked, 13 not available
```

Each key file is described by its metadata and by the rule of the command that reads it. Its content is never read:

```text
CREDENTIAL REFERENCES
  runner of installation  "<config>/secrets/runner"
          present; regular file; mode 0600; owned by the inspecting account; single link; private
          rule satisfied; reader private file reader; content not read
```

`OK` means that nothing was refused among what was checked: here the layout and the posture of the key files. The nine items not checked include the running services, the gateway arguments, the engine state and the content of the keys. The complete outputs are [report.local-fresh.txt](examples/report.local-fresh.txt) and [report.local-fresh.json](examples/report.local-fresh.json).

### A prepared installation, on the host that prepared it

This is the directory published by `agentsandbox setup prepare`, as the [SSO guide](../sso/README.md) describes. Pass the directory itself; its `tenants.json` and its `oidc.json` are inspected together:

```sh
"$AIGUARD_DIAGNOSE" --installation-dir /absolute/private/ai-guard/new-installation
```

The report shows the public SSO parameters, the tenants of the registry, those of the capacity plan and their exact correspondence:

```text
TENANTS
  registry  1 tenant  "ai-guard-demo"
  capacity plan  1 tenant  "ai-guard-demo"
  correspondence  match
```

The example was produced on a new private directory, from the public demonstration manifest. Preparation contacts nothing and starts nothing:

```sh
AIGUARD_SSO_TRY="$HOME/ai-guard-diagnose-sso-try"
mkdir -m 700 "$AIGUARD_SSO_TRY"
(
  umask 077
  set -C
  cat docs/setup/sso/examples/local-demo.json > "$AIGUARD_SSO_TRY/manifest.json"
)
"$AIGUARD_BIN" setup prepare --input "$AIGUARD_SSO_TRY/manifest.json" --output-dir "$AIGUARD_SSO_TRY/installation"
"$AIGUARD_DIAGNOSE" --installation-dir "$AIGUARD_SSO_TRY/installation"
"$AIGUARD_DIAGNOSE" --installation-dir "$AIGUARD_SSO_TRY/installation" --json
```

The complete outputs are [report.prepared-v2.txt](examples/report.prepared-v2.txt) and [report.prepared-v2.json](examples/report.prepared-v2.json). The manifest names public demonstration identities: keep such an installation for trials only. Its OIDC file declares the demonstration transport, so the report carries the warning `identity.sso_demonstration_transport` and its result line counts one warning; an OIDC file on the `https_only` transport has no such finding.

An installation that lives in a container volume, such as the one of the Compose SSO DEMO, is inspected inside that volume: see the next section.

### Inspect a prepared installation inside its volume

The Compose procedure of the [SSO guide](../sso/README.md) prepares the installation in a named volume, under the account of the control containers. The tool therefore runs where the installation is: in a one-shot container of the control image, with the volume mounted read-only and without any network.

This section applies only to a control image that contains the tool: an image built from a source whose [control Dockerfile](../../../images/control.Dockerfile) builds and copies `ai-guard-diagnose`. An image built earlier does not contain it, and the first `run` below then fails before anything is inspected.

The commands of this section have not been run. Neither the build of the control image nor any `docker` command below was executed for this guide: the read-only mount of the volume, the account `1000:1000` inside the container and the exit code returned by the engine are derived from the Compose files, the control Dockerfile and the code of this repository. Check each result the first time you run them.

Take both values from the existing installation, without guessing a name. With `docker inspect` on the gateway container, read `.Image`, the immutable ID of the image it runs, and the `.Name` of its `/data` mount. If that image does not contain the tool, build the control image from a source at least as recent as that gateway, and read its ID with `docker image inspect`. The subshell stops on the first failure: a mistyped volume name ends at `volume inspect`, before `run` could create a volume of that name.

```sh
AIGUARD_CONTROL_IMAGE_ID="sha256:inspected-current-control-image-id"
AIGUARD_INSTALLATION_VOLUME="existing-inspected-installation-volume"
(
set -eu
docker volume inspect --format '{{.Name}}' "$AIGUARD_INSTALLATION_VOLUME"
docker run --rm --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --entrypoint ai-guard-diagnose "$AIGUARD_CONTROL_IMAGE_ID" --version
docker run --rm --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --mount "type=volume,source=$AIGUARD_INSTALLATION_VOLUME,target=/data,readonly,volume-nocopy" \
  --entrypoint ai-guard-diagnose "$AIGUARD_CONTROL_IMAGE_ID" --installation-dir /data/installation
)
```

The first `run` mounts nothing and prints the version of the tool. The second writes the report:

- **Same path.** The volume is mounted at `/data`, where the supplied Compose files mount it: the files of an installation record their final paths. Mounted elsewhere, the installation is reported with `layout.references_outside_inspected_root`.
- **Same account.** `1000:1000` is the user of the control services in the supplied Compose files. The private-file rule accepts only the owner of a file.
- **Read-only, no network.** The root file system of the container and the volume are read-only. An active check gets no answer from this container: leave `--active` out.
- **Running services.** The services may stay up: the tool opens none of the stores of the installation.
- **Exit code.** `docker run` returns the exit code of the tool, so the codes of this guide apply. A failure of the engine itself is not a report.

Add `--json` or `--show-paths` after the directory, as on a host; the paths printed are those of the container. For a Podman installation, use the corresponding `podman` commands against its own existing volume, and qualify its UID mapping separately.

### Provider profiles

The profile file of a config directory is inspected with the directory. A file given by `--providers-file` is inspected in its place, as `agentsandbox run` reads it:

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --providers-file "$AIGUARD_CONFIG/providers-subscriptions.json"
```

The public input [providers.two-modes.json](examples/providers.two-modes.json) declares Codex CLI and Claude Code, each with a subscription profile and an API profile, with references relative to the profile file. To see its report without any real credential, create a new directory and two empty private files. The tool opens no file that a profile names as its credential, so their content does not matter:

```sh
AIGUARD_TRY="$HOME/ai-guard-diagnose-try"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_TRY"
(
  umask 077
  set -C
  cat docs/setup/diagnostics/examples/providers.two-modes.json > "$AIGUARD_TRY/providers.two-modes.json"
  : > "$AIGUARD_TRY/secrets/providers.env"
  : > "$AIGUARD_TRY/secrets/claude-subscription.env"
)
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json"
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --json
```

The Codex subscription file is left absent on purpose. The report lists the four profiles, then the profile a managed launch selects. No selection option is given here, so every part of the selection is automatic: subscription profiles come before API profiles, and `auto_priority` then names the Codex subscription.

```text
PROVIDERS
  profile file  "<providers-file>"  loaded
  automatic priority  "codex-subscription" "claude-subscription" "codex-api" "claude-api"
```

```text
  selection of a managed launch
          requested client auto; mode auto; profile auto; model auto
          resolved profile "codex-subscription"; revision 1; client codex; mode subscription; model
          not declared; chosen by auto
```

The file of that profile is absent, so a launch would ask for a login. The process exits with code 3, and the report says that no API profile answers in its place:

```text
  [error] provider.login_required
          The profile that the requested selection resolves is a subscription profile and its
          credential file does not exist. A managed launch with this selection refuses with
          login_required and never tries an API profile.
          provider profiles  1 in total "codex-subscription"
```

The complete outputs are [report.providers-two-modes.txt](examples/report.providers-two-modes.txt) and [report.providers-two-modes.json](examples/report.providers-two-modes.json).

### The profile a managed launch selects

Give the tool the selection options of `agentsandbox run`, with the same values: `--client`, `--auth-mode`, `--client-profile` and `--model`. An option that is left out is resolved automatically, as `run` does. The choice is made by the product function that a managed launch uses: the tool restates no rule, and that function reads no credential file. `$AIGUARD_CODEX_MODEL` is the model variable set in the [provider guide](../providers/README.md).

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --client claude-code --auth-mode subscription
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --client claude-code --auth-mode subscription --json
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --client codex --auth-mode api --model "$AIGUARD_CODEX_MODEL"
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --client-profile codex-subscription
```

With the first two commands the explicit subscription choice is kept. The Claude subscription profile is resolved and the process exits with code 0. The absent Codex file now belongs to a profile that this selection does not resolve: it is a warning.

```text
given      requested client claude-code (from --client)
given      requested authentication mode subscription (from --auth-mode)
```

```text
  selection of a managed launch
          requested client explicit; mode explicit; profile auto; model auto
          resolved profile "claude-subscription"; revision 1; client claude_code; mode subscription;
          model not declared; chosen by auto
```

The complete outputs are [report.providers-explicit-subscription.txt](examples/report.providers-explicit-subscription.txt) and [report.providers-explicit-subscription.json](examples/report.providers-explicit-subscription.json).

| Situation | Finding | Exit code |
|---|---|---|
| The selection resolves one profile | `provider.selection_resolved` | unchanged |
| The resolved profile is in API mode | `provider.api_billing_notice`, a warning: usage is billed by the provider | unchanged |
| The resolved profile is a subscription profile and its credential file is absent | `provider.login_required`, and `provider.subscription_never_falls_back_to_api` when the file also declares API profiles | 3 |
| The resolved profile is an API profile and its credential file is absent | `provider.credential_missing` | 3 |
| The credential file of the resolved profile is unsafe: its type, its mode, its link count, or a directory that leads to it | `provider.credential_unsafe` | 3 |
| No declared profile answers the request, for example a subscription request on a file of API profiles | `provider.selection_no_profile`. No profile of the other mode is used in its place: for a subscription request on a file that declares API profiles, `provider.subscription_never_falls_back_to_api` says so. | 3 |
| Several profiles match and nothing decides, or the request contradicts the requested profile | `provider.selection_ambiguous` | 3 |
| A requested profile or model identifier is refused by the product validator | `provider.selection_invalid` | 3 |
| Selection options are given and no profile file was loaded | `provider.selection_not_resolved`, not checked | unchanged |
| The credential file of a profile that the selection does not resolve is absent or unsafe | `provider.unselected_profile_reference_absent` or `provider.unselected_profile_reference_unsafe`, warnings | unchanged |
| The credential file of a profile conforms, and its directory is not a private 0700 directory | `reference.private_rule_not_evaluated`, not checked: only its reader checks the file system and the access entries of the file | unchanged |

With no selection option, the report is computed for the fully automatic managed launch: the selection that `agentsandbox run` builds when `--client`, `--auth-mode`, `--client-profile` and `--model` are all left out, for example with `--prompt` or `--providers-file` alone. When the product refuses that selection, because several profiles match and neither `default_profile` nor `auto_priority` names one of them, or because no profile matches, the report has `provider.selection_ambiguous` or `provider.selection_no_profile`, status `error` and exit code 3, although a selection that names a declared profile with `--client-profile ID` resolves it. Give the tool the selection options of the launch you intend, to get the verdict of that launch.

The selection establishes neither the presence of the client in the worker image, nor its version, nor the validity of the account or of the subscription: these stay among the limits of every report. The content of the selected credential is not checked either: `agentsandbox run` validates it before authorization.

### Other inputs

A registry or an OIDC file can be given alone, as they are passed to the gateway. The public registry example records container paths, so on a workstation the files it names are reported `unknown` and the process exits with code 4:

```sh
"$AIGUARD_DIAGNOSE" --tenants-file config/tenants.example.json
"$AIGUARD_DIAGNOSE" --tenants-file /absolute/private/ai-guard/tenants.json --oidc-config /absolute/private/ai-guard/oidc.json
```

The engine program is approved by the rule the product applies before any engine call. It is hashed, never executed:

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --runtime docker
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --runtime podman --runtime-program /absolute/path/to/podman
```

The trust material and the transport of the decision engine are gateway options, stored nowhere. Give the same values as the gateway launcher to have them validated; nothing is contacted. An option that the run cannot use, such as an event bound for a registry without a policy that depends on it, is reported as not checked:

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --jev-url https://decision.example.invalid/v1 --jev-ca-file /absolute/path/public-ca.pem
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --gateway-ca-file /absolute/path/public-ca.pem
"$AIGUARD_DIAGNOSE" --installation-dir /absolute/private/ai-guard/new-installation --max-events 5000
```

### Ask the identity provider, on request

By default the identity provider of an OIDC file is never contacted, and the report says so:

```text
  [not_checked] identity.idp_reachability_not_checked
          The identity provider was not contacted: this tool opens no connection unless an active
          check is asked for. The gateway contacts the provider when a sign-in starts.
          scope    file oidc_file
          basis    observed
          requires  --active oidc-discovery, with an OIDC file that passes the offline rules
```

The check contacts the hosts that the inspected file names. Run the tool once without `--active`, read `issuer` and `keys endpoint` in the identity block, and add `--active oidc-discovery` only for a provider you mean to contact from this host.

`--active oidc-discovery` runs the discovery of the product's own OIDC client against that provider, once, from where the tool runs:

```sh
"$AIGUARD_DIAGNOSE" --installation-dir /absolute/private/ai-guard/new-installation --active oidc-discovery
"$AIGUARD_DIAGNOSE" --tenants-file /absolute/private/ai-guard/tenants.json --oidc-config /absolute/private/ai-guard/oidc.json --active oidc-discovery --json
```

The report then says what was done. Its `mode` line reads `active checks run`, an `active` line follows with `oidc discovery; network used by these checks only`, and the identity block reads `identity provider  contacted`. In the JSON document `mode.network` is `active_checks_only`, `mode.active_checks_executed` is `oidc_discovery`, and `identity.sso.idp_contacted` is `true`.

| Result | Finding | Status |
|---|---|---|
| The provider answers, confirms the file, and its keys are usable | `active.oidc_discovery_ok` | `ok` |
| No usable answer from where the tool runs | `active.oidc_discovery_transport_unavailable` | `unknown`, exit code 4 when there is no error |
| An answer that is not a discovery | `active.oidc_discovery_provider_rejected` | `error` |
| The discovery document does not confirm the file | `active.oidc_discovery_metadata_mismatch` | `error` |
| The key set is refused | `active.oidc_discovery_keys_invalid` | `error` |
| The check is asked for, and the OIDC file does not pass the offline rules or no OIDC file is inspected | `identity.idp_reachability_not_checked`. Nothing is contacted. | `not_checked` |

The check runs only for an OIDC file that passes every offline rule. What it sends, its bounds and what it never prints are stated under "The active discovery", in the section on secret hygiene.

## Read the report

### Text report

The text is plain, in English, without colour, within 100 columns unless one of your own values is longer. A string that comes from your files or your command line is always printed between double quotes.

| Line or block | Content |
|---|---|
| First line | Tool name, package version, report version. |
| `mode` | What the run did: offline, nothing written, no file named as a credential was read, no network. "Named" means named by the files inspected in this run. The third part reads `a file later named as a credential was read` when `mode.credential_contents_read` is `detected_after_read`. When an active check ran, the first part reads `active checks run` and the last part moves to the `active` line. |
| `active` | Present only when an active check ran: the checks that contacted something, then `network used by these checks only`. |
| `read` | The closed list of files opened for content in this run, or `no file content`. |
| `inspected`, `given`, `assumed` | One line per input: its display and where it comes from. `assumed` marks a value this tool supplied because none was given. |
| `result` | `OK`, `ERROR` or `UNKNOWN`, then the count of errors, unknowns, warnings, items not checked and items not available. |
| `omitted` | Present whenever something that was evaluated is not listed: the count of reference entries, of data entries and of tenant identifiers that are not printed, because a list reached its bound or because the detail lists were emptied to respect the size bound. |
| One block per axis | The facts of the axis, then its findings. An axis without fact and without finding is not printed. |
| `NOT CHECKED` | Every item that this run did not examine, with what resolves it. |
| `NOT AVAILABLE IN THIS BUILD` | Every item that nothing in this build can examine offline. |
| Last line | `exit` and the exit code. |

A finding prints its status and its code, its message, its details when it has some, the tenants or profiles it concerns, then:

| Label | Meaning |
|---|---|
| `scope` | What the finding is about: one input, the tenants, the provider profiles, one file read for content, the host, or the build. |
| `basis` | `observed`, `declared` or `inferred`, then the file and the field the finding is drawn from. |
| `applies` | The command that applies the rule. |
| `assumes` | The assumed inputs the verdict depends on. |
| `requires` | For an item not checked: the option of this tool, the existing command, or the place that holds the answer. |
| `action` | What to do next. An action never proposes to weaken TLS, containment or audit. |

The header gives counts only: the findings stand in the blocks of their axes. Each one starts with its status between brackets, then its code, as in `[error] provider.login_required`. To find the errors and the inconclusive observations of a long report, search the text for `[error]` and `[unknown]`, for example with `grep -F '[error]'`, or read `findings` in the JSON document and keep the entries whose `status` is `error` or `unknown`.

### Statuses

| Status | Meaning | Exit code |
|---|---|---|
| `ok` | A product validator accepted the item, or the offline rules restated in this tool found no violation. The message says which. | unchanged |
| `info` | A neutral fact. | unchanged |
| `warning` | Usable, but deviating or at risk. | unchanged |
| `error` | The consuming command refuses this for certain, or two files contradict each other, or this tool withheld a file that it never opens: a file given below `secrets`, `mounted-secrets` or `tenants` of an inspected directory, or one that an inspected directory or an inspected file designates as a key file, a credential, a setup record or a store. That is a refusal of this tool, not of the consuming command. | 3 |
| `unknown` | The observation was attempted and is inconclusive. | 4 when there is no error |
| `not_checked` | Not examined in this run. `requires` names what resolves it. | unchanged |
| `not_available` | This diagnostic has no offline inspector for it. Other explicit product commands may examine a narrower scope; `requires` is `null`. | unchanged |

A file-system fact that conforms never produces an `ok` for a credential: it is a necessary condition, and only the consuming command checks the content.

### Basis and scope

| `basis` | Meaning |
|---|---|
| `observed` | A file-system fact, or the result of a product function called by this tool. |
| `declared` | A file says so. |
| `inferred` | The consequence of a cited rule or of an assumed input. The verdicts on the registry and on the OIDC file are of this kind: this tool restates the structural rules of the gateway loader, and the loader itself did not run. |

| `scope` `kind` | Meaning |
|---|---|
| `input` | One entry of `inputs`, named by `input`. |
| `tenants` | The tenants of the registry and of the plan. |
| `provider_profiles` | The declared provider profiles. |
| `file` | One file read for content, named by `file`. |
| `host` | The host the tool runs on. |
| `build` | A limit of this build, listed in `limits`. |

Entries that share a code, a scope and the same details are one finding. The tenants or the profiles it concerns are its `subjects`: an exact `total`, at most 16 `ids`, and the number `omitted`.

### Axes

The axes are independent. A fault on one axis does not change what another axis reports: single sign-on is inspected without any AI provider, and a provider profile without any SSO file.

| Axis | Content |
|---|---|
| `layout` | Which layout each inspected directory holds, and the state of a preparation. |
| `topology` | Single local tenant, or the tenants of a registry. |
| `identity` | Technical key references by presence; the public parameters of an OIDC file. |
| `tenants` | The registry, the capacity plan and their exact correspondence. |
| `credential_references` | Every referenced key file, provider credential and setup record, by metadata. |
| `providers` | The provider profile file and its profiles: client, authentication mode, credential source. The profile a managed launch selects. |
| `inference` | The decision-engine transport given on the command line. |
| `trust` | The CA bundles: the one of the OIDC file, and those given by option. |
| `engine` | The engine program, approved or refused, never executed. |
| `data` | The stores, by presence and sidecars. No store is opened. |
| `output` | What the report itself masked or omitted. |

### Exit codes

| Code | Meaning | Output |
|---|---|---|
| 0 | A report was written; it has no `error` and no `unknown`. | Report on stdout; stderr empty. |
| 3 | A report was written; it has at least one `error`. | Same. |
| 4 | A report was written; it has no `error` and at least one `unknown`. | Same. |
| 2 | Usage error. | One closed sentence on stderr; stdout empty. |
| 1 | Refusal: no report at all. | One closed sentence with a next step on stderr; stdout empty. |

Exit code 0 means that nothing was refused among what was checked. It does not mean that the installation starts: `warning`, `info`, `not_checked` and `not_available` never change the exit code, and several of them are present in every report. The `result` line says how much was not examined.

Without `--client`, `--auth-mode`, `--client-profile` or `--model`, the provider findings and the exit code are those of a managed launch that leaves these options out. To gate on the launch you actually run, give this tool the same options. `providers.selection.requested` says which parts were automatic: they are listed neither in `inputs` nor in `assumes`.

### Refusals and usage errors

A usage error never repeats your argument. It prints one of three sentences, each followed by `run ai-guard-diagnose --help`:

| Sentence | Cause |
|---|---|
| `unknown option or unexpected argument` | An option that does not exist, or a positional argument. |
| `an option has a value outside its closed set` | For example a `--runtime` other than `docker` or `podman`, a `--client` or an `--auth-mode` outside its values, or a `--max-events` outside 1 to 1000000. |
| `invalid command line` | Any other error: two options that exclude each other, or an option given without the option it needs. The sentence does not say which rule was broken: the three rules are stated in the help, beside the options they concern, and in the notes of the Options section. |

A refusal writes nothing on stdout:

| Text on stderr | When |
|---|---|
| `ai-guard-diagnose: an argument path cannot be used: it contains a parent traversal, or the current directory is unavailable. No report was written. Use a path without parent traversal.` | A path argument contains `..`, or a relative argument cannot be resolved. |
| `ai-guard-diagnose: the report was withheld by the output guard. No report was written. Rerun with --json; if it persists, report a defect with the command line only.` | The final output guard would still rewrite the report. This is not expected. |
| `ai-guard-diagnose: the report exceeds its size bound. No report was written. Inspect one target at a time.` | The document exceeds 524288 bytes even with its detail lists emptied. |
| `ai-guard-diagnose: the report could not be written. Check the output destination.` | The single write to stdout failed. |
| `ai-guard-diagnose: internal error. No report was written. Report a defect with the command line only.` | A defect of the tool. |

The first and the fourth of these refusals are produced by runs of the tool. The output guard, the size refusal and the internal error are implemented and were produced by no run.

## Options

| Option | Default | Meaning |
|---|---|---|
| `--config-dir DIR` | `.agentsandbox`, only when no target option is given | Historical config directory, as given to `init`, `run` and `providers`. Its `providers.json` is inspected too, unless `--providers-file` names another file. |
| `--installation-dir DIR` | none | Directory created by `setup prepare`, or its retained staging directory. Its own registry and OIDC file are inspected. |
| `--tenants-file FILE` | none | Tenant registry, as given to the gateway. |
| `--oidc-config FILE` | none | OIDC file, as given to the gateway. |
| `--providers-file FILE` | `providers.json` of the config directory | Provider profiles, as given to `run`. |
| `--client` `auto`, `codex` or `claude-code` | `auto` | AI client asked from a managed launch, as given to `run`. |
| `--auth-mode` `auto`, `api` or `subscription` | `auto` | Authentication mode asked from a managed launch, as given to `run`. |
| `--client-profile ID` | none | Provider profile asked from a managed launch, as given to `run`. |
| `--model ID` | none | Model asked from a managed launch, as given to `run`. It replaces the model of the profile. |
| `--max-events N` | 100000, reported as assumed when a tenant policy depends on it | Gateway event bound, from 1 to 1000000, used to validate the audit-capacity and workload-audit policies of the tenants. A policy refused at the assumed bound is reported `unknown`, by `registry.policy_refused_at_assumed_event_bound`; refused at a bound you give, it is an `error`. |
| `--activity-max-events N` | 1000 | Per-session live provider activity event budget of the broker registry, from 1 to 5000. With `--max-events`, it sizes the tenant audit: below 50 sessions the report warns by `audit.live_activity_budget_capacity`. |
| `--activity-max-bytes BYTES` | 2097152 | Per-session live provider activity byte budget of the broker registry, from 65536 to 8388608. The 256 MiB physical audit cap divided by it below 50 sessions is reported by `audit.live_activity_budget_capacity`. |
| `--jev-url URL` | none | Decision-engine URL to validate. Never contacted. Not together with `--jev-local-url`. |
| `--jev-local-url URL` | none | Local decision-engine URL to validate. Never contacted. Not together with `--jev-url`. |
| `--jev-allow-local-http` | off | States that the gateway is started with its explicit local HTTP permission. Judged only together with a decision-engine URL. |
| `--jev-ca-file FILE` | none | Public CA bundle given to the gateway for the decision engine. |
| `--gateway-ca-file FILE` | none | Public CA bundle used by the clients of an HTTPS gateway. |
| `--runtime` `docker` or `podman` | none | Engine whose program is approved without being executed. |
| `--runtime-program ABS` | the fixed system locations of the product | Absolute engine program. Needs `--runtime`. |
| `--json` | off | The versioned JSON document instead of text. |
| `--show-paths` | off | Absolute paths instead of placeholders. |
| `--active` `oidc-discovery` | none | Active check to run. Nothing is contacted without it. `oidc-discovery` asks the identity provider of the inspected OIDC file for its discovery document and its keys. |

- The target options are `--config-dir`, `--installation-dir`, `--tenants-file`, `--oidc-config` and `--providers-file`. When none is given, `.agentsandbox` is inspected, even when only `--runtime` or a decision-engine option is present: its absence is then an error.
- `--installation-dir` brings its own two files. It is refused together with `--tenants-file` or `--oidc-config`.
- `--jev-url` and `--jev-local-url` exclude each other. `--runtime-program` needs `--runtime`.
- `--client`, `--auth-mode`, `--client-profile` and `--model` say which managed launch the report is computed for. They change nothing in the installation. When no profile file is loaded they resolve nothing, and the report says so.
- `--jev-allow-local-http` describes the gateway. It relaxes nothing: the product validator still decides. It accepts plain HTTP only for a loopback address, and it refuses this permission together with a CA bundle, whatever the URL.
- `--jev-allow-local-http` and `--jev-ca-file` are judged as a transport only together with `--jev-url` or `--jev-local-url`. Without a URL they are listed among the inputs, the report says that no transport was judged, by `inference.decision_transport_inputs_without_url`, and a bundle is judged by its grammar alone. The tool validates the transport and the bundle only: it does not check the option set of the gateway, which requires a backend selection, a model and, for the TypeSafe backend, a key file, and refuses an incomplete or mixed set.
- `--max-events` is used only when a tenant of an inspected registry declares an audit-capacity or a workload-audit policy; a registry written by `setup prepare` declares none. Otherwise the value is listed among the inputs, nothing is validated with it, and the report says so by `registry.event_bound_not_used`.
- A path argument that contains `..` is refused before anything is read.
- `--active` is the only option that makes the tool open a connection. It needs an OIDC file that passes the offline rules; otherwise nothing is contacted and the report says so.

## Configuration priorities

The tool reads no configuration of its own. This table states where each value comes from in the product, so that a report can be compared with the right source. The first match wins.

| Setting | Order |
|---|---|
| Compose interpolation | The shell environment, then the file named by `--env-file`, then the default written in the Compose file. |
| `agentsandbox` options | The flag, then its built-in default. The CLI never reads a dotenv file. |
| Gateway decision-engine options | The flag, then its named variable: `JEV_URL`, `JEV_API_KEY_FILE`, `JEV_MODEL`, `JEV_CA_FILE`. |
| Gateway decision-engine backend | `--jev-backend`, then `JEV_BACKEND`, then a legacy URL option alone, then no decision engine. |
| Provider profile file | `--providers-file`, then `providers.json` of the config directory. |
| Provider profile selected by a launch | An explicit profile, then `default_profile` among the candidates, then a single candidate, then `auto_priority`. Under automatic authentication mode, subscription candidates exclude API candidates. The tool asks the product for this choice, for the selection options it is given. |
| File references | Absolute as written. Relative to the directory of the file that declares them: registry, OIDC file, provider profile file. |
| `ai-guard-diagnose` | Command-line options only. `--max-events` is assumed to be 100000 when it is not given. |

What is not stored in an installation cannot be read from it: the arguments of the gateway, the selected decision engine, the store path of a historical directory and the activation of SSO are reported as not checked. An explicit subscription choice stays a subscription choice: neither the product rule nor this tool replaces it with an API profile. The reverse does not hold: billed API use does not need an explicit request. Under automatic authentication mode the product can resolve an API profile when no subscription profile matches the requested client, for example with `--client` alone; when it does, the report carries `provider.api_billing_notice`. Two requests rule an API profile out: `--auth-mode subscription`, and `--client-profile` when it names a subscription profile.

## Unknown, not-checked and not-available states

A value that was not observed is never reported as a success, and never as zero.

### `unknown`: observed, inconclusive

| Code | When | What resolves it |
|---|---|---|
| `layout.inspector_identity_differs` | The directory belongs to another account, or could not be examined from this one. Nothing below it is described. | Run the tool as the control account that owns the installation. |
| `registry.policy_refused_at_assumed_event_bound` | A tenant policy whose verdict depends on the gateway event bound is refused at the bound this tool assumed, 100000, because `--max-events` was not given. The gateway may be started with another bound. | Give the event bound of the gateway with `--max-events N`. A gateway started without that option uses 100000 and refuses the policy. |
| `reference.not_found_from_here` | A referenced path outside the inspected directory was not found. It may exist where its consumer runs. | Run the tool where the gateway runs, or add `--show-paths` to see the recorded path. |
| `reference.inspector_identity_differs` | A referenced file belongs to another account. Its consumer may run as that account, so its rule was not judged. The same code is reported for a registry of another account that this account cannot read. | Run the tool as the control account. |
| `reference.metadata_unavailable` | The metadata of a referenced path could not be read, for a reason other than absence. | Check the permissions of the directories that lead to it. |
| `active.oidc_discovery_transport_unavailable` | The active discovery got no usable answer from where the tool runs. The gateway may reach the provider from another network. | Run the check where the gateway runs; check that the issuer is reachable and that the CA bundle of the OIDC file covers its certificate. |

### `not_checked`: not examined in this run

`requires` is printed as shown here.

| Code | What is not examined | `requires` |
|---|---|---|
| `topology.running_services_not_checked` | Whether the gateway, the dashboard or a worker is running. | `agentsandbox healthcheck --url URL` |
| `topology.gateway_arguments_not_persisted` | Listening addresses, policy file and event bound. | `the command line or the Compose file of the launcher` |
| `identity.sso_not_inspected` | Single sign-on, when no OIDC file was given. | `--oidc-config FILE or --installation-dir DIR` |
| `identity.sso_gateway_enablement_not_persisted` | Whether the gateway is started with the OIDC file. | `the command line of the gateway launcher` |
| `identity.idp_reachability_not_checked` | Whether the identity provider answers. | `--active oidc-discovery, with an OIDC file that passes the offline rules` |
| `oidc.loader_acceptance_not_checked` | Acceptance of the OIDC file by the gateway, which also compares it with the durable authentication state. | `the gateway checks the stores and the durable authentication state when it starts` |
| `registry.loader_acceptance_not_checked` | Acceptance of the registry by the gateway, which also reads every key and opens every store. | `the gateway checks token content, token uniqueness and the stores when it starts` |
| `registry.event_bound_not_used` | Anything with the event bound given by `--max-events`, when no decoded registry declares a policy that depends on it. | `a registry whose tenants declare an audit-capacity or a workload-audit policy` |
| `tenants.correspondence_not_checked` | The comparison of tenants and plan, when one of the two files is missing or undecoded. For a historical config directory and an OIDC file without a registry, the action states the rule of a gateway started without a registry: it serves the single tenant `local`. | `both --tenants-file FILE and --oidc-config FILE, or --installation-dir DIR` |
| `reference.content_not_checked` | The content of every key, token and credential file. | `the consuming command checks the content when it starts` |
| `reference.private_rule_not_evaluated` | The file system and the extended access entries of a conforming private file whose directory is not a private 0700 directory. | `the consuming command checks the file system and the access entries of the file when it opens it` |
| `provider.not_inspected` | The provider profiles, when no location was given or the config directory could not be inspected. | `--config-dir DIR or --providers-file FILE` |
| `provider.credential_content_not_checked` | The content of the provider credentials. | `agentsandbox run validates the selected credential before authorization` |
| `provider.manual_launch_path_not_checked` | The manual launch path, which does not use the profile file. | `agentsandbox run` |
| `provider.selection_not_resolved` | The selection, when selection options are given and no profile file was loaded. | `a provider profile file that the product loader accepts` |
| `inference.decision_engine_selection_not_persisted` | Which decision engine the gateway selects. | `the command line or the Compose file of the launcher` |
| `inference.decision_transport_not_checked` | The decision-engine transport, when the CA bundle given with it could not be used and the URL alone is not refused. | `a CA bundle that the product accepts, given with --jev-ca-file FILE` |
| `inference.decision_transport_inputs_without_url` | The decision-engine transport, when the local HTTP permission or a CA bundle is given without a URL. | `--jev-url URL or --jev-local-url URL` |
| `engine.program_not_inspected` | The engine program, when no engine was named. | `--runtime docker or --runtime podman` |
| `engine.state_not_checked` | Whether the engine runs, its version and its rootless mode. | `agentsandbox doctor --runtime docker or agentsandbox doctor --runtime podman` |
| `data.location_not_persisted` | The store of a historical config directory: its path is a gateway option. | `the command line or the Compose file of the launcher` |
| `data.signed_checkpoint_not_checked` | Signed tenant checkpoints and independent rollback continuity; no checkpoint, anchor or database is read by this diagnostic. | `explicit ai-guard-audit-checkpoint verify with tenant checkpoint evidence and an independently held trust anchor` |

### `not_available`: not inspected by this diagnostic

| Code | What this diagnostic cannot examine offline | Where the answer is established |
|---|---|---|
| `identity.local_named_accounts_not_available` | Local named accounts and memberships: they live in the tenant stores. | Nothing in this build, offline. |
| `tenants.durable_plan_comparison_not_available` | The comparison of the plan with the one stored in the authentication audit. It is listed beside a match and beside a mismatch: two files that agree can still differ from the stored plan. | The gateway, when it starts. |
| `inference.local_model_backend_not_available` | Image, weights, memory budget and readiness of a local model backend. | The [local-AI guide](../local-ai/README.md). |
| `inference.model_tls_outputs_not_available` | The outputs of model TLS provisioning. | Nothing in this build. |
| `trust.certificate_validity_period_not_available` | Validity dates: an expired certificate is not detected. | The tooling of your certificate authority. |
| `data.offline_inspection_not_available` | This diagnostic never opens SQLite connections to tenant stores or authentication audits, so it does not inspect schema, binding, audit format or integrity. | Gateway startup and an explicit online `verify-audit` check establish their own local checks. Signed tenant checkpoint verification is the separate offline operation below. |
| `provider.qualified_versions_not_available` | A list of qualified client versions: this build contains none. | Your own record of the versions built into the worker image. |
| `provider.client_presence_not_available` | Whether Codex CLI or Claude Code is present. | The worker, when it starts. |
| `provider.client_version_not_available` | The version of a client. | The worker, when it starts. |
| `provider.account_validity_not_available` | Whether an account or a subscription is valid. | The provider, at the first use. |
| `identity.arbitrary_oidc_provider_not_qualified` | Compatibility with a given identity provider. | A real sign-in, as the SSO guide describes. |
| `inference.internal_profile_not_available` | A managed internal-inference profile: a provider profile carries no URL, format or CA field. | The [enterprise guide](../enterprise/README.md) states what is delivered. |
| `topology.kubernetes_launch_not_available` | Worker launch through the Kubernetes adapter. | `agentsandbox kubernetes doctor` inspects an authority without admitting workers. |
| `layout.sibling_staging_not_inspected` | A staging directory left beside a published installation: it is not searched for. | Pass the retained staging directory as `--installation-dir`. |
| `tool.linux_macos_posture_policy_only` | A network or FUSE file system: the posture policy refuses it without examining it. | Keep the installation on a supported local file system. |

The last nine are limits of the build. They are in the `limits` section of every report, with the scope `build`.

## Layouts and the commands that read them

Two layouts exist. The ordinary legacy paths read the historical one; the gateway reads a prepared installation through its registry and OIDC file. Online `verify-audit` also accepts an explicit viewer file for a prepared tenant.

| Layout | Created by | Recognised by | Read by |
|---|---|---|---|
| Historical config directory | `agentsandbox init` | `secrets` or `mounted-secrets` | `init`, `up`, `down`, `run`, `worker-status`, `worker-resume`, `worker-stop`, `export-workspace`, `verify-audit` and `providers` |
| Prepared installation | `agentsandbox setup prepare` | `preparation.json` or `completion.json` | The gateway, through `--tenants-file` and `--oidc-config` |

A gateway started without a registry serves one tenant, named `local`. Keep a separate historical config directory for the legacy commands. For a prepared tenant, use the [local guide](../local/README.md) and `agentsandbox verify-audit --viewer-token-file FILE --gateway URL`; this online path reads the explicit viewer file, not the prepared registry. A registry given alone with `--tenants-file` is reported as `hand_written_registry`.

`ai-guard-diagnose` never opens a SQLite connection. To verify signed tenant evidence offline, deliberately run `ai-guard-audit-checkpoint verify` with an independently held anchor, following the [signed-checkpoint guide](../audit-signatures/README.md). Optional database comparison opens a consistent read-only snapshot and may create SQLite sidecars and the reserved open-lock file. Those effects belong to that explicit operation, not to this diagnostic. A local hash chain, a signed checkpoint and independent anti-rollback continuity are distinct checks.

`layouts[].kind` is one of `absent`, `historical_config_dir`, `prepared_installation`, `setup_staging_directory`, `mixed`, `hand_written_registry` and `unrecognized`. `preparation` is `complete`, `completion_marker_absent`, `incomplete` or `no_intent`.

| Situation | Finding | Action |
|---|---|---|
| The directory does not exist | `layout.target_absent` | Check the path first, or pass the directory of an installation that exists elsewhere. Otherwise create an installation with `init` or `setup prepare`. |
| The directory holds neither layout | `layout.unrecognized` | Give the directory passed to `init`, or the one created by `setup prepare`. When it holds `tenants.json` or `oidc.json` without a setup record, pass those files with `--tenants-file` and `--oidc-config`: the directory is not inspected as an installation. When its name has the form of a staging name, its registry was read to tell what the directory is, and the message says so. |
| A prepared installation given as `--config-dir` | `layout.prepared_passed_as_config_dir` | Pass it as `--installation-dir`. |
| A historical directory given as `--installation-dir` | `layout.historical_passed_as_installation_dir` | Pass it as `--config-dir`. |
| Both layouts in one directory | `layout.mixed` | Preserve the directory. Do not initialise it again; use a separate directory for the legacy commands. |
| A published installation without its completion marker | `layout.completion_marker_absent` | Stop the controls, then run `agentsandbox setup resume --recovery-dir DIR --confirm-control-stopped`. |
| A retained staging directory | `layout.setup_staging_directory` | Stop the controls, then run the same `setup resume` on that directory. Do not regenerate credentials. |
| A staging directory and its recorded final path both exist | `layout.staging_and_final_both_exist` | Preserve both and do not resume. Establish which one holds the installation first. |
| A staging name without a preparation record | `layout.staging_without_intent` | Preserve it and prepare into a different new destination. |
| The files record paths outside the inspected directory | `layout.references_outside_inspected_root` | The gateway reads the recorded paths. Run the tool where the installation is mounted at its recorded path. |

A staging directory is recognised by its name, `.setup-` followed by 24 hexadecimal digits, when it holds no registry yet or when its registry records paths outside it. Its registry and its OIDC file are decoded and judged, but they record the final paths: the key files and the stores they name are not described. They are known all the same: a file option or a CA reference that names one of them is not opened, with one exception that the order of the reads leaves. A profile file is read before the OIDC file: when `--providers-file` names the authentication audit that the OIDC file records, that audit is read as a profile file if it is at most 65536 bytes, the bound of the profile loader, and the report then says so by `reference.file_read_before_it_was_named`. Nothing else at the final path is known: a file there that neither the registry nor the OIDC file of the staging directory names, such as a setup record of the final directory, is not recognised. The tool never derives the name of a staging directory and never searches for one.

A directory that is not private, that is a symbolic link or that is not a directory is reported as `layout.posture_refused`. Its message says which: a path that is not a directory, a mode other than 0700, or, when the mode conforms, a directory above it, a link in its path or its file system. The observed mode and owner are not printed as facts of the layout, and the three causes of the second case are not told apart: `ls -ld` on the directory and on each directory above it shows the first two. On macOS the product has a further cause, which the message names: an extended access entry on the directory itself, or one that permits a modification on a directory above it. This is read in the product code: no run was made on macOS. Neither this tool nor the product repairs an existing path.

## Secret hygiene

### Files read for content

`mode.files_read_for_content` is the closed list of what a run opened. Nothing else is read. Reading a file can update its access time, as any read does; nothing else changes.

| Role | File | Read as |
|---|---|---|
| `tenant_registry` | `tenants.json` of an installation, or `--tenants-file` | As the gateway reads it: a regular file of at most 1 MiB, not a symbolic link. No owner, mode or link count is checked. A file that is not read is told apart by its metadata: absent, of another account, or refused by that rule. |
| `oidc_file` | `oidc.json` of an installation, or `--oidc-config` | A private file of at most 1 MiB: mode 0600, single link, owned by the control account. |
| `oidc_ca_bundle` | The `ca_bundle_file` of the OIDC file | A private file, then the bundle grammar of the product. |
| `provider_profiles` | `providers.json`, or `--providers-file` | A private file of at most 65536 bytes, by the product loader. |
| `decision_engine_ca_bundle` | `--jev-ca-file` | A regular single-link file that other accounts cannot write, of at most 65536 bytes. |
| `gateway_ca_bundle` | `--gateway-ca-file` | Same. |
| `engine_program` | The engine program | Hashed by the approval. It is listed whenever an approval ran on an existing program, even when a later check refused it. |

Files are opened in a fixed order, and each one is compared, before it is opened, with every key file, credential, setup record and store known at that moment. The files at the fixed names of the inspected directories come first: `tenants.json` of an installation directory, then `providers.json` of a config directory. The files given by option follow: `--tenants-file`, `--providers-file`, the OIDC file, the bundles, then the engine program. The `tenants.json` of an installation directory that has a staging name is read to tell what the directory is: it is listed as read even when nothing of that directory is described.

### Files never opened

Key files, token files, provider credentials, the setup records (`secret-material.json`, `preparation.json`, `completion.json`, `authentication-creation.json`, `authentication-origin.json`) and every store with its sidecars are described by their metadata: presence, type, mode, owner, link count. `content` is `not_read`, except for the one case that the report itself names: `read_before_named`, for a file reported by `reference.file_read_before_it_was_named`. No `.env` file, OAuth cache or keychain is read. The five setup records of a prepared installation are listed with the role `setup_material`; `authentication-creation.json` exists only for a version 2 preparation and is not listed when it is absent.

The private-file readers also judge the directories that lead to a file. The tool asks the reader itself for that verdict, without restating its rule: the reader walks those directories and is stopped before it opens anything in them. Directories are opened for that question, never the file. A refusal is reported by `provider.credential_unsafe`, `provider.unselected_profile_reference_unsafe` or `reference.private_rule_violated`, with a message that names the path. For a file that conforms below a directory that the reader accepts and that is not a private 0700 directory, the tool does not check the file system or the extended access entries of the file itself: `reference.private_rule_not_evaluated` says so, and only the consuming command checks them when it opens the file. On macOS the walk of the reader also refuses a directory whose extended access entries permit a modification: the message names that cause, which is read in the product code and was not executed, since every run of the tool was on Linux.

Every inspected directory reserves `secrets`, `mounted-secrets`, `tenants`, its authentication audit and its setup records, whatever option gave the directory and whatever it turns out to hold. A file option or a CA reference that points there is never opened, and is reported instead. `tenants.json` and `oidc.json` of an installation directory are compared in the same way before they are opened at their fixed names:

| Option or reference | Finding |
|---|---|
| `--tenants-file`, or `tenants.json` of an installation directory | `registry.file_unreadable` |
| `--oidc-config`, or `oidc.json` of an installation directory | `oidc.file_unavailable_or_not_private` |
| `--providers-file` | `provider.file_unavailable_or_not_private` |
| A CA reference or a CA option | `trust.ca_reference_is_a_credential_reference` |
| `--runtime-program` | `engine.program_is_a_credential_reference` |

Consequence: a registry, an OIDC file, a profile file, a bundle or an engine program that you keep below `secrets`, `mounted-secrets` or `tenants` of an inspected directory is not read. A second name of a key file, a setup record or a store that the inspected files name, such as a hard link, is recognised too. A path is compared as it is written and as it resolves: a symbolic link, or a linked directory on the way, that leads to such a file is recognised. A reference that is itself a symbolic link names its target as well: its reader refuses the link, and an option that names the target directly is not opened. Resolving a path opens no file and lists no directory.

A file withheld in this way is reported as an `error` and the exit code is 3, whatever the consuming command would do with it: it is a refusal of this tool, not of that command. To have such a file read, keep it outside those directories, or run the tool without the directory option that reserves them. Without that option only the files that the inspected files name stay protected: the setup records of that directory, and anything else below those three directories, are unknown to the run.

What this protection does not cover:

- A file given by option that lies outside every inspected directory, and that no file read before it names, is opened as what the option says it is. Give each option the file it describes. The content is never printed: an invalid file yields an error category, a line and a column.
- The order of the reads protects one direction only. `--tenants-file` is read before `--providers-file`: a registry option that names a credential of a profile file given by option is read as a registry. `providers.json` of a config directory is read before `--tenants-file`: if the registry named that very file as a key file, it would already have been read as a profile file. A profile file is read before the OIDC file: a profile option that names the authentication audit recorded by that OIDC file, at a path that no inspected directory reserves, is read as a profile file. When a file read later names a file already opened, the report says so: `mode.credential_contents_read` is `detected_after_read`, the `mode` line of the text changes, and the error `reference.file_read_before_it_was_named` gives the role of the file that was opened. Where the report lists that file, in `credential_references` or in `data`, its entry carries `content` `read_before_named`; a file that is not described, such as one recorded by a staging directory, has no entry. The two findings on content, `reference.content_not_checked` and `provider.credential_content_not_checked`, then name that error instead of saying that no such file was opened. `none` means that no opened file is named as a key file, a credential, a setup record or a store by the files inspected in this run. A credential that no inspected file names cannot be recognised.
- `tenants.json` and `oidc.json` of an installation directory are opened by their fixed names after that comparison. A second name of a setup record, of the authentication audit or of a key file that an earlier file names is not opened. A second name of a key file below `tenants` is not recognised when it stands for `tenants.json`: only the registry names those files, and the tool lists no directory. `oidc.json` is read by the private-file rule, which refuses a file that has a second name.
- A CA reference that points at a private file the tool cannot recognise as a key, a record or a store is read as the gateway would read it. Its content is never printed.
- A registry or an OIDC file names nothing when it is not read or not decoded: the key files, the stores and the audit that it would have named are then known only through the reserved names of an inspected directory.
- The comparison with the reserved files and the open are two separate path resolutions. A change made by the same account, or by root, between the two (a renamed file, a replaced linked directory) is outside this guarantee, as it is for the product. When the opened path still leads to a reserved file at the end of the run, the report says so by `reference.file_read_before_it_was_named`; when the change was undone, it does not. This follows from the code: no run exercised such a change.
- A reserved directory is recognised by its name, as written and as resolved, and a named file by its path and by its device and inode. On a volume that ignores case, a file that no inspected file names, kept below `secrets`, `mounted-secrets` or `tenants` of an inspected directory and given under another case of that directory name, has not been verified to be refused: no run was made on such a volume.

### Paths, identifiers and URLs in the output

- Without `--show-paths`, a path below an inspected directory is shown relative to a placeholder: `<config>`, `<installation>`, `<tenants-file-dir>` or `<oidc-config-dir>`. A file given by option is shown as `<tenants-file>`, `<oidc-config>`, `<providers-file>`, `<jev-ca-file>`, `<gateway-ca-file>` or `<runtime-program>`. The root of the file system is never such a directory: when `--config-dir` or `--installation-dir` is `/`, or when a registry or an OIDC file given alone lies directly below `/`, no path is shown relative to it. A path is then shown relative to another inspected directory that holds it, or else by its role and an ordinal; the profile file at its fixed name in a config directory that is `/` is shown as `[path not shown]`, which is read in the code, since no run reached that case.
- Any other path is shown by its role and an ordinal, never by a file name, for example `runner key reference #1 (outside the inspected directory)`. A provider credential is shown as `provider reference #N`, with its source kind and whether it lies inside the directory of the profile file. N is the rank of its profile among the sorted identifiers.
- `--show-paths` prints absolute paths. Share such a report only where those paths may be shared.
- A default report hides paths, not names. It still prints tenant identifiers, the public parameters of an OIDC file (issuer, endpoint URLs, client identifiers, audience, provider label, web origin, web redirect, CLI callback path and the revision of the claim mapping), profile identifiers, models and credential variable names, and a decision-engine URL given on the command line: share it only where these may be shared.
- A URL is rebuilt from its parsed value: scheme, host, port and path. User information, query and fragment are never printed. A URL that cannot be parsed is shown as `[unparseable URL]`.
- Control characters and bidirectional controls are escaped, so that no value can forge a line. So are the characters that are invisible by default (zero-width characters, soft hyphen, word joiner and invisible operators, fillers, variation selectors, tag characters) and every space other than the ASCII space: two values that differ only by such a character are shown differently. A label that legitimately holds such a character, for example a no-break space, a zero-width joiner or a variation selector, is shown escaped too. An escaped character is written `\u{…}` with its hexadecimal code point, and a backslash is doubled. A value is bounded to 512 bytes, a path to 1024, with a truncation mark.
- A value that the output filter of the product would rewrite is replaced whole: `[masked value]`, `[masked path]`, or a numbered `[masked tenant identifier N]` or `[masked profile identifier N]`. The report then carries the warning `output.value_masked`. A path that would be rewritten stays masked with `--show-paths` too.
- The text of a loader, of a parser or of the file system is never copied. A refusal is reported by a closed reason, and an invalid JSON file by its error category and position.

### The active discovery

`--active oidc-discovery` is the only network operation of the tool. It calls the discovery of the product's own OIDC client, built from the OIDC file exactly as the gateway builds it, and changes none of its bounds:

- **Two GET requests, at most.** The discovery document, at `/.well-known/openid-configuration` below the issuer, then the `jwks_uri` of the file. The keys are not asked for when the discovery document does not confirm the file. No other URL is contacted: an endpoint announced by the provider is compared with the file, never adopted.
- **The file chooses the hosts.** Both URLs come from the inspected OIDC file. For a file on the `https_only` transport no address filter is applied to them: a host on a private network is contacted like any other. A file that declares the demonstration HTTP transport passes the offline rules only when its issuer is a loopback address literal or the fixed demonstration issuer `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo`, with its endpoints below that issuer. Nothing is printed before the contact. Run the tool once without `--active`, read `issuer` and `keys endpoint` in the identity block, `issuer` and `jwks_uri` in the JSON document, and add `--active oidc-discovery` only for a provider you mean to contact from this host.
- **No credential.** No token, no key, no client secret and no cookie is sent. The token endpoint is never contacted.
- **Bounded.** No proxy, no redirect followed, no retry. The time limit and the size bound are the `limits` of the provider block of the file: 5000 milliseconds and 131072 bytes when the file sets none.
- **TLS unchanged.** Certificates are verified against the CA bundle of the OIDC file when it declares one, against the public roots otherwise. Plain HTTP is used only for a file that declares the isolated demonstration transport. No option relaxes this.
- **Nothing of the answer is printed.** The product client returns a closed result and nothing else: no status line, header, body or transport text reaches this tool, so none can reach a report. The report carries one closed code.
- **Nothing is written.** The check stores nothing and changes nothing in the installation.

An `ok` says that this provider answered this client from this host at this moment. It proves neither a sign-in, nor that the gateway reaches the provider from its own network, and it qualifies no identity provider.

What was executed: the check ran against a controlled issuer on the loopback of a test container, over the demonstration HTTP transport only. A run that the issuer confirmed sent two requests over HTTP/1.1, the discovery document then the keys, with the header names `accept` and `host` and no body; header values were not recorded. Not executed: an HTTPS request, with or without a CA bundle, any real identity provider, and the results `active.oidc_discovery_provider_rejected` and `active.oidc_discovery_keys_invalid`. That the client uses no proxy, follows no redirect and does not retry is read in the client of the product, not observed.

### Host reads of the engine check

`--runtime` runs the approval of the product, which is not free of host reads: it asks the password database for the home of the control account, looks at `/run/user/<uid>` and at the system directories of the helper path, and hashes the program, up to 512 MiB. This can take seconds. Only four facts reach the report: the engine kind, the display of the approved entry, its SHA-256 and the policy version. The environment, the working directory, the account home and the canonical path stay out.

## Limits and non-qualified areas

- **Files, not a running system.** The report says nothing about running services, the state of the engine, the worker image or the content of a store.
- **Half of the gateway loader.** The `ok` of a registry or of an OIDC file covers the structural rules restated in this tool and the product validators it calls. It is always paired with `registry.loader_acceptance_not_checked` or `oidc.loader_acceptance_not_checked`: only the gateway reads the keys, opens the stores and compares the durable state. The restated rules are compared with the loaders by tests of this repository, on their field names and on a corpus of accepted and refused files: a default, an alias or a type change in a loader that no case exercises is not detected.
- **An authentication policy, before the first initialisation.** `oidc.authentication_policy_invalid` covers two product checks. The gateway applies the second, that the policy funds the capacity plan of a version 2 file, only after it has opened or created the authentication audit. As observed through the loader function of the gateway, a gateway asked to initialise the audit of such a file refuses after it has created the audit file, and leaves that file in place; the report does not mention that file. This was not executed through the gateway binary started with `--initialize-oidc-audit`. The tool reports that policy offline: inspect a new OIDC file before the gateway is first started with that option.
- **Closed reasons depend on product texts.** A reason is recognised by comparing the refusal of a product validator with known texts. An unrecognised refusal is reported with the reason `unclassified`; the verdict itself stays exact. Two engine refusals keep the reason `unclassified` by design, an extended access entry and a refused `/run/user/<uid>`: no closed reason names them, and each has its own message and action.
- **A selection, not a launch.** The profile a managed launch selects is the choice of the product, for the selection options given to this tool. Nothing else of a launch is established: the credential of the selected profile is not opened, and the rules that `run` applies afterwards, such as `--profile observe` with `--network bridge` and the place of a credential inside the project, are not applied. A missing or unsafe credential is an error only for the profile that the selection resolves.
- **No transport verdict without a usable bundle.** When the bundle given with `--jev-ca-file` is refused or unreadable, the transport is judged neither valid nor invalid, unless the URL is refused even without a bundle. The report then lists `inference.decision_transport_not_checked` among the items not checked.
- **No validity dates.** An expired certificate is not detected.
- **Key files by their metadata.** An empty or oversized key file, and one file referenced for two identities, are certain refusals of the gateway or of the legacy commands that this tool does not detect: they are covered only by `reference.content_not_checked` and `registry.loader_acceptance_not_checked`.
- **One account, local file systems.** The posture rules refuse another owner and an unsupported file system. That is reported as `unknown` or `error`, never as an empty result.
- **Container volumes.** An installation in a named volume can be inspected only where the tool exists: in a container of a control image whose Dockerfile builds and copies the tool, as the Quick Start describes. That journey depends on your image build and on your engine; its commands have not been run, and this guide qualifies neither.
- **One active check, on request.** `--active oidc-discovery` is the only operation that opens a connection. Without it `mode.active_checks_executed` is empty and `mode.network` is `none`. No engine, no decision engine and no AI provider is ever contacted: nothing in this build checks them actively.
- **Platforms.** The posture policy is the product's, for local Linux and macOS file systems. This guide qualifies no platform, no engine, no identity provider and no AI provider.
- **English output.** Messages and actions are in English in both guides.

### What was not executed

The statements of this guide rest on runs of the tool where a run could be made, and on the code elsewhere. The following were not executed:

- **Platforms.** Every run of the tool was on Linux, on ARM64, in a container without network. Nothing was run on macOS: the macOS branches of the posture policy, extended access entries included, are read in the product code only.
- **The release build.** `cargo build --release --locked`, the first command of this guide, was not run: the tool was built and run in the debug profile, offline.
- **Case-insensitive volumes.** None was used: see the last item of "What this protection does not cover".
- **A second account.** A registry that the inspecting account cannot read, and a store or a store directory that belongs to another account, were not built. Their verdicts, `reference.inspector_identity_differs` and `reference.metadata_unavailable`, rest on the classification of file metadata, which a unit test covers for the registry only.
- **Stores.** The data axis was run through the tool, with the gateway as the reference, for a store without sidecars, the `-wal` and `-shm` sidecars of a gateway that runs or was stopped without a clean close, an absent tenant store, a sidecar without its store, a store that other accounts can read, a store directory that is not private, a store path with a parent traversal, a reserved or non-ASCII store name, and an absent authentication audit. Not executed: a `-journal` sidecar, and, for the authentication audit, every refusal other than its absence. The tool was run once while a gateway process held the stores of the installation: it exited with 0 and reported `data.sidecars_present`, and the gateway process was still running afterwards. That the gateway kept serving requests during that run was not checked.
- **Recovery.** What `setup resume` does on a published installation whose authentication audit is absent was not executed: the message of `data.authentication_audit_missing_from_prepared_installation` states the refusal of the gateway only.
- **Engines.** No engine was present at the fixed system locations: `--runtime` without `--runtime-program` was run only up to `engine.program_absent_from_fixed_entries`, and the approval was exercised on a marker program given by `--runtime-program`.
- **Providers and clients.** No AI provider, no model, no Codex CLI and no Claude Code was contacted or run.

## JSON contract, version 1

`--json` writes one document on stdout, of at most 524288 bytes. Its `schema` is `ai-guard-diagnose-report` and its `version` is `1`.

```json
  "mode": {
    "network": "none",
    "mutation": "none",
    "credential_contents_read": "none",
    "files_read_for_content": [
      "provider_profiles"
    ],
    "active_checks_executed": []
  },
```

### Compatibility rule

Every field is always present: a section or a value that was not inspected is `null`, never a status word.

| Change | Within version 1 |
|---|---|
| Adding a field, a code, a `details` kind or a value of a closed list | Allowed. A consumer ignores a field it does not know and treats an unknown code by its `status`. |
| Removing or renaming a field or a code; changing the status, the axis or the basis of a code; changing the meaning of an exit code | Not allowed: it changes `version`. |
| Retiring a code | The code is never reused. |

The first public version 1 baseline is the corrected composed integration freeze: its fields, codes and closed values are listed here. Existing axes are retained, including `trust.decision_transport_invalid` on `trust`. The IDs `provider.selection_resolution_not_available` and `identity.idp_reachability_not_available` were removed before first publication; they stay reserved and will never be recycled. No previously deployed version 1 is implied by that prepublication history.

The Rust types of this repository (`diagnostics::report::ReportV1`) decode the reports of their own build only: they refuse a code or a closed value they do not know, and they drop a field they do not know. A consumer of the JSON document applies the rule above itself. No test decodes a report of another build.

### Document

| Field | Content |
|---|---|
| `schema`, `version` | The identity of the contract. |
| `tool` | `name` and `package_version`. |
| `mode` | `network`, `none` or `active_checks_only`; `mutation`, always `none`; `credential_contents_read`, `none` or `detected_after_read`; `files_read_for_content`; `active_checks_executed`, the active checks that ran, whatever their result: `oidc_discovery`. |
| `summary` | `status` (`ok`, `error` or `unknown`), `exit_code`, `counts` by status (`ok`, `info`, `warning`, `error`, `unknown`, `not_checked`, `not_available`), `truncated`, and `omitted`: `reference_entries`, `data_entries`, `tenant_ids`. |
| `inputs` | One entry per input: `name`, `origin` (`cli` or `default`) and `display`. The selection options are the inputs `client`, `auth_mode`, `client_profile` and `model`, listed when they are given. |
| `layouts` | One entry per inspected directory or registry: `input`, `kind`, `preparation`, `oidc_file_version`. |
| `topology` | `tenant_model` (`single_local` or `registry`) and `tenant_count`; `null` when nothing describes it. |
| `identity` | `technical_key_references` counted as `present`, `absent` and `unknown`; `sso`, `null` when no OIDC file was decoded. |
| `tenants` | `registry` and `plan`, lists of identifiers; `registry_total` and `plan_total`, the number of distinct identifiers each file declares; `correspondence` (`match`, `mismatch` or `null`). |
| `credential_references` | One entry per referenced file. |
| `providers` | `file` with its `state` (`absent`, `refused`, `invalid` or `loaded`) and `display`; `default_profile`; `auto_priority`; `profiles`; `selection`, `null` when no profile file was loaded. |
| `inference` | `decision_engine_transport`: `kind` (`remote` or `local`), `url`, `allow_local_http`, `ca_bundle_given`; `null` when no URL was given. |
| `trust` | One entry per CA bundle: `role`, `display`, `read`, `certificate_count`. |
| `engine` | `kind`, `entry_display`, `sha256`, `policy_version`; `null` unless a program was approved. |
| `data` | One entry per store. |
| `findings` | The findings, ordered by axis, code and scope. |
| `limits` | The limits of the build, in the same shape as a finding. |

An identifier is an object: `id` and `masked`. `masked` is `true` when a numbered placeholder stands for the identifier.

`sso` carries the public parameters of the OIDC file: `file_version`, `issuer`, `transport` (`https_only` or `isolated_demo_http`), `provider_label`, `web_origin`, `web_client_id`, `cli_client_id`, `api_audience`, `authorization_endpoint`, `token_endpoint`, `jwks_uri`, `end_session_endpoint`, `web_redirect_uri`, `cli_callback_path`, `mapping` (`revision` and `rule_count`), `ca_bundle_declared`, and `idp_contacted`, `true` when the active discovery ran against the provider, whatever its result.

A profile of `providers` carries `profile`, `revision`, `client` (`codex` or `claude_code`), `auth_mode` (`api` or `subscription`), `model`, `source_kind` (`api_env_file`, `codex_auth_file` or `claude_oauth_env_file`) and `variable_name`. Its reference is the entry of `credential_references` whose `owner` is that profile.

`selection` carries `requested` and `resolved`. `requested` says how each part of the selection was asked: `client`, `auth_mode`, `profile` and `model` are each `explicit` or `auto`; the values of the options are in `inputs`. `resolved` is `null` when the product refuses the selection. Otherwise it carries `profile`, `revision`, `client`, `auth_mode`, `model` (the requested model, else the model of the profile, else `null`) and `chosen_by`: `profile` when the profile was asked for by name, `default_profile`, or `auto`.

### Reference and data entries

| Field | Values |
|---|---|
| `owner` | `kind` (`installation`, `tenant` or `provider_profile`), `id`, `masked`. |
| `role` | Reference: `runner`, `viewer`, `decision_engine`, `projection`, `provider`, `setup_material`. Data: `tenant_store`, `authentication_audit`. |
| `consumer` | The reader whose rule applies: `gateway_token_reader`, `private_file_reader`, `compose_projection`, `setup_recovery`. |
| `location` | `inside_root` or `outside_root`. |
| `display` | The path, as the output rules show it. |
| `presence` | `present`, `absent` or `unknown`. |
| `entry_kind` | `regular_file`, `directory`, `symbolic_link`, `other`, or `null`. |
| `mode` | The permission bits as four octal digits, or `null`. |
| `owner_is_inspector`, `single_link` | `true`, `false`, or `null` when the entry was not found. |
| `private_rule` | `satisfied`, `violated` or `not_evaluated`. `violated` also covers a file whose reader refuses a directory that leads to it. `not_evaluated` depends on the reader. The gateway token reader checks no owner, mode or link count. For the private-file reader, the file conforms and its directory is not a private 0700 directory, so the entry check of the product did not run: `reference.private_rule_not_evaluated` says what stays unchecked. For a projection and for a setup record, this tool applies no private-file rule. |
| `sidecars` | Data only: `wal`, `shm`, `journal`. |
| `content` | `not_read`, or `read_before_named` for a file reported by `reference.file_read_before_it_was_named`: it was opened as another input before a file read after it named it. For a store, also when one of its sidecars was. |

The lists are bounded in what they print, never in what they evaluate: at most 512 entries and 262144 bytes for each of `credential_references` and `data`, 256 identifiers per tenant list, 16 per subject. Entries tied to an error, an unknown or a warning come first. What is not printed is counted in `omitted`, and every total stays exact; the text report then has its `omitted` line. `truncated` `false` does not mean that every entry is listed: read `omitted`. When the document still exceeds its bound, the detail lists are emptied, `truncated` is `true` and the finding `output.detail_omitted` is added.

### Findings

```json
      "details": {
        "kind": "would_refuse_with",
        "class": "login_required"
      }
```

| Field | Content |
|---|---|
| `code`, `axis`, `status`, `basis` | As in the catalogue below. |
| `scope` | `kind`, with `input` or `file` when the kind names one. |
| `origin` | `sources`, each a `file` and a `field`; `consumer`, the command that applies the rule; `assumes`, the assumed inputs. |
| `message`, `action` | English sentences of the catalogue. An action names your own input by its display. |
| `requires` | A sentence for `not_checked`, `null` otherwise. |
| `subjects` | `kind` (`tenants` or `provider_profiles`), `total`, `ids`, `omitted`; or `null`. |
| `details` | One closed shape selected by `kind`, or `null`. |

| `details` `kind` | Fields |
|---|---|
| `tenant_set_difference` | `only_in_registry`, `only_in_plan`, `omitted`. |
| `json_error` | `category` (`syntax`, `data`, `eof` or `io`), `line`, `column`. |
| `reason` | `reason`, one of the closed reasons below. |
| `policy` | `policy`: `worker_quota`, `kubernetes_held_create`, `kubernetes_session_reservation`, `kubernetes_session_recovery`, `kubernetes_session_control_recovery`, `audit_capacity`, `workload_audit`, `relay_retention`. |
| `would_refuse_with` | `class`: `login_required` or `credential_missing`. |
| `sidecars` | `sidecars`. |
| `engine_program` | `explicit_program`: `true` for a program given by option, `false` for the fixed locations. |

| `reason` | Reported by |
|---|---|
| `version_or_token_bound`, `web_origin_or_callback`, `claim_mapping_invalid`, `claim_mapping_ambiguous`, `url_unparseable`, `provider_block_invalid` | `oidc.gateway_block_invalid` |
| `duplicate_or_invalid_profile`, `unknown_or_duplicate_priority`, `default_profile_missing` | `provider.configuration_ambiguous` |
| `json_invalid`, `version_or_size`, `profile_identifier`, `model_identifier`, `source_for_mode`, `variable_for_client`, `path_not_absolute`, `path_traversal` | `provider.configuration_invalid` |
| `explicit_conflict`, `several_match_without_priority` | `provider.selection_ambiguous` |
| `requested_profile_not_configured`, `no_profile_matches` | `provider.selection_no_profile` |
| `profile_identifier`, `model_identifier`, for a value given by option | `provider.selection_invalid` |
| `account_home_unavailable`, `helper_directories_refused`, `program_or_directory_refused` | `engine.program_refused` |
| `unclassified` | Any of these codes, for a refusal whose text is not recognised. |

`engine.program_refused` has three closed reasons. Its message and its action name the cause that the product gave:

| Cause named by the message | `reason` | Next step |
|---|---|---|
| Nothing exists at the given path | `program_or_directory_refused` | Give an existing program with `--runtime-program`, or leave that option out. |
| The given path is not absolute | `program_or_directory_refused` | Give the absolute path. |
| The form of a path: a parent traversal, a control character, a path that is not UTF-8, or the home directory of the account | `program_or_directory_refused` | Give an absolute path without traversal; check the home directory of the control account. |
| The path cannot be resolved: a link that leads nowhere, or a directory that cannot be traversed | `program_or_directory_refused` | Give the path of the program itself. |
| The path resolves to the root of the file system | `program_or_directory_refused` | Give the path of the program file. |
| The directory that holds the program belongs to another account, or group or others can write it | `program_or_directory_refused` | Use a directory that only root or the control account can write. |
| The program is not a regular executable file of root or of the control account, or group or others can write it | `program_or_directory_refused` | Give such a file. |
| The program is larger than 512 MiB | `program_or_directory_refused` | Give the program itself. |
| The engine at a fixed location is a link to a program outside the locations approved without an explicit path | `program_or_directory_refused` | Pass that program with `--runtime-program ABS`. |
| The file system is not supported | `program_or_directory_refused` | Install the engine on a supported local file system. |
| The directory of the program cannot be opened without following a link | `program_or_directory_refused` | Run the tool as the control account; check its access to each directory on the way. |
| The program cannot be opened for reading | `program_or_directory_refused` | Run the tool as the control account; check that it can read the program. |
| The password database gives no usable home directory for the account | `account_home_unavailable` | Run the tool as the control account. |
| `/usr/bin`, `/bin`, `/usr/sbin` or `/sbin` is not owned by root, or group or others can write it | `helper_directories_refused` | Restore root ownership without group or other write permission. |
| `/run/user/<uid>` exists and is not a private directory of the account | `unclassified` | Restore its ownership and mode 0700. |
| An extended access entry on the program, on its directory or on a system directory of the helper path | `unclassified` | Use an installation without extended access entries. |

Seven of these sixteen causes were produced by runs of the tool: rows 1, 2, 4, 6, 7, 11 and 12 of the table. The nine others are bound to the texts of the product by a test that reads its source, and were produced by no run. The refusals that concern the fixed system locations were not run against a real engine, and the extended access entry exists only on macOS: this guide qualifies none.

`oidc.gateway_block_invalid` names, under `basis`, the fields that the product check behind its reason reads:

| `reason` of `oidc.gateway_block_invalid` | Fields named under `basis` | Where the SSO guide states their rules |
|---|---|---|
| `version_or_token_bound` | `gateway.version`, `gateway.provider.limits.max_token_bytes`, `gateway.provider.limits.pkce_ttl_seconds` | "Prepare the private gateway JSON" and "Request and session limits" |
| `url_unparseable` | `gateway.web_origin`, `gateway.provider.web_redirect_uri` | "Register public clients and the API audience" |
| `web_origin_or_callback` | `gateway.web_origin`, `gateway.provider.web_redirect_uri`, `gateway.provider.transport` | "Register public clients and the API audience" and "Prepare the private gateway JSON" |
| `claim_mapping_invalid` | `gateway.provider_label`, `gateway.mapping.revision`, `gateway.mapping.rules`, `gateway.mapping.default_capabilities` | "Prepare the private gateway JSON" and "Configure effective permissions" |
| `claim_mapping_ambiguous` | `gateway.mapping.rules` | "Configure effective permissions" |
| `provider_block_invalid` | `gateway.provider` | "Register public clients and the API audience" and "Prepare the private gateway JSON" |
| `unclassified` | `gateway` | The whole block: the refusal is one that this tool does not recognise. |

For the provider object the product validator returns one closed value. `provider_block_invalid` therefore covers every rule of `gateway.provider`: its version, distinct client identifiers and audience, issuer and endpoint schemes for the declared transport, distinct endpoints, redirect, callback path, logout returns and the limits. The tool does not say which one failed. An `http` issuer under the `https_only` transport is one of these cases: compare the `issuer` and `transport` lines of the identity block.

### Catalogue of codes

One code has one status, one axis and one basis. Some codes have a second message for a named situation, for example a reserved file; the code and its status do not change.

#### Layout

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `layout.historical_config_dir` | `info` | `layout` | `observed` | The directory has the historical layout created by `init`. |
| `layout.prepared_installation` | `info` | `layout` | `observed` | The directory has the layout published by `setup prepare`. |
| `layout.hand_written_registry` | `info` | `layout` | `observed` | A registry was given alone; its references are resolved as the gateway resolves them. |
| `layout.target_absent` | `error` | `layout` | `observed` | The inspected directory does not exist. The action starts with the path: a directory given by mistake, or an installation that exists elsewhere. |
| `layout.unrecognized` | `error` | `layout` | `observed` | The directory holds neither layout. A directory that holds `tenants.json` or `oidc.json` without a setup record has its own message, and an action that names the two file options. The message makes no statement on what was opened: the list of the files read does. When the registry of a directory with a staging name was read to tell what the directory is, the message says so. |
| `layout.posture_refused` | `error` | `layout` | `observed` | The directory is refused by the private-directory rule, or is not a directory. The message tells a mode other than 0700 from a refusal that comes from a directory above it, from a link in its path or from its file system; it names the extended access entries of macOS as a further cause, from the product code. |
| `layout.inspector_identity_differs` | `unknown` | `layout` | `observed` | The directory belongs to another account or could not be examined. |
| `layout.mixed` | `error` | `layout` | `observed` | The directory holds both layouts. |
| `layout.prepared_passed_as_config_dir` | `error` | `layout` | `observed` | A prepared installation or a staging directory was given as `--config-dir`. |
| `layout.historical_passed_as_installation_dir` | `error` | `layout` | `observed` | A historical config directory was given as `--installation-dir`. |
| `layout.legacy_commands_do_not_read_this_layout` | `info` | `layout` | `inferred` | The legacy commands read a historical directory, never a prepared installation. |
| `layout.references_outside_inspected_root` | `warning` | `layout` | `observed` | The files of the installation record paths outside the inspected directory. |
| `layout.setup_staging_directory` | `error` | `layout` | `observed` | A retained staging directory: the preparation was not published. |
| `layout.staging_and_final_both_exist` | `error` | `layout` | `observed` | The staging directory and a path recorded by its registry both exist. |
| `layout.staging_without_intent` | `error` | `layout` | `observed` | A staging name without a preparation record. |
| `layout.completion_marker_absent` | `warning` | `layout` | `observed` | A published installation without its completion marker. |
| `layout.sibling_staging_not_inspected` | `not_available` | `layout` | `observed` | Limit: a staging directory beside an installation is not searched for. |
| `tool.linux_macos_posture_policy_only` | `not_available` | `layout` | `observed` | Limit: the posture policy covers local Linux and macOS file systems. |

#### Topology

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `topology.single_local_tenant` | `info` | `topology` | `inferred` | A gateway started without a registry serves one tenant, named `local`. |
| `topology.registry_tenants` | `info` | `topology` | `declared` | The registry declares the tenants counted in the report. |
| `topology.running_services_not_checked` | `not_checked` | `topology` | `observed` | No service is started or asked for its state: running services are not checked. |
| `topology.gateway_arguments_not_persisted` | `not_checked` | `topology` | `observed` | The arguments of the gateway and of the dashboard are not stored in the installation. |
| `topology.kubernetes_launch_not_available` | `not_available` | `topology` | `observed` | Limit: worker launch through the Kubernetes adapter is not available. |

#### Identity

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `identity.sso_not_inspected` | `not_checked` | `identity` | `observed` | No OIDC file was given. SSO is optional and enabled only by a gateway option. |
| `identity.sso_file_valid_offline` | `ok` | `identity` | `inferred` | The OIDC file passes the offline rules of the gateway loader. The verdict is drawn from the file alone. |
| `identity.sso_gateway_enablement_not_persisted` | `not_checked` | `identity` | `observed` | A valid file does not prove that the gateway is started with it. |
| `identity.sso_demonstration_transport` | `warning` | `identity` | `declared` | The OIDC file declares the isolated demonstration transport: sign-in over plain HTTP, which the product limits to a loopback issuer and to the distributed demonstration issuer. Not for a normal installation. |
| `identity.local_named_accounts_not_available` | `not_available` | `identity` | `observed` | Local named accounts live in the tenant stores. |
| `identity.idp_reachability_not_checked` | `not_checked` | `identity` | `observed` | The identity provider was not contacted: no connection is opened unless the active check is asked for. Also reported when the check is asked for and cannot run. |
| `active.oidc_discovery_ok` | `ok` | `identity` | `observed` | Active check: the provider answered, its discovery document confirms the OIDC file, and its key set holds a usable signing key. |
| `active.oidc_discovery_transport_unavailable` | `unknown` | `identity` | `observed` | Active check: no usable answer from where the tool runs. Connection, TLS, time limit or size bound; no further detail exists. |
| `active.oidc_discovery_provider_rejected` | `error` | `identity` | `observed` | Active check: the provider answered, but not with the JSON documents of a discovery. Its answer is not printed. |
| `active.oidc_discovery_metadata_mismatch` | `error` | `identity` | `observed` | Active check: the discovery document does not confirm the issuer, the endpoints or the capabilities that the OIDC file requires. |
| `active.oidc_discovery_keys_invalid` | `error` | `identity` | `observed` | Active check: the key set of the provider is refused by the product client. |
| `identity.arbitrary_oidc_provider_not_qualified` | `not_available` | `identity` | `observed` | Limit: compatibility with a given identity provider is not presumed. |
| `oidc.file_absent` | `error` | `identity` | `observed` | The OIDC file does not exist at the given path. |
| `oidc.file_unavailable_or_not_private` | `error` | `identity` | `observed` | The OIDC file is refused by the private-file rule, or is a reserved file. It was not read. |
| `oidc.json_invalid` | `error` | `identity` | `inferred` | The OIDC file is not the closed JSON document the gateway expects. |
| `oidc.version_plan_pair_invalid` | `error` | `identity` | `inferred` | Version 1 carries no capacity plan; version 2 requires one. |
| `oidc.gateway_block_invalid` | `error` | `identity` | `observed` | The gateway block is refused by the product validator, with a closed reason. The fields that the reason is about are listed under `basis`; the table under the reasons names them. |
| `oidc.capacity_plan_invalid` | `error` | `identity` | `observed` | The capacity plan is refused by the product validator. |
| `oidc.authentication_policy_invalid` | `error` | `identity` | `observed` | The authentication audit policy is refused by a product validator of the audit: the policy check of the gateway, or the check that the policy funds the capacity plan. |
| `oidc.reference_invalid` | `error` | `identity` | `inferred` | A file reference is empty or leaves its directory by parent traversal. |
| `oidc.loader_acceptance_not_checked` | `not_checked` | `identity` | `observed` | Acceptance by the gateway at start is not checked. |

#### Tenants

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `registry.file_unreadable` | `error` | `tenants` | `inferred` | The registry does not exist, cannot be read as the gateway reads it, or is a reserved file. An absent file given by option and an installation directory without its `tenants.json` each have their own message and action. |
| `registry.json_invalid` | `error` | `tenants` | `inferred` | The registry is not the closed JSON document the gateway expects. |
| `registry.version_unknown` | `error` | `tenants` | `inferred` | The registry version is not 1. |
| `registry.empty` | `error` | `tenants` | `inferred` | The registry declares no tenant. |
| `registry.tenant_id_invalid` | `error` | `tenants` | `observed` | A tenant identifier is refused by the product validator. |
| `registry.tenant_id_duplicate` | `error` | `tenants` | `inferred` | A tenant identifier appears more than once. |
| `registry.reference_empty` | `error` | `tenants` | `inferred` | A tenant has an empty store or key file reference. |
| `registry.storage_invalid` | `error` | `tenants` | `observed` | A tenant must declare exactly one storage selector: `data` or a valid `storage` profile. Correct the registry supplied with `--tenants-file`. |
| `registry.mysql_unavailable` | `error` | `tenants` | `observed` | This gateway cannot serve the declared MySQL profile. Keep the destination in the registry supplied with `--tenants-file` and use a gateway that supports the profile. |
| `registry.policy_invalid` | `error` | `tenants` | `observed` | A tenant policy is refused by the product validator; the details name the policy. An audit-capacity or a workload-audit policy is judged for the event bound given with `--max-events`. |
| `registry.policy_refused_at_assumed_event_bound` | `unknown` | `tenants` | `inferred` | An audit-capacity or a workload-audit policy is refused at 100000 events, the gateway default that this tool assumes when `--max-events` is not given. A gateway started with another bound may accept it. |
| `registry.event_bound_not_used` | `not_checked` | `tenants` | `observed` | An event bound was given with `--max-events` and nothing was validated with it: no decoded registry declares an audit-capacity or a workload-audit policy. |
| `audit.live_activity_budget_capacity` | `warning` | `tenants` | `inferred` | With the per-session live provider activity budget, a tenant audit holds fewer than 50 sessions by the gateway event bound or by the 256 MiB physical cap; the audit has no rotation yet and refuses every write once full. |
| `registry.offline_rules_passed` | `ok` | `tenants` | `inferred` | The registry passes the offline rules of the gateway loader. Token content is not checked. |
| `registry.loader_acceptance_not_checked` | `not_checked` | `tenants` | `observed` | Acceptance by the gateway at start is not checked. |
| `tenants.plan_matches_registry` | `ok` | `tenants` | `inferred` | The plan and the registry name exactly the same tenants. |
| `tenants.plan_set_mismatch` | `error` | `tenants` | `inferred` | The plan and the registry name different tenants; the details list each side. The action is to restore the registry to the tenant set of the plan: the plan that an authentication audit of format 4 stores cannot be changed, and the gateway refuses a file whose plan differs from it. The plan of a file can be changed only while no such audit exists: before the audit of a hand-written file is first initialised, or for the 3 to 4 migration of the SSO guide. |
| `tenants.plan_required_for_multi_tenant` | `error` | `tenants` | `inferred` | Several tenants with a version 1 OIDC file, which has no plan. |
| `tenants.single_tenant_without_plan` | `ok` | `tenants` | `inferred` | One tenant and a version 1 file: the two files are compatible. |
| `tenants.correspondence_not_checked` | `not_checked` | `tenants` | `observed` | The tenants and the plan were not compared. Without a registry beside a historical config directory, the action says what a version 1 and a version 2 file need for the single tenant `local`. |
| `tenants.durable_plan_comparison_not_available` | `not_available` | `tenants` | `observed` | The plan stored in the authentication audit cannot be read offline. |

#### Credential references

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `reference.absent` | `error` | `credential_references` | `observed` | A referenced file does not exist at its recorded path, inside the inspected directory. For a key of a historical config directory, the action names `agentsandbox init`, which creates the missing key and preserves the existing ones. |
| `reference.not_found_from_here` | `unknown` | `credential_references` | `observed` | A referenced path outside the inspected directory was not found from here. |
| `reference.not_a_regular_file` | `error` | `credential_references` | `observed` | A referenced path is a symbolic link or is not a regular file. |
| `reference.private_rule_violated` | `error` | `credential_references` | `inferred` | A file read through the private-file rule has another mode than 0600, several links, or rights granted to another account; or its reader refuses a directory that leads to it. |
| `reference.private_rule_not_evaluated` | `not_checked` | `credential_references` | `observed` | A file read through the private-file rule conforms and its reader accepts the directories that lead to it, but its directory is not a private 0700 directory: the entry check of the product did not run, and the file system and the access entries of the file are not checked. |
| `reference.token_file_accessible_to_other_accounts` | `warning` | `credential_references` | `observed` | The gateway token reader checks no mode, and the mode of this key file grants access to its group or to other accounts. Its content was not read: nothing is said of its acceptance. A mode that also denies the account of the gateway the read gets the same warning, and the gateway cannot open such a file. |
| `reference.inspector_identity_differs` | `unknown` | `credential_references` | `observed` | A referenced file belongs to another account; its rule was not judged. Also a registry of another account that this account cannot read. |
| `reference.metadata_unavailable` | `unknown` | `credential_references` | `observed` | The metadata of a referenced path could not be read. |
| `reference.content_not_checked` | `not_checked` | `credential_references` | `observed` | No file that the inspected files name as a key, token or credential file is opened: content is not checked. When a file was read before it was named, the message names `reference.file_read_before_it_was_named` instead. |
| `reference.setup_secret_material_present` | `info` | `credential_references` | `observed` | The setup recovery record that holds the generated tokens is present. It was not opened. |
| `reference.file_read_before_it_was_named` | `error` | `credential_references` | `observed` | A file opened as an input is named, by a file read after it, as a key file, a credential, a setup record or a store. Its content was read; it is not printed. Its entry, where the report lists the file, carries `content` `read_before_named`. |

#### Providers

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `provider.not_inspected` | `not_checked` | `providers` | `observed` | No provider location was given, or the config directory could not be inspected. |
| `provider.file_absent` | `info` | `providers` | `observed` | The config directory has no `providers.json`: no managed profile is declared. |
| `provider.file_unavailable_or_not_private` | `error` | `providers` | `observed` | The profile file is refused by the private-file rule, is absent where an option names it, or is a reserved file. |
| `provider.configuration_valid` | `ok` | `providers` | `observed` | The profile file is accepted by the product validator, which reads that file alone. |
| `provider.configuration_invalid` | `error` | `providers` | `observed` | The profile file is refused by the product validator, with a closed reason. |
| `provider.configuration_ambiguous` | `error` | `providers` | `observed` | A duplicate profile, an unknown or duplicate priority, or a default profile that does not exist. |
| `provider.login_required` | `error` | `providers` | `inferred` | The profile that the selection resolves is a subscription profile and its credential file is absent. No API profile is tried. |
| `provider.credential_missing` | `error` | `providers` | `inferred` | The profile that the selection resolves is an API profile and its credential file is absent. |
| `provider.credential_unsafe` | `error` | `providers` | `inferred` | The credential file of the profile that the selection resolves is a link, is not a regular file, or violates the private-file rule; or its path passes through an entry that is not a directory; or its reader refuses a directory that leads to it. |
| `provider.unselected_profile_reference_absent` | `warning` | `providers` | `inferred` | The credential file of a profile that the selection does not resolve is absent. |
| `provider.unselected_profile_reference_unsafe` | `warning` | `providers` | `inferred` | The credential file of a profile that the selection does not resolve is unsafe, its path passes through an entry that is not a directory, or its reader refuses a directory that leads to it. |
| `provider.credential_content_not_checked` | `not_checked` | `providers` | `observed` | No file that the profile file names as a provider credential is opened: its content is not checked. When a file was read before it was named, the message names `reference.file_read_before_it_was_named` instead. |
| `provider.manual_launch_path_not_checked` | `not_checked` | `providers` | `observed` | The manual launch path does not use the profile file. |
| `provider.launch_requires_observe_and_bridge` | `info` | `providers` | `inferred` | A managed launch with provider credentials requires `--profile observe` and `--network bridge`. |
| `provider.selection_resolved` | `ok` | `providers` | `observed` | The requested selection resolves to one profile, by the choice of the product, which reads no credential file. |
| `provider.selection_ambiguous` | `error` | `providers` | `observed` | The product refuses the selection as ambiguous: several profiles match and nothing decides, or the request contradicts the requested profile. |
| `provider.selection_no_profile` | `error` | `providers` | `observed` | No declared profile answers the selection. No profile of another authentication mode is used in its place. |
| `provider.selection_invalid` | `error` | `providers` | `observed` | A requested profile or model identifier is refused by the product validator. |
| `provider.selection_not_resolved` | `not_checked` | `providers` | `observed` | Selection options were given and no profile file was loaded: nothing was resolved. |
| `provider.subscription_never_falls_back_to_api` | `info` | `providers` | `inferred` | A subscription is requested or resolved and the launch refuses: the API profiles of the file were not tried. The message says which case it is: a resolved profile whose credential is refused, a refused subscription request, or a file that declares no subscription profile. The action states the automatic rule: an API profile can be resolved when no subscription profile matches the requested client, and never for a request made with `--auth-mode subscription`. |
| `provider.api_billing_notice` | `warning` | `providers` | `inferred` | The resolved profile is in API mode: usage is billed by the provider. |
| `provider.qualified_versions_not_available` | `not_available` | `providers` | `observed` | Limit: this build contains no list of qualified client versions. |
| `provider.client_presence_not_available` | `not_available` | `providers` | `observed` | Limit: client presence is probed only inside the worker. |
| `provider.client_version_not_available` | `not_available` | `providers` | `observed` | Limit: client version is probed only inside the worker. |
| `provider.account_validity_not_available` | `not_available` | `providers` | `observed` | Limit: only the provider establishes the validity of an account. |

#### Inference

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `inference.decision_engine_selection_not_persisted` | `not_checked` | `inference` | `observed` | The decision engine is selected by gateway options that are not stored. The gateway refuses an incomplete or mixed set of those options, which this tool does not check. |
| `inference.decision_transport_valid` | `ok` | `inference` | `observed` | The given transport is accepted by the product validator. The engine was not contacted, and the option set of the gateway is not checked. |
| `inference.decision_transport_not_checked` | `not_checked` | `inference` | `observed` | The given transport was not judged: the CA bundle given with it could not be used. |
| `inference.decision_transport_inputs_without_url` | `not_checked` | `inference` | `observed` | A local HTTP permission or a CA bundle for the decision engine was given without a URL: no transport was judged. |
| `inference.local_model_backend_not_available` | `not_available` | `inference` | `observed` | No offline inspection of a local model backend. |
| `inference.model_tls_outputs_not_available` | `not_available` | `inference` | `observed` | No reader for the outputs of model TLS provisioning. |
| `inference.internal_profile_not_available` | `not_available` | `inference` | `observed` | Limit: no managed internal-inference profile for Codex CLI or Claude Code. |

#### Trust

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `trust.oidc_ca_bundle_accepted_by_grammar` | `ok` | `trust` | `observed` | The CA bundle of the OIDC file is accepted: certificates only, at most 8 and 65536 bytes. |
| `trust.decision_engine_ca_accepted_by_grammar` | `ok` | `trust` | `observed` | The bundle given for the decision engine is accepted by the bundle grammar. |
| `trust.gateway_ca_accepted_by_grammar` | `ok` | `trust` | `observed` | The bundle given for the clients of an HTTPS gateway is accepted by the bundle grammar. |
| `trust.ca_bundle_invalid` | `error` | `trust` | `observed` | A bundle is refused by the bundle grammar. Its content is not printed. |
| `trust.ca_incompatible_with_transport` | `error` | `trust` | `observed` | A CA bundle requires the `https_only` transport. |
| `trust.ca_unavailable_or_not_private` | `error` | `trust` | `observed` | A bundle does not exist at its path, or is refused by the file rule of its consumer: each case has its own message. It was not read. |
| `trust.ca_reference_is_a_credential_reference` | `error` | `trust` | `observed` | A CA reference points at a reserved file. It was not opened. |
| `trust.decision_transport_invalid` | `error` | `trust` | `observed` | The given decision-engine transport is refused by the product validator: the form of the URL, plain HTTP outside a loopback address or without the local permission, or that permission together with a CA bundle, whatever the URL. |
| `trust.certificate_validity_period_not_available` | `not_available` | `trust` | `observed` | Validity dates are not evaluated. |

#### Engine

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `engine.program_not_inspected` | `not_checked` | `engine` | `observed` | No engine was named. |
| `engine.state_not_checked` | `not_checked` | `engine` | `observed` | The engine is never executed: its state is not checked. |
| `engine.program_approved` | `ok` | `engine` | `observed` | The program passes the approval rule of the product. It was hashed, not executed. |
| `engine.program_absent_from_fixed_entries` | `error` | `engine` | `observed` | No program of this kind exists at the fixed system locations. `PATH` is never searched. |
| `engine.program_refused` | `error` | `engine` | `observed` | The program is refused by the approval rule, with a closed reason. The message and the action name the cause that the product gave: the table under the reasons lists each of them. |
| `engine.program_is_a_credential_reference` | `error` | `engine` | `observed` | The given engine program is a reserved file. It was not opened, approved or hashed. |

#### Data

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `data.offline_inspection_not_available` | `not_available` | `data` | `observed` | This diagnostic never opens SQLite connections to stores; schema, tenant binding, audit format and integrity are not examined here. |
| `data.signed_checkpoint_not_checked` | `not_checked` | `data` | `observed` | Signed tenant checkpoints and independent rollback continuity require the separate explicit checkpoint verifier. |
| `data.location_not_persisted` | `not_checked` | `data` | `observed` | The store path of a historical directory is a gateway option. |
| `data.store_absent` | `warning` | `data` | `observed` | No store exists at the recorded path, whose name the gateway accepts. The gateway creates an empty one when it starts. |
| `data.authentication_audit_absent` | `warning` | `data` | `observed` | No authentication audit exists at the path recorded by an OIDC file given alone. The gateway refuses to start, unless a new audit is explicitly initialised. |
| `data.authentication_audit_missing_from_prepared_installation` | `error` | `data` | `observed` | The authentication audit that `setup prepare` created for the inspected installation is absent. The documented start, without `--initialize-oidc-audit`, refuses. |
| `data.store_posture_refused` | `error` | `data` | `observed` | A store is refused by the private-entry rule, its recorded path contains a parent traversal, or its file name is one the gateway refuses: not ASCII, or a reserved SQLite name. |
| `data.parent_directory_not_private` | `error` | `data` | `observed` | The directory that holds a store is refused by the private-directory rule: its owner, mode 0700, the directories above it, a link in its path or its file system. The message does not tell these causes apart. |
| `data.orphan_sidecars` | `error` | `data` | `observed` | Sidecar files exist without their store. |
| `data.sidecars_present` | `info` | `data` | `observed` | Sidecar files exist beside a store: it is in use, or was not closed cleanly. |

#### Output

| Code | Status | Axis | Basis | Meaning |
|---|---|---|---|---|
| `output.value_masked` | `warning` | `output` | `observed` | A value looks like a credential to the output filter and is shown as a placeholder. |
| `output.detail_omitted` | `info` | `output` | `observed` | The report exceeded its size bound and its detail lists were emptied. Totals stay exact. |
