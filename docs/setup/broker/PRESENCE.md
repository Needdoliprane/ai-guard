# Publish the trusted broker's presence

[Français](PRESENCE.fr.md) · [Broker setup](README.md) · [Workload identities and grants](../workloads/README.md)

The foreground Rust service can publish its own authenticated presence to the existing tenant broker registry. Enable this separately for each Registry wire2 profile with `presence_file`. Profiles without that field, and Registry wire1, retain their previous behavior. This publishes a broker declaration and the gateway's reception time; it does not inspect workers, prove runtime health, release quota, or establish complete federation across hosts.

## Prepare authority and private storage

1. Use a gateway with schema6 or7 and the existing tenant namespace. Follow the [explicit migration procedure](../workloads/README.md#prepare-storage-without-activating-execution); this does not activate supervision.
2. From trusted control, [issue a broker workload credential](../workloads/README.md#prepare-and-record-a-credential-operation-from-the-cli) for the approved broker/incarnation. Retain its credential ID and private token file. Then [prepare and apply a broker grant](../workloads/README.md#prepare-a-broker-grant-or-revocation-from-the-cli) with the trusted runner for the same broker, credential and incarnation, with **exactly one authority** matching this profile's authority ID and runtime kind, the intended lease and future expiry. The general API permits several authorities; this publisher requires the exact single-authority list. The service neither creates nor renews that authority.
3. Create a **new dedicated private state directory** outside all project/export trees, owned by the control account and mode0700. Preserve an existing directory when resuming. Create the configuration and token as private regular files, mode0600 with private0700 parents; links, unsafe ancestry and substitutions are refused. Do not copy the runner into the workload token file.
4. Copy [presence.json](examples/presence.json) into that private configuration directory and replace every placeholder and path. It is a typed JSON input, not a dotenv file or an installed identity. A token is referenced by path only and must never appear in the JSON, arguments or logs.

Add this field to the intended profile in the [complete Registry wire2 template](README.md#write-the-private-operator-registry), keeping its existing settings:

```json
"presence_file": "/absolute/private/ai-guard/presence/config.json"
```

`broker serve --config /absolute/private/ai-guard/operator/registry.json` reads this file at startup. Its gateway, tenant, namespace, installation, profile and runtime expectations come from that registry. The gateway derives the actual tenant from the authenticated workload proof; configuration cannot select a different tenant. Keep all control material outside source/export trees even though the service also installs workspace exclusions and checks the combined secret-protection capacity before opening presence state. Existing service limits remain cumulative:64 protected-file entries and64 distinct secret literals (256KiB total), including other profiles and control inputs.32 configured profiles do not guarantee capacity for32 publishers; an over-capacity configuration is refused rather than truncated.

| Presence V1 field | Contract |
|---|---|
| `version` | Exactly 1; unknown fields are refused; configuration ≤16 KiB. |
| `broker_id`, `credential_id`, `incarnation_id` | Canonical, non-nil UUID strings matching the approved binding and grant. No generated replacement on failure. |
| `authority_id` | The approved authority label, at most 256 bytes, without control characters. Runtime kind comes from the operator profile. |
| `broker_token_file` | Absolute private file; token of 32–4096 ASCII bytes without whitespace/control characters, optionally followed by one LF. |
| `ca_bundle_file` | Optional absolute private certificate-only PEM file for HTTPS, ≤64 KiB and 1–8 certificates. When supplied, replaces built-in roots for this client. No CA private key or TLS bypass. Omit or use `null` for built-in roots. |
| `state_dir` | Existing private directory, mode `0700`, dedicated to this profile and scope. It retains ownership, anchor, pending operations and receipts. |
| `interval_seconds` | Default 30; range 5–150. Twice the interval must not exceed the grant's lease. |
| `max_operations` | Default 8192; range 1–32768 retained operations. Bounds new operations, while an already-counted pending operation still requires enough receipt capacity. |
| `max_state_bytes` | Default 67108864 (64 MiB); positive and at most 134217728 (128 MiB). Includes retained stream evidence and reserved receipt capacity before POST. |

The HTTP client accepts HTTPS or **numeric loopback HTTP** for deliberate local use. It uses only fixed identity/self/heartbeat routes, disables ambient proxies, redirects and transparent retries, and bounds responses and timeouts. A name resolving to loopback is not numeric loopback. The token and any CA file are retained and revalidated; editing live files is not hot reload.

## Start, observe and stop

Start the same [foreground broker command](README.md#start-the-foreground-service). The service acquires its local role/listener before starting an owned publisher. No job, engine operation or model request is needed to publish presence. The publisher authenticates its scope, reads the current broker view, durably records its exact next operation before POST, and retains the validated receipt. A lost acknowledgement retries the same operation; it does not invent a new sequence.

Read the resulting entry using technical `GET /api/v1/brokers` with a runner/viewer proof, the broker's own `GET /api/v1/brokers/self` with its Broker proof, or the [named Brokers dashboard view](README.md#read-registered-brokers-after-migration7) with explicit `read_broker_registry` permission and schema7. Check authority, incarnation, reception time, lease and freshness. A heartbeat is `declared`; gateway reception is `observed`; `runtime_inventory_complete` remains false. An old entry after shutdown may remain current until its lease expires. Neither a current nor a stale entry proves a worker's state.

Normal service shutdown signals and joins publisher tasks before releasing its local role. `--recovery-only` does not open presence configuration/state or publish a heartbeat; it retains its existing stop-recovery contract. This does not qualify power-loss recovery, an OS service or an autonomous runtime deadline.

## Diagnose and resume without erasing evidence

| Condition | Behavior and operator action |
|---|---|
| Transport or generic temporary unavailability | Initial attempt plus at most three retries with backoff of 100, 250 and 500 milliseconds. Exhaustion stops that publisher. Diagnose connectivity, then explicitly restart with the same state and authority. |
| Authentication, grant, revision, clock, protocol or capacity refusal | Stops the publisher without periodic automatic retries. Preserve configuration, pending operation, receipts and control evidence; repair the exact cause before explicit restart. No runner fallback, regrant or credential renewal. |
| Local stream corruption, missing historical evidence or exhausted stream budget | Preserve evidence; the publisher stops or is skipped at startup. This alone does not disable normal broker job admission. Do not reset the stream to bypass reconciliation. |
| Unsafe/missing/changed private configuration, credential, CA or retained directory | Refuse startup or fence new business admission. Publication failure does not excuse unsafe control material. |

Fully acknowledged state can resume after an explicit credential rotation only with the same credential identity, subject and grant and the exact expected heartbeat history. An unresolved pending operation cannot cross a changed/revoked grant or binding revision. For a deliberate fresh grant with a new approved incarnation and matching new binding, prepare a new configuration and state directory while preserving the old ones; the immutable old anchor cannot be repurposed. The server retains the global heartbeat revision and older facts. Missing state with heartbeat history for that same incarnation requires reconciliation; do not create an empty replacement directory, UUID or anchor to bypass it.

Local limits and [cumulative workload audit limits](../workloads/README.md#set-a-bounded-workload-audit-policy) both apply. 8192 operations at a nominal 30-second interval correspond to about 68 hours 16 minutes; at 5 seconds, about 11 hours 23 minutes. These calculations exclude HTTP/filesystem time and are not upper bounds on elapsed time: the interval sleep follows each cycle. Byte limits, other audit occupancy or grant expiry may stop publication earlier. `429 audit_workload_budget_exhausted` requires operator action; restart, rotation and a new incarnation do not reset the broker's cumulative audit usage. There is no qualified archival/purge procedure. Preserve state rather than deleting evidence for capacity.

The implementation performs three complete history checks/synchronizations during a successful new-operation cycle without retry. A cached snapshot avoids an additional scan, while the complete checks before POST, on acknowledgement and on reopen remain required. Altered history may be detected after the identity/self reads; no new POST is authorized by the cache. Cost still grows with history; sustained capacity and the default retention horizon remain to be measured. This guide does not promise indefinite unattended publication. See [validation and remaining limits](../../engineering/VALIDATION.md) before choosing an operational retention policy.
