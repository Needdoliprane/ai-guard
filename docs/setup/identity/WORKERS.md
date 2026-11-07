# Named human worker registry reads

This administrative view reads the existing tenant worker/broker registry. It requires a local named identity or an admitted OIDC access token, an offered **administration** context and explicit `read_worker_registry`. It covers that authenticated tenant's infrastructure metadata; `read_metadata`, `read_tenant_metadata`, `read_broker_registry`, realm administration and account names confer no implicit access. A human session receives no runner/viewer/broker credential.

## Upgrade the existing storage explicitly

Stop every consumer and automatic restart. Preserve the existing tenant volume, credentials, runtime/enforcement anchors and consumed claims. Use a matching product build for gateway, dashboard, broker, workload/native clients and offline maintenance. Old binaries do not recognize schema10 and refuse it. Migration never activates execution or grants/reactivates a permission.

An existing schema9 requires exactly9→10. Earlier stores require each next migration with its own new verified backup; setup still creates5 and rejects infrastructure registry capabilities. Follow [5→6→7](../workloads/README.md#enable-named-broker-registry-reads), then the delivered explicit targets8 and9 on the same store with separate new backup paths, before10. These intermediate storage markers do not authorize or qualify Kubernetes activity. Do not edit `user_version`, reuse a backup path or bootstrap replacement storage.

From a validated7, run the same maintenance command shown below twice first: use `--target-version 8` with a new `before-held-creates-v8.sqlite` backup, verify its7→8 report, then `--target-version 9` with a new `before-held-policy-v9.sqlite` backup and verify8→9. Keep the same actual `--data`, `--tenant-id`, capacity and stopped-consumer boundary. Stop on any refusal; never skip a target or reuse a successful backup destination.

For a validated9 store, replace all paths, tenant and capacity below with the existing installation's actual values:

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/ai-guard/audit.sqlite" \
  --tenant-id "<configured-tenant-id>" \
  --backup "/absolute/private/ai-guard/before-human-workers-v10.sqlite" \
  --target-version 10 --max-events 100000 --confirm-control-stopped
```

Container-owned storage uses its inspected existing volume and actual control UID/GID, with no socket/network and all consumers stopped; see [offline identity maintenance](README.md#prepare-storage-and-authority). The prepared tenant path is `/data/installation/tenants/<tenant>/audit.sqlite`, which must not be confused with another installation's store.

Check the exit and report's `from_version:9`, `to_version:10`, backup path and genesis sequence/hash. The migration verifies a fresh exact9 backup, then commits `control.identity.workers_access.genesis` and version10 atomically. It adds no tables or indexes, clears no budgets, changes no memberships and activates no execution. Audit-capacity failure keeps source9/head unchanged; preserve any backup already created.

For a restore, retain the upgraded source and restore the verified backup into a new private destination under the existing fencing/recovery procedure. A9 backup remains9, with no worker-read capability; it does not acquire10 from the new binary. Reopening a10 store validates its marker, identity projection and audit. Neither restore nor cold reopen authorizes replaying starts or bypassing inactive supervision.

## Grant only the approved tenant access

Use the existing [typed membership mutation procedure](README.md#authorize-named-broker-registry-readers), with `read_worker_registry` explicitly added to the intended membership instead of inferring it from another capability. Preserve its exact principal, tenant, organization, other reviewed capabilities, prior revision and next revision. Use a new operation UUID and the existing private `--mutation-file`; caller `actor` is refused. Keep the installation's `--max-events` and audit policy consistent. Only trusted control/offline authority may introduce or reactivate this tenant-wide right; an organization membership administrator may retain/remove it, never add/reactivate it.

OIDC also requires the explicit verified-access-token claim ceiling. This is a **single rule fragment** inside the existing reviewed `gateway.mapping.rules`, not a complete configuration or a role creation command:

```json
{
  "claim_path": ["resource_access", "ai-guard-api", "roles"],
  "equals": "guard-worker-reader",
  "capabilities": ["read_worker_registry"]
}
```

Use the actual approved client/claim/role; update a matching existing rule instead of duplicating it. Increment `gateway.mapping.revision` and restart with the same data/configuration. Never put an administrative capability in `default_capabilities`. Both active membership and ceiling are necessary; a role alone grants nothing and cannot select another tenant. No distributed discovery/default manifest automatically grants this right.

Reauthenticate, choose the offered administration context, then open **Workers**. A worker-only context opens that view without resource, event, broker, export or SSE calls. Withdraw the right through an audited membership revision or an updated ceiling/new login. Context/identity refusal purges page, cursor and detail; late responses cannot restore them. Revocation cannot retract already received bytes.

## Read scope, failures and remaining obligations

`GET /api/v1/human/workers` accepts only `limit` (default25,1–100) and an opaque cursor. The closed response is bounded to256KiB; traversal is bounded to1000pages/16MiB within the context lifetime (at most300s). It uses one checked authorization/projection/read-audit transaction per page, then reauthentication before publication. Empty pages share the durable human F1 tenant/principal budgets. A receipt proves recording of the read, not delivery or runtime execution. Cursors are specific to Workers, identity, context, authorization version and limit.

The whitelist contains identifiers/revision, control state/quota, declared session reference, selected runtime/resource limits and last observed runtime metadata. It excludes commands, argv/env/content, credential references/names, host/volume/container paths/names and arbitrary errors. Broker associations are historical ownership of an authority label and may postdate a worker fact; they do not prove which physical broker/incarnation launched or inspected it. Unknown runtime inventory/usage stays unknown; no page-derived tenant total or bill is presented.

Loading, empty, error, forbidden and stale reads remain distinct. A later transport/storage error keeps the permitted last page dated/stale; an identity/context/permission refusal removes it and open details. Visible-page polling is at most every30seconds, without an end-to-end notification deadline. No implicit worker SSE channel is granted. On `429 audit_read_budget_exhausted`/`contact_operator`, automatic reads stop: preserve audit/policy/receipts, repair capacity, then use the explicit operator-intervention retry. A503 is never an empty registry or viewer fallback.

**The personal worker view remains an obligatory open gap.** A declared `launch.session_id` cannot establish human ownership. The authenticated initiator/request/delegation/admission chain must be implemented and qualified before that view can be claimed complete. This administration slice does not qualify remote broker transport, the complete unified cloud/personal journey, real runtime confinement, capacity100 or a production release. Software fixtures are not real deployment qualification.

### Historical identifiers, dates and exact exit codes

The four human worker identifiers (`worker_id`, `request_id`, `attempt_id`, `session_id`) retain the control contract's canonical UUID format, including nil; the Workers keyset cursor accepts that same position. Broker, read-receipt and credential identities keep their own validation contracts.

Runtime date fields have separate `known|absent|unknown` states: `observed_started_at_status`, and `observed_at_status`, `started_at_status`, `finished_at_status` inside `last_observation`. `known` means an interpretable supplied value, not coherent chronology, actual completion or independent observation coverage. Known values are canonical UTC with nanoseconds preserved; absent values are null/`absent`, and uninterpretable runtime starts are null/`unknown` without the raw invalid text. The historical start classifier treats nonpositive starts as unknown. `observed_at` has no added positive-epoch restriction; a valid RFC3339 finish at epoch0 remains known. A known UTC rendering must fit the published four-digit RFC3339 year domain and roundtrip the same instant and nanoseconds. A valid required source whose UTC rendering crosses that domain becomes null/`unknown`; malformed raw `observed_at` remains a storage error, and `absent` is never permitted for this mandatory source. Contradictory chronology retains the control's InDoubt/quota state, with duration unknown. The UI computes no duration from these timestamps.

The human `exit_code` is null or a canonical signed decimal **string** within i64 bounds (`-9223372036854775808` to `9223372036854775807`). The frontend validates with BigInt and displays it exactly; it never converts it to Number. Original technical WorkerV1/Observe records and their integer/date contracts remain unchanged.

## Compose worker and broker pages

`GET /api/v1/human/federation/workers` composes the existing human worker and broker pages for one authenticated tenant. It requires the **same offered administration context** to carry both `read_worker_registry` and `read_broker_registry`. Add each right only through its existing trusted membership procedure and, for OIDC, its approved verified-access-token claim ceiling; no migration, role, service or automatic grant is added. Rust checks both current rights before either inventory read, then reauthenticates and checks both with the original context and authorization version before publication. `/api/v1/human/workers` keeps its existing worker-only permission and descriptive associations.

The closed query has four optional fields: `worker_limit`, `worker_cursor`, `broker_limit`, `broker_cursor`. Limits are independent, default25 and1–100. Each leg’s opaque cursor belongs to this route, its leg, identity, context, authorization version and limit; another route’s or opposite leg’s cursor is refused before either read receipt. Omit a cursor to reread that leg’s first page. Every successful composition reads both legs and consumes two existing durable human F1 receipts, including empty/reread pages. Each traversal retains its1000page/16MiB bound within the context’s at most300seconds.

The closed v1 response has `source:"tenant_federation_registry"`, `consistency:"independent_snapshots"`, `coverage:"partial"`, `runtime_inventory:"unknown"`, and unchanged `workers`/`brokers` page objects. Keep each page’s separate `evaluated_at`, `read_receipt`, `has_more` and `next_cursor`; `workers.consistency` remains `"snapshot_per_page"` and `brokers.consistency` remains `"live_keyset"`. Each page remains bounded to256KiB; the aggregate limit derives from their real shared constant: `2 * HUMAN_MAX_PAGE_BYTES + 4096`, currently528384bytes. Two checked snapshots and live keyset traversals provide no global snapshot, tenant total or complete runtime inventory.

This composition includes registered brokers with no worker row on the current worker page. Do not label those brokers idle: an unseen/stale/revoked broker or missing observation leaves activity unknown. Broker associations remain historical labels, not proof of which broker/incarnation launched or inspected a worker. Control state, held quota and last observations retain their own meaning. If either read or publication fails, no composite is returned; a first durable receipt remains if the second read fails. Preserve audit and follow the existing401/403/400/429/503 recovery guidance; never substitute an empty leg or viewer access.

The existing Rust dashboard proxy accepts the exact API path, with human authentication and context checks. **The frontend does not yet present this composed view.** UI integration, authenticated remote-broker transport, personal ownership, real disconnection/recovery and capacity100 remain separate open work. Those journeys require their own runtime/deployment evidence before any broader qualification claim; this API adds no runtime access or billable usage.
