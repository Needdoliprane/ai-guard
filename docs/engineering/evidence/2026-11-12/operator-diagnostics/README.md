# Operator diagnostics — composed qualification, 12 November 2026

The final source composition passes **nine exact binary configuration parities and 85 normal CLI cases**, plus the offline all-targets check. One manual exporter is ignored and **NOT_RUN in this final campaign**. Root independently verified all 301 raw-evidence hashes, the 2,552 source contents/modes and exact-container closure. This record does not transfer historical Claude or generation results to a new source, qualify a release or close a production gate.

## Delivered behavior and integration

`ai-guard-diagnose` produces actionable text and JSON reports from named installation/configuration inputs. It describes layouts, declared tenants/policies, provider selection and reference metadata, with explicit unknown, not-checked and not-available states. It grants no execution authority. Default inspection writes nothing and opens no network socket; referenced credential/store/setup-record contents are not opened. An explicitly supplied input is read according to its option; the guides document the limits when its credential role is discovered only after reading.

Root's bounded source review and corrections preserve:

| Item | Composed behavior |
|---|---|
| CEIR-01 | `relay_retention_policy` is accepted by the diagnostic mirror and checked with the real product validator. The ninth parity covers null, valid and refused inputs, including unknown fields. |
| CEIR-02 | Guidance distinguishes the diagnostic's unexamined audit data, online `verify-audit` with an explicit prepared viewer file, and offline signed-checkpoint verification with an independently held anchor. The diagnostic invokes neither verifier. |
| CEIR-03 | The source handoff refers to the recorded Clippy decision. Historical comparison of attributable warnings is not strict Clippy qualification. |
| Provider helper Q1 | Reuses pure product selection. Explicit subscription refusal does not select a paid API profile; credential content is not read by selection. |
| OIDC opt-in Q2 | `--active oidc-discovery` requests bounded discovery/JWKS only. The composed test uses synthetic loopback peers; no real IdP, login or token exchange is qualified. |
| D/E composition | The control Dockerfile retains builds/COPY for `agentsandbox`, `ai-guard-audit-checkpoint` and `ai-guard-diagnose`; both setup index entries remain. Image execution was not tested here. |
| JSON v1 | First composed publication baseline: schema `ai-guard-diagnose-report`, version 1, current axes including Trust and 132 catalogue codes. Two prepublication removals reserve their identities; no ID reuse or migration from an earlier public v1 is claimed. |

See the [English guide](../../../../setup/diagnostics/README.md), [French guide](../../../../setup/diagnostics/README.fr.md) and [original source handoff](../../2026-11-10/operator-diagnostics/README.md). Historical results remain attached to their original source and environment.

## Source identity

| Item | Identity |
|---|---|
| Incoming Claude source / documentary head | `469d6259c305c415695968334bf3a1eb5b355a5c` / `cf272b20ff126d75074d7347852c51454df58562` |
| Public base of final composition | `862e182fe3f9ff6a6028b2b2060bf36f83ba3cab` |
| Final E delta | 45 paths: 34 corrected generation postimages retained, 9 actual normalized reports promoted, 2 EN/FR count corrections |
| Complete input | 2,552 regular files, mode 0644; USTAR 29,221,888 bytes |
| Input archive SHA-256 | `c2b907a8d19cab26b49337a58f30de2e78966fbad9bc7a9a48ae32a8cd9362c5` |
| Input manifest SHA-256 | `270002cb771d72c810393fb87a162308545a59d8a288fc8f4fb3034ea3fa3adf` |
| Actual live-source export | 2,552 regular files; USTAR 29,399,040 bytes |
| Live-source export SHA-256 | `8a96898abef49150418d8ff86765b6d2971c2573664cb0a6bea333474120ec4b` |

The 2,507 public paths outside E45 remain byte-identical to the pinned public blobs. The freeze excludes private Core/Full3 work. Input and live export have different tar layouts; verified equality covers the path set, payloads and modes. Documentary additions accompanying publication are not represented as having been present during this source campaign.

## Final results and exact command forms

The admitted RUN built and resolved the actual lib, binary and CLI-test artifacts, checked their three real lists, then ran the nine selectors below in the **agentsandbox binary libtest**, not the library artifact. Each returned RC 0, one pass, zero failed and zero ignored. All names share the prefix `gateway_config::diagnostics_parity_tests::`:

```text
diagnostics_parity_envelope_field_names_equal_the_loaders
diagnostics_parity_registry_file_is_read_as_the_loader_reads_it
diagnostics_parity_registry_structural_rules
diagnostics_parity_registry_policies
diagnostics_parity_token_content_is_outside_the_restated_half
diagnostics_parity_oidc_file_envelope_and_validators
diagnostics_parity_oidc_references_and_ca_bundle
diagnostics_parity_authentication_policy_is_checked_after_the_file_loader
diagnostics_parity_relay_retention_policy
```

The normal CLI suite ran once, serially, and reported:

```text
test result: ok. 85 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 84.07s
```

All 86 distinct identities matched the expected list. `regenerate_documented_examples_into_tmpdir` was the sole named ignore; it was not executed or counted as a pass. No report reader or regeneration ran in this final campaign. `first_run_states`, `fresh_historical_directory_is_explained`, `documented_examples_are_the_real_output` and `guides_follow_the_binary` now pass on the promoted source. The suite also passes default no-network/no-mutation, secret-reference handling, explicit-subscription refusal, JSON/catalogue and `active_oidc_discovery_loopback` oracles. Synthetic peers/files do not establish real installation or provider availability.

These command forms ran inside the admitted QA. Angle-bracket names denote the exact artifacts resolved from Cargo JSON and listed before execution; they are not literal shell commands.

