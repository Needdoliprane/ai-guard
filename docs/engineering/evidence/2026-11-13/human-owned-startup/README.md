# Owned human startup candidate

Base: `6cc6b3a25f4dd1fea4ead3180cde1d58fb912001`. This change implements the reviewed W0 R2 vertical: real complete source startup, bounded candidate discovery, own-source local identity/resources, explicit closure of other modern routes, and server ownership through accepted/queued work and cleanup.

New regression sources are `api/human_sources/tests.rs`, `audit/human_modern/tests.rs`, `human_authority/startup/tests.rs` and the public configuration digest tests under `gateway_config/human_sources/`. They use actual private SQLite fixtures migrated by the existing coordinator; no UUID, candidate or readiness flag replaces a source proof. The A/B tests retain A's mutex or `BEGIN IMMEDIATE` through B's authenticated identity, opaque context and independently observed durable resource receipt/floor. Neighbor Store observations must remain empty. Own Busy and timeout restoration are separate tests.

Execution status: **NOT_RUN**. No Cargo/rustc/project binary, container engine, provider, IdP or cluster was executed by this implementation agent. Targeted trusted rustfmt and `git diff --check` are static checks only. Compilation, software assertions, independent review, Docker/Podman/Kubernetes and production qualification remain to be established on an identified frozen source. No historical PASS is transferred and no audit fingerprint or G01–G12 gate is closed.

The existing legacy modes and Full3 dispatch are retained. The new mode does not implement offline manifest/envelope provisioning, fresh cross-source membership discovery, modern OIDC sessions/revocation, human approval/launch, egress permission or network filtering. The paired [operator guide](../../../../setup/identity/OWNED_SOURCES.md) explains actual flags and refusal/recovery behavior. Source/design review and eventual execution evidence must remain distinct.

## Later software validation — 13 November 2026

The paragraphs above retain the original implementation source, agent activity
and `NOT_RUN` status. The later [operator and owned-startup validation](../human-sources-operator/README.md)
records 107 unique named functions passing on frozen source
`dbee1d5f90abd884badf5cd255b69c436f38ae4a`, in two campaigns of 34 and 73,
with independent result review. It includes the operator producer added after
this W0 record. Its source, build results, preserved harness failures and limits
are identified in that report and [RESULTS.json](../human-sources-operator/RESULTS.json).
This addition does not rewrite the implementation-time evidence or qualify
another checkout, physical power loss, a business worker/runtime, IdP, provider,
frontend, release or production gate. The deliberate two-GET profile and its
remaining mandatory work stay explicit in the current operator guides.
