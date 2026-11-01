# A containerized judge with verified TLS

[Français](TLS.fr.md) · [Local AI](README.md) · [Test certificates](../../demo-pki.md)

Choose this profile **instead of** the HTTP model project. Gateway → Rust relay uses verified HTTPS; relay → `http://model:8080` uses a separate internal network and **is not encrypted**. No model port is published. This is a trusted local installation, with advisory model decisions, not an authenticated enterprise inference service. Control ingress/dashboard transport stays unchanged.

Choose explicitly between fresh local POSIX volumes (below) and [host binds](#alternative-host-binds-on-supported-filesystems). Docker Desktop host projections can be rejected by the strict filesystem reader; do not relax its checks or silently switch profiles. The volumes route imports snapshots without mounting host TLS sources into consumers. Qualification and remaining gaps are recorded in [engineering](../../engineering/CURRENT_STATE.md); these instructions alone establish no runtime, Podman, cloud, SSO or model-quality qualification.

## Prepare images and private sources

Follow the [workstation guide](../local/README.md) first: initialize the private configuration outside the checkout and set `AIGUARD_CONFIG`. Keep the same engine account, context and endpoint throughout. From the repository root, build or import trusted images before activation:

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
"$AIGUARD_BIN" build --runtime docker --source .
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml build model
docker image inspect --format '{{.Id}}' agentsandbox-control:local
docker image inspect --format '{{.Id}}' ai-guard-local-ai:qwen2.5-0.5b-q4km
```

Record both exact `sha256:` image IDs. For a deliberately non-rootless Docker Desktop daemon, add `--allow-rootful-runtime` to the Rust `build` and `model-tls provision` commands; this remains an explicit security downgrade. With an approved custom engine program/Unix endpoint, pass `--runtime-program /absolute/path/to/runtime` and `--runtime-endpoint unix:///absolute/path/to/socket` consistently and configure Compose to use that same authority. No engine socket is mounted into an application.

Create **new** paths; the subshell stops on error and refuses overwrites:

```sh
(
  set -eu
  umask 077
  mkdir -m 700 "$AIGUARD_CONFIG/local-judge-tls-volumes"
  ./target/release/ai-guard-demo-pki --out "$AIGUARD_CONFIG/local-judge-tls-volumes/issued" --dns llm.ai-guard.test
  set -C
  cat .env.exemple.local-qwen-tls-volumes > "$AIGUARD_CONFIG/local-qwen-tls-volumes.env"
)
```

Inspect public issuer, SAN, validity and root fingerprint using the [test-certificate guide](../../demo-pki.md). The local test tool defaults to seven-day leaf validity and does not install system trust or provide automatic renewal/revocation. Use an approved server identity for a different PKI; never import a CA private key. The three input files must be private0600 with private0700 parents, owned by the control account on a supported filesystem, without links or unsupported ACLs. Do not loosen these originals to make a container read them.

The server name is a lowercase ASCII DNS alias, with labels of1–63 characters and at most253 characters overall. IP addresses, empty labels, leading/trailing hyphens, `localhost` and its subdomains are refused. The supplied topology also reserves `model`, `gateway`, `dashboard`, `ingress` and `local-ai-ingress`; reusing these service names would make routing ambiguous. Use the same accepted name in the certificate and selectors.

Edit the new `local-qwen-tls-volumes.env`: match `AGENTSANDBOX_CONFIG_DIR` to `init`; replace `AI_GUARD_LOCAL_AI_IMAGE` with the inspected model `sha256:` ID. Keep context512, one inference slot, CPU2 and model memory/memory+swap1,000,000,000bytes unless deliberately reconfigured and measured. The relay has its own192MiB budget. A weight file size or a successful health check does not establish peak RAM or judgment quality. Never build the default Dockerfile over a custom model tag.

## Provision fresh volumes explicitly

Replace the public placeholder below with the inspected control ID. The output directory must not already exist; its parent is private. This command prepares materials and selectors, without starting the model, gateway, worker, network or a decision request:

```sh
AIGUARD_CONTROL_IMAGE_ID="sha256:replace-with-inspected-control-image-id"
"$AIGUARD_BIN" model-tls provision \
  --ca-file "$AIGUARD_CONFIG/local-judge-tls-volumes/issued/root-ca.pem" \
  --cert-file "$AIGUARD_CONFIG/local-judge-tls-volumes/issued/server-chain.pem" \
  --key-file "$AIGUARD_CONFIG/local-judge-tls-volumes/issued/server.key" \
  --server-name llm.ai-guard.test \
  --image "$AIGUARD_CONTROL_IMAGE_ID" \
  --output-dir "$AIGUARD_CONFIG/local-judge-tls-volumes/selection" \
  --runtime docker
```

Success publishes `manifest.json` and `volumes.env` together in the new `selection` directory. Keep the private manifest with the configuration. The selector file contains exactly:

| Variable | Consumer |
|---|---|
| `AI_GUARD_LOCAL_AI_TLS_CA_VOLUME` | External CA volume: gateway and relay, read-only at `/run/ai-guard-trust/root-ca.pem`. |
| `AI_GUARD_LOCAL_AI_TLS_IDENTITY_VOLUME` | External identity volume: relay only, read-only at `/run/ai-guard-tls/server-chain.pem` and `server.key`. |
| `AI_GUARD_LOCAL_AI_TLS_SERVER_NAME` | Relay network alias, gateway HTTPS URL and relay health URL; validated against the certificate. |
| `AI_GUARD_LOCAL_AI_TLS_NETWORK` | New `ai-guard-judge-UUID` internal network, created later by the model Compose project. |
| `AI_GUARD_CONTROL_IMAGE` | Inspected immutable control image ID used for provisioner, relay and control stack. |

No key, credential, TLS source path or private-key digest belongs in these selectors. The model and workers receive neither volume. Gateway never receives the server identity. CA private keys stay with the operator and are never imported. Audit, runner and viewer remain separate from model TLS materials.

A failure or timeout preserves new volumes and diagnostic identifiers. An unconfirmed stop or publication remains **unknown**, without activation. Preserve all retained resources and investigate the exact recorded runtime; do not manufacture a selector, adopt an existing volume, repair permissions, retry into the same output path or delete evidence to obtain success. A later attempt uses new sources/output and new volumes after reconciliation. Existing installations and older volumes are preserved.

## Start and diagnose

Stop only an already active HTTP or bind model project first, using its original configuration; do not run a second model. Reconcile active operations before changing the control stack. Keep the **original control project name** and audit volume: the commands use `ai-guard-local`. Pass the base env **first**, then the published `volumes.env`; the latter supplies all five selectors. Do not source either file. Remove conflicting inherited `AI_GUARD_*`/`JEV_*` settings, because shell variables can override Compose interpolation. Select exactly one judge override.

```sh
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml config --quiet
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml up --no-build --pull never -d
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml ps
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml config --quiet
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml up --no-build --pull never -d
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml exec -T gateway agentsandbox healthcheck --ca-file /run/ai-guard-trust/root-ca.pem --url https://llm.ai-guard.test:8443/health
```

The sample DNS is `llm.ai-guard.test`; when provisioning another name, use that same name in the manual health URL. The Compose alias, gateway URL and automatic relay healthcheck interpolate the selector. `up --no-build --pull never` requires the exact prepared local images; a missing image is a refusal, without downloading or rebuilding during activation.

The model project creates `judge_link` with `internal: true`; the control override joins this existing network. Its `external: true` refers to ownership by another Compose project, not Internet access. Verify the actual network Internal flag and participants: gateway=control+judge_link, relay=judge_link+inference, model=inference only; workers join neither. Both TLS volumes must be the manifest's local volumes, mounted read-only by the intended consumers, never binds. Check service `ps` and, if necessary, bounded engine inspect output without disclosing private data. Never publish raw keys, source files or arbitrary diagnostics.

The in-gateway health command verifies real HTTPS and the upstream model health. Gateway `/healthz` alone does not exercise this route. The relay accepts only `GET /health` and JSON non-streaming `POST /v1/chat/completions` without query strings:64KiB request/response, one inference, immediate429 when busy, body read2s/upstream5s and no retry. Missing CA, wrong SAN, expiration, OOM, unhealthy model, rejected filesystem or missing network stays an explicit failure; repair the cause, never disable TLS or widen networks. A slow/malformed/unavailable model closes the decision. Local Allow becomes effective Review; Deny remains a refusal. No local advice can lift a mandatory Rust/audit/runtime refusal.

For Podman, replace `--runtime docker` with `--runtime podman`, `docker` with `podman`, and only the control base with `deploy/podman/compose.yaml`. Retain env order, image IDs, endpoints and project names; require a Compose provider supporting these options. Docker and Podman require separate real qualification. Docker Desktop observations do not qualify Podman, cloud, SSO, integrated inference or the model's semantics.

## Stop, resume, renew and retire

Stop new admissions and reconcile active operations before stopping control. Use the same two env files, project names and files for every lifecycle command, including the gateway override. For a planned stop and restart:

```sh
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml stop
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml stop
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml start
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml start
```

Re-run both `ps` and the in-gateway HTTPS diagnostic after restart. `stop` preserves containers, volumes and networks. An outage closes affected decisions; do not silently switch engines. If containers need recreation, use the same `up --no-build --pull never -d` commands. Do not remove the judge network while a gateway still uses it.

For renewal, prepare a new private source generation and provision into a different new output directory. Keep previous manifests, images, audit and sources. Stop/reconcile affected activity; select the new `volumes.env` and recreate relay and gateway with the same project names, then verify HTTPS. Files are loaded once; no hot reload, automatic rotation or revocation is promised. A deliberate rollback selects the prior compatible manifest/images and recreates both consumers; verify validity and health before resuming. An expired identity is not a usable rollback.

Retirement is an operator action after retention/backup review: identify this generation's exact volumes from its manifest, verify no container or other installation uses them, remove only its stopped containers and unused network with the matching Compose `down` **without `-v`**, then remove only those two approved TLS volume names with the engine's targeted `volume rm`. Never run `prune`, `down -v`, delete an audit/workspace volume, or automatically remove sources/manifests. If ownership, stop, publication or use is unknown, retain the resource and keep cleanup open.

## Alternative: host binds on supported filesystems

The existing [bind model profile](../../../deploy/local-ai/compose.tls.yaml), [gateway override](../../../deploy/docker/compose.jev-local-tls.example.yaml) and [env example](../../../.env.exemple.local-qwen-tls) remain an explicit alternative **only where the real container filesystem is accepted by the strict TLS reader**. Host POSIX modes alone do not qualify a Docker Desktop projection. Never retry a rejected bind by weakening checks.

Prepare a distinct new directory using the same test PKI procedure:

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
  set -C
  cat .env.exemple.local-qwen-tls > "$AIGUARD_CONFIG/local-qwen-tls.env"
)
```

Only the three projections are0444 under their private0700 parent; originals stay0600. Never mount the issuance directory. Edit `AI_GUARD_LOCAL_AI_TLS_MATERIAL_DIR` to the new `mounted-tls` directory and select the inspected control/model image IDs in this env. The bind profile uses the fixed SAN `llm.ai-guard.test`. Use one env file (`local-qwen-tls.env`), model project `ai-guard-local-ai-tls`, model file `deploy/local-ai/compose.tls.yaml`, and control override `deploy/docker/compose.jev-local-tls.example.yaml`. Apply the same config/start/stop/restart procedure above with these explicit substitutions; do not pass `volumes.env`. Its in-gateway health command uses `--ca-file /run/ai-guard-trust/local-judge-ca.pem`. Verify the same HTTPS URL and network boundaries before admissions. Renewal prepares new projections and recreates relay/gateway; preserve old sources, images and audit. This alternative never imports into or cleans the volume profile.
