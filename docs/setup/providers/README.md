# Codex, Claude Code and decision engines

[Français](README.fr.md) · [Setup index](../README.md) · [Local installation](../local/README.md)

Configure three independent choices: your ai-guard identity, the coding client and its subscription/API authentication, and the decision engine used by the control plane. Signing into an IdP does not sign into Codex or Claude Code. A Jev key authorizes a control-plane decision service; it never belongs in a worker credential file.

## Prepare clients and a small first task

Start the [local stack](../local/README.md) and keep its shell variables, engine and gateway URL. Choose a project without private credentials. Select a model available to your account and set a short task/time budget before starting: an API request can be billed by its provider, while a subscription follows that provider's eligibility and usage rules.

Build the worker with exact client versions. The following versions are explicit reference pins, not instructions to select the latest release:

```sh
"$AIGUARD_BIN" build --runtime docker --source . --codex-version 0.165.0 --claude-code-version 2.1.140
```

Use `--runtime podman` for Podman. Apply the explicit rootful exception from the local guide only if chosen for this installation. No range, wildcard or implicit latest version belongs in an approved image. Building the client does not log it in. To use a different supported version, rebuild deliberately and retain its resolved version/image identity with the session configuration.

Managed selection constructs the provider invocation; you do not supply its argv. `--client-profile` selects a provider profile, while `--profile` selects the ai-guard decision policy. Do not combine managed options with manual argv, a non-generic `--provider` or `--secret-env-file`.

For either Codex authentication mode, choose a model available to the selected account. This shell value is passed through `--model`; ai-guard does not read it as an environment setting:

```sh
AIGUARD_CODEX_MODEL="replace-with-your-approved-model-id"
```

## API credentials

Create a new private credential input outside the project. The parent below is created by `init`; create the file without overwriting anything:

```sh
(
  umask 077
  set -C
  cat .env.exemple.providers > "$AIGUARD_CONFIG/secrets/providers.env"
)
```

Open it in a private editor and fill only the keys you intend to use. Keep the regular file `0600` and its parent `0700`. The format is literal `NAME=value`: no `export`, shell interpolation or surrounding shell quotes. Do not source it. The example contains blank `OPENAI_API_KEY` and `ANTHROPIC_API_KEY`; a blank unselected entry does not supply a credential. Never place a key in command arguments, a prompt, a screenshot or Git.

Create profile references and inspect the configured choices:

```sh
"$AIGUARD_BIN" providers init --config-dir "$AIGUARD_CONFIG" --env-file "$AIGUARD_CONFIG/secrets/providers.env"
"$AIGUARD_BIN" providers status --config-dir "$AIGUARD_CONFIG" --json
```

`providers init` creates `providers.json` with `codex-api` and `claude-api`; it preserves an existing configuration and does not read the credential contents. `status` inspects configuration, not account validity or worker capabilities. Those prerequisites must be checked before the actual task; a service health check does not authenticate a provider.

Start the API task with that explicit model:

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network bridge --client codex --auth-mode api --client-profile codex-api --model "$AIGUARD_CODEX_MODEL" --timeout 120 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
```

For Claude Code/API, select the exact requested model rather than an alias:

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network bridge --client claude-code --auth-mode api --client-profile claude-api --model claude-haiku-5-5 --timeout 120 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
```

Replace the runtime for Podman and add a deliberate rootful exception when needed. `observe + bridge` is an explicit choice for these credential-bearing sessions. Bridge permits outbound traffic; it is not a destination allowlist. Treat data and the selected credential as accessible to the provider client, code inside its worker and the engine authority. Output masking does not prevent their access.

## Subscription credentials

Obtain subscription credentials voluntarily through each provider's official flow, using a dedicated context outside the project. Do not copy a whole host home directory, search OAuth caches or extract the OS keychain. Keep API keys out of subscription inputs so that the selected authentication mode stays unambiguous.

For Codex, prepare a new private `CODEX_HOME` and request file credential storage. The official device flow is subject to your account/workspace settings:

