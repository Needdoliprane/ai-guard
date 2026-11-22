# Adopt existing human sources

[Français](README.fr.md) · [Setup index](../README.md) · [Named local access](../identity/README.md) · [SSO](../sso/README.md)

This operator workflow adopts an existing local tenant union in place: each
tenant Store must already be format13 and the optional authentication audit
must already be Auth4. It prepares private files, runs the existing13→14/4→5
coordinator, then publishes the activation configuration used by the gateway.
The [dated software validation](../../engineering/evidence/2026-11-13/human-sources-operator/README.md)
covers this workflow and its adopted HTTP profile on a frozen Linux ARM64
source. Other deployment profiles and production qualification remain open.

Prepare the named identities and their explicit memberships **before** adoption
using the [identity maintenance guide](../identity/README.md). Preserve the real
tenant history, credential files, installation identity and migration backups.
Use the existing explicit legacy migrations to reach Store13 and, when selected,
[Auth4 with the original policy and exact finite tenant capacity plan](../sso/README.md).
This command does not provision a person or issue a new credential. The registry
still refers to the existing private runner/viewer files, with distinct secrets
of 32–512 UTF-8 bytes without whitespace after trimming. A secret is never passed
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
When other tenant sources are skipped, `contexts_complete=false` means their
memberships remain unknown; a single-tenant union can report complete contexts.
Available identity/resources reads and their durable receipts do not qualify
the remaining handlers, remote brokers, cloud, restart after physical power
loss or production readiness.

## Prepare a tenant for the egress policy registry

The SQLite maintenance command upgrades one Store14 in an adopted owned
installation. Stop the gateway and every consumer first. Keep the prior
activation and select an absolute new output directory. Use a location outside
the source directories and the directories holding the prior manifest and
technical envelope, including the initial activation directory. Aliases of
those directories are subject to the same check. Generate a new operation UUID
and retain it with the command.

```sh
agentsandbox human-sources upgrade-egress \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --tenant tenant-a --operation-id 22222222-2222-4222-8222-222222222222 \
  --output-dir /absolute/control/egress-tenant-a --confirm-control-stopped
```

Replace the example tenant, UUID and paths with those of the installation. For
an installation already using explicit references, supply the current
`--human-sources-manifest`, `--human-credentials-envelope`, `--tenants-file` and
`--human-control-config-revision` in place of the initial activation. Reuse the
current `--max-events` if customized and the existing `--oidc-config` and
`--human-oidc-mapping-revision` if selected.

The command saves a verified copy in `backup/source.sqlite` and confirms the
committed deny-all egress genesis before publishing `closure/manifest.json`.
On `store15_closure_published`, select that manifest and
`technical-credentials.json` from the same output directory in the gateway's
explicit `owned_sources` configuration. Keep the other configuration inputs
and revisions. The previous manifest has become stale for the migrated source.
Use the latest published closure to prepare the next tenant. The registry starts
with no allowed destinations. Runtime enforcement has its own qualification.

Keep all files after an interrupted run. Run the same command with `--resume`,
the same prior activation or manifest, operation UUID and directory after
verifying that the controls are stopped. An existing closure must match the
confirmed genesis. If the final output directory was never published, preserve
its staging sibling and begin in a fresh directory after checking offline that
the selected source remains Store14. A backup with no recorded receipt requires
reconciliation before retrying. Preserve that copy and the journal for diagnosis.

- `store15_output_protected` refuses a location overlapping the retained
  layout. Choose a new directory outside it.
- `store15_output_not_fresh` requires an available new destination. Preserve
  existing files and use `--resume` only for the same prepared operation.
- `store15_control_active` reports owner contention before a SQLite
  transition. Retry after the controls have stopped. Use `--resume` if the same
  preparation was already published.
- `store15_preparation_publication_unconfirmed` leaves preparation files to
  inspect before resuming. `store15_transition_unconfirmed` can follow a durable
  SQLite commit. Preserve every file and resume the same operation to confirm.

The command reports publication and leaves gateway readiness to startup.

## In-place identity markers on adopted stores

After adoption, the fenced human rights capabilities need the identity marker
`human_rights_v2` on each tenant Store14/15. With every consumer stopped, run
the offline in-place maintenance once per tenant, each with a new private
maintenance directory outside the preparation and source directories. The
command checks the actual directory identities, including alternate names
resolved by the filesystem.

