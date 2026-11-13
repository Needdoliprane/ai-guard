# Kubernetes V2 session runtime (LOT A) — source handoff, 12 November 2026

**Nothing in this record has been compiled or executed.** The isolated QA
environment was not admitted for this lot, so every test listed below is
`NOT_RUN`, and so are `cargo check`, `clippy` and `cargo fmt`. The only checks
performed are static and listed under "Static checks": `rustfmt --check`,
`git diff --check`, `git apply --check` of the proposed patches and a
`git grep` review of the effect-guard implementors. This record was written
before any QA run and **must be revised after the first admitted run**. It
qualifies no Kubernetes behaviour, closes no gate and closes no risk. The
product is not production-ready.

Handoff update, 2026-11-12T23:28Z: the static checks were rerun on the code
head `2b1ddcb4ab1280c43a24139614350e8c0526cc30` (section "Static checks"), the
QA proposal revision 2 and the proposed-wire inventory were prepared for the
integrator, and `QA_ADMISSION.json` was still absent, so nothing ran.

User guide: [English](../../../../setup/kubernetes-runtime/README.md) ·
[Français](../../../../setup/kubernetes-runtime/README.fr.md). Contract:
[Kubernetes admission](../../../KUBERNETES_ADMISSION.md).

## Source identity

| Item | Value |
|---|---|
| Branch | `claude/kubernetes-v2-runtime-20261112` |
| Base | `77f7edd6da9a2895a1277754c6144f893bcb8aea` |
| S1 | `405e46eb697e19763075745a32c862702a4ab3b2` exact V2 request, per-attempt workspace claim, journal inventory r2 |
| S2a | `ed77f7e0850e061845f5f4ab8d93bdf4a7ff0214` precondition-bound effect transport behind a sealed guard |
| S1 gate fix | `86aca2ab0d56c8d129eefa9762e091dc4397f314` never flock a journal lock whose preparation is absent (gate review B1) |
| S3 code | `79c4fc1e7ff915b9270086bc3acfdaf657b6a9f0` before-effect lifecycle, successor recovery, observe-only report |
| S3 guide | `10593e0447c57c7412be8e3b89b410406c086b45` EN/FR guide and Docker Desktop plan |
| This record | `1c03d2b2c25a71aee8673c913f2d3285cb16fe2a`, on top of `10593e0` |
| Gate repair (S2a N1/N7) | `7ab5276859fd1bff1b8b589477120b3e79b143a3` transport cancellation check, connection-counting test peer, `allow` on the transport module |
| Gate repair (S3 N2–N4, N6–N8) | `0e9b48bc9948853c82424d444f43820add066e4c` report `NoRequestRetained` and terminating claim, explicit last revalidation, predecessor-close replay test |
| Gate repair (S1 N4/N6, S3 N7) | `218a6a3026d287785f4dbf39aaeaf3f14b38646a` journal tamper and abandonment idempotence tests, binding doc |
| Gate repair (S1 N1) | `c273d0815b86165ad9fbba65d08daae172176ae6` `expect(dead_code)` outside tests on `bootstrap.rs` |
| Gate repair (docs) | `cc2421885f0ffe40a71feefb8c3481ffb0cb1c0a` EN/FR guide and this record (GATE_S3 N1, N3, N4, N9, N10; GATE_S1 N9) |
| Final review repair (code) | `f0f2ad5f089750fa1676ecc71a06cc5f79ed9a77` journal exclusion regression, transport cancellation isolated by a test, instance constructor narrowed to `runtime::kubernetes`, report `names` omitted when not inspected, effect outcome and recovery policy docs |
| Final review repair (docs) | `2b1ddcb4ab1280c43a24139614350e8c0526cc30` EN/FR guide and this record (recovery policy is trusted input, plan labels, network wording, claim inspection errors) |
| Code head | `2b1ddcb4ab1280c43a24139614350e8c0526cc30`, tree `cde1ee55b392c619df592bfce3fe46a85c4ae9cb`, `git archive --format=tar` SHA-256 `39af1307e068368fbbecce80731d3dfadcfe1b8f710e3e26a6afb9ece769037f`, 2615 tracked files |
| This handoff record | the `docs(evidence)` commit that adds this update on top of `2b1ddcb`; it changes only this file, and no source, test or build input reads it |
| SHA proposed for QA | `2b1ddcb4ab1280c43a24139614350e8c0526cc30`. Predicted harness freeze line, computed natively from the same archive (to confirm by the harness): `files=2615 manifest=631d6a8e83132afc13c8af4b0ce19777b5499e77d23c12ae23228d19da66f69f` |
| Frozen QA source | none: no QA run was admitted. A QA run must name a committed SHA of this branch. |
| SHA actually tested | none (`NOT_RUN`) |

No shared or Root-owned file is changed by these commits: no `Cargo.toml`,
`Cargo.lock`, `src/main.rs`, `src/lib.rs`, `src/api*`, `src/audit*`,
`src/control/**`, `src/kubernetes_cli.rs`, `src/bin/**`, `images/**`, setup
index or governance document. Changes to such files are proposed patches,
listed below and not applied.

## What the branch contains

Layers follow the lot design (section 0):
- **A. Library entry with real inputs, no product caller:**
  `KubernetesRuntime::inspect_session_workspace_claim(name)` and the public
  descriptive `KubernetesSessionWorkspaceClaimV2`.
- **B. Pure crate-internal mechanics:** exact V2 Pod request renderer, admitted
  201 projection, exact observation parser, journal record formats.
- **C. Lifecycle mechanics without a production caller (missing seam R2):**
  journal inventory r2 with `abandoned-v2.json`, observe-only reopen,
  `retain_session_request`, `abandon_before_effect`, `resume_abandonment`
  (same incarnation and successor recovery through the existing Store API) and
  `inspect_session_report`.
- **D. Closed effect mechanics:** held CREATE, gate-release PATCH, DELETE with a
  UID precondition and exact observation, reachable only through a sealed guard
  whose only implementation is `cfg(test)`.
- **Not implemented:** S2b (preamble-only attach and the worker waiting mode,
  blocked on Root Q1/Q6) and S5 (effect permits, blocked on R1/R3/R7).

Operational for an operator today: only the existing Runner `control-recovery`
route and `agentsandbox kubernetes doctor`, both unchanged by this branch.

## Operational code, missing seams, missing qualification

| Category | Content | Source |
|---|---|---|
| Operational code (product callers exist) | None added by this branch. Existing and unchanged: Runner `control-recovery`, `agentsandbox kubernetes doctor`. | base `77f7edd` |
| Code without a product caller | Layer A public entry `inspect_session_workspace_claim` (`src/runtime/kubernetes/session_instance.rs:617-642`); layers B and C crate-internal; layer D behind the sealed guard (`session_effect.rs:38`, `:47`), whose only implementor is `cfg(test)` (`session_effect.rs:221`, `:223`) | `2b1ddcb` |
| Missing seams (Root) | R1 central V2 effect states; R2 broker routes (production caller of layer C, recovery policy taken from the tenant gateway configuration); R3 armed deadline; R4 frames after Ready; R5 storage policy; R7 activation; R10 tenant authority disjointness; R11 successor confirmation; proposed patches 0001–0004 not applied | CONTRACT §4, §5 and addenda |
| Not implemented in this run | S2b (preamble-only attach, worker waiting mode; Q1, Q6) and S5 (effect permits; R1, R3, R7) | DESIGN §3.6, §3.8 |
| Missing qualification | Every test, `cargo check`, `clippy` and `cargo fmt` of this branch (`NOT_RUN`); any real Kubernetes, Docker Desktop, NetworkPolicy, CSI or cgroup behaviour | sections below |

## Static checks (performed, native, no code executed)

| Check | Command form | Result |
|---|---|---|
| Formatting of every owned Rust file touched by S3 | `rustfmt +1.99.1 --edition 2024 --check <file>` on `session.rs`, `session_disk.rs`, `session_tests.rs`, `session_lifecycle.rs`, `session_lifecycle_tests.rs`; and on `src/runtime/kubernetes.rs`, which follows the whole owned `runtime/kubernetes` module tree | rc 0 each |
| Formatting after the gate repair | `rustfmt --edition 2024 --check <file>` (1.10.0-stable) on `session_transport.rs`, `session_test_peer.rs`, `session_effect.rs`, `session_lifecycle.rs`, `session.rs`, `session_pod.rs`, `bootstrap.rs` (each follows its own test modules) and on `src/runtime/kubernetes.rs` | rc 0 each |
| Formatting after the final review repair | `rustfmt --edition 2024 --check <file>` (1.10.0-stable) on `src/runtime/kubernetes.rs` (whole owned module tree), on each changed file (`session_effect.rs`, `session_effect_tests.rs`, `session_instance.rs`, `session_lifecycle.rs`, `session_lifecycle_tests.rs`, `session_tests.rs`, `session.rs`, `session_disk.rs`) and on `tests/kubernetes_session_runtime.rs`; a badly formatted probe file returned rc 1 | rc 0 each |
| Whitespace of the lot diff | `git diff --check 77f7edd6da9a2895a1277754c6144f893bcb8aea..HEAD` | rc 0 |
| Proposed patches against the base text | `git apply --check <patch>` in the lot worktree, where each target file is unchanged from the base | OK for 0001–0004 |
| Effect-guard implementors (delivery review check, rerun before integration) | `git grep -n 'SessionEffectGuardV2 for\|Sealed for'` | two lines, `session_effect.rs:221` and `:223` since `f0f2ad5` (`:200`, `:202` before), both under `#[cfg(test)]` (the loopback guard, including its `ignoring_cancellation` constructor); no other implementor |

