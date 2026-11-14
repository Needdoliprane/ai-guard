# Enterprise deployment

[Français](README.fr.md) · [Setup index](../README.md) · [Local installation](../local/README.md) · [Providers](../providers/README.md)

Deploy the same Rust control plane with your organization's identity, tenant policy, trusted network and AI backends. Keep one authorized dashboard per tenant across its workers and brokers. Shared control services are not tenant workers, and sharing an installation does not authorize communication between tenants.

## Decide the installation boundaries

Record the following before creating credentials or starting workloads. Values belong to your deployment; the repository does not supply enterprise hostnames, CA authority, NAS access or an identity administrator.

| Decision | Record |
|---|---|
| Topology | Control host/platform, Docker or Podman endpoint, broker ownership, persistent data locations and deployment mode. |
| Human identity | Local named accounts or OIDC issuer, public Web/CLI client IDs, exact redirect URIs, audiences and verified claim mapping. |
| Organizations and tenants | Membership authority, personal/shared/admin permissions, tenant database locations and resource/retention policy. |
| Coding clients | Codex/Claude Code, subscription/API/internal inference, explicit profiles, image and version pins. |
| Decision engine | Rust, real Jev or local Jev-like model, policy and outage behavior. |
| Transport | Public URLs, DNS/SAN names, TLS termination, public trust chains, service identity and allowed destinations. |
| Operations | Backup/restore owner, rotation, updates, incident handling, audit export and disk/resource alerts. |

Local topology, SSO and inference are independent axes. An internal model does not require an IdP, and an external IdP does not authorize external model traffic. Reject an incompatible combination rather than silently weakening TLS, isolation or audit.

Use the Docker/Podman single-host procedure below as the base. Kubernetes requires its own runtime adapter, workload identity, transport and recovery configuration; a metadata value such as `runtime=kubernetes` or `deployment_mode=cloud` does not create pods or infrastructure. Adding high availability or changing the per-tenant SQLite storage requires a documented migration, not a second process pointed at the same files.

## Prepare images and private configuration

Build the release CLI and images as in the [local guide](../local/README.md), including the exact coding-client versions needed by your organization. For offline operation, build or import every required image, model and dependency beforehand; keep an inventory of versions, digests and licenses. Use your own image registry or offline transfer process. No runtime should download a replacement or switch to a public cloud backend implicitly.

Provision a private configuration directory owned by the trusted control account. The parent must already be suitable for that account; do not solve ownership errors by running all control operations as root. From the repository root, initialize it and create a new env file:

```sh
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_CONFIG="/srv/ai-guard/config"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
(
  umask 077
  set -C
  cat .env.exemple.entreprise > "$AIGUARD_CONFIG/enterprise.env"
)
```

Edit this private file to match the actual path, image and host ports. The [enterprise example](../../../.env.exemple.entreprise) is **Compose transport configuration**, with optional non-secret Jev URL/model settings. It does not create users, membership, OIDC policy or internal-model routing. Those belong to separately validated identity, tenant and inference configuration. Do not add imagined environment variables and assume that the CLI consumes them.

Start the base services behind your chosen trusted ingress:

```sh
docker compose --env-file "$AIGUARD_CONFIG/enterprise.env" -f deploy/docker/compose.yaml up -d
docker compose --env-file "$AIGUARD_CONFIG/enterprise.env" -f deploy/docker/compose.yaml ps
"$AIGUARD_BIN" healthcheck --url http://127.0.0.1:8787/healthz --url http://127.0.0.1:4000/healthz
```

For Podman, use `podman compose` and `deploy/podman/compose.yaml`. Adjust health URLs if you changed published host ports. The base publishes only loopback HTTP; do not expose it directly by changing the bind address. Put the organization-facing HTTPS entry point in your explicitly configured transport architecture. Retain the same env file, project name, engine endpoint and overrides for every lifecycle command.

