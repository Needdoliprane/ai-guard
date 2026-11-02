# Local SSO installation — design and implementation plan

Goal: prepare a new local installation that can authenticate the imported demo users through the existing Web/CLI OIDC clients. This implements the maintainer's requested ready-to-use local configuration. It does not qualify Keycloak or complete human execution delegation by itself.

Base: `d6cb136b0309d2c231ca129d4a103416af3f1fee`. Worktree: `codex/sso-setup`. The main checkout's changes and the runtime qualification checkout are outside this worktree's scope. No automatic baseline Cargo build: the same commit has its targeted tests/static checks, and the runtime attempt owns the execution lease.

## Decisions within the approved architecture

- Keep the gateway as OIDC/PKCE owner, dashboard as its fixed authentication proxy, SQLite per tenant and a separate private authentication journal. No new service or authority.
- Add a Rust offline preparation CLI. Run it as the actual control UID in a new private volume; never require a host-owned0600 OIDC file to be readable by container UID1000.
- A manual subject manifest alone would leave the fresh demo's installation incomplete. Assign explicit stable IDs to the three application users in the fresh realm import and provide the corresponding public preparation manifest. These are declared demo identities, not identities inferred from usernames. Verify import behavior on a real fresh Keycloak before claiming it works. The master administrator gets no application membership.
- Existing realms are never reset, migrated or adopted implicitly. Enterprise/existing installations provide an explicitly reviewed manifest with their actual verified issuer/subjects. Preserve that distinction in both languages.
- Keep the isolated HTTP demo issuer exactly `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo`; reject incompatible advertised overrides. Browser/CLI hostname resolution is an explicit prerequisite. Do not edit OS DNS/trust or bypass TLS as part of this implementation.
- Only the gateway joins the explicitly selected identity internal network. Dashboard stays on control; ingress remains the only publication relay. Workers join neither network.

## Rust contract

Add `agentsandbox setup prepare --input FILE --output-dir DIR` and `agentsandbox setup resume --recovery-dir DIR --confirm-control-stopped`. `FILE=-` accepts bounded stdin, allowing public demo JSON to enter the preparation container without a filesystem-owner exception. Preparation never contacts an IdP, engine, model or cloud.

Use a closed version1 input, with concrete public Rust types:

```text
version
gateway: existing OidcGatewayConfigV1
authentication_policy: existing AuthenticationAuditPolicyV1
ca_bundle_pem: optional public certificate-only bundle
tenants: [{tenant_id, organization_id, worker_quota: existing QuotaPolicyV1,
  identities: [{principal: existing PrincipalRefV1,
                display_name, capabilities: existing HumanCapabilityV1[]}]}]
```

Only HumanOidc principals matching the configured issuer are accepted in this command. Validate configuration, approved client policy, tenant/organization identifiers, capabilities, duplicates and all identities before filesystem mutation. Bound input to262144bytes; other existing type bounds remain applicable. No raw secrets in input/output/logs. A configured role ceiling never creates a membership.

The optional CA bundle uses the existing provider limits and replaces trust roots only as already implemented. Reject private-key blocks and non-certificate material before persisting it as private `ca-bundle.pem`; it is not a private CA key or an OS trust installation. The generated OIDC file refers to it only when explicitly supplied.

The destination is exclusively new beneath a validated private parent. Retain an immutable, private recovery intent before creating credentials/databases. It contains the input fingerprint, generated installation/operation identities, intended paths and ownership bindings. Persist credentials once; recovery reuses the exact retained material/IDs, never regenerates or silently adopts another namespace. Refuse divergent input, substituted paths or existing unrelated destinations.

Produce this layout (paths in generated files refer to the final destination):

```text
DIR/tenants.json              existing gateway registry version1
DIR/oidc.json                 existing private OIDC file version1
DIR/authentication.sqlite    initialized separate authentication audit
DIR/tenants/<tenant>/audit.sqlite  schema5, existing verified migrations
DIR/tenants/<tenant>/runner   private generated technical credential
DIR/tenants/<tenant>/viewer   private generated technical credential
DIR/preparation.json         retained identities, progress and receipts
DIR/completion.json          completed preparation evidence
```

Use existing control filesystem capabilities, migration/audit and identity mutation APIs. Do not add a schema or change default migration behavior. Record account and membership mutations with a derived trusted offline actor and persistent operation IDs. No manufactured resources, grants or historical initiator attribution. All private files0600/directories0700, actual control UID.

Publish the completed directory only after all durable steps/receipts succeed, using existing exclusive publication primitives. Preserve staging/recovery material on failure, including a post-publication sync failure. Normal gateway startup reopens both prepared stores and authentication audit, without `--initialize-oidc-audit` or repeated mutations. Restart does not enroll users again and loses live sessions explicitly.

## Configuration contract

Add opt-in Docker and Podman local SSO Compose entry points and one public demo preparation JSON. Do not require the legacy base Compose's host secret interpolation when the prepared private volume is used. A self-contained SSO Compose file is acceptable where merge semantics would otherwise require unrelated host secrets; it must preserve the existing control/ingress restrictions.

Preparation is an explicit one-shot profile/command; ordinary `up` reuses the completed volume. Gateway reads `/data/installation/tenants.json` and `/data/installation/oidc.json`. Dashboard uses shared tenants and the exact configured OIDC Web origin. Pin/select the actual identity-network name, never guess another project's generated name. No engine socket, original-project mount, credentials in ingress, or network access from preparation.

## Ownership and verification

- Rust owner: new `src/setup.rs` and adjacent focused modules, `src/main.rs` CLI registration, minimal visibility changes to existing helpers if required, `tests/setup_prepare.rs`. No deployment/frontend edits.
- Configuration owner: `deploy/docker/compose.sso.yaml`, `deploy/podman/compose.sso.yaml`, demo realm IDs, `docs/setup/sso/examples/local-demo.json`, `.env.exemple.local-keycloak-sso`. No Rust or other documentation edits.
- Orchestrator: contract coherence, final integration, EN/FR guides and verification evidence. Initial profile JSON is the exact shared DTO above; coordinate proposed changes before editing either side.
- Luna: all builds/tests/Compose rendering and actual private interoperability recipes. One execution lease; no test/build during the active G09 attempt.
- Independent reviewer: private filesystem/publication/recovery, audit/identity authority, tenant isolation and network mounts.

### Task1 — preparation and recovery

- [ ] Write product regressions: new schema5 stores/journal/memberships, exact restart/recovery identity, no implicit admin from master/username, malformed/duplicate manifest, unsafe paths, existing destination, audit failure and substituted recovery material.
- [ ] Observe the relevant RED with Luna before implementing behavior. Keep failures at the intended oracle, not broken fixture setup.
- [ ] Implement preparation/resume using durable existing APIs and exclusive publication; no unwrap/panic in fallible business paths.
- [ ] Verify focused setup tests and necessary migration/human/OIDC canaries; fmt/strict Clippy after composition. Do not replay unrelated E2E/model tests.

### Task2 — usable fresh demo

- [ ] Render both Compose profiles and compare actual commands, volumes, identities and networks with this contract.
- [ ] Verify actual fresh realm import subjects and gateway/CLI discovery through a private auxiliary recipe. Do not push Keycloak-specific test tooling.
- [ ] Verify generated configuration and user/admin ceilings with product integration tests; no fabricated activity.
- [ ] Document fresh build, DNS prerequisite, preparation, normal start/reopen, failure/resume and existing-realm/enterprise differences in EN/FR. Limitations remain visible.
- [ ] Independent review, relevant fixes/retests, commit/push and explicit worktree preservation/inventory/removal. A successful lot does not close the global production gates.
