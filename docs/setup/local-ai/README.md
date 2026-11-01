# Local AI: a small shared inference backend

[English](README.md) · [Français](README.fr.md) · [Configuration index](../README.md)

Use a local model for private/offline inference and advisory analysis. The default is **Qwen2.5-0.5B-Instruct Q4_K_M**, served by llama.cpp. It is a text-generation model; an embedding-only model cannot replace a chat or decision-generation endpoint.

The backend can serve enterprise-inference and local-judge profiles, with separate callers, authorization, context and quotas. It does not become an authority to allow actions. Mandatory Rust policy, audit and runtime restrictions remain independent. Model advice may be incomplete or wrong.

## Prerequisites and packaged files

Prepare the [local ai-guard stack](../local/README.md), including `agentsandbox-control:local`. The model project uses the same Rust ingress binary and its own isolated inference network. No engine socket, original project or control credential is mounted into the model.

| File | Purpose |
|---|---|
| [`.env.exemple.local-qwen`](../../../.env.exemple.local-qwen) | Public image, ports and CPU/RAM/context defaults. |
| [`deploy/local-ai/compose.yaml`](../../../deploy/local-ai/compose.yaml) | Model and fixed-destination Rust ingress. |
| [`deploy/local-ai/Dockerfile`](../../../deploy/local-ai/Dockerfile) | Pinned llama.cpp runtime and checksum-verified GGUF weight. |
| [Combined example](../../../.env.exemple.local-keycloak-qwen) | Same model settings plus the local Keycloak settings. |
| [TLS example](../../../.env.exemple.local-qwen-tls) | Internal HTTPS route for the containerized judge; follow the [TLS guide](TLS.md). |