Read [versions, build identity and manual updates](../release/README.md#versions-and-updates) before replacing an installation. The common 0.0.1 product version and captured-source Build ID are descriptive; record actual image identities, preserve active session bindings and armed enforcement, and apply a reviewed change explicitly. No update controller or automatic rollout is provided.

## Configure tenants and permissions

Create distinct tenant identities and distinct runner/viewer credentials; keep a separate SQLite database for each tenant. The [public registry example](../../../config/tenants.example.json) contains tenants A/B, container paths and exact quotas. Generate their four distinct credentials and create a new private registry copy:

```sh
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG/tenant-a"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG/tenant-b"
(
  umask 077
  set -C
  cat config/tenants.example.json > "$AIGUARD_CONFIG/tenants.json"
)
```

A trusted operator registry is selected with gateway `--tenants-file`. Each tenant entry follows this version1 schema; the public file supplies both entries:

```json
{
  "version": 1,
  "tenants": [
    {
      "tenant_id": "tenant-a",
      "data": "/data/tenant-a.sqlite",
      "runner_token_file": "/run/secrets/tenant-a-runner",
      "viewer_token_file": "/run/secrets/tenant-a-viewer",
      "worker_quota": {
        "version": 1,
        "max_active_workers": 3,
        "max_cpu_millis": 6000,
        "max_memory_bytes": 3221225472,
        "max_timeout_seconds": 3600
      }
    }
  ]
}
```

These paths are **container paths** for a gateway container. In your reviewed deployment configuration, mount `$AIGUARD_CONFIG/tenants.json` read-only at `/run/config/tenants.json`, mount the four projections below as read-only secrets, and supply durable storage at the database paths:

| Private host projection | Gateway secret path |
|---|---|
| `<config-dir>/tenant-a/mounted-secrets/runner` | `/run/secrets/tenant-a-runner` |
| `<config-dir>/tenant-a/mounted-secrets/viewer` | `/run/secrets/tenant-a-viewer` |
| `<config-dir>/tenant-b/mounted-secrets/runner` | `/run/secrets/tenant-b-runner` |
| `<config-dir>/tenant-b/mounted-secrets/viewer` | `/run/secrets/tenant-b-viewer` |

Never copy the same secret to another identity. Replace the base gateway command with `gateway --listen 0.0.0.0:8787 --container-listen --tenants-file /run/config/tenants.json --policy /app/policies/default.json`, removing the conflicting single-tenant `--data`, `--runner-token-file`, `--viewer-token-file` and local quota options. Replace the base single-tenant secret mounts with the four tenant projections. The dashboard's `--shared-tenants` mode forwards each authorized viewer scope instead of using one fixed tenant viewer token; remove its fixed `--viewer-token-file` option and unneeded fixed secret mount when choosing that mode. Do not combine mutually exclusive configurations. Retain your reviewed deployment files for every start/stop; the unmodified base command above starts a single-tenant stack.

The tenant registry describes technical tenant credentials and quotas, not human membership. Human permissions are established separately from verified identity. A user may read their own sessions and explicitly shared resources; tenant-wide metadata, sensitive contents, exports, administration and execution are separate capabilities. A user's tenant comes from authenticated membership, never a free-form header, query, request body or worker label. Apply restrictions in Rust to lists, details, exports and live streams. Permission changes must remove newly forbidden data from views and caches.

Keep `runner` in trusted control; neither an IdP role nor a browser user receives it. Workers use only their own bounded execution/relay identity. Human, broker, worker and AI-provider identities are distinct. Record initiator, effective actor and causal relation without treating an agent action as proof of an explicit human instruction. Historical shared-key events retain their original provenance.

Set quotas centrally for the tenant, not once per broker. The example admits three workers; a useful local multi-tenant check is A1/A2 for tenant A and B1 for tenant B. The supported configuration ceiling of 100 workers per tenant is a sizing target, not a capacity guarantee for an arbitrary host. Worker count, CPU/RAM allocation and admission duration need distinct limits. Changing a quota does not by itself stop an existing worker. Allocation-time metering is not measured CPU consumption, a price or an invoice; preserve `billable=false` for that measurement.

## Connect local identity or an enterprise IdP

Local named identity preserves the same permissions and attribution contract without an Internet dependency. Enable OIDC when your organization chooses SSO. Use the [Keycloak/OIDC guide](../keycloak/README.md) to configure your provider; the public discovery realm/accounts are a separate opt-in context and must never seed a normal installation.

For enterprise OIDC, configure two **public** clients for Web and CLI with Authorization Code + PKCE S256. Do not distribute a `client_secret`. Register exact redirect URIs and permitted origins; avoid broad wildcard callbacks. Rust owns ephemeral private verifiers, state/nonce correlation, code exchange and token validation. Web login must return to the initiating browser transaction and requested local view, consuming the callback once. The frontend does not authorize itself from unverified token contents.

Use issuer plus subject as the stable external identity. Validate signature, issuer, intended audience, validity and the relevant access-token claims before mapping organizations, tenants and permissions. Configure the mapping for your IdP instead of relying on a universal role name. A realm administrator or account named `admin-ai-guard` is not automatically an ai-guard administrator. Explicitly test an ordinary user, an authorized administrator, a forbidden tenant and a revoked membership in your deployment acceptance procedure.

The same configured issuer must be reachable with the same identity from the browser, the CLI and the Rust component performing discovery, JWKS retrieval and code exchange. Plan internal/external DNS and routes accordingly; a browser-visible URL is not proof that a container can contact it. The default control network is internal. Connecting it to an IdP requires an explicit network decision, trusted public CA material and bounded approved destinations. Do not add general Internet egress merely to resolve an issuer name. Public JWKS can support verification without distributing private signing keys, but it does not eliminate the code-exchange route.

## Install trusted TLS and service identities

Have the PKI operator issue certificates for the actual endpoint names and purposes. Record SANs, issuing chain, key usage, validity, renewal and revocation responsibilities. Distribute only the **public CA chain** to the appropriate browser, control and worker trust stores. Private CA keys remain with the CA; worker containers never receive them. A local demonstration CA is not a substitute for your enterprise issuance policy.

Maintain TLS verification for IdP, model and control connections. Do not use `--insecure`, disable certificate validation or replace DNS names with unrelated IP addresses. Test the intended name and full chain from each actual caller. A public bundle must reach the client that performs the connection; installing it on the host alone does not configure a container or a provider CLI.

Use separate service/workload identities for broker-to-control and worker relay channels, with scope, rotation and revocation. mTLS authenticates a service peer; it does not grant a human role or tenant access by itself. Never send human SSO tokens or global runner credentials to workers. Termination, internal hops, proxies and allowlists must form one explicit deployment policy; an HTTPS edge does not establish encryption on every hop.

## Configure internal AI and decision services

For a local single-installation judge, the [private-CA container profile](../local-ai/TLS.md) connects the gateway through a fixed Rust TLS relay to the existing small model. Only the public CA reaches the gateway; no CA private key enters a container. The final relay-to-model HTTP hop stays on its separate internal network. This profile does not deliver workload mTLS, enterprise inference authorization or cloud transport qualification. Configure those independently for a shared service.

For an internal coding model, record the endpoint URL, exact model/revision, wire format, compatible client/adapter, credential **reference**, public CA chain and approved destinations. Bind the resolved settings to the authorization whenever they affect execution. Keep identity settings separate from provider and model settings. Use the [provider profiles](../providers/README.md) for supported Codex/Claude subscription/API authentication; do not repurpose a model credential as an ai-guard user login.

“OpenAI-compatible” needs a precise protocol: Chat Completions, Responses, streaming/events and tool calls have different contracts. A Chat Completions response does not establish Responses support or Claude Messages compatibility. Select the supported adapter explicitly and refuse incompatible features. Do not infer a routing change from arbitrary `OPENAI_BASE_URL`/`ANTHROPIC_BASE_URL` entries in a dotenv file.

For offline use, confirm that the chosen client, authentication and model operate without external login, refresh or downloads. If the configuration needs an unavailable external service, stop with that prerequisite rather than falling back to a paid public API. Give workers only the selected credential, public trust material and authorized destinations. Review provider settings that can open connectors, tools or additional upstreams.

Decision services remain a separate choice: Rust rules, authenticated real Jev, or a local Jev-like model. No engine may lift a mandatory audit, isolation or policy refusal. For Jev, follow the [separate raw-key and Compose override procedure](../providers/README.md#choose-the-decision-engine); its default egress override is unrestricted and needs your network review. Do not place the Jev key in a worker or model server.

For the small local model, use the [local-AI guide](../local-ai/README.md), pin runtime and weights, and configure context, output, timeout, CPU and container memory. The model family is at most 4B parameters with a 500 MB–1 GB RAM target; measure actual cgroup peak/OOM behavior for the selected configuration. Weight-file size is not RAM usage. Validate bounded structured decisions and uncertainty; a successful text-generation request does not prove judgment quality or equivalence to real Jev.

## Operate and recover

Before admitting work, check control health, authenticated scope, durable audit availability, runtime authority, quota, selected provider and approved destinations. Check a harmless authorized task and its attributed trace, then an expected permission refusal. A health endpoint cannot replace those checks. Centralize the tenant's events and workers in one dashboard, retaining broker identity, freshness and partial/disconnected states.

| Situation | Operating procedure |
|---|---|
| IdP or model unavailable | Display the failed dependency and preserve the refusal/unknown state. Keep authorized local-identity operation separate from any SSO policy; never bypass a required login or switch model/engine implicitly. |
| Broker disconnected | Show its last observation as stale. Retain durable deadlines/claims and reconcile that broker's authority before another start. Absence of fresh events is not zero activity. |
| Stop requested | Persist `worker-stop`, then check observed terminal state. A receipt acknowledging durable intent is not proof that a process stopped. Preserve unknown outcomes without issuing a false success. |
| Credential or membership revoked | Prevent new unauthorized actions, invalidate affected access/streams and apply the relevant execution-revocation policy. Rotate credentials through their owner; do not rewrite audit history. |
| Disk/audit limit reached | Block actions whose mandatory audit cannot be persisted. Apply an approved retention/export process; do not delete evidence to make admission succeed. |
| Export or restore uncertain | Preserve the source and any published destination. Reconcile identities/claims before reactivating execution; do not replay a consumed start. |

Keep user content, relay payloads, diagnostic logs and audit metadata subject to separate access and retention policies. Mask secrets before persistence or dissemination. Signed checkpoints and export verification preserve independent audit evidence; external anchoring is conditional on the configured anchoring policy. Record missing, truncated or partially observed data explicitly.

For backup or update, stop new admissions, reconcile active workers, stop services coherently and preserve each tenant's SQLite data/WAL state together with the corresponding configuration, plans, bindings and retained volumes. Encrypt/restrict backups according to their contents. Do not put an active SQLite WAL database on a NAS as an ordinary shared file. A NAS may be an explicitly authorized backup destination without becoming the live database or an assumed PKI service.

Restore into a controlled destination and check schema/version compatibility, audit continuity, credentials, broker/runtime identity, deadlines and consumed claims before enabling execution. Unknown authority stays unavailable for execution until reconciled; restoring a database does not grant a new start budget. Preserve a rollback checkpoint of the pre-update state and use an explicit compatible procedure; a failed migration does not imply an automatic safe rollback.

For the base Docker stack, a planned service stop retains data:

```sh
docker compose --env-file "$AIGUARD_CONFIG/enterprise.env" -f deploy/docker/compose.yaml stop
```

Include every reviewed override used at startup, including the Jev override when selected; use the corresponding Podman command if appropriate. Restart with the same project/env/files, validate health and recovery state, then admit new work. Avoid `down -v`, `prune`, overwriting an existing realm/configuration or deleting a database as a repair procedure.