```sh
agentsandbox human-sources human-rights install \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --tenant-id tenant-a \
  --maintenance-dir /absolute/control/human-rights-tenant-a \
  --confirm-control-stopped
agentsandbox human-sources human-rights resume \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --tenant-id tenant-a \
  --maintenance-dir /absolute/control/human-rights-tenant-a \
  --confirm-control-stopped
```

For an already published explicit closure, use its manifest, technical
credential envelope, tenant registry and control revision together. Supply
`--max-events` if the closure uses a value other than `100000`. For OIDC, also
supply `--oidc-config` and `--human-oidc-mapping-revision` with the same values
used at publication. Use private directories with mode `0700` for the closure,
its configuration and the runner and viewer token files. Keep the maintenance
directory outside those directories and every source directory.

```sh
agentsandbox human-sources human-rights install \
  --human-sources-manifest /absolute/control/published/closure.json \
  --human-credentials-envelope /absolute/control/published/technical-credentials.json \
  --tenants-file /absolute/control/config/tenants.json \
  --human-control-config-revision 7 \
  --tenant-id tenant-a \
  --maintenance-dir /absolute/control/human-rights-tenant-a \
  --confirm-control-stopped
```

For `resume`, reuse the same explicit arguments and maintenance directory.
`human_rights_closure_refused` means the explicit closure or one of its inputs
could not be verified. Preserve the files and verify that the manifest,
envelope, configured tokens and revisions match the same publication.

`install` takes the owner locks of the whole union, writes a verified SQLite
backup, journals its intent, backup and confirmation, and appends every pending
marker in one transaction. The maintenance plan identifies the selected
source. `resume` needs the same tenant and resynchronizes the plan and its
directories before opening the store. It can finish or re-confirm that
maintenance without appending a second marker. A plan created by a test build
before source binding is refused with `human_rights_journal_conflict`. Preserve
its plan, journal and backup for review. The
activation and store formats are unchanged. The refusal codes, the return path
and the capabilities these markers admit are described in
[named local access](../identity/README.md#already-adopted-owned-stores-formats-14-and-15).

After an interruption, `resume` preserves copies that have no published backup
receipt. If the original backup location is occupied, it writes a new verified
copy and publishes its receipt under the same intention. Once that receipt is
published, every retry uses its copy and reports the same `backup_path`. A
missing or divergent copy for a published receipt stops the maintenance.

`human_rights_backup_inventory_limit` means `backups/` contains more than 27
entries before creating a copy or more than 32 entries when verifying journal
records. The creation limit reserves five entries for the SQLite file, its
sidecars and the initialization lock. The count includes every direct entry
under `backups/`, including retained copies and SQLite support files.

`human_rights_backup_directory_invalid` means `backups/` or an inventory entry
could not be verified as private. The refusal line reports the state without
printing private entry names. For either refusal, keep the maintenance directory
and sources intact for offline operator review. Verify the identity, owner and
permissions of the original directory and the bound above. Resume with the same
tenant, source arguments and maintenance directory once those checks can pass.
If the limit still prevents resume, retain the whole directory for review with
the maintainer before starting another maintenance.

## Validation scope

The [dated operator and owned-startup evidence](../../engineering/evidence/2026-11-13/human-sources-operator/README.md)
records **107 unique named functions passing on the same frozen source**, in
**two separate campaigns: 34 then 73**, with no failed or ignored function or
accepted omission. This is not one run of 107 tests. The first campaign's
all-targets check passes; the second explicitly reuses that exact-source result
without repeating the check. Each campaign compiles the selected test artifacts
and checks the normal CLI help. The evidence records actual registration,
results, independent review, source preservation and resource closure.

The [initial W0 implementation record](../../engineering/evidence/2026-11-13/human-owned-startup/README.md)
retains its historical source and implementation-time `NOT_RUN` statement; its
appendix links the later validation. The
[earlier source-validation evidence](../../engineering/evidence/2026-11-13/human-source-validation/README.md)
also keeps its own sources and limited results. These histories are not
rewritten or transferred to another checkout.

The validation covers isolated Linux ARM64 software with real private SQLite
fixtures and local HTTP/CLI behavior. It does not qualify physical power loss,
native macOS, business workers or their Docker/Podman/Kubernetes adapters, a real
IdP or provider, frontend/browser behavior, a full test suite, strict Clippy, a
release or 100 workers. The owned profile deliberately adopts only identity and
resources GETs; selected OIDC stays retained with its known handlers closed.
Human approval/launch, egress control and modern identity provisioning remain
outside this profile. No production gate or audit finding is closed.
