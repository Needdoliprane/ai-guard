# Adopt existing human sources

[Français](README.fr.md) · [Setup index](../README.md) · [Named local access](../identity/README.md) · [SSO](../sso/README.md)

This operator workflow adopts an existing local tenant union in place: each
tenant Store must already be format13 and the optional authentication audit
must already be Auth4. It prepares private files, runs the existing13→14/4→5
coordinator, then publishes the activation configuration used by the gateway.
The implementation and its tests are candidates; execution of the new bootstrap
tests is **NOT_RUN** until a dated validation report records the actual source
and commands. No production gate is closed by these instructions.

Prepare the named identities and their explicit memberships **before** adoption
using the [identity maintenance guide](../identity/README.md). Preserve the real
tenant history, credential files, installation identity and migration backups.
Use the existing explicit legacy migrations to reach Store13 and, when selected,
[Auth4 with the original policy and exact finite tenant capacity plan](../sso/README.md).
This command does not provision a person or issue a new credential. The registry
still refers to the existing private runner/viewer files, with distinct secrets
of32–512 characters without whitespace after trimming. A secret is never passed
in an argument or printed by this workflow.

Stop **all** consumers of these databases first. `--confirm-control-stopped` is
the operator's attestation; it does not fence an older process. Preparing opens
the real legacy SQLite wrappers and may change SQLite/WAL state. It is not a
read-only diagnostic. Keep source/configuration files in their existing private
directories; new preparation, journal and backup parents must be distinct from
the source parents. Do not move or replace a prepared directory or a bound input.

For an installation without OIDC, use an absolute new output path:

```sh
agentsandbox human-sources prepare \
  --tenant-registry /absolute/control/tenants.json \
  --installation-id 11111111-1111-4111-8111-111111111111 \
  --control-config-revision 7 --max-events 100000 \
  --output-dir /absolute/control/human-preparation \
  --confirm-control-stopped
```

The UUID above is an example: supply the real retained installation identity.
When OIDC is selected, add both `--oidc-config /absolute/control/oidc.json` and
`--oidc-mapping-revision 9`. The private OIDC configuration must be version2,
retain an explicit public CA bundle, and name the exact Auth4 capacity plan for
the configured tenants. The mapping revision is a positive integer distinct
from the provider mapping's own string revision. The public projection includes
the effective policies; credentials and private filesystem identities remain in
the private recovery/seals only. OIDC HTTP routes remain closed in this owned
startup profile, even though the selected Auth5 ledger belongs to the union.

The first durable document, `recovery.json`, records the operation, source,
closure and technical credential UUIDs, generation1, input byte bindings and
physical expectations. Later publications derive from it. The complete private
catalogue is `recovery.json`, `technical-credentials.json`,
`maintenance-plan.json`, `preparation-complete.json`, `activation.json`,
`activation-complete.json`, plus `journal/` and `backups/`. The envelope contains
private credentials; do not publish or attach these files to an issue.

```sh
agentsandbox human-sources check \
  --preparation-dir /absolute/control/human-preparation
agentsandbox human-sources apply \
  --preparation-dir /absolute/control/human-preparation \
  --confirm-control-stopped
agentsandbox gateway \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --jev-backend rust-rules
```

The activation flag is the normal startup route. Do not combine it with direct
source, registry, credential, revision, capacity, initialization or OIDC options.
HTTP binding, server TLS and process/decision settings retain their normal
configuration. The gateway independently reconstructs the expected union and
performs real owner acquisition, SQLite replay, durable floors, incarnation CAS
and source retention on **every** startup. A manifest or publication seal alone
cannot authorize a source or prove an active incarnation.

After an interruption, preserve all sources and artifacts in place:

```sh
agentsandbox human-sources resume-prepare \
  --preparation-dir /absolute/control/human-preparation \
  --confirm-control-stopped
agentsandbox human-sources resume \
  --preparation-dir /absolute/control/human-preparation \
  --confirm-control-stopped
```

Use `resume-prepare` for deterministic file preparation. Missing later files
are created exclusively; complete identical files are revalidated and synced.
A directory with no valid recovery, a changed input (even before the first
envelope), or an existing truncated/divergent document is a conflict. Nothing is
overwritten, repaired, deleted or assigned replacement UUIDs. Reconcile the
preserved evidence explicitly before proceeding. `resume` requires a real
coordinator intention or a complete published activation; it cannot start a new
operation. `apply` may start the persisted operation. Both pass the same plan,
backups, journal and revisions to the coordinator. Partial commits are preserved.

The coordinator rechecks physical/logical/configuration/format expectations and
SQLite integrity under its retained union and writers. Its backup and history
prefix are observed **at apply**. Prepare does not freeze all data changes on
the same inode; a valid change that preserves the recorded expectations can be
examined by the coordinator. No prepared head or legacy fencing is promised.
Temporary startup admission is closed and drained before activation is published.
If the final activation and seal were already published identically, a retry
returns `already_published_files` without another coordinator/startup invocation.
This acknowledgement does not recheck SQLite.

`check` performs private file/metadata validation only. It reads no SQLite main,
WAL, SHM or backup content, acquires no owner and writes/syncs nothing. Its result
is `prepared_files_valid`, `activation_files_valid`, `incomplete` or `conflict`.
These are file statuses, never Ready, database integrity, authentication success
or zero activity. A partial migration/activation requires explicit resume.
Publication is ordered, with each seal last; there is no multi-file or
multi-database atomicity claim, nor a power-loss qualification.

Use a private **human** credential with the [identity/resources commands](../identity/README.md)
after startup; runner/viewer tokens are technical credentials. The available
owned HTTP profile is described in [owned sources](../identity/OWNED_SOURCES.md).
It retains `contexts_complete=false`; available identity/resources reads and
their durable receipts do not qualify the remaining handlers, remote brokers,
cloud, restart after physical power loss or production readiness.

## Validation scope

The [initial W0 implementation record](../../engineering/evidence/2026-11-13/human-owned-startup/README.md)
describes its older source and scope, which lacked this operator producer. The
[earlier source-validation evidence](../../engineering/evidence/2026-11-13/human-source-validation/README.md)
also retains its identified sources and limited results. Neither report
qualifies this bootstrap or transfers its results to the current checkout.
The 55 bootstrap tests and the 76 W0 functional tests remain **NOT_RUN** for
this source until dated execution evidence records otherwise; a W0 preflight
does not execute those functional tests. This guide introduces no new result
for an IdP, network filtering, container runtime, cloud or production gate.