Handoff rerun on `2b1ddcb4ab1280c43a24139614350e8c0526cc30` (2026-11-12T23:17Z,
worktree clean at that SHA, `rustfmt 1.10.0-stable (58cf8f9c35 2026-10-13)`):

| Check | Exact command | Result |
|---|---|---|
| Formatting, every changed owned Rust file (21) | `rustfmt --edition 2024 --check <file>` for each of `src/runtime/kubernetes.rs`, `src/runtime/kubernetes/{admission,admission_profile,admission_tests,bootstrap,observation,session,session_disk,session_effect,session_effect_tests,session_instance,session_instance_tests,session_lifecycle,session_lifecycle_tests,session_pod,session_pod_tests,session_test_peer,session_tests,session_transport,transport}.rs` and `tests/kubernetes_session_runtime.rs` | rc 0 for each of the 21 files; no write mode |
| Whitespace | `git diff --check 77f7edd6da9a2895a1277754c6144f893bcb8aea 2b1ddcb4ab1280c43a24139614350e8c0526cc30` | rc 0, no output |
| Perimeter | `git diff --name-status 77f7edd6da9a2895a1277754c6144f893bcb8aea 2b1ddcb4ab1280c43a24139614350e8c0526cc30` | 24 paths (14 added, 10 modified; +8221/−92), all in the owned perimeter: `src/runtime/kubernetes/**`, `src/runtime/kubernetes.rs` (own module wiring, the `pub use` of `KubernetesSessionWorkspaceClaimV2` and the module doc), `tests/kubernetes_session_runtime.rs`, the two guide files and this record. Nothing outside it. |
| Proposed-wire targets untouched | `git diff --quiet 77f7edd… 2b1ddcb… -- src/bin/ai-guard-provider.rs images/worker.Dockerfile docs/setup/README.md docs/setup/README.fr.md src/kubernetes_cli.rs` | rc 0 |
| Secrets, host paths, `.env` | grep of the added lines of the base..head diff for private-key blocks, provider/cloud token prefixes, JWT shapes, long quoted literals, `/Users/`, `/home/<user>`, `/private/`, `/var/folders`, the coordination folder name and `.env` | no secret: only synthetic fixture tokens named `public-session-…-fixture/loopback/stranger/successor-token-…`; no host path: only the container paths `/tmp` and `/home/agent` in Pod spec fixtures; no `.env` reference |
| Effect-guard implementors | `git grep -n 'SessionEffectGuardV2 for\|Sealed for' 2b1ddcb… -- src tests` | `session_effect.rs:221` and `:223`, both under `#[cfg(test)]`; no other implementor |
| Proposed patches | `git apply --check <patch>` in the lot worktree; post-images computed in a private scratch directory outside any repository | rc 0 for 0001–0004; post-image SHA-256 equal to each patch header |

These are static. They do not replace `cargo check`, `clippy` or any test.

Lint attributes outside tests: the V2 modules carry
`cfg_attr(not(test), expect(dead_code, reason))`, except `session_transport.rs`,
which carries `allow`. Its only item is called by the effect functions of
`session_effect.rs`, which are dead outside tests and expected; whether rustc
then reports that callee as dead was not verified without a compiler, and an
unfulfilled expectation would fail `clippy -D warnings`. `bootstrap.rs`
(base items without a non-test caller) received the same `expect`. Whether the
rest of the crate is clippy-clean at base `77f7edd` is unknown until an
admitted run.

## Tests: all NOT_RUN

Expected counts are exact; a count of 0 or any other count is `FAIL`. Command
forms for the admitted profile (`--frozen --offline`, serial):
`cargo test --frozen --offline -j 1 --lib <filter> -- --test-threads=1` and
`cargo test --frozen --offline --test <target> -- --test-threads=1`.
Integration-test names are top-level and carry no crate prefix.

| Selector | Expected | Status |
|---|---|---|
| `--lib runtime::kubernetes::session_lifecycle::tests::` (9 S3 + 1 gate repair) | 10 passed | `NOT_RUN` |
| `--lib runtime::kubernetes::session::tests::` (6 base + 6 S1 + 1 gate fix B1 + 2 gate repair + 1 final review repair) | 16 passed | `NOT_RUN` |
| `--lib runtime::kubernetes::session_effect::tests::` (S2a; one case extended by the gate repair and again by the final review repair) | 5 passed | `NOT_RUN` |
| `--lib runtime::kubernetes::session_pod::tests::` (S1) | 7 passed | `NOT_RUN` |
| `--lib runtime::kubernetes::session_instance::tests::` (S1) | 3 passed | `NOT_RUN` |
| `--lib runtime::kubernetes::admission::tests::` (3 base + 1) | 4 passed | `NOT_RUN` |
| `--test kubernetes_session_runtime` (S1) | 3 passed | `NOT_RUN` |
| `cargo check --all-targets -j 1`; `cargo clippy --all-targets -j 1 -- -D warnings`; `cargo fmt --all -- --check` | RC 0 | `NOT_RUN` |
| Neighbour regressions, once: `--test kubernetes_adapter`, `--test kubernetes_transport`, `--test kubernetes_transport_lifetime`, `--test kubernetes_cli`, `--test kubernetes_preflight`, `--test kubernetes_preflight_cli`, `--test kubernetes_session_control_recovery`, `--test kubernetes_held_create_recovery_api`, `--lib runtime::kubernetes::admission::`, `--lib runtime::kubernetes::prerequisites_tests::`, `--lib runtime::kubernetes::bootstrap::` | base counts, except `--lib runtime::kubernetes::admission::`: base count + 1, because that filter also selects the new `admission::tests::v1_quantity_helpers_keep_the_v1_forms` counted in the row above; 0 failed | `NOT_RUN` |

The 48 lot tests are `#[test]` or `#[tokio::test]` without `#[ignore]` or a
platform `cfg`. Neighbour counts predicted by counting test attributes in the
source (a count, not a run; the eight integration files are unchanged
base..head, and `tests/support/mod.rs` declares no test module):
`kubernetes_adapter` 4 · `kubernetes_transport` 13 passed and 1 ignored
(`proxy_environment_child`, an internal child of another test) ·
`kubernetes_transport_lifetime` 3 · `kubernetes_cli` 6 ·
`kubernetes_preflight` 2 · `kubernetes_preflight_cli` 9 ·
`kubernetes_session_control_recovery` 6 · `kubernetes_held_create_recovery_api` 1 ·
`--lib runtime::kubernetes::admission::` 40 (39 at base + 1) ·
`--lib runtime::kubernetes::prerequisites_tests::` 8 ·
`--lib runtime::kubernetes::bootstrap::` 8 (its tests use the changed
`session::tests::fixture`). A listed count that differs is investigated before
it is called a product result.

The exact commands, per-function `--exact` selectors, timeouts and the run
order for an admitted run are in the private coordination folder,
`handoff/kubernetes-v2-runtime/QA_READY.r2.json` (status
`PROPOSED_ROOT_ADMISSION`, execution `NOT_RUN`). `qa.sh clippy` has no `--rev`
option: it is proposed with `--no-sync` right after a `--rev 2b1ddcb…` run, so
it lints the same frozen commit.

The S3 tests, by name, with their real components:
`abandon_closes_the_real_reservation_once_and_replays_after_a_lost_response`,
`successor_incarnation_closes_through_recovery_and_replays_through_receipt_lookup`,
`successor_without_recovery_policy_is_refused_before_any_store_call`,
`close_by_another_operation_is_classified_by_observation`,
`abandon_is_refused_when_any_effect_checkpoint_name_is_present`,
`other_reservation_or_other_broker_cannot_reopen_or_abandon`,
`reconcile_report_classifies_without_adoption_or_quota_change`,
`reconcile_report_refuses_another_authority_before_any_request` and
`live_owner_makes_observe_only_reopen_busy`. They use real tenant SQLite
Stores, real private journal directories, real broker identities (a revoked
original and an authenticated successor incarnation, plus a second broker) and,
for the report, the real HTTPS client against one owned raw TLS loopback peer.
Both Stores of a test are `tenant-a`: none is a cross-tenant test. The intra-crate
TLS fixtures need `/usr/bin/openssl` in the QA image; its absence is a harness
`BLOCKED`, not a product failure.

