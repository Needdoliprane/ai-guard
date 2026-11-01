# A containerized judge with verified TLS

[Français](TLS.fr.md) · [Local AI](README.md) · [Test certificates](../../demo-pki.md)

Choose this profile **instead of** the HTTP model project. It reuses the pinned image and resource limits, without a second model. Gateway → Rust relay uses verified HTTPS; relay → `http://model:8080` stays on a separate internal network and **is not encrypted**. No model port is published on the host. Control ingress/dashboard stays unchanged. This profile is for one trusted installation, not an authenticated multi-user enterprise inference service.

## Prepare private material

Prepare release binaries, control image and pinned model image using the local guides. Set `AIGUARD_CONFIG` as in the workstation guide. From the repository root, create a **new** directory; this subshell stops on error and refuses an existing directory:

```sh
(
  set -eu
  umask 077
  mkdir -m 700 "$AIGUARD_CONFIG/local-judge-tls"
  ./target/release/ai-guard-demo-pki --out "$AIGUARD_CONFIG/local-judge-tls/issued" --dns llm.ai-guard.test
  mkdir -m 700 "$AIGUARD_CONFIG/local-judge-tls/mounted-tls"
  install -m 444 "$AIGUARD_CONFIG/local-judge-tls/issued/root-ca.pem" "$AIGUARD_CONFIG/local-judge-tls/mounted-tls/root-ca.pem"
  install -m 444 "$AIGUARD_CONFIG/local-judge-tls/issued/server-chain.pem" "$AIGUARD_CONFIG/local-judge-tls/mounted-tls/server-chain.pem"
  install -m 444 "$AIGUARD_CONFIG/local-judge-tls/issued/server.key" "$AIGUARD_CONFIG/local-judge-tls/mounted-tls/server.key"
)
(
  umask 077
  set -C
  cat .env.exemple.local-qwen-tls > "$AIGUARD_CONFIG/local-qwen-tls.env"
)
```

Inspect public issuer, SAN, validity and root fingerprint before use. The test tool defaults to seven-day server validity; no automatic renewal or revocation service. Originals remain0600, host parents0700. Only these three projections are0444 under their private0700 parent:

| Material | Consumer |
|---|---|
| Public root | Gateway trust; relay's HTTPS diagnostic. |
| Server chain and leaf key | Model relay only. |
| Private root/intermediate CA keys | Operator only, never projected/mounted. |

Model and workers receive none. Never mount the issuance directory. No system trust or DNS is changed; Compose supplies `llm.ai-guard.test`.

Edit the new env file: `AGENTSANDBOX_CONFIG_DIR` matches `init`; `AI_GUARD_LOCAL_AI_TLS_MATERIAL_DIR` names `mounted-tls`, not `issued`. `AI_GUARD_LOCAL_AI_TLS_NETWORK` must match between both projects and be distinct for another installation. Other model settings keep their HTTP meanings. The CLI does not source dotenv. Use exactly one judge override and remove conflicting inherited `JEV_*` deployment settings.

## Start and diagnose

If HTTP inference is active, stop only that model project first; preserve images and control data. Keep the **original control project name** to reuse its intended audit volume. The example uses the documented default `ai-guard-local`:

```sh
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local-ai-tls -f deploy/local-ai/compose.tls.yaml config --quiet
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local-ai-tls -f deploy/local-ai/compose.tls.yaml up --no-build -d
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local-ai-tls -f deploy/local-ai/compose.tls.yaml ps
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls.example.yaml config --quiet
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls.example.yaml up --no-build -d
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls.example.yaml exec -T gateway agentsandbox healthcheck --ca-file /run/ai-guard-trust/local-judge-ca.pem --url https://llm.ai-guard.test:8443/health
```

If the default model image is missing, explicitly build `model` before `up` or import your offline image. Never build the default Dockerfile over a custom model tag. For Podman use `podman compose` and replace only the control base with `deploy/podman/compose.yaml`, keeping env/project names. Actual support and effects require separate Docker/Podman qualification.

The model project creates judge_link **internal**; external in the gateway override means another Compose project owns it, not Internet access. Verify actual Internal property and participants: gateway=control+judge_link, relay=judge_link+inference, model=inference only; workers join neither. The control ingress has no TLS file. Missing network or wrong CA/SAN/validity stays an explicit failure; never add egress or bypass TLS.

The diagnostic runs inside the gateway, verifies HTTPS and reaches real model health. Gateway `/healthz` alone does not probe that route. Only `GET /health` and JSON, non-streaming `POST /v1/chat/completions`, without query strings, are relayed. Limits:64KiB request/response, one inference and immediate429 when busy, body read2s/upstream5s, no retry. Slow, malformed or unavailable advice fails closed; local Allow remains effective Review and cannot authorize execution. Transport availability does not qualify model judgment.

## Stop, renew and recover

Use the same model env/project/file with `stop` or `start`. Retain the gateway override on **every** control lifecycle command. Outage closes affected decisions. Do not remove a shared network still used by the gateway or delete audit volumes.

Before changing certificates/policy, stop new admissions and reconcile active operations. Prepare a new private material directory and env reference, then recreate relay and gateway; files are read once, without hot reload. Re-run the in-gateway HTTPS diagnostic. Keep previous material/images/audit for deliberate rollback. Do not disable TLS, widen networks or silently switch judges. Executed checks and remaining limits are in the [engineering ledger](../../engineering/CURRENT_STATE.md).