```sh
mkdir -m 700 "$AIGUARD_CONFIG/secrets/codex-subscription" &&
  CODEX_HOME="$AIGUARD_CONFIG/secrets/codex-subscription" codex -c 'cli_auth_credentials_store="file"' login --device-auth
```

Use a compatible host Codex CLI solely for this deliberate login. Preserve an existing directory instead of overwriting it; choose another new private directory when necessary. Keep the resulting `auth.json` private (`0600`) and treat it as a password. The profile must reference a ChatGPT subscription credential, without a non-null API key. See [Codex authentication](https://learn.chatgpt.com/docs/auth?surface=cli).

For Claude Code, use its official subscription-token flow in a private, unrecorded terminal:

```sh
claude setup-token
```

Put the returned value in a new private file `<config-dir>/secrets/claude-subscription.env`, as the sole entry `CLAUDE_CODE_OAUTH_TOKEN=value`. Create/edit it privately with mode `0600`; do not put the value into a shell command or capture the token screen. Check subscription eligibility and expiration with the provider. This token is for the supported Claude Code authentication flow; it does not grant arbitrary account features. See [Claude Code authentication](https://code.claude.com/docs/en/authentication).

Create a **new** private `<config-dir>/providers-subscriptions.json`, leaving existing API profiles untouched. The [provided JSON example](../../../config/providers-subscriptions.example.json) contains the non-secret reference below; adapt the Codex path if you chose another directory:

```json
{
  "version": 1,
  "auto_priority": ["codex-subscription", "claude-subscription"],
  "profiles": [
    {
      "id": "codex-subscription",
      "revision": 1,
      "client": "codex",
      "auth_mode": "subscription",
      "credential": {
        "kind": "codex_auth_file",
        "path": "secrets/codex-subscription/auth.json"
      }
    },
    {
      "id": "claude-subscription",
      "revision": 1,
      "client": "claude_code",
      "auth_mode": "subscription",
      "credential": {
        "kind": "claude_oauth_env_file",
        "path": "secrets/claude-subscription.env"
      }
    }
  ]
}
```

Keep the configuration `0600` as well. Relative references resolve against its parent, not the project or working directory. Use normal file paths without links or traversal. The canonical Claude source name is `claude_oauth_env_file`; the previous spelling `claude_o_auth_env_file` remains accepted, and loading a configuration does not rewrite its file. Older binaries may reject files written with the canonical name. Inspect this selected file explicitly, then start the desired subscription session:

```sh
"$AIGUARD_BIN" providers status --config-dir "$AIGUARD_CONFIG" --providers-file "$AIGUARD_CONFIG/providers-subscriptions.json" --json
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network bridge --providers-file "$AIGUARD_CONFIG/providers-subscriptions.json" --client codex --auth-mode subscription --client-profile codex-subscription --model "$AIGUARD_CODEX_MODEL" --timeout 120 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network bridge --providers-file "$AIGUARD_CONFIG/providers-subscriptions.json" --client claude-code --auth-mode subscription --client-profile claude-subscription --model claude-haiku-5-5 --timeout 120 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
```

These are separate tasks; run only the one you intend to perform. Use `--interactive` instead of a batch-only workflow when you need an authorized real terminal, retaining explicit client/mode/profile choices. `--interactive` cannot be combined with `--capture-output`. Terminal continuity and session recovery do not imply that a consumed start can be attached again.

## Explicit selection and Auto

An explicit client, mode and profile take precedence. The public choice, model, image and configuration provenance must be visible before authorization. `--model` overrides a profile model; it must be available through the selected account and mode. A subscription request never silently becomes a billed API request.

`--client auto --auth-mode auto` considers only profiles in the selected configuration. Configured subscriptions are considered before API profiles, then a compatible `default_profile` and explicit `auto_priority` resolve the choice. An unresolved ambiguity is refused. To make both modes available, create a deliberately reviewed configuration containing their profiles, unique IDs and explicit priority. An ambient API key or host CLI login does not add a profile.

Credential references are private control data. Only the selected credential is prepared for the worker. A changed source file does not rewrite an already authorized pending request: obtain a new decision for new credentials or changed execution settings. Preserve pending plans and bindings for reconciliation instead of editing them to force a restart.

## Choose the decision engine

| Engine | Intended use and boundaries |
|---|---|
| Deterministic Rust | Decisions without a model or external model credential. Rules have incomplete semantic coverage; unknown intent or transitive effects must stay explicit. |
| Real Jev | Authenticated TypeSafe decision service with model provenance. Its credential remains in the control plane. |
| Local Jev-like model | Containerized small model with pinned weights/runtime, bounded validated output and explicit uncertainty/timeouts. It is distinct from real Jev and cannot override a mandatory refusal. |

Select and record the engine independently from Codex/Claude and ai-guard SSO. Do not change engines silently after a refusal, outage or timeout. All engines retain central authorization, tenant isolation, mandatory audit and runtime confinement. See [local AI](../local-ai/README.md) for model preparation and resource limits, and [enterprise](../enterprise/README.md) for internal inference and offline deployment.

For real Jev, create a new private raw-key file `<config-dir>/secrets/jev`, mode `0600`. Enter **only the raw key** in an editor, without `JEV_API_KEY=`. Do not place it in `providers.env`. Refresh the control projection:

```sh
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
```

In your private Compose env file, add the non-secret settings `JEV_URL=https://api.typesafe.ai/v1/systemone` and `JEV_MODEL=jev-latest`. Restart with the explicit override, retaining the same stack identity:

```sh
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml -f deploy/docker/compose.jev.example.yaml up -d --force-recreate
```

With Podman, replace `docker compose` and the base file with `podman compose` and `deploy/podman/compose.yaml`; the Jev override path remains the same. The override mounts the Jev projection only into the gateway and adds **unrestricted outbound connectivity to that service**. Review that network decision separately from worker networking. CLI `up` does not load this override automatically. Keep it on every subsequent stack start that requires Jev.

The adapter uses the authenticated [System One API](https://docs.typesafe.ai/api/system-one) and a [Choice question](https://docs.typesafe.ai/primitives/choice) for `allow`, `deny` or `review`. `jev-latest` is an evolving alias: retain the actually returned model with the result. A decision score is not a calibrated probability of safety. Audit or decision-service failure blocks the protected action; removing a key is not a safe automatic fallback to another engine.

## Diagnose provider problems

| Result | Next action |
|---|---|
| Client missing or incompatible | Rebuild the approved worker with an exact compatible version; check the resolved image. A host installation does not install the worker client. |
| Credential missing, unsafe or login required | Check the explicitly referenced file, owner/private modes and chosen authentication mode. Complete the provider's official login or rotate the selected API key. Never substitute another mode silently. |
| Model unavailable or account refused | Verify that exact model and account entitlement with the provider; make a new explicit choice and request authorization again. |
| Auto ambiguous | Select a profile explicitly or set a compatible default/priority in a new reviewed configuration. |
| Network or TLS error | Check the approved destination and trusted public CA chain. Do not disable TLS verification or open all worker destinations to repair a certificate error. |
| Timeout or partial output | Inspect session/runtime status before retrying. Preserve the original attempt and its uncertainty; an API retry may incur another charge. |
| Jev unavailable | Inspect the gateway decision-service configuration and control egress. Preserve the refusal; do not launch a worker on the host or switch engines implicitly. |

After a task, use `worker-status`, audit inspection and the [fresh-destination export procedure](../local/README.md#inspect-and-export-changes). A client exit code or generated text does not prove all requested changes. Provider events describe what the provider reports; runtime observations and independent audit carry their own provenance. Retain omission/truncation indicators rather than treating a bounded capture as a complete transcript.

Stop active sessions with `worker-stop` and confirm observed terminal state before shutting down control services. Revoke or rotate credentials through their provider when required. Preserve retained session bindings with the same confidentiality as credentials; do not erase them to manufacture a clean recovery.