Tests added by the gate repair, by name:
`predecessor_close_replays_for_a_successor_through_receipt_lookup` (lifecycle),
`tampered_workspace_or_request_record_is_corrupt_and_preserved` and
`abandonment_retention_is_idempotent_and_a_divergent_close_conflicts` (session).
Extended cases: a cancellation raised during the anchor reads and the
accepted-connection count in
`refused_guard_cancellation_or_changed_anchor_sends_zero_effect_requests`; the
`NoRequestRetained` interpretation, a terminating claim and the
accepted-connection count in the report tests; a hand-written
`preparation_abandoned` close as positive control in
`abandon_is_refused_when_any_effect_checkpoint_name_is_present`.

Test added by the final review repair (`f0f2ad5`):
`excluded_known_identity_is_never_read_and_is_preserved` (session). It
excludes, one at a time, the lock, the preparation and the workspace record,
expects `Corrupt` from both openers with every byte unchanged, uses an
exclusion outside the journal as positive control and expects
`UnsafeMaterial` once a protected identity has moved. It also gives
`SessionJournalProtection::exclude_known_file` its first caller in the test
build, which was predicted to fail `clippy --all-targets -D warnings`
(final correctness review NB1). Extended cases: the mid-anchor cancellation
case of `refused_guard_cancellation_or_changed_anchor_sends_zero_effect_requests`
now uses the test-only `LoopbackEffectGuardV2::ignoring_cancellation`, with a
direct positive control, so only the transport check can refuse it; the
report test asserts that `names` is absent from the serialized report when
not inspected and an object when inspected.

Reproduce-first, by source reading only (not executed):
- `held_lock_is_busy_before_any_inventory_judgement` (S1): base `77f7edd`
  judged the inventory before the lock and returned `Corrupt`. The test calls
  an S1 function, so it cannot run on the base as written.
- `lone_lock_is_judged_without_taking_the_creators_flock` (gate fix): `405e46e`
  and `ed77f7e` are expected to return `Busy` where the fix returns `Corrupt`.
- Gate repair: at `1c03d2b` the report test expects `NoEffectIssuedLocally` for
  a journal without a request and for a terminating claim, where the repair
  expects `NoRequestRetained` and `WorkspaceClaimChanged`.
- Final review repair: without the transport check of
  `session_transport.rs:120-122`, the guard that ignores the flag lets the
  CREATE through, the peer answers it and the outcome is
  `Ok(OwnershipUnknown)` instead of `EffectRefusedBeforeSend`. At `cc24218`
  the report serialized `"names": null`, so the new absence assertion fails
  there.

## Proposed wires (not applied)

Files in the private coordination folder, `handoff/kubernetes-v2-runtime/proposed-wire/`:

| Patch | Target | Pre-image SHA-256 | Post-image SHA-256 | Condition |
|---|---|---|---|---|
| 0001 provider waiting mode | `src/bin/ai-guard-provider.rs` | `336a2927…9572` | `be44ee5e…998a` | S2b only; does not compile before `session_worker` exists |
| 0002 provider ownership | `images/worker.Dockerfile` | `d348f870…11ae` | `d681c6a5…6395` | independent |
| 0003 setup index rows | `docs/setup/README.md`, `README.fr.md` | `0f68ed91…38e0`, `a4b0c0c6…a728` | `aec3c4d0…7e51`, `31882d15…61d1` | none |
| 0004 `inspect-workspace-claim` | `src/kubernetes_cli.rs` | `5624909f…9738` | `4fca493c…8e4d` | optional (Q3); its CLI test is still to write |
| 0005 `inspect-session` | `src/kubernetes_cli.rs` | — | — | not written: conditional on R2 |

Git blob identities (pre-image at `77f7edd` → post-image): 0001
`acfcecb3…` → `04e0c58f…`; 0002 `f4464ff2…` → `cc18e9d5…`; 0003
`541fa2a7…` → `eadf3863…` and `70fb4919…` → `5d17bdd1…`; 0004 `f89d78b3…` →
`a10060e6…`. The full identities, purpose, invariants and blocking decisions
of each patch are in `handoff/kubernetes-v2-runtime/proposed-wire/PROPOSED_WIRES.md`.
Applied alone on the base, 0001 does not compile (no `session_worker` before
S2b), 0004 does not compile (it needs this branch's
`inspect_session_workspace_claim` and re-export) and 0003 leaves two links
dangling (the guide exists only on this branch). 0002 touches a file of lot
B's perimeter and must be checked against the composed text.

## Limits and what remains open

- No real Kubernetes behaviour is qualified: CREATE, gate, patch, attach,
  delete, Pod Security, RBAC verbs, `ReadWriteOncePod`, omitted-boolean encoding
  and quantity forms (EXT-14), log persistence, Docker Desktop, NetworkPolicy,
  CSI and cgroup enforcement, quota settlement after an effect, scale.
- Missing seams for the control owner: R1 (central effect states, claim UID
  never reused, declared `request_sha256`), R2 (broker routes, the only way the
  lifecycle gets a production caller), R3 (durable armed deadline), R4 (frames
  after Ready), R5 (storage policy), R7 (activation), R10 (tenant authority
  disjointness, prerequisite of attach), R11 (successor confirmation).
- Decisions pending: Q1 (attach identity argument), Q3 (proposed patch 0004,
  and 0005 once R2 exists), Q6 (waiting-mode spelling), Q7 (per-attempt claim
  and never-reuse rule), Q9 (`WaitForFirstConsumer`). External facts still to
  confirm against the official v1.36 sources: EXT-8 (RBAC verb of the attach
  upgrade), EXT-10a/b/c (`ReadWriteOncePod` and CSI, Docker Desktop's default
  class and its binding mode), EXT-14 (omitted false booleans and quantity
  forms in an admitted Pod).
- Proposed patch 0004 maps every `InvalidAuthority` of the claim inspection to
  "the name must be a DNS label; nothing was requested". The runtime also
  returns `InvalidAuthority` after the PVC GET if its authority fails
  validation (`session_instance.rs:175-177`); that path is unreachable when the
  CLI has validated the same authority first, but the mapping rests on that
  convention, not on a typed refusal reason (GATE_S1 N8). Recorded, not fixed.
- The successor outcome `SuccessorLookupNotFound` is ambiguous by design: a
  successor cannot observe a closed reservation.
- The request bytes bind the workspace claim by name only, and the request
  digest does not carry the claim UID. The journal cannot detect a
  `workspace-v2.json` rewritten with another UID by the same operating-system
  user. The UID must be re-inspected just before CREATE and after scheduling,
  and bound by the central claim (R1, S5 permit precondition).
- An `OwnershipUnknown` CREATE outcome after a validated 201 whose authority
  recheck failed does not keep the server-assigned UID; until R1 decides the
  state shape, the operator re-inspects the Pod by name.
- The module-level `expect(dead_code)` is not a first-caller tripwire: the
  effect barrier (RC-P5-G15) rests on the sealed guard having no non-test
  implementor, checked by the `git grep` above.
- The successor recovery policy is trusted input: the Store verifies only that
  the policy it is given approves the authenticated broker, and records its
  hash. A production caller (R2) must take it from the tenant's
  `session_recovery_policy` in the gateway configuration.
- `validate_admitted_session_pod` is the only constructor of the instance
  identity and accepts raw bytes; its provenance from this process's own 201
  is a code convention (now restricted to `runtime::kubernetes`), not a type
  guarantee. A response type that only the transport can mint, or R1's
  acknowledged instance, is an S5 obligation.
- `RefusedNotApplied` (release 409/422) leaves the instance state unknown, and
  `DeletionRequested` (DELETE 200/202) does not inspect the body: neither is a
  Held or terminal proof, and S5 must re-observe the exact instance.
- No NetworkPolicy is rendered, created or verified. Without an enforced
  default-deny ingress and egress policy, a released Pod can reach any address;
  the guide's plan keeps step 5 `BLOCKED` until an observed CNI enforcement
  test passes.