```sh
/usr/local/cargo/bin/cargo test --frozen --offline \
  --lib --bin agentsandbox --test diagnose_cli --no-run --message-format=json

# Once for each resolved lib, agentsandbox-bin and diagnose_cli libtest artifact:
<resolved-libtest-executable> --list

# Once per exact prefixed selector above:
<resolved-agentsandbox-bin-libtest> <exact-selector> --exact --nocapture --test-threads=1

# Normal CLI suite; no --ignored:
<resolved-diagnose_cli-libtest> --nocapture --test-threads=1

/usr/local/cargo/bin/cargo check --frozen --offline --all-targets --message-format=json
```

Build started at 02:57:20 UTC; all-targets check ended at 03:03:41 UTC and closure completed at 03:03:58 UTC. Build/check each have one successful Cargo `build-finished` record and no error diagnostic. The build bound was 1,200 s; lists and each parity 30 s, CLI suite and check 900 s each. These are QA bounds, not product latency promises. RUN, source verification/export and exact-CID closure returned 0. The stopped container reports exit 137, `Running=false`, `OOMKilled=false`; stop/removal success is distinct from that recorded exit status.

## Separate generation and promotion

Generation r2 used corrected source `a710fcab22604d457716d7988157b16b12c45437fa531701c81c16aceafbb9c9`, not the final `c2b907…` composition. Nine parities and the normally ignored exporter were explicitly executed (ten software parents PASS), followed by the all-targets check. The four normal example/guide parents were then NOT_RUN and remained so until the final campaign above.

The exporter wrote nine raw and nine normalized reports plus a manifest into a new private TMPDIR. A Rust reader with a closed list and bounded descriptor reads captured the 19 members. Root verified the physical capture; independent content review found no material defect in the five texts/four JSON examples, including counts, selection and explicit unexamined audit state. The only normalization replaces the package version `0.1.0` with `PACKAGE_VERSION`; no message or counter was reconstructed from old goldens. Nine normalized reports were promoted, with two EN/FR guide counts corrected from eight to nine not-checked items.

Generation r1 remains **FAIL_HARNESS**, before any parent: the macOS Bash 3.2 controller mishandled an empty argv array. r2 corrected that controller array using a nonempty reviewed form; the product source, selectors, bounds and QA profile were unchanged. This is not an expected product RED, a safety refusal or a retroactive PASS. The original Claude source history, historical warnings/results and raw evidence remain preserved.

| Separate retained generation item | SHA-256 |
|---|---|
| r2 RUN | `0d61a1b2ad1cbb6ed9ecbe03f8553d9ed6f2cf1c7aeebd74107b5a12bcc6ae80` |
| r2 raw-evidence manifest, 335 entries | `8d753d7dff280e746a8d5e5286f88b6a2f6e71f79dcd9e7252265a22ada51cb4` |
| Captured 19-member reports USTAR | `6b0795747f0449bedd84d5459567e0ecb0b73e82c63fd19430fa942ff83ccca6` |

## QA profile and retained evidence

Linux ARM64 image `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`, Rust 1.99.1, offline Cargo, jobs 1, incremental 0, dev/test debug 0. UID/GID 1000:1000, network none, readonly root, all capabilities dropped, no-new-privileges, 4 CPU, 9 GiB memory, container swap 0 and 128 PIDs. Inputs/public registry were mounted readonly; no engine socket, original project or real credentials were supplied.

Final cgroup memory was 2,097,934,336 bytes current and 7,221,514,240 bytes peak against a 9,663,676,416-byte limit. `oom`, `oom_kill` and `oom_group_kill` were zero. These describe compilation/QA, not worker consumption or capacity.

Raw logs and archives remain private under the identifiers `qa-claude-e-final-20261112-r1` and `qa-claude-e-generation-20261112-r2`. Root's capture/content review records remain private as well. No private absolute path or raw report is required to interpret this curated record.

| Final retained item | SHA-256 |
|---|---|
| RUN | `ebf973d388df26fe92ea58f5080631b4da21ae732ba1d0d14eec7372895f3f33` |
| Raw-evidence manifest, 301 entries | `7d360df9095b29b90d387234007a4199a8c3d5397a7b7baaecafef7d79f24f31` |
| Luna structured result | `7820f8e0e958261375188154bc59323aae1527dd4295e7e6c0361708dbced02c` |
| Normal CLI suite stdout | `1e33f17322f550731252e621ed2a1281ff12531bace60f10c2329b0ebf33be41` |
| All-targets Cargo JSON | `5e3600a53c83b5b18583f95d3da220be5f47ce9efda4d36700c91069264e7535` |
| Root independent final verification | `f25f4fcf30c19d74e01fe997ecfadbd8e44c1d9488b61fb39051da46fe839285` |

The Root verification was written after the 301-entry manifest and is not covered by it. Its separate digest above identifies the independent check; no historical manifest was rewritten to include it.

## Qualification limits

- No real Docker/Podman/Kubernetes business execution, worker launch, actual provider/model, subscription/API availability, real IdP/enterprise TLS, native macOS, control-image build/execution, release or complete repository suite was qualified by this lot.
- Strict Clippy was not run on the final composition. Historical non-strict warning comparison stays historical and cannot satisfy the strict gate. A successful all-targets check is not a warning-free qualification.
- The diagnostic does not establish store schema/integrity, signed-checkpoint validity, independently anchored rollback continuity, recovery-record content, authenticated permissions or launch readiness merely from a recognized layout. Those require their own consumers and proofs.
- No global audit finding or gate G01–G12 closes. G09 remains FAIL and G12 BLOCKED; the [gate register](../../../PRODUCTION_READINESS.md), [validation](../../../VALIDATION.md) and [delivery register](../../../CLAUDE_DELIVERIES_20261110.md) retain their distinct roles.
