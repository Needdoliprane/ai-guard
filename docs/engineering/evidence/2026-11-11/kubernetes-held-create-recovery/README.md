# Kubernetes held-CREATE recovery — reproduced baseline

This follows run-2 fingerprint k8s-held-create-quota-stranded-after-broker-grant-loss. The original audit and its source remain distinct from this reproduction on public 300cb4a plus test-only additions. This is software control-state evidence, without a Kubernetes engine or workload.

## Observed baseline

One offline lib-test build returns RC0. Both positive controls pass before the three failures: the original current-incarnation cancellation releases quota with a durable receipt, and the schema/scope control exercises actual migrations, authentication, V1 refusals and V2 scope refusals.

The D6 parent then authenticates a real successor grant/binding for the same stable broker after revocation. The old identity remains refused, the exact original initial reservation still holds quota, and the ordinary Reconcile issuer refuses the successor. It reaches its expected RC101 scope oracle at quota_recovery_tests.rs:227. This establishes the missing closed recovery path. It does not justify weakening Reconcile or releasing quota on expiry, inactivity or an absent Pod.

Two separate compatibility parents reach their expected RC101 MigrationRequired oracles at schema12_tests.rs:216 and :288. Actual schema12 migration/integrity and the production V2 issuer succeed first. One attempts Reserve; the other Cancels an unchanged V1 reservation after real migration and independent inactive Store reopen. These reveal the consumer's missing (12,2) case; they are distinct from D6's successor-identity restriction.

| Result | Observed count |
|---|---:|
| Canary PASS | 2 |
| Exact expected RED, each RC101 | 3 |
| Unexpected failure | 0 |

Experiment, closure and final result return RC0. This means the reproductions matched their oracles, not that the product passed. The exact QA container is removed; OOMKilled=false and oom/oom_kill events are zero. The QA cgroup peak is 5,998,678,016 bytes, a compilation/QA observation rather than worker usage. Exit137 belongs to the controlled sleep-holder stop after target commands completed.

## Source and execution evidence

Frozen source: 2,462 regular files, 26,527,232 bytes, SHA256 23e545277c426c7c1af156ebc15a29943f4ee3840ccc35e3fe4e153cd97447b5.
Manifest: 0a1e159f162dcbab4b5a66f60064338c859f944d7ab437500411102d62de53ed.
Actual live post-run source export, captured before stop: 26,695,680 bytes, SHA256 091f64d2289111c57ca2fd8b2ec66c772221315557c8f5e5d2b309caa6d65cb4. Root verifies every path/payload against that manifest. Input and post-run tar headers differ; source payload equality was checked independently.

The two test-only lots share one build. No product code changes in this baseline and no whole suite is repeated. The lib artifact is selected from Cargo's actual JSON output, lists each of the five exact parents uniquely, then executes each once with --exact --nocapture --test-threads=1. One build has a 1,200-second bound and each parent a 60-second bound. No all-targets check is repeated for this RED.

Execution uses the same isolated Linux ARM64/Rust 1.99.1 profile as the [relay composition](../relay-history-owner/README.md): pinned image, UID1000, no external network, read-only root, dropped capabilities, NNP/seccomp, four CPUs, 9GiB RAM/container swap0, 128PIDs, 1,024FDs and 4GiB bounded scratch. Sources/registry inputs are read-only; only explicit environment variables enter target commands. The actual source export is bounded to 32MiB before exact-CID cleanup. Raw source, logs and Root/Luna verification remain private in qa-kubernetes-quota-recovery-20261111-red-r1.

## Correction status and limits

The minimal schema12 compatibility correction is independently reviewed and has a separate GREEN campaign. Its result is not inferred from this baseline. D6 requires a different, exact close-only recovery capability with current actor and historical creator separated, an initial-state fence, audit/quota atomicity and an authorized historical receipt lookup. That implementation and explicit semantic migration13 are in progress in separate worktrees. Existing Reconcile remains strict.

No cluster, Docker/Podman worker, provider, model, user-data migration, remote service or credential is used. No new API route or migration command is declared available here. Real runtime recovery, storage incidents, multi-host operation, old-reader/restore compatibility and all production gates remain open. See the [run-2 register](../../../AUDIT_RUN_2_20261110.md).