The first image build downloads the public runtime and the weight of 491,400,032 bytes. The weight revision and SHA256 are fixed in the Dockerfile. Building requires support for `ADD --checksum` (Dockerfile frontend 1.6+); see [Docker's reference](https://docs.docker.com/reference/dockerfile/#add---checksum). Prepare the resulting image on a connected build machine, then transfer it with your normal image distribution process for offline use. Runtime startup does not download a model.

## Configure

| Setting | Default | Meaning |
|---|---|---|
| `AI_GUARD_LOCAL_AI_IMAGE` | `ai-guard-local-ai:qwen2.5-0.5b-q4km` | Locally built image containing the model. |
| `AI_GUARD_LOCAL_AI_MODEL` | `qwen2.5-0.5b-q4km` | Model alias returned by the API. |
| `AI_GUARD_LOCAL_AI_PORT` | `18090` | Host port, bound to 127.0.0.1. |
| `AI_GUARD_LOCAL_AI_MEMORY_BYTES` | `1000000000` | Model-container memory and memory+swap request. |
| `AI_GUARD_LOCAL_AI_CPUS` | `2.0` | Allocated CPU limit, not measured CPU consumption. |
| `AI_GUARD_LOCAL_AI_CONTEXT` | `512` | Context size; increasing it raises memory requirements. |
| `AI_GUARD_LOCAL_AI_DEFAULT_TOKENS` | `64` | Default prediction length; a request may override it. It is not a mandatory quota. |
| `AI_GUARD_CONTROL_IMAGE` | `agentsandbox-control:local` | Trusted Rust ingress image. |

The model runs one inference slot, batch/ubatch 32, no GPU layers, UID 65532, read-only rootfs, dropped capabilities, no-new-privileges and PID limit 64. The ingress has a separate 192 MiB limit; the 1 GB model budget is not the memory of the whole installation. Both services disable engine logging. Resource configuration is not a guarantee that an arbitrary model will fit. The [llama.cpp request setting](https://github.com/ggml-org/llama.cpp/blob/b11718/tools/server/server-context.cpp#L1801-L1802) takes precedence over the prediction default; mandatory output quotas belong in the authorized Rust adapter, not this default.

## Build and start

From the repository root:

```sh
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml config --quiet
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml build model
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml up --no-build -d
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml ps
```

Wait for the ingress health check, which queries the actual model health endpoint. Diagnose the service state before making an inference; `running` alone does not establish readiness.

```sh
./target/release/agentsandbox healthcheck --url http://127.0.0.1:18090/health
```

For the combined context, replace only the env-file path with `.env.exemple.local-keycloak-qwen` and retain this model project name. The Keycloak and ai-guard projects keep their own names and networks.

## API and application profiles

The local endpoint is `http://127.0.0.1:18090/v1`, with model alias `qwen2.5-0.5b-q4km`. This profile exposes **Chat Completions**; “OpenAI-compatible” does not imply support for every OpenAI API, Codex Responses, tools, images or Claude Messages.

An optional bounded manual request, containing only a public synthetic prompt:

```sh
curl --fail --silent --show-error --max-time 30 --noproxy '*' \
  http://127.0.0.1:18090/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"qwen2.5-0.5b-q4km","messages":[{"role":"user","content":"Say hello briefly."}],"max_tokens":16,"temperature":0,"stream":false}'
```

For a managed inference profile, configure the exact protocol, URL, model, credential reference if required, public CA chain for HTTPS, allowed destinations and per-caller limits. Select the matching Rust adapter for the client protocol; do not redirect Codex or Claude by supplying an arbitrary base URL and assuming equivalence. Keep local-judge and enterprise callers separate even when they share weights. A provider subscription and ai-guard SSO remain unrelated to this backend.

The loopback endpoint has no model API credential in this local profile. Do not publish it on the LAN or reuse this transport as a multi-user enterprise endpoint. Enterprise routing adds the authentication, authorization and TLS settings described in the [enterprise guide](../enterprise/README.md).

### Connect a local judge

Select the gateway's `local-openai` backend explicitly; its URL includes `/v1/chat/completions`. It accepts one non-streamed assistant answer containing exactly `outcome` (`allow`, `deny`, `review` or `unknown`) and `reason`. Malformed, truncated and tool-call responses are refused. A local Allow is advisory and becomes effective Review, so it cannot authorize execution. The [decision-engine guide](../providers/README.md#choose-the-decision-engine) covers effective decisions and provenance.

For a gateway on the same host as the model's loopback publication, an isolated integration context uses its own private audit path and port. From the repository root, with the local guide's `AIGUARD_BIN` and `AIGUARD_CONFIG` variables:

```sh
"$AIGUARD_BIN" gateway \
  --listen 127.0.0.1:18787 \
  --data "$AIGUARD_CONFIG/local-judge-audit.sqlite" \
  --runner-token-file "$AIGUARD_CONFIG/secrets/runner" \
  --viewer-token-file "$AIGUARD_CONFIG/secrets/viewer" \
  --policy "$PWD/policies/default.json" \
  --jev-backend local-openai \
  --jev-local-url http://127.0.0.1:18090/v1/chat/completions \
  --jev-local-model qwen2.5-0.5b-q4km \
  --jev-allow-local-http
```

Use this context's explicit gateway URL for callers; it has a separate audit history. Keep the existing stack's database and services intact. Remove settings for other Jev backends from the process environment before starting it. This model endpoint has no key, so no local key-file option is needed. An authenticated HTTPS endpoint can use a separate private raw-key file through `--jev-local-key-file`.

Loopback is relative to the calling process. A gateway container cannot reach the host publication using its own `127.0.0.1`; adding the model Compose file does not wire the applications together. A container deployment needs an explicitly approved route and verified HTTPS trust. Do not open unrestricted HTTP or disable TLS validation to bridge that gap. Host commands, adapter protocol checks, model resource measurements and a complete container deployment have separate qualification evidence in the [engineering state](../../engineering/CURRENT_STATE.md).

## Replace the model or move offline

For a gateway container, the [local TLS profile](TLS.md) supplies a private-CA route, with startup, renewal and recovery instructions. It replaces the HTTP model project rather than starting a second model.

Select a text-generation/instruct model of at most 4 billion parameters and measure its actual memory with the chosen quantization/context. Build a separate image with pinned runtime, weight revision, checksum and applicable licenses. Keep the same `/app/llama-server` entrypoint and `/models/model.gguf` layout for this Compose profile, or provide a reviewed Compose definition for another runtime.

Set `AI_GUARD_LOCAL_AI_IMAGE` to that prepared image and `AI_GUARD_LOCAL_AI_MODEL` to its alias. Start with `up --no-build`; running the default build against a custom tag would replace its contents with the default Dockerfile output. Retain the previous image for a deliberate rollback. Transfer images through your controlled registry or image archive before going offline; changing an alias alone does not change the weight.

## Diagnose and operate

- **Missing image or weight/build failure:** prepare the exact image first. A checksum mismatch is a refusal, not permission to skip integrity checks.
- **OOM or unhealthy server:** inspect the service state and memory limits, lower context/output/concurrency, or choose a smaller model. Raising memory is an explicit operator choice.
- **Unsupported protocol:** choose a compatible adapter/model; do not silently call a cloud provider.
- **Timeout:** bound requests and generated tokens. The Rust ingress also bounds headers/body waits; arbitrary long generation is not promised by this local preset.
- **Wrong TLS name or CA:** correct the issuer/endpoint SAN and public chain; never disable verification.

Stop or restart only this project:

```sh
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml stop
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml start
```

There is no mutable model-data volume to delete. Preserve images, ai-guard audit and Keycloak data independently. An availability check is not a benchmark of model judgment; a small model must not authorize an action by itself.
