# Named local access

[Français](README.fr.md) · [Setup index](../README.md) · [Local workstation](../local/README.md) · [OIDC](../keycloak/README.md)

Use a named local account to inspect explicitly authorized resources without an identity provider. A human credential is separate from the technical runner/viewer credentials and from Codex or Claude authentication. Membership in a tenant does not grant access to all its sessions. This guide covers local account maintenance and read/export access; the [engineering ledger](../../engineering/CURRENT_STATE.md) records implementation and qualification separately.

## Prepare storage and authority

Use the same trusted Rust binary as the gateway, a private control directory, and the tenant database already bound by the control. Never copy the database into a worker or expose the engine socket to a gateway. Stop the gateway, brokers and automatic restarts before offline maintenance. `--confirm-control-stopped` is an operator attestation, not a fencing mechanism. A normal reopen does not restore execution authority.

The examples below assume an existing schema4 database. Replace paths and tenant with the installation's actual values. A CLI `--tenant-id` checks that existing binding; it does not select a remote user's scope. Upgrade older databases one exact step at a time following the [broker storage procedure](../broker/README.md#upgrade-storage-explicitly). Keep a distinct, new verified backup for every migration.

```sh
AIGUARD_BIN="/absolute/path/to/agentsandbox"
AIGUARD_DATA="/absolute/private/ai-guard/audit.sqlite"
AIGUARD_TENANT="configured-tenant-id"
AIGUARD_PRIVATE="/absolute/private/ai-guard"
"$AIGUARD_BIN" migrate-control \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --backup "$AIGUARD_PRIVATE/before-human-access-v5.sqlite" \
  --target-version 5 --max-events 100000 --confirm-control-stopped
```

Schema5 adds the durable human-read audit protocol. Changing `user_version` manually cannot replace migration. Update all readers together: old binaries refuse newer schemas. No automatic downgrade, restore, activation or account creation occurs. Provision, resume-provision and apply can maintain validated schemas4–10 with inactive supervision; ordinary HTTP human reads support5–10 and report `migration_required` before emitting their read audit on an older store. Broker registry reads and the new membership capability require7. The [workload guide](../workloads/README.md#enable-named-broker-registry-reads) covers explicit5→6→7 maintenance, including a store created by `setup prepare` at5.

For a container-owned database, run offline maintenance as the existing control UID/GID against its existing data volume, with every other consumer stopped. A host CLI cannot assume ownership of a Docker Desktop volume. The one-shot container needs only that data volume and its trusted image; it needs neither a socket, runner credential, provider secret nor network. Preserve the original Compose project/volume identity. Do not initialize a replacement database to bypass a failed upgrade.

For the standard Docker installation, inspect the existing `/data` mount's `.Name` on the stopped gateway with `docker inspect`. Build the current control image, then inspect its immutable ID with `docker image inspect --format '{{.Id}}' agentsandbox-control:local`; the old stopped gateway may still reference an older image. Set the actual values below, without guessing a volume name. These commands use an existing schema4 store and retain both one-shot containers. Their names and backup/output paths must be new; the subshell stops on the first failure:

```sh
AIGUARD_CONTROL_IMAGE_ID="sha256:inspected-current-control-image-id"
AIGUARD_AUDIT_VOLUME="existing-inspected-audit-volume"
(
set -eu
docker volume inspect --format '{{.Name}}' "$AIGUARD_AUDIT_VOLUME"
docker run --name ai-guard-offline-human-v5 --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777 \
  --mount "type=volume,source=$AIGUARD_AUDIT_VOLUME,target=/data,volume-nocopy" \
  "$AIGUARD_CONTROL_IMAGE_ID" migrate-control --data /data/audit.sqlite \
  --tenant-id "$AIGUARD_TENANT" --backup /data/before-human-access-v5.sqlite \
  --target-version 5 --max-events 100000 --confirm-control-stopped
docker run --name ai-guard-offline-human-alice --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777 \
  --mount "type=volume,source=$AIGUARD_AUDIT_VOLUME,target=/data,volume-nocopy" \
  "$AIGUARD_CONTROL_IMAGE_ID" human provision --data /data/audit.sqlite \
  --tenant-id "$AIGUARD_TENANT" --name Alice --organization engineering \
  --output-dir /data/human-alice --max-events 100000 --confirm-control-stopped
)
```

Check each exit before continuing; retain failure evidence and use the matching resume command only for its exact preparation. The standard image entrypoint is the Rust CLI. For a Podman installation, use the same account/authority and corresponding `podman` commands against its own existing volume; qualify its UID mapping separately. Copy only `/data/human-alice/credential` from the retained provisioning container into a **new private host destination** for the user, using `docker cp` without `--archive`, then verify owner/mode0600. Never print it or distribute recovery records. Do not subsequently run the host provisioning example against a different database; the two maintenance paths are alternatives.

## Provision an account and retain its recovery

The output parent must already be private (`0700`). Choose a new output directory; the command refuses to overwrite or adopt an existing one.

```sh
"$AIGUARD_BIN" human provision \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --name "Alice" --organization "engineering" \
  --output-dir "$AIGUARD_PRIVATE/human-alice" \
  --max-events 100000 --confirm-control-stopped
```

The command prepares `credential`, `recovery.json` and `completion.json` privately. It generates a random human credential without printing it. Account, credential binding and initial membership commit atomically with their audit. Standard output reports identifiers and file paths, not the credential. Default account capabilities are read metadata, read content and export; resource grants remain separately required. Restrict capabilities with `--capabilities read-metadata` when appropriate.

After an interrupted or ambiguous result, preserve the exact output and database. Resume the same preparation:

```sh
"$AIGUARD_BIN" human resume-provision \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --output-dir "$AIGUARD_PRIVATE/human-alice" \
  --max-events 100000 --confirm-control-stopped
```

Resume verifies the retained namespace, credential digest, paths, files and operation identities. It does not rotate a credential or recreate missing preparation. Protect recovery/completion files as control records. Deliver only the user's credential through your private channel; do not distribute the full operator preparation. Keep the user's credential file `0600` under a private parent and outside the project being executed.

## Grant resources explicitly

An account starts with no automatically attributed historical sessions. The trusted operator binds a known session, sandbox, agent or project to an organization and a human owner; optional shares specify metadata, content and export independently. A display name, matching project ID or tenant membership cannot establish authorship or access.

Offline `human apply --data ... --tenant-id ... --mutation-file ... --confirm-control-stopped` accepts a private JSON object with `version`, a fresh `operation_id` and typed `change`. It refuses an input `actor`: operator authority comes from the checked control filesystem and effective UID. Changes are `account`, `credential`, `membership` or `resource_grant`, with the exact expected previous revision and the next record revision. Initial binding of historical resources belongs to the trusted operator. An exact replay uses the same operation ID and bytes; a changed meaning needs a new operation ID. See the [typed identity model](../../../src/identity/types.rs) for the complete field contract.

For a first session binding, save this **template** in a new private0600 JSON file under the private parent. Replace every `<...>` value: the operation ID is a fresh canonical lowercase non-nil UUID, the session must already be known in this tenant, and the owner's two UUIDs come from that account's provision result. The organization must match its membership. This example shares nothing with other principals.

```json
{
  "version": 1,
  "operation_id": "<new-operation-uuid>",
  "change": {
    "action": "resource_grant",
    "expected_revision": 0,
    "grant": {
      "version": 1,
      "tenant_id": "<configured-tenant-id>",
      "organization_scope": "engineering",
      "resource": {"kind": "session", "resource_id": "<known-session-id>"},
      "owner": {
        "kind": "human_local",
        "installation_id": "<account-installation-uuid>",
        "account_id": "<account-uuid>"
      },
      "revision": 1,
      "active": true,
      "shares": []
    }
  }
}
```

```sh
"$AIGUARD_BIN" human apply \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --mutation-file "$AIGUARD_PRIVATE/alice-session-grant.json" \
  --max-events 100000 --confirm-control-stopped
```

Deactivate a credential, membership, account or grant through a new audited mutation, retaining its identity and increasing its revision. For this unchanged grant at revision1, a revocation uses a new operation UUID, `expected_revision:1`, grant `revision:2` and `active:false`, preserving its tenant, organization, resource and owner. A conflicting revision is a refusal: inspect the retained operator record rather than guessing. Do not delete the database, edit projections directly or reuse another account's credential. After rights change, old contexts expire or are refused; reacquire an offered context. Provisioning itself does not launch a worker or delegate runner authority.

## Authorize named broker registry readers

`read_broker_registry` is a separate administration capability for infrastructure metadata across the authenticated tenant. It does not expose resource content, raw broker grants, credentials, worker inventory or export. Existing `read_tenant_metadata` keeps its organization filter. Neither old memberships nor an administrator's name or IdP role acquires the new capability automatically.

First perform the [explicit migration to7](../workloads/README.md#enable-named-broker-registry-reads) with all consumers stopped. On validated schema7 with inactive supervision, a trusted filesystem operator can create a **new** local reader with only this capability:

```sh
"$AIGUARD_BIN" human provision \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --name "Infrastructure reader" --organization "engineering" \
  --capabilities read-broker-registry \
  --output-dir "$AIGUARD_PRIVATE/human-infrastructure-reader" \
  --max-events 100000 --confirm-control-stopped
```

Use the installation's actual event limit instead of `100000` if different. When it uses an explicit human audit policy, also pass the same `--audit-capacity-file` as the other offline identity commands. The default provisioning capabilities remain `read-metadata,read-content,export`; the new option is never a default. Requesting it below7 fails before creating the preparation directory. `resume-provision` can replay an old exact preparation after migration without adding a grant or rotating its credential.

For an **existing** local or OIDC account, use `human apply` and its actual membership record. The following is a closed JSON **template for a membership currently at revision1 with only `read_metadata`**. Replace every placeholder, keep the exact principal, tenant, organization and membership ID, and preserve every existing capability that is still intended. A real record at another revision requires its actual `expected_revision` and next `revision`:

```json
{
  "version": 1,
  "operation_id": "<new-operation-uuid>",
  "change": {
    "action": "membership",
    "expected_revision": 1,
    "membership": {
      "version": 1,
      "membership_id": "<existing-membership-uuid>",
      "principal": {
        "kind": "human_local",
        "installation_id": "<existing-installation-uuid>",
        "account_id": "<existing-account-uuid>"
      },
      "tenant_id": "<configured-tenant-id>",
      "organization_scope": "engineering",
      "revision": 2,
      "active": true,
      "capabilities": ["read_metadata", "read_broker_registry"]
    }
  }
}
```

For OIDC the same `principal` position is `{"kind":"human_oidc","issuer":"<exact-approved-issuer>","subject":"<verified-subject>"}`. Use the existing principal bytes; changing principal or organization on a membership is refused. An initial membership uses `expected_revision:0` and `revision:1`, after its account exists. Save the reviewed mutation as a new private0600 file, without an `actor` field:

```sh
"$AIGUARD_BIN" human apply \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --mutation-file "$AIGUARD_PRIVATE/broker-reader-membership.json" \
  --max-events 100000 --confirm-control-stopped
```

Only authenticated trusted control or this checked offline authority can introduce or reactivate the tenant-wide capability. An organization administrator with `manage_memberships` may retain it on an already active membership, keep it dormant while inactive, remove it or deactivate the membership within that administrator's existing rights. The administrator cannot add it, restore it after removal, or reactivate an inactive membership carrying it, for themselves or another user. Do not create a second membership to change its immutable principal/organization or evade this boundary. OIDC additionally requires the explicit verified-claim ceiling described in the [SSO guide](../sso/README.md#opt-in-to-broker-registry-reads).

After the intended control services reopen, authenticate normally and choose a currently offered **administration** context containing `read_broker_registry`. The dashboard exposes **Brokers** and **Brokers enregistrés du tenant.**; a context with only this capability opens that view directly. It does not automatically read resources, events, exports or their SSE stream. There is no `human brokers` CLI or broker command in the foreground SSO CLI; use the dashboard or the [human HTTP contract](../../api.md#registre-brokers-humain--human-broker-registry). Never enter a runner key in the browser.

The table and **Détails du broker** show the server evaluation time, current incarnation, grant/heartbeat revisions, declared authorities, last heartbeat reception, expiry and durable read receipt. **Bail courant** means the recorded lease is current; it is not proof of a running worker. **Heartbeat d’une ancienne incarnation** preserves the last received assertion without attributing it to a new incarnation. A successful empty page means no registered row at that position and evaluation time, never zero runtime activity. Rows come from the real trusted registration/heartbeat operations in the [workload guide](../workloads/README.md#grant-a-broker-and-interpret-its-heartbeat), not synthetic events or manually edited SQLite.

Pagination is live, at most100 rows and256KiB per response; the dashboard requests25. Changes can occur between pages, and no total or snapshot is supplied. Keep the exact opaque cursor with the same context, authorization version and limit. The server traversal budget is1000 pages/16MiB within the context's absolute lifetime of at most300seconds. This is a traversal bound, not a replenishable audit allowance.

**Actualiser** rereads the selected view. Automatic refresh is triggered at most every30seconds while Brokers is visible; network latency, queueing, a suspended browser and a disconnected control can extend the time before any result. This is no response or permission-withdrawal deadline. A403 removes rows, details and cursors and offers **Relire mes accès**. Changing context, leaving Brokers or disconnecting aborts its reads and clears its page; late responses cannot refill the old view.

Each page, including an empty one, requires a durable receipt sharing the existing human tenant/principal audit budgets. Authorization, bounded projection and receipt commit in one Store transaction; the route reauthenticates before publication. A receipt records the read, not proof of delivery, execution or an externally signed checkpoint. An audit failure returns no page. A503 `migration_required` requires offline migration; a429 `audit_read_budget_exhausted` with `contact_operator` requires intervention, not timer retries. The dashboard retains any previous page as dated/stale and stops automatic reads even through visibility changes or reselection. Preserve database, policy and receipts, inspect occupancy/reserves and repair the cause. After intervention use **Relire après intervention de l’opérateur** for one explicit attempt; a current success permits refresh again. Temporary capacity/transport failures remain distinct. Never delete audit, rotate credentials or reset storage to recover capacity.

This registry view does not qualify multi-broker transport, federation, worker/session aggregation, runtime completeness, capacity100 or a production release. It changes no quota or billing contract: allocation remains nonbillable, and unobserved usage remains unknown. See the engineering records for the actual validation scope of the matching sources.

## Human-context capacity

The gateway reserves a fixed share of its human-context registry for each tenant declared and validated in its trusted configuration. The total remains 4096 live contexts. With two configured tenants, each has 2048 places; an idle tenant's share is not lent to another. For `N` tenants, each share is `4096 / N` rounded down, with one additional place for each of the first `4096 % N` tenant IDs in sorted order. Empty, duplicate, invalid or more than 4096 tenant configurations are rejected. A request cannot register an unknown tenant.

The existing bound of 64 contexts still applies to the same principal or credential across all tenants. Credential rotation does not grant that principal another 64 places. Tenant membership, personal/administration modes and permissions still come from Rust authorization; a header or JSON field cannot freely select a tenant. A tenant's share does not guarantee 64 contexts for every one of its users.

If some authorized contexts cannot obtain a place, the identity response can return the available contexts with `contexts_complete=false`. Those contexts remain usable within their permissions: a full tenant A share does not hide an available tenant B context for a person belonging to both. A partial response is not a complete membership inventory and does not mean no activity or lost rights. If capacity prevents every context for that identity, the route returns 429 `capacity_exhausted` with `retry_action:"retry"`. Wait for expiry or ask an authorized administrator to remove unnecessary memberships, then read identity again. Authentication and audit failures keep their own errors; in particular, a revoked local credential returns 401 `identity_revoked` with `authenticate`.

Contexts still last at most 300 seconds, bounded by the authentication proof. Reading identity again does not renew an existing binding: its ID, authorization version and expiry stay unchanged. Expiration, revocation, read revalidation and encrypted cursors retain their existing behavior. Never rotate credentials, reset storage or delete audit to recover capacity.

This partition applies only to the gateway's in-memory human-context registry. It reserves no CPU, ingress, Store I/O or OIDC transaction capacity and promises no throughput, latency or overall SSO/cloud availability. The [engineering ledger](../../engineering/CURRENT_STATE.md) records the matching source, tests and remaining qualification; this guide is not evidence of a pass.

## Read from the CLI

Restart only the intended control services with the upgraded binary and existing data. Choose the gateway origin, not its `/api` path. HTTPS is verified; local HTTP requires a literal loopback IP such as `127.0.0.1`. Ambient proxies, redirects and automatic retries are disabled by this client.

For an HTTPS gateway signed by an approved private CA, add `--gateway-ca-file /absolute/approved/gateway-ca.pem` to each `human identity`, `resources`, `events`, `export` and `resume-export` command. Use a checked certificate-only public PEM bundle, at most 65536 bytes and 8 certificates, in a protected directory. A private `0600` file or a read-only `0444` projection is supported; mutable aliases, links, untrusted ownership/ancestors and unsupported ACLs are refused. The bundle replaces the built-in roots for this gateway client, preserving chain, SAN and validity checks. Without the option, the existing embedded WebPKI trust is unchanged. The option requires HTTPS and does not change system, browser or OIDC-provider trust.

```sh
AIGUARD_GATEWAY="https://approved-gateway.example.invalid"
AIGUARD_GATEWAY_CA="/absolute/approved/gateway-ca.pem"
AIGUARD_CREDENTIAL="$AIGUARD_PRIVATE/human-alice/credential"
"$AIGUARD_BIN" human identity \
  --gateway "$AIGUARD_GATEWAY" --gateway-ca-file "$AIGUARD_GATEWAY_CA" \
  --credential-file "$AIGUARD_CREDENTIAL"
```

Replace that nonfunctional example origin and CA path with the operator-approved values. The CLI loads one immutable trust snapshot per invocation; replacing the file does not change an in-flight command. A retained export binds the exact bundle bytes as well as the gateway, credential and context. Keep the same bytes for `resume-export`, including when moving the bundle to another checked path. A changed bundle requires a new explicit export into a fresh destination; retain the unresolved preparation. `gateway CA configuration is invalid or unavailable` means the file or PEM must be corrected before retrying. `--gateway-ca-file requires an HTTPS gateway origin` means the selected transport is incompatible. A connection/TLS refusal never disables verification or falls back to another trust source.

```sh
AIGUARD_GATEWAY="http://127.0.0.1:8787"
AIGUARD_CREDENTIAL="$AIGUARD_PRIVATE/human-alice/credential"
"$AIGUARD_BIN" human identity \
  --gateway "$AIGUARD_GATEWAY" --credential-file "$AIGUARD_CREDENTIAL"
```

Copy an exact context ID offered by that response. The control derives its tenant, organization, mode and capabilities from the authenticated identity. A context ID cannot authenticate a request. Personal and administration are distinct modes, and administration is offered only when authorized. Contexts last at most300seconds and do not renew implicitly.

```sh
AIGUARD_CONTEXT="exact-context-id-from-human-identity"
"$AIGUARD_BIN" human resources \
  --gateway "$AIGUARD_GATEWAY" --credential-file "$AIGUARD_CREDENTIAL" \
  --context "$AIGUARD_CONTEXT" --limit 25
```

Use an actual resource ID returned there, and explicitly choose metadata or content:

```sh
AIGUARD_SESSION="authorized-session-id"
"$AIGUARD_BIN" human events \
  --gateway "$AIGUARD_GATEWAY" --credential-file "$AIGUARD_CREDENTIAL" \
  --context "$AIGUARD_CONTEXT" --resource-kind session \
  --resource-id "$AIGUARD_SESSION" --view metadata --limit 25
"$AIGUARD_BIN" human export \
  --gateway "$AIGUARD_GATEWAY" --credential-file "$AIGUARD_CREDENTIAL" \
  --context "$AIGUARD_CONTEXT" --resource-kind session \
  --resource-id "$AIGUARD_SESSION" --view metadata \
  --output-dir "$AIGUARD_PRIVATE/alice-session-export"
```

CLI reads paginate with the exact opaque `next_cursor` and unchanged selection. No total is inferred from one page. Each returned batch requires a durable nominative read receipt; that receipt proves control recording, not delivery or complete audit-chain verification. Metadata excludes commands, prompts/results and free-text decision reasons; content needs separate rights. Export additionally needs export permission.

Export publishes a new private destination. Its client budget is25pages,4MiB and60seconds from the initial preparation; resuming does not reset that deadline. An incomplete result remains explicit. Retain the adjacent `.preparing` directory after failure. `human resume-export` takes the same gateway, credential file, context and final output directory; it verifies the retained selection. Once60seconds have elapsed, an incomplete export cannot resume even if its300second context remains valid. An expired budget or refused/expired context requires a newly authorized explicit export into another fresh destination, while retaining unresolved files.

## Dashboard and failures

Select named local access in the dashboard connection form, enter only the personal credential using your private credential viewer, then select one of the offered contexts. The credential remains separate from the legacy viewer connection. Switching identity/mode/context removes previous data, cursors and exports. The resource list and real-time events use the same Rust authorization as the CLI; the browser never accesses a container engine.

| Result | Operator or user action |
|---|---|
|401 authentication refusal | The human session closes. Check active credential/account and reconnect explicitly. |
| Context expired/revoked | Discard that context's data and cursors; read identity again and choose a currently offered context. |
| Resource not found/refused | Check explicit resource rights. An inaccessible identifier does not reveal whether another user owns it. |
|503 `migration_required` | Stop controls, preserve a verified backup and apply the explicit storage migration; no viewer fallback. |
|429 `audit_read_budget_exhausted`, `contact_operator` | Stop automatic retries and ask the operator to inspect the durable audit budget. |
| Other 429 capacity or page limit | Follow the stated retry/reduce-page action; do not widen permissions. |
| Audit/storage unavailable | Reads requiring audit fail closed. Repair storage/capacity before retrying. |

The SSE client resumes only after a complete validated frame. Retries may duplicate events; possession of a cursor proves neither delivery nor download. Revocation stops future authorized reads, not bytes already received. Local identity, OIDC identity, workload identity and AI-provider authentication remain independent concerns.

## Audit budgets and stream capacity

Human identity, resource, event and export reads require their own durable receipt.
Delivered SSE batches also require a receipt; empty SSE polls do not. Their
cumulative budgets are shared by all credentials, contexts and sessions of the
same canonical principal within a tenant. Reconnecting, rotating a credential or
restarting the gateway does not restore the budget. Local and OIDC identities
remain distinct; the product does not infer that they identify the same person.

With no explicit policy, the event and payload-byte capacities each allocate:

| Admission | Default limit or reserve |
|---|---|
| All human receipts in a tenant | Half of physical capacity |
| One principal's human receipts | One eighth of physical capacity |
| Reserve for control, protected from ordinary ingestion, human and workload receipts | One quarter of physical capacity |
| Reserve for identity maintenance, protected from ordinary, control, human and workload writes | One eighth of physical capacity |

Reserves are subtracted from the actual total occupancy; ordinary events already
stored still consume capacity. Identity maintenance includes revocation and other
authorized identity changes, so that reserve is finite too. Bytes count the exact
prepared UTF-8 payload stored in the audit, rather than response bytes or SQLite
file size. The physical payload limit is 256 MiB; `--max-events` sets the event
limit. Very small automatic capacities that cannot represent protected human
budgets disable new human reads explicitly, without raising the physical limit
or invalidating existing storage. Ordinary human reads require validated schema5–10;
broker registry reads require7–10, worker registry reads require10, and both consume the same cumulative human budgets.

For schema5–10, ordinary ingestion stops at physical capacity minus both
reserves, in events and bytes independently. Trusted control facts, decisions,
typed control mutations and offline genesis records may use the control share
but stop before the identity reserve. Identity maintenance may use the remaining
physical capacity. The authenticated Rust route or typed control method selects
this class; `kind`, provenance and other payload labels cannot select it.
The [closed control-event route](../../api.md#ingestion-et-faits-du-contrôle--ingestion-and-control-facts)
accepts the three fixed D5 audit kinds only through runner delivery. Their
descriptive receipts do not prove retirement or authorize an execution.

This admission change uses the existing policy and durable audit occupancy;
it does not migrate or reset the database. A valid historical audit already above
the new ordinary threshold remains readable and exact replays remain idempotent;
new ordinary writes are refused. Existing occupancy can also leave no control
share available. Preserve it and inspect the actual limits rather than deleting
records or changing credentials. `429 audit_write_budget_exhausted` with
`contact_operator` is cumulative and has no timer retry.

[audit-capacity.json](examples/audit-capacity.json) gives the resolved defaults
for `--max-events 100000`. Copy it into your existing private control directory
with mode0600. For a local gateway, add
`--audit-capacity-file /absolute/private/ai-guard/audit-capacity.json` to its
existing launch command. This option conflicts with `--tenants-file`: with a
tenant registry, put the same JSON object in each relevant tenant's optional
`audit_capacity_policy` field instead. Keep all other fields and existing data
paths. Offline `human provision`, `resume-provision` and `apply` accept the same
file option. Use the same intended policy and `--max-events` across consumers;
the offline command defaults to `1000000` events and the gateway to `100000`.
Policy changes never clear previously recorded usage. Start-up diagnostics print
the resolved policy, its source and fingerprint, without proving free capacity.

`429 audit_read_budget_exhausted` with `retry_action:"contact_operator"` is a
cumulative refusal: do not retry on a timer. Preserve the database, policy and
incomplete exports, then ask the operator to inspect occupancy and policy. An
explicit increase can permit further reads only if physical capacity and
reserves allow it. No qualified archive/purge operation is supplied here;
deleting events or restarting is not recovery. Physical exhaustion or unavailable
mandatory storage remains503. A neighboring unavailable profile may produce a
partial identity response; it must not be treated as proof that access vanished.

The dashboard preserves this 429 and `contact_operator` action when its human
identity preflight reaches the budget refusal, including when the configured
capacity disables human reads. It supplies a fixed local message and no
`Retry-After` header. Temporary transport failures and unrecognized preflight
errors remain 503 `identity_unavailable` with `retry`; they do not establish that
the durable budget is exhausted.

Human SSE admission allows four live streams per canonical principal and
24 human streams per tenant, within the existing shared total32. Eight places
remain available to technical streams when the human pool is full. These fixed
admission bounds are separate from Store I/O permits and receipt budgets. Close
unused streams before retrying a concurrency refusal. A retained Body keeps its
permits until drop, including after a terminal error or observed EOF; a client
that does not poll can occupy its share. Every poll still rechecks expiry and
revocation. These bounds do not establish a limit on all HTTP sockets or an
autonomous eviction guarantee.

## Authorize named worker registry readers

The separate administration capability `read_worker_registry` requires explicit9→10 migration and trusted membership approval. Follow [named worker registry reads](WORKERS.md) for the local/OIDC procedure, shared audit budgets, restore behavior, data limits and the obligatory personal-attribution gap. No existing broker/resource permission grants this right.