- Oracle assumptions, to confirm at the first admitted run (adjust the oracle
  if they fail; this is not a product RED): `accepted == requests` assumes one
  request per connection (`pool_max_idle_per_host(0)` in `transport.rs` and the
  peer's `connection: close`);
  `predecessor_close_replays_for_a_successor_through_receipt_lookup` assumes the
  Store's recovery authorization accepts a historical same-incarnation
  `CloseBeforeEffect` and the lookup returns `replayed: true` (source reading
  of `audit/kubernetes_session/authorization.rs` and `mutations.rs`).
- The public claim inspection reports an invalid name as `InvalidAuthority`
  (its message says the authority configuration is invalid); the guide states
  this mapping, and proposed patch 0004 gives both errors dedicated CLI
  messages.
- Gates G05, G07 and G11 stay open (G11 `BLOCKED`). The absence of any
  production implementation of the effect guard is the barrier that keeps
  Kubernetes launch closed (risk RC-P5-G15); an independent review of this lot
  is required before integration.

## Future Docker Desktop single-node plan (reference, not executed)

The reproducible plan, with its exact prerequisites, storage conditions,
read-only steps 0–3 and the effect steps 4–12 kept `BLOCKED`, is section 8 of
the guide: [English](../../../../setup/kubernetes-runtime/README.md#8-future-docker-desktop-single-node-qualification-plan-not-executed) ·
[Français](../../../../setup/kubernetes-runtime/README.fr.md#8-plan-futur-de-qualification-docker-desktop-à-un-nœud-non-exécuté).
No step was run. Every step that changes a cluster requires an explicit
maintainer authorization that this lot does not grant, and the admitted QA
profile has no network, so a cluster step would need a separately admitted
profile.

## Reviews and dispositions

Private notes, kept outside this repository in the lot coordination folder
(SHA-256 at 2026-11-12T23:27Z). A review by reading is not a test.

| Review | Verdict | Disposition | Note (SHA-256) |
|---|---|---|---|
| Design review of revision 1 | B1–B8 blocking, N1–N19 | all accepted or accepted with the stated option; DESIGN §15 | `notes/kubernetes-v2-runtime/DESIGN_REVIEW.md` (`bb2dd142…1a85`); design rev 2 `DESIGN.md` (`45f547b0…a32f`) |
| Gate S1 | `REQUEST_CHANGES`, 1 blocking (B1 lock-first flock) | B1 fixed by `86aca2a`; N1 (`c273d08`), N3 (CONTRACT addenda), N6, N9 fixed; N4 partly fixed (`218a6a3`); N5 recorded as S5 obligation 1; N2 deferred to the first admitted clippy run; N7 kept (reproduce-first by reading only); N8 open with patch 0004 | `notes/kubernetes-v2-runtime/GATE_S1.md` (`9b307a37…4056`) |
| Gate S2a | `GO_WITH_CHANGES`, 0 blocking | C1, C2, N1, N3, N4, N7 fixed; C3, N5 recorded for S5/R1; N2 deferred; N6 kept as a runtime assumption; N8 rejected for this run; N9 carried by `QA_READY.r2.json` | `notes/kubernetes-v2-runtime/GATE_S2a.md` (`653101c1…bd5d`) |
| Gate S3 | `GO_WITH_CHANGES`, 0 blocking | C1, N1–N4, N6–N10 fixed (`0e9b48b`, `218a6a3`, `c273d08`, `cc24218`); C2 deferred; C3, N5 recorded for S5/R1; C4 carried by `QA_READY.r2.json` | `notes/kubernetes-v2-runtime/GATE_S3.md` (`12d8552b…c0ec1`) |
| Final review, security and contract boundary | `GO_WITH_CHANGES`, 0 blocking | N1, N2, N3, N5, N6, N7 fixed; N4 narrowed now, typed response deferred to S5 (obligation 6); N3 rename rejected | `notes/kubernetes-v2-runtime/FINAL_REVIEW_security.md` (`bc950f6b…b6cb`) |
| Final review, correctness | `GO_WITH_CHANGES`, 0 blocking | NB1, NB2, NB3, NB5 fixed (NB5 in docs); NB4, NB6 recorded for the coordinator and carried by `QA_READY.r2.json` | `notes/kubernetes-v2-runtime/FINAL_REVIEW_correctness.md` (`bf8ed1ce…f5d`) |

Each disposition is appended to its review file; the contract addenda are in
`handoff/kubernetes-v2-runtime/CONTRACT.md` (`c6ba8df3…8314`). No independent
review has looked at `f0f2ad5` and `2b1ddcb` after their repair; one is
advisable before integration ("compared is not audited").

## Resources

No container, volume, image, cluster object, network access, push or merge
was created or performed for this record or for the handoff update. The only
resources created by the lot are the worktree directory and the local branch
`claude/kubernetes-v2-runtime-20261112`, which stay with the integrator.

## Contre-revue de la réparation finale / Re-review of the final repair (2026-11-13T00:00Z)

Appended; every section above is kept as written. Where they differ, this
section is current. **Nothing was compiled or executed** for this re-review or
for this update: the review is by reading and native static checks only, and
a review by reading is not a test.

| Item | Value |
|---|---|
| Scope reviewed | committed content only: the final review repair `cc2421885f0ffe40a71feefb8c3481ffb0cb1c0a..2b1ddcb4ab1280c43a24139614350e8c0526cc30` (`f0f2ad5` code, `2b1ddcb` docs) and the docs-only handoff commit `c13e10356fac051f1b3facd22645007016ba3820` |
| Reviewer | an independent re-reviewer that wrote none of this lot and none of the two final reviews |
| Verdict | `GO_WITH_CHANGES`, 0 blocking, 5 non-blocking (NR1–NR5) |
| Note | `notes/kubernetes-v2-runtime/REREVIEW_final_repair.md` (`681578cd…5b15`), private, in the lot coordination folder |
| Repair commits after the re-review | none: NR1–NR4 are not repaired in this run |
| Code SHA for QA | unchanged, `2b1ddcb4ab1280c43a24139614350e8c0526cc30`; `QA_READY.r2.json` (`3e2480e6…ddcb`) stays the current proposal and no revision 3 was written |
| This record | the `docs(evidence)` commit that adds this section on top of `c13e103`; it changes only this file, and no source, test or build input reads it |
| SHA actually tested | none (`NOT_RUN`) |

The statement under "Reviews and dispositions" that no independent review had
looked at `f0f2ad5` and `2b1ddcb` is superseded by this re-review, which
remains a reading: compared is still not audited, and nothing was run.

### Finding dispositions verified at `2b1ddcb`

| Finding | Status |
|---|---|
| Final security review, blocking | `not_applicable` (none raised) |
| Final correctness review, blocking | `not_applicable` (none raised) |
| Security N1, N2, N3, N5, N6, N7 | closed; N2 leaves a heading residual (NR2) and N5 introduced a plan sequencing gap (NR1) |
| Security N4, instance constructor over raw bytes | `partially_closed`: narrowed to `pub(super)` with a never-re-adopt doc; the typed response is an S5 entry gate (NR4) |
| Correctness NB1 | closed by source reading only; the clippy run that alone confirms it is `NOT_RUN` |
| Correctness NB2, NB3, NB4, NB6a, NB6b | closed |
| Correctness NB5 | closed in the docs; the code mapping is kept as proposed (GATE_S1 N8 residual recorded under "Limits") |
| Correctness NB6c, oracle assumptions | `deferred_documented`: listed under "Limits" as runtime assumptions to confirm at the first admitted run |
| GATE_S3 N5 | closed for coordination: the stage plan now lists CONTRACT S5 obligations 1–8 and the R2 recovery-policy item |

### Non-blocking items still open (not repaired)

- **NR1, plan sequencing (medium; fix before the plan is used).** Guide
  section 8.4 keeps step 5 `BLOCKED` until a CNI enforcement test of the
  default-deny policy passes, but the only network step (9) comes after step
  5, and section 8.1 does not record the CNI or whether it enforces
  NetworkPolicy. Proposed fix, EN and FR together: record the CNI plugin, its
  version and its enforcement (to verify against official sources, not
  assumed; without enforcement, steps 5–12 stay `BLOCKED`); add a step 4a,
  under its own explicit maintainer authorization, with a dedicated
  non-session probe Pod under the same policy testing DNS, a direct IPv4 and
  IPv6 address, the node, a cluster Service IP and `169.254.169.254`; make
  step 5 depend on step 4a and keep step 9 as the confirmation from the
  released session Pod. Until this is fixed, the plan must not be followed past
  step 4.
- **NR2, heading wording (low).** Guide section 8.3 is titled "Read-only
  steps" / "Étapes en lecture seule", but its steps 1 and 3 change the cluster
  (Namespace, NetworkPolicy, ServiceAccount, Role; a PVC) and each requires an
  explicit maintainer authorization that this work does not grant. The words
  "read-only steps 0–3" under "Future Docker Desktop single-node plan" above
  carry the same inaccuracy: only steps 0 and 2 are read-only. The proposed
  rename is "Preparation (operator mutations, authorization required) and
  read-only product checks" / "Préparation (mutations de l'opérateur,
  autorisation requise) et vérifications en lecture seule du produit".
- **NR3, trait doc (low).** `session_effect.rs:45-46` says that an implementor
  still refuses a raised cancellation, while the only implementor has a
  `cfg(test)` mode, `LoopbackEffectGuardV2::ignoring_cancellation`, that skips
  that check. Proposed doc fix: every production implementor must also refuse
  it (CONTRACT S5 obligation 5); the test-only mode exists to isolate the
  transport check. A delivery grep can also check that `refuses_cancellation`
  appears only under `#[cfg(test)]`.
- **NR4, N4 residual (S5 entry gate).** `validate_admitted_session_pod`
  (`session_instance.rs:275-278`) still builds a `SessionInstanceV2` from any
  byte slice inside `runtime::kubernetes`. It has one non-test caller, on its
  own validated 201. A response type that only the transport can construct for
  a `CreateHeld` 201 must replace `&[u8]` before the first S5 permit (CONTRACT
  addendum 23:15Z and stage plan, obligation 6).
- **NR5, predictions only QA can settle (`NOT_RUN`).** On the frozen
  `2b1ddcb`: `cargo check --all-targets -j 1` and
  `cargo clippy --all-targets -j 1 -- -D warnings` RC 0; the seven lot
  selectors of "Tests: all NOT_RUN" with their exact counts (48 in total; 0 or
  any other count is `FAIL`); the neighbour `--lib runtime::kubernetes::admission::`
  40. An optional mutation check in an admitted QA scratch only, never
  committed: remove `session_transport.rs:120-122` and expect the cancellation
  test to fail.

### Checks for this update (native, no project code run)

| Check | Exact command form | Result |
|---|---|---|
| Worktree state before the update | `git status --porcelain=v1 --ignored` at `c13e103` | empty |
| Code unchanged since the QA proposal | `git diff --stat 2b1ddcb4ab1280c43a24139614350e8c0526cc30 c13e10356fac051f1b3facd22645007016ba3820` | this file only |
| Proposed-wire targets untouched | `git diff --quiet 77f7edd… HEAD -- src/bin/ai-guard-provider.rs images/worker.Dockerfile docs/setup/README.md docs/setup/README.fr.md src/kubernetes_cli.rs`; blob identity of each target at base, `HEAD` and in the worktree | rc 0; identical blobs, pre-image SHA-256 equal to each patch header |
| Proposed patches against their declared base `77f7edd` | `git apply --check <patch>` in the lot worktree, for 0001–0004 | rc 0 each; no revised patch needed |
| Re-review note identity | `shasum -a 256` | `681578cd…5b15` |

The re-review note also reports, at `2b1ddcb`: `rustfmt --edition 2024
--check` rc 0 on the nine changed files it checked; `git diff --check` rc 0 for
`cc24218..c13e103` and `77f7edd..c13e103`; the effect-guard grep giving only
`session_effect.rs:221` and `:223`, both `#[cfg(test)]`; test attribute counts
equal to the selectors above, none `#[ignore]`; `QA_READY.r2.json` parsing.

### Status after the re-review

`QA_ADMISSION.json` is absent, so no QA command was issued: every test,
`cargo check`, `clippy` and `cargo fmt` stays `NOT_RUN`, and any run that
requires a cluster stays `BLOCKED`. S2b and S5 are not implemented, layer C
still has no production caller (R2), and the lot is not complete under its
mandate. Gates G05, G07 and G11 stay open (G11 `BLOCKED`); RC-P5-G15 still
rests on the sealed effect guard having no non-test implementor. No gate or
risk is closed. The product is not production-ready.

## Pre-QA fixes of the re-review items (2026-11-13T00:25Z)

Appended; every section above is kept as written. Where they differ, this
section is current. **Nothing was compiled or executed** for these fixes:
`QA_ADMISSION.json` is absent, so no QA command was issued.

| Item | Value |
|---|---|
| Scope | re-review NR1, NR2 and NR3, fixed below. NR4 stays an S5 entry gate and NR5 stays QA-only; both are out of scope. |
| Code commit | `d4c6dec2c0776edfa688185c6384565724e4d762` "fix(kubernetes): effect guard doc states the production cancellation obligation": the doc comment of `SessionEffectGuardV2` only (`session_effect.rs:45-49`, +5/−2). No item, signature, visibility, `cfg` or behaviour changes. |
| Guide commit | `c468474a999ebf0383eb291b851940aa228d6925` "docs(kubernetes): CNI probe before any release and accurate 8.3 heading in the V2 plan", EN and FR (NR1, NR2) |
| This record | the `docs(evidence)` commit that adds this section on top of `c468474`; it changes only this file |
| Code SHA for QA | `d4c6dec2c0776edfa688185c6384565724e4d762`, tree `241ecb4ce09a1d91102a01b3107b1e43c08790bd`, `git archive --format=tar` SHA-256 `c896aafa7695dca2d0a21d85eecee9d892cde71cfad916029fcc05ab57112413`, 2615 tracked files. It supersedes `2b1ddcb` as the SHA proposed for QA: `git diff --name-only 2b1ddcb d4c6dec -- src tests` gives `src/runtime/kubernetes/session_effect.rs` only (the doc comment). No test was added or removed, so the expected counts above are unchanged. |
| QA proposal | `QA_READY.r2.json` still names `2b1ddcb`; a coordinator revision naming `d4c6dec` and its recomputed freeze line is needed before any admission. No QA proposal was written by these fixes. |
| SHA actually tested | none (`NOT_RUN`) |

The commits after `d4c6dec` change only the two guide files and this record.
No source or test reads them: a `git grep` of `include_str!`, `include_bytes!`
and `docs` path literals in `src` and `tests` at `c468474` names neither the
guide nor this record.

### Dispositions of the re-review items

| Item | Disposition |
|---|---|
| NR1 (medium), plan sequencing | Fixed in the guide by `c468474`. Section 8.1 records the CNI plugin, its version and whether it enforces NetworkPolicy, as facts to verify against official sources and never assumed. A new step 4a, under its own explicit maintainer authorization, uses a dedicated non-session probe Pod under the default-deny policy of step 1 to attempt egress to the cluster DNS, a direct IPv4 and a direct IPv6 address, the node, a cluster Service IP and `169.254.169.254`, with a positive control and retained evidence. Step 5 depends on step 4a, step 1 points at it, step 9 stays the confirmation from the released session Pod, and section 7 points at step 4a. The private lot design carries the same order in a dated addendum. No step was run. |
| NR2 (low), heading wording | Fixed in the guide by `c468474`: section 8.3 is titled "Preparation (operator mutations, authorization required) and read-only product checks" / "Préparation (mutations de l'opérateur, autorisation requise) et vérifications en lecture seule du produit", and states that steps 0 and 2 are read-only while steps 1 and 3 change the cluster. The wording of this record is corrected below by an appended line. |
| NR3 (low), trait doc | Fixed by `d4c6dec` (doc only): every production implementor must also refuse a raised cancellation (CONTRACT S5 obligation 5), and the `cfg(test)` `LoopbackEffectGuardV2::ignoring_cancellation` mode skips that check only to isolate the transport check in one test. The delivery grep list gains the `refuses_cancellation` row below. |
| NR4 | Out of scope, unchanged: S5 entry gate (CONTRACT addendum 23:15Z and stage plan, obligation 6). |
| NR5 | Out of scope (QA only). Its predictions now apply to `d4c6dec`. The optional mutation check still targets `session_transport.rs:120-122`, a file these fixes do not touch. |

Correction 2026-11-13T00:25Z: under "Future Docker Desktop single-node plan
(reference, not executed)" above, the words "read-only steps 0–3" are
inaccurate. Steps 0 and 2 are read-only; steps 1 and 3 change the cluster, and
each requires an explicit maintainer authorization, as guide section 8.3 now
states. The plan also has a step 4a, which must pass before step 5.

Correction 2026-11-13T00:25Z: NR1, NR2 and NR3, listed above under
"Non-blocking items still open (not repaired)", are repaired by `d4c6dec` and
`c468474`. The plan still has no step that may run without its own explicit
maintainer authorization, and the "Limits" item on NetworkPolicy now reads with
step 5 depending on the probe of step 4a.

Correction 2026-11-13T00:25Z: since `d4c6dec`, the sealed effect trait is at
`session_effect.rs:50` (`:47` before) and the effect-guard implementor lines
are `session_effect.rs:224` and `:226` (`:221` and `:223` at `2b1ddcb`, as
cited above), both still under `#[cfg(test)]`. `Sealed` stays at `:38`.

### Delivery greps (added and rerun on `d4c6dec`, native)

| Check | Exact command | Result |
|---|---|---|
| Effect-guard implementors | `git grep -n 'SessionEffectGuardV2 for\|Sealed for' d4c6dec2c0776edfa688185c6384565724e4d762 -- src tests` | `session_effect.rs:224` and `:226`, each preceded by `#[cfg(test)]` (`:223`, `:225`); no other implementor |
| Test-only cancellation switch (NR3) | `git grep -n 'refuses_cancellation' d4c6dec2c0776edfa688185c6384565724e4d762 -- src tests` | four lines, all in `session_effect.rs` and all inside `#[cfg(test)]` items: `:194` (field of the `#[cfg(test)]` struct `LoopbackEffectGuardV2`, `:187-195`), `:207` and `:218` (its `#[cfg(test)]` inherent impl, `:196-222`) and `:234` (its `#[cfg(test)]` trait impl, `:225-259`). Expected at every delivery: no occurrence outside a `#[cfg(test)]` item. |
| Its constructor | `git grep -n 'ignoring_cancellation' d4c6dec2c0776edfa688185c6384565724e4d762 -- src tests` | defined at `session_effect.rs:212` (the `#[cfg(test)]` inherent impl), named in the trait doc at `:48`, and called only at `session_effect_tests.rs:851`, a file included only by `#[cfg(test)] #[path = "session_effect_tests.rs"] mod tests;` (`session_effect.rs:543-545`) |

### Checks for these fixes (native, no project code run)

| Check | Exact command form | Result |
|---|---|---|
| Worktree state before the fixes | `git rev-parse HEAD`; `git status --porcelain=v1 --ignored` | `d722cdb0c6dff527e0148e7f781d89093c3dd719`; empty |
| Formatting of the changed Rust file | `rustfmt --check --edition 2024 src/runtime/kubernetes/session_effect.rs` (rustfmt 1.10.0-stable, 58cf8f9c35 2026-10-13; check mode, follows its test module); a badly formatted probe file outside the repository gave rc 1 | rc 0 |
| Whitespace | `git diff --check` on each change before its commit | rc 0 |
| EN/FR parity of the guide | code spans, headings, `4a` mentions and R/Q/G/EXT identifiers counted in both files | 118 code spans each, identical except the translated claim-name placeholder; 15 headings each; 9 `4a` mentions each; identifier counts identical |
| Build inputs | `git grep` of `include_str!`, `include_bytes!` and `docs` path literals in `src` and `tests` at `c468474` | neither the guide nor this record is read |

### Status after the pre-QA fixes

`QA_ADMISSION.json` is absent (checked 2026-11-13T00:30Z), so no QA command was
issued: every test, `cargo check`, `clippy` and `cargo fmt` stays `NOT_RUN`,
and any run that requires a cluster stays `BLOCKED`. No plan step was run.
S2b and S5 are not implemented, layer C still has no production caller (R2),
and the lot is not complete under its mandate. Gates G05, G07 and G11 stay
open (G11 `BLOCKED`); RC-P5-G15 still rests on the sealed effect guard having
no non-test implementor. No gate or risk is closed. The product is not
production-ready.

## Corrections avant QA / Pre-QA fixes (2026-11-13T00:25Z)

Appended by the handoff refresher; every section above is kept as written.
Where they differ, this section is current. **Nothing was compiled or
executed** for this update. It changes no source, test or guide file: it
records the verification of the pre-QA fixes and the QA proposal that now
names their code SHA.

| Item | Value |
|---|---|
| Items fixed | NR1 (medium, plan sequencing) and NR2 (low, 8.3 heading) by the guide commit `c468474a999ebf0383eb291b851940aa228d6925`, EN and FR; NR3 (low, trait doc) by `d4c6dec2c0776edfa688185c6384565724e4d762`, a doc comment only (`session_effect.rs:45-49`, +5/−2, every changed line a `///` line). Details under "Pre-QA fixes of the re-review items" above. |
| Not fixed | NR4 (typed `CreateHeld` response, S5 entry gate) and NR5 (QA only): out of scope, unchanged |
| New tests | none. No `#[test]` or `#[tokio::test]` attribute was added, removed or renamed between `2b1ddcb` and `d4c6dec`: every test file is unchanged and `session_effect.rs` holds no test attribute. The 48 lot tests (session 16, session_lifecycle 10, session_effect 5, session_pod 7, session_instance 3, admission::tests 4, `--test kubernetes_session_runtime` 3) and the neighbour counts are those of "Tests: all NOT_RUN" above, all `NOT_RUN` |
| Independent verification | `GO_WITH_CHANGES`, 0 blocking, 6 non-blocking (V1–V6); NR4 and NR5 `not_closed`. Private note `notes/kubernetes-v2-runtime/VERIFY_preqa_fixes.md` (`1020036c…8e21`) in the lot coordination folder. Reading and native static checks only; a review by reading is not a test. |
| Code SHA for QA | `d4c6dec2c0776edfa688185c6384565724e4d762`, tree `241ecb4ce09a1d91102a01b3107b1e43c08790bd`, `git archive --format=tar` SHA-256 `c896aafa7695dca2d0a21d85eecee9d892cde71cfad916029fcc05ab57112413`, 2615 tracked files, all regular (mode 100644); predicted harness freeze line `files=2615 manifest=210d6c52c409927952feee0714d142a3d903c35ce00c77e559f4eeb315d6cd50` (to confirm by the harness) |
| Current QA proposal | `handoff/kubernetes-v2-runtime/QA_READY.r3.json`, SHA-256 `2d86006f871eddcafdcb90c899646bb19ef6fe8ac41f9055f5bce764d4d99eea`: revision 3, status `PROPOSED_ROOT_ADMISSION`, execution `NOT_RUN`, every `--rev` on `d4c6dec`, labels `k8s-v2-r3-*`, same profile, harness, selectors and counts as revision 2. It supersedes `QA_READY.r2.json` (`3e2480e6…ddcb`); r2 and r1 `QA_READY.json` (`2038bebf…44ec`) are kept unchanged and are void for admission. |
| This record | the `docs(evidence)` commit that adds this section on top of `98b289529400f1905b5fd24cca9fb44996ccad8f`; it changes only this file, and no source, test or build input reads it. A commit cannot name itself: its SHA is in the lot INDEX of the coordination folder. |
| SHA actually tested | none (`NOT_RUN`) |

### Verification items still open (non-blocking, not repaired here)

| Item | Content | Owner and status |
|---|---|---|
| V1 | Step 4a oracle: a failed attempt is evidence only for a destination shown reachable without the policy, from a comparable Pod, with the same tool and protocol; name targets with a known listener; the positive control uses the same tool | guide owner, EN and FR together, before the plan is used; open |
| V2 | Step 4a: connection or handshake probes only, no request path or body; mask the output before it is retained | guide owner, before the plan is used; open |
| V3 | Retain the NetworkPolicy UID and resourceVersion and the Namespace UID at 4a and require them unchanged before step 5 | guide owner, before the plan is used; open |
| V4 | Coordinator housekeeping: `QA_READY.r2.json` froze `2b1ddcb`; `CONTRACT.md:648` and the lot INDEX still cite `session_effect.rs:221`/`:223` | QA proposal: addressed by `QA_READY.r3.json`. Line citations: corrected by a line appended to the lot INDEX (`:224`/`:226` since `d4c6dec`, as recorded above); `CONTRACT.md` is not edited |
| V5 | Source comments cite unversioned private documents ("CONTRACT S5 obligation 5", "DESIGN section 0"); the 8.4 heading now also holds step 4a, which is not an effect step | Codex at integration; open |
| V6 | Items only QA can settle on `d4c6dec`: check, clippy `-D warnings` (no doc lint predicted from `:45-49`), the 7 lot selectors and the neighbour counts | carried by `QA_READY.r3.json`; `NOT_RUN` |

The plan of guide section 8 still has no step that may run without its own
explicit maintainer authorization. V1–V3 refine step 4a and must be settled
before that step is used; they change no code.

### Freeze prediction

The predicted freeze line was computed from Git objects only, without
extracting an archive: for each entry of `git ls-tree -r -z d4c6dec`,
`git cat-file blob <oid> | shasum -a 256`, one line `<sha256>  ./<path>` in Git
index order, which equals the `LC_ALL=C` order of the paths (`sort -c` on the
path column), then the SHA-256 of that listing. Run the same way, it gives
`files=2615 manifest=631d6a8e…f69f` for `2b1ddcb` and
`files=2615 manifest=a4061f52…24f6` for `c13e103`, exactly the lines that
revision 2 recorded from extracted archives. The `d4c6dec` listing differs from
the `2b1ddcb` listing in two lines: this record (the `d722cdb` section) and
`src/runtime/kubernetes/session_effect.rs`.

### Proposed wires (rerun, none applied)

Every patch declares the base `77f7edd`, and no patch declares a lot commit as
its base. Each target blob (`src/bin/ai-guard-provider.rs`,
`images/worker.Dockerfile`, `docs/setup/README.md`, `docs/setup/README.fr.md`,
`src/kubernetes_cli.rs`) is identical at the base, at `98b2895` and in the
worktree, and its SHA-256 equals the pre-image of each patch header, so
`git apply --check <patch>` ran in the lot worktree: rc 0 for 0001, 0002, 0003
and 0004, and `git status --porcelain=v1 --ignored` stayed empty. The patch
files and `PROPOSED_WIRES.md` (`14f41082…2255`) are unchanged, and no
`.r2.patch` was needed. Patch 0004 still compiles only with this branch: it
uses `inspect_session_workspace_claim` in `session_instance.rs`, which
`d4c6dec` does not touch.

### Checks for this update (native, no project code run)

| Check | Exact command form | Result |
|---|---|---|
| Worktree state before the update | `git rev-parse HEAD`; `git status --porcelain=v1 --ignored` | `98b289529400f1905b5fd24cca9fb44996ccad8f`; empty |
| Code change since the previous proposal | `git diff --name-status 2b1ddcb d4c6dec`; `git diff --quiet 2b1ddcb d4c6dec -- src tests ':(exclude)src/runtime/kubernetes/session_effect.rs'` | two paths (`session_effect.rs` and this record); rc 0; 0 changed lines of `session_effect.rs` outside `///` lines |
| Test attributes | `git show <rev>:<file> \| grep -cE '^\s*#\[(tokio::)?test(\(\|\])'`, and the same for `#[ignore]`, at `2b1ddcb` and `d4c6dec` | identical counts, listed in the row "New tests" above; 0 `#[ignore]` in the lot files |
| Formatting | `rustfmt --check --edition 2024 src/runtime/kubernetes/session_effect.rs` (rustfmt 1.10.0-stable, 58cf8f9c35 2026-10-13; check mode only) | rc 0 |
| Whitespace | `git diff --check 2b1ddcb d4c6dec`; `git diff --check 77f7edd HEAD` | rc 0; rc 0 |
| Build inputs | `git grep -n -E 'include_(str\|bytes)!' d4c6dec -- src tests`; `git grep -n -E 'kubernetes-runtime\|evidence/2026-11-12' d4c6dec -- src tests`; `git diff --quiet 77f7edd d4c6dec -- Cargo.toml Cargo.lock` | other files only; 0 lines; rc 0 |
| Effect guard | `git grep -n 'SessionEffectGuardV2 for\|Sealed for' d4c6dec -- src tests`; `git grep -n 'refuses_cancellation' d4c6dec -- src tests` | `session_effect.rs:224` and `:226`, each preceded by `#[cfg(test)]`; `:194`, `:207`, `:218`, `:234`, all inside `#[cfg(test)]` items |
| Source identity | `git rev-parse d4c6dec^{tree}`; `git archive --format=tar d4c6dec \| shasum -a 256`; `git ls-tree -r d4c6dec` modes | values of the table above; 2615 entries, all mode 100644; no `.gitattributes` |
| QA proposal | `plutil -convert xml1 -o /dev/null QA_READY.r3.json`; a malformed control | rc 0; rc 1 |
| Admission | presence of `handoff/kubernetes-v2-runtime/QA_ADMISSION.json` | absent at 2026-11-13T00:45:41Z (`date -u`) |

### Status after the pre-QA fixes and their verification

`QA_ADMISSION.json` is absent, so no `qa.sh` (not even `status`), docker,
podman or kubectl command was issued: every test, `cargo check`, `clippy` and
`cargo fmt` stays `NOT_RUN`, any run that requires a cluster stays `BLOCKED`,
and no plan step was run. The current proposal is `QA_READY.r3.json` on
`d4c6dec`. S2b and S5 are not implemented, layer C still has no production
caller (R2), and the lot is not complete under its mandate. Gates G05, G07 and
G11 stay open (G11 `BLOCKED`); RC-P5-G15 still rests on the sealed effect guard
having no non-test implementor. No gate or risk is closed. The product is not
production-ready.

## Final pre-QA round: step 4a refinements V1–V3 (2026-11-13T01:20Z)

Appended; every section above is kept as written. Where they differ, this
section is current. **Nothing was compiled or executed** for this round:
`QA_ADMISSION.json` is absent (checked 2026-11-13T01:25:40Z), so no QA
command was issued, and no plan step was run. This round changes the two guide
files and this record only.

| Item | Value |
|---|---|
| Scope | verification items V1, V2 and V3 of the private note `VERIFY_preqa_fixes.md` (`1020036c…8e21`), fixed in the guide, EN and FR together. V5 (Codex at integration), V6 (QA only), NR4, S2b and S5 are out of scope and unchanged. |
| Guide commit | `07b7c5cbd357ff958b91d7d22b607d2d8a8cc9c4` "docs(kubernetes): baseline oracle, connection-only probes and policy binding for plan step 4a" (EN +48/−3, FR +52/−3), on top of `2e624c69131dc8f13f407cf72a8fb0421adead05` |
| This record | the `docs(evidence)` commit that adds this section on top of `07b7c5c`; it changes only this file |
| Code SHA for QA | unchanged: `d4c6dec2c0776edfa688185c6384565724e4d762` (tree `241ecb4ce09a1d91102a01b3107b1e43c08790bd`). `git diff --quiet d4c6dec 07b7c5c -- src tests` gives rc 0, and no source or test reads the guide or this record, so `QA_READY.r3.json` still names the code to test. |
| New tests | none (docs only) |
| SHA actually tested | none (`NOT_RUN`) |

### Dispositions of V1–V3

| Item | Disposition |
|---|---|
| V1, step 4a oracle | Fixed by `07b7c5c`. Step 4a names its targets, each with a listener known before the step: the cluster DNS Service IP on 53/UDP and 53/TCP, the `kubernetes` Service IP on 443/TCP, the node InternalIP on a port with a known listener, one IPv4 and one IPv6 listener outside the tenant Namespace, and `169.254.169.254` on 80/TCP. A baseline Pod, built from the same reviewed manifest and image in a separate baseline Namespace with the same Pod Security level and no NetworkPolicy, probes each target with the same tool, protocol, port and timeout, under the same authorization request. A target the baseline does not reach is "not discriminated", never evidence of enforcement. The step passes only if every target is refused from the probe Pod, the positive controls succeed and the DNS, `kubernetes` Service, node and IPv4 targets are discriminated. Any target reached from the probe Pod is a stop. The positive controls use the same tool and protocol and only show that the tool works. Whether a NetworkPolicy applies to a Pod's own loopback traffic or to Pod–node traffic, and the capability rule of Pod Security `restricted` (`NET_RAW`, hence no ICMP), are written as facts to verify against the official Kubernetes documentation, never asserted. |
| V2, probe content and evidence masking | Fixed by `07b7c5c`. Probes are a connection or a handshake only. A TCP probe sends no application data: no request path, header or body, and never a request to a metadata credentials path at `169.254.169.254`. The 53/UDP probe sends a single DNS query for a fixed reviewed name. Every output is reviewed before it is retained, secret-like content is masked, and the record names each masking. |
| V3, binding to the observed policy | Fixed by `07b7c5c`. Step 4a retains the UIDs of both Namespaces and the list of NetworkPolicies of each Namespace, with each policy's UID and `resourceVersion`. Just before the release, step 5 re-reads the tenant Namespace and its NetworkPolicies, read-only. It refuses, and step 4a is re-run under a new authorization, if the Namespace UID, the list or any UID or `resourceVersion` changed. The whole list is bound, not only the default-deny policy, because a policy added later could change what step 4a observed. The re-read is not an atomic precondition of the release, and actor exclusivity stays `BLOCKED`. |
| Guard line citations | Already corrected: the line "Correction 2026-11-13T00:25Z: since `d4c6dec`, the sealed effect trait is at `session_effect.rs:50` …" above gives `session_effect.rs:224` and `:226` for the implementors. It applies to every earlier `:221`/`:223` citation of this record (the "Code without a product caller" row, both effect-guard check rows and the re-review paragraph) and to `:47` for the trait. `git grep -n 'SessionEffectGuardV2 for\|Sealed for' 07b7c5c -- src tests` still gives `:224` and `:226`. No further line is added. |

Correction 2026-11-13T01:20Z: V1, V2 and V3, listed above under "Verification
items still open (non-blocking, not repaired here)", are repaired by
`07b7c5c`. V4 stays as recorded there. V5 stays open for Codex at integration.
V6 stays QA only (`NOT_RUN`).

### Checks for this round (native, no project code run)

| Check | Exact command form | Result |
|---|---|---|
| Worktree state before the round | `git rev-parse HEAD`; `git status --porcelain=v1 --ignored` | `2e624c69131dc8f13f407cf72a8fb0421adead05`; empty |
| Paths changed by the guide commit | `git diff --name-status 2e624c6 07b7c5c` | `M` on the two guide files only |
| No source or test change since the code SHA | `git diff --quiet d4c6dec 07b7c5c -- src tests` | rc 0 |
| Whitespace | `git diff --check 2e624c6 07b7c5c`; `git diff --check 77f7edd 07b7c5c` | rc 0; rc 0 |
| Build inputs | `git grep -n -E 'kubernetes-runtime\|evidence/2026-11-12' 07b7c5c -- src tests` | 0 lines |
| EN/FR parity of the guide | code spans, headings, `4a` mentions, R/Q/G/EXT identifiers and link targets counted in both files | 129 code spans each, identical except the translated claim-name placeholder; 15 headings each; 15 `4a` mentions each; identifier multisets identical; link targets identical except the language of the Kubernetes guide links |

### Status after the final pre-QA round

`QA_ADMISSION.json` is absent, so no `qa.sh` (not even `status`), docker,
podman or kubectl command was issued. Every test, `cargo check`, `clippy` and
`cargo fmt` stays `NOT_RUN`, any run that requires a cluster stays `BLOCKED`,
and no plan step was run. Step 4a and every later step still need their own
explicit maintainer authorization, and whether the Docker Desktop CNI enforces
NetworkPolicy remains an external fact. The current QA proposal is still
`QA_READY.r3.json` on `d4c6dec`. S2b and S5 are not implemented, layer C still
has no production caller (R2), and the lot is not complete under its mandate.
Gates G05, G07 and G11 stay open (G11 `BLOCKED`). No gate or risk is closed.
The product is not production-ready.

## Dernier tour avant QA / Final pre-QA round (2026-11-13T01:20Z)

Appended by the handoff refresher. Every section above is kept as written.
Where they differ, this section is current. **Nothing was compiled or
executed** for this update. It changes no source, test or guide file. It
records the independent verification of the step 4a round above and the QA
proposal that stays current.

| Item | Value |
|---|---|
| Items fixed | V1 (step 4a oracle), V2 (connection or handshake probes only, masked output) and V3 (binding of the 4a observation to both Namespace UIDs and to the full NetworkPolicy list of each Namespace, UID and `resourceVersion`, re-read before step 5) by the guide commit `07b7c5cbd357ff958b91d7d22b607d2d8a8cc9c4`, EN and FR together. The guard-line correction (`session_effect.rs:224`/`:226` since `d4c6dec`, trait at `:50`) was already in this record at 00:25Z; the section above states that it covers the earlier `:221`/`:223` and `:47` citations, and no duplicate line was added. Details under "Final pre-QA round: step 4a refinements V1–V3" above. |
| Not fixed | V4 `CONTRACT.md:648` citation (coordinator file, not edited), V5 (Codex at integration), V6 (QA only, `NOT_RUN`), NR4 (S5 entry gate), S2b and S5 (not implemented): out of scope, unchanged |
| New tests | none. The round changed docs only: `git diff --quiet d4c6dec 5f00bf1 -- src tests` gives rc 0. The 48 lot tests and the neighbour counts are those of "Tests: all NOT_RUN" above, all `NOT_RUN` |
| Independent verification | `GO_WITH_CHANGES`, 0 blocking, 5 non-blocking (N1–N5, listed below); V5, V6, NR4, S2b, S5 and the lot INDEX SHAs `not_closed`. Private note `notes/kubernetes-v2-runtime/VERIFY_final_preqa.md` (`2a4023b2…afd1`) in the lot coordination folder. Reading and native static checks only; a review by reading is not a test. |
| Code SHA for QA | unchanged: `d4c6dec2c0776edfa688185c6384565724e4d762`, tree `241ecb4ce09a1d91102a01b3107b1e43c08790bd`, `git archive --format=tar` SHA-256 `c896aafa7695dca2d0a21d85eecee9d892cde71cfad916029fcc05ab57112413`, predicted harness freeze line `files=2615 manifest=210d6c52c409927952feee0714d142a3d903c35ce00c77e559f4eeb315d6cd50` (to confirm by the harness) |
| Current QA proposal | `handoff/kubernetes-v2-runtime/QA_READY.r3.json`, SHA-256 `2d86006f871eddcafdcb90c899646bb19ef6fe8ac41f9055f5bce764d4d99eea`, revision 3, status `PROPOSED_ROOT_ADMISSION`, execution `NOT_RUN`, every `--rev` on `d4c6dec`. No new revision: the code did not change. `QA_READY.r2.json` (`3e2480e6…ddcb`) and r1 `QA_READY.json` (`2038bebf…44ec`) are kept unchanged, superseded and void for admission. |
| This record | the `docs(evidence)` commit that adds this section on top of `5f00bf1b021454e636f471c59557bfc415b3cfc8`; it changes only this file, and no source, test or build input reads it. A commit cannot name itself: its SHA is in the lot INDEX of the coordination folder. |
| SHA actually tested | none (`NOT_RUN`) |

### Verification items still open (non-blocking, docs only)

They apply before step 4a is ever used, not before QA admission, and none of
them touches `d4c6dec`.

| Item | Content | Owner and status |
|---|---|---|
| N1 | The step 4a pass rule does not require the tenant NetworkPolicy list to be exactly the step 1 default-deny objects, so a policy that selects session-Pod labels could pass 4a unseen by the probe Pod | guide owner, EN and FR, before step 4a is used; open |
| N2 | The V3 binding leaves out cluster-scoped network policy objects and the Namespace labels (`resourceVersion`) | guide owner, before step 4a is used; open |
| N3 | The pass rule says "refused", while a probe result is recorded as refusal or timeout | guide owner, wording; open |
| N4 | The DNS query name and the chosen IPv4/IPv6 listeners may be external, which adds Internet effects from the unrestricted baseline Pod | guide owner, before step 4a is used; open |
| N5 | Consistency nits: the listener wording (`169.254.169.254` has none) and the Pod Security version of the baseline Namespace | guide owner; open |

The note also records, outside V1–V3: NetworkPolicy semantics that may make
the node and `kubernetes` Service targets unreachable to discriminate on a
single node, the Pod start window before a policy applies, ingress never
observed, and step 9 probe rules still a one-line list. Each Kubernetes
behaviour it cites is marked "to verify" against the official pages and is not
asserted here.

### Proposed wires (rerun, none applied)

Every patch declares the base `77f7edd`, and no patch declares a lot commit as
its base. Each target blob (`src/bin/ai-guard-provider.rs` `acfcecb3…`,
`images/worker.Dockerfile` `f4464ff2…`, `docs/setup/README.md` `541fa2a7…`,
`docs/setup/README.fr.md` `70fb4919…`, `src/kubernetes_cli.rs` `f89d78b3…`) is
identical at the base, at `5f00bf1` and in the worktree, so
`git apply --check <patch>` ran in the lot worktree: rc 0 for 0001, 0002, 0003
and 0004, and `git status --porcelain=v1 --ignored` stayed empty. The patch
files (`7c2a451a…dcd4`, `4914f13b…2528`, `b989a436…6e6f`, `aa2e95f6…3558`) are
unchanged, and no new patch was needed.

### Checks for this update (native, no project code run)

| Check | Exact command form | Result |
|---|---|---|
| Worktree state before the update | `git rev-parse HEAD`; `git status --porcelain=v1 --ignored` | `5f00bf1b021454e636f471c59557bfc415b3cfc8`; empty |
| Verification report identity | `shasum -a 256 notes/kubernetes-v2-runtime/VERIFY_final_preqa.md` | `2a4023b219d1fd3fc129be3fa083211d7f8cd2141bdf000f78966b091386afd1`, equal to the workflow value |
| QA proposal identity | `shasum -a 256 QA_READY.r3.json` | `2d86006f871eddcafdcb90c899646bb19ef6fe8ac41f9055f5bce764d4d99eea`, equal to the earlier record |
| Wire targets | `git rev-parse 77f7edd:<path>`, `git rev-parse HEAD:<path>`, `git hash-object <path>` | identical for the five targets |
| Admission | presence of `handoff/kubernetes-v2-runtime/QA_ADMISSION.json` | absent at 2026-11-13T01:36:50Z and 01:37:57Z (`date -u`) |

### Status after the final pre-QA round

`QA_ADMISSION.json` is absent, so no `qa.sh` (not even `status`), docker,
podman or kubectl command was issued. Every test, `cargo check`, `clippy` and
`cargo fmt` stays `NOT_RUN`, any run that requires a cluster stays `BLOCKED`,
and no plan step was run. The QA proposal is `QA_READY.r3.json` on `d4c6dec`.
S2b and S5 are not implemented, layer C still has no production caller (R2),
and the lot is not complete under its mandate. Gates G05, G07 and G11 stay
open (G11 `BLOCKED`). No gate or risk is closed. The product is not
production-ready.
