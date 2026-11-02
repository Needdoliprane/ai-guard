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

An explicit `--demo-defaults` option checks only five public deployment variables against the frozen demo endpoints; generic preparation does not read them. Stdin has a five-second monotonic input deadline under ordinary sole-consumer pipe semantics, in addition to the byte bound. It does not claim a hard deadline against blocked kernel I/O or a foreign concurrent reader.

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

The destination is exclusively new beneath a validated private parent. Persist dedicated private credential material once, then bind its identity/hash in an immutable private intent before any database or membership mutation. The intent also contains the input fingerprint, generated installation/operation identities, intended paths and ownership bindings. Recovery reuses that exact material/IDs, never regenerates or silently adopts another namespace. A crash before a complete retained intent is diagnosis-only; preserve the partial directory. Refuse divergent input, substituted paths or existing unrelated destinations.

Produce this layout (paths in generated files refer to the final destination):

```text
DIR/tenants.json              existing gateway registry version1
DIR/oidc.json                 existing private OIDC file version1
DIR/authentication.sqlite    initialized separate authentication audit
DIR/tenants/<tenant>/audit.sqlite  schema5, existing verified migrations
DIR/tenants/<tenant>/runner   private generated technical credential
DIR/tenants/<tenant>/viewer   private generated technical credential
DIR/preparation.json         retained identities, progress and receipts
DIR/secret-material.json     private credential recovery material, never printed
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

- [x] Write product regressions: new schema5 stores/journal/memberships, exact restart/recovery identity, no implicit admin from master/username, malformed/duplicate manifest, unsafe paths, existing destination, audit failure and substituted recovery material.
- [x] Observe the relevant RED with Luna before implementing behavior. Keep failures at the intended oracle, not broken fixture setup.
- [x] Implement preparation/resume using durable existing APIs and exclusive publication; no unwrap/panic in fallible business paths.
- [x] Verify focused setup tests and necessary migration/human/OIDC canaries; fmt/strict Clippy after composition. Do not replay unrelated E2E/model tests.

### Task2 — usable fresh demo

- [x] Render both Compose profiles and compare actual commands, volumes, identities and networks with this contract.
- [ ] Verify actual fresh realm import subjects and gateway/CLI discovery through a private auxiliary recipe. Do not push Keycloak-specific test tooling.
- [ ] Verify generated configuration and user/admin ceilings with product integration tests; no fabricated activity.
- [x] Document fresh build, DNS prerequisite, preparation, normal start/reopen, failure/resume and existing-realm/enterprise differences in EN/FR. Limitations remain visible.
- [x] Independent review, relevant fixes/retests, commit/push and explicit worktree preservation/inventory/removal. A successful lot does not close the global production gates.

## Evidence and open checks — 2 November

Current follow-up: published source `96fe9be79343267d5283aa3e03fea2b6f70087f8`, image `sha256:72bfb9119fe552c6100ffaeb07cbbd8bd86f9449270b19f3904e97ca2dd8c5c7`, build/rollout RC0. Gateway/dashboard/ingress run healthy as 1000:1000, read-only with CapDropALL/no-new-privileges; the prepared volume, completed setup and IdP are preserved. Real Keycloak Web recipes pass for user-1 (7/7, RC0), user-2 (8/8) and the final admin-ai-guard recipe (13/13, RC0), including authenticated identity, authorized reads and logout. Earlier administrative failures retain their results; only the demonstrated final harness defect was corrected, without a product change. [Dated Web evidence and limits](VALIDATION.md#2-novembre--sso-web-réel-sur-la-page-de-fin-corrigée). Host DNS resolution and exact discovery now pass on the QA macOS workstation after an authorized administrator intervention, independently of the browser mapping. A current native binary and real CLI login/reads, refresh/revocation, adversarial permission refusals and delegation remain unqualified. [Separate DNS evidence](VALIDATION.md#2-novembre--dns-hôte-keycloak-résolu). G09 remains FAIL, 0/12 global gates closed. The following bullets retain earlier evidence at their original source freezes, including the then-blocked host DNS.

- Initial feature RED: one real CLI case failed on the missing `setup` command; no compilation/setup failure. Capture `ai-guard-luna-setup-prep-20261102-yVeiXY`.
- Initial candidate: five CLI integration cases and two library cases PASS, fmt PASS, source hashes stable. Capture `ai-guard-luna-setup-green-20261102-ftUnVn`. This precedes subsequent corrective changes.
- Compose structure/rendering PASS for Docker and Podman configurations; this does not qualify a Podman engine. Missing external identity network fails explicitly. Source realm/public manifest use the same three declared IDs.
- Real Keycloak26.8.1 fresh import PASS with image `74dfae33caf76590f87454e601112cf0eeb5bc28841f3186b1185cbc94ffed78`, no network or published ports. Official CLI reads returned the exact three imported IDs. Evidence `ai-guard-keycloak-import-corrected.XA9hpO`; owned container stopped, volume/image retained. Earlier failed invocation omitted the public Web-origin variable; its intended ID oracle was NOT_RUN, not a demonstrated substitution defect. Web/CLI product interoperability remains NOT_RUN.
- Independent source review found three reliability defects, without a demonstrated security-boundary crossing: unbounded stdin wait, impossible genesis capacity accepted before mutation, and whole-file32MiB synchronization bound on legitimately larger databases.
- Stdin's retained-open real-pipe regression is RED before the corrective five-second poll/read loop. Corrective source review passed; final pipe canaries are still pending. Evidence `ai-guard-luna-stdin-red-20261102-sWoT3T`.
- One-byte genesis budget produced a real RED: a stage directory remained after refusal. Exact shared genesis sizing is now implemented before storage creation; final regression/boundary results pending. Evidence `ai-guard-luna-setup-red2-20261102-dYMeWj`.
- First large-database test failed at a fixture tenant mismatch before the size oracle. The fixture now derives its actual tenant from the manifest; the exact case is running. Do not count that first failure as a product RED or change the file-size limit without reaching the intended case.
- Genesis correction follow-up: the exact serialized-capacity boundary and the before-staging one-byte-budget case each pass1/1 on the shared builder. Independent source review approves its unchanged persistent format and redaction checks.
- A second large-store preparation failed its size precondition: each single128000-byte string was truncated by the real sanitizer's32768-byte field bound. It did not exercise resume. The corrected fixture uses four distinct31500-byte strings per event, still below the event bound, and retains real Store appends and explicit physical-size checks. This case is being rerun; no product size-cap change yet.
- Corrected real large-store case reached its intended RED:270normal appends, all Store owners closed, main database actually>32MiB, then resume refused at `persistent file exceeds its bound`. Runtime60.86s,1test, RC101; capture `ai-guard-luna-setup-largered-r2-20261102-JdomWN`.
- Descriptor-only synchronization is implemented and independently source-reviewed: private-entry/named-inode checks, nofollow retained descriptor, UID/mode/ACL/link/filesystem checks before/after fsync, no payload reads. Its sole product caller runs after every SQLite owner is dropped. Existing read limits and directory/publication barriers remain.
- Final Compose image-selection renders pass: the new IdP tag is selectable, legacy fallback remains, both control image selections match. Podman YAML was parsed by Docker Compose; a Podman engine is still NOT_RUN. Capture `ai-guard-luna-compose-render-20261102-hrwV5D`.
- Composed integration executable outputs show8/8setup and24/24authentication tests PASS with source hashes stable; the original tool session lost its final numeric Cargo exit, which is **NOT_RECORDED**, not fabricated as0. Capture `ai-guard-luna-setup-final-20261102-Rnt38Y`. No duplicate run was started while its recovered Cargo PID remained active.
- A subsequent unit compilation overlapped a Root-coordination error: a private local variable was renamed `path`→`_path` to remove a disabled-metrics warning before the asynchronous launch report was received. This is a known semantic-preserving delta, but that compilation is not counted as a frozen final-source proof. It must finish naturally; then setup units, descriptor canaries and strict Clippy run from the frozen final source. Final `persistent.rs` SHA `59ddf6b0794fe48bbca379993bda4f5c66266dbb361c20459fcfd4af7b4b6dc8`; storage SHA `87dca92a72a92abe97c820c5794ae60d161327bb5683636eea3beaeb58234532`.
- Final-source setup units PASS4/4, RC0, including the real>32MiB double-resume case;124.85s. Before/after source manifests are identical SHA `2e014ebfbaca991705f50398e692ce2d7edd1d586fda4e20c71b8158e399ebc1` in the same final capture. This supersedes the overlapping unit compilation as final-source proof.
- The two new descriptor canaries FAIL before their intended helper oracle: the default temporary path contains a macOS filesystem alias rejected by `Directory::open_existing`. This is a fixture precondition failure, not a reproduced helper defect. Canonicalize only the temporary parent, retain the production no-symlink policy, then rerun these two cases without repeating the4/32 unchanged behavioral tests.
- Corrected descriptor fixtures PASS2/2, RC0,0.07s; final file SHA `db9be765c45f3eb2930f021d9a7d5b4a7214abc69354bb1cd0f8a5a591c4812a`. Before/after source manifests are identical SHA `439f4cba11b5d0dacb61c80aecb343a1a91281faf13d5e0046f249f79de5d7e0`. The production helper bytes are unchanged from the reviewed final source; the only new delta canonicalizes the two test temporary parents. Independent review approves both that fixture correction and the earlier local-variable rename.
- Independent launch preflight found no blocking flag/path/topology mismatch; EN/FR now explicitly build the matching native host CLI, distinct from the Linux image binary. Hostname resolution and port18080 availability must be resolved before the fresh host SSO recipe; preserve the existing older IdP stack and its data.
- Strict Clippy `cargo clippy --locked --offline --all-targets -- -D warnings` PASS, RC0,2min47; final fmt check PASS, RC0. Final Clippy manifests share SHA `439f4cba11b5d0dacb61c80aecb343a1a91281faf13d5e0046f249f79de5d7e0`. Rust/application sources stayed frozen. The original G09 release application/test hashes remain unchanged.
- Relative documentation links PASS100/100 across the six touched guides/plan; independent source and launch/configuration reviews have no remaining blocking finding in this lot. The publication scan found no known literal secret or nonfixture secret pattern. Its `.env` filename match is the explicitly reviewed31-line public example; its private-key marker is a deliberately invalid refusal fixture, not key material.
- Actual prepared-volume/gateway/Web/CLI qualification remains open. Host DNS currently lacks the demo name, and administrator authentication is required to add it; no password requested or OS trust/DNS change performed by the agent. Existing owned IdP ingress can be stopped reversibly after the recorded resource inventory, preserving every container, volume and image. No production gate is closed by this lot.
- Publication/integration complete: source7a08d18 and integration/docs d343848 remote heads verified exactly. Clean SSO worktree, ancestor check and absence of open consumers verified; private committed archive SHA `14ea3e62e76842039f4c0b399035da51a0619c82e314d1cc735d0722d88167e7`, then normal worktree removal RC0. Local/remote branch, main changes, retained guardians, data and shared Cargo target preserved.
- Follow-up runtime proof: control image6040058a built from published d343848, fresh volume preparation RC0 with actual1000:1000/700/600 metadata, then normal gateway/dashboard/ingress startup and public status HTTP200. Private QA CIDR overrides declared; no standard address-pool or worker qualification. `compose run --no-build` was rejected before any preparation; local help confirmed its removal, and four EN/FR prepare/resume commands are corrected. `up --no-build` is retained. The first live browser scenario fails at the public entry-page check before login; user flow and native CLI remain open. [Current runtime evidence](VALIDATION.md#2-novembre--installation-conteneurisée-sso-neuve).
