# Human source validation — 13 November 2026

[Français](README.fr.md)

**Targeted PASS on the public reduced candidate:** eight exact canaries and the all-target check. This result belongs to the frozen composition below. The private identity GET result is separate and is not transferred to it.

## Sources and results

| Composition | Source archive SHA256 | Verified result |
|---|---|---|
| Earlier private cost diagnostic, 2614 files | `9dfe8a38ea7c1d11d9c16ea67b0e2b5aab1d0c11e1ab3fc5a072a5c0bfe1271c` | Diagnostic capture complete; business HTTP503, not availability PASS |
| Private baseline, 2614 files | `866a0e5527a34d55aa4766229abd51960ab18b7cd5f96e07c3c7c346f276cfd0` | One real identity GET: HTTP503, expected assertion failure, parent RC101 |
| Corrected private candidate, 2614 files | `3de3abe4c891c3a050d523cfe4545d6e375481eccdc3303f1b5f6a62b30e27a3` | Same frozen GET parent: HTTP200; nine PASS including eight canaries; all-target check PASS |
| Public reduced candidate, 2622 files | `8c97f261f299922123ff161f1b4938e916e1a17b6751e318650dcb81708c9769` | Eight exact canaries PASS, zero failure/ignored; all-target check PASS |

The public candidate is base `ceb7b1f1c298a0f31044eca20a09cb2101c4d7c2` plus four reviewed postimages. It contains neither the private owned API module nor its GET acceptance parent. Its result does not qualify that HTTP path. The baseline orchestration returning zero meant that the expected RC101 failure was classified; it was not a test PASS.

The earlier instrumented GET took **2301.7 ms** around the complete in-process helper and returned `503 identity_unavailable`. It recorded **9498 file revalidation calls**, **131556 directory revalidation calls**, and **19 `BeginChecked` entries**. These are calls, not distinct files/directories or successful commits. Store-call durations were not measured. FS spans are inclusive and overlap: do not sum their durations or bytes into a timing breakdown or physical IO total. This single observation suggests repeated work; it does not establish its complete timing cause or frequency.

## Candidate change and preserved checks

The reviewed source-validation path goes from **14 ancestor walks to 7**, as a code fact. Already fresh observations are passed through the same compound validation rather than repeated between helpers. This change adds no success cache or retry and establishes no latency guarantee.

| File | Reviewed postimage SHA256 |
|---|---|
| [src/control_fs.rs](../../../../../src/control_fs.rs) | `13f583213890b23d891c42f8eab6d561a886b2d3c94b336122fa1dde08601447` |
| [src/control_fs/source_owner.rs](../../../../../src/control_fs/source_owner.rs) | `1d649dd776d81fc53b79a90b1df0507f57e214b37111dec10fc185d3707cb3f5` |
| [src/human_authority.rs](../../../../../src/human_authority.rs) | `21474eb338b1444bb4c6af9aa792672a60d75ae6a933a96c3d3aedc0566ad3e2` |
| [src/human_authority/tests.rs](../../../../../src/human_authority/tests.rs) | `2bfb62a0c960376cc6f29df38981321842e4af37e9a634b219208f29b5e0a139` |

Symlink, DAC/ACL, parent/entry identity and double mount-domain observations remain enforced. The two-second epoch, authorization and fail-closed behavior are unchanged. Initial-parent I/O observation failures keep the unavailable branch (`StorageUnavailable` on acquisition); entry/mount binding failures keep `StorageBindingMismatch`. Original error causes remain available to the structured EMFILE/ENFILE classifier. Descriptor shortage still refuses without adding an invalid-owner latch. Its canaries inject structured errors; no real descriptor exhaustion was performed.

## Exact selection and command forms

The selection contains four new R1/R2 `human_authority` canaries, two existing `source_owner` canaries and two existing epoch/token neighbours. They are not eight new tests.

```text
human_authority::tests::source_mount_validation_keeps_exact_parent_and_entry_bindings
human_authority::tests::an_accepted_epoch_rejects_parent_substitution_after_a_successful_source_check
human_authority::tests::source_mount_failures_keep_parent_unavailable_and_entry_binding_categories
human_authority::tests::initial_parent_observation_phase_keeps_structured_descriptor_shortage_causes
control_fs::source_owner::tests::same_mount_private_source_exposes_actual_parent_and_entry_identities
control_fs::source_owner::tests::same_mount_source_keeps_private_entry_alias_and_permission_guards
human_authority::tests::an_expired_epoch_does_not_latch_unrelated_future_operations
human_authority::tests::retained_token_revalidates_under_an_existing_write_epoch_without_reacquiring_gate
```

Recorded command forms, inside the admitted isolated QA only; `BIN_LIB` is the actual Cargo JSON library-test artifact, checked for name/kind/profile/path. `SELECTOR` is one exact name from the list. Each selector ran once after its exact list; zero selected tests never counts PASS. Harness ceilings were 1200s for build/check, 8s for each list and 45s for each parent; the business deadline was unchanged.

```sh
cargo test --frozen --offline --lib --no-run --message-format=json
"$BIN_LIB" "$SELECTOR" --exact --list
"$BIN_LIB" "$SELECTOR" --exact --nocapture --test-threads=1
cargo check --frozen --offline --all-targets --message-format=json
```

The public check reports `build-finished: success=true`, zero errors and **353 warning diagnostics**. Strict Clippy was not run. The 2622 post-run payloads/modes match the frozen source; source-export SHA256 is `6c308d094459e68d40860b109d43476d57e53abe266cf3788c99d92320ffac0f` (payload/mode equality, not identical tar packing). Root verified 337 raw-file hashes. Collection/closure commands returned zero, no OOM was recorded, and the owned container was absent afterward. The holder exit code 137 is recorded separately from test exits.

## Evidence identifiers and limits

Private records remain private; only their verified SHA256 identifiers are published here.

| Record | SHA256 |
|---|---|
| Cost diagnostic receipt | `9dab51495bf4294fae5e03ed3110ca09f3587a63451023af745d19f96c7c55ed` |
| Baseline receipt | `6e6fadf10fa12d48869138b97c2ae747a4450f3c056c1606213c5bdb92876b43` |
| Corrected private receipt | `db786d559aed13a560c41f795c8765b2c62f8411a8058e0c6b1e16535ce3ed4d` |
| Targeted R2 source review, GO_SOURCE | `6623413665ce93105fb8161aa38a10aa321233446b83d244a78ab631c1e5319d` |
| Public reduced result receipt | `3dda47fd774a11765e07d7ff5284af5ac8a86fc341814ca78ed0c21aa267a97f` |

The R1 NO_GO and the R2 review's historical NOT_RUN fields remain unchanged. Later executions are separate evidence. No general latency, availability, storage-failure or production qualification follows. Real SSO/IdP, providers, workers/business runtimes, the full suite and Clippy remain NOT_RUN in this lot. No audit fingerprint or production gate is closed. See the [architecture contract](../../../ARCHITECTURE_CONTRACT.md), [test strategy](../../../TEST_STRATEGY.md) and [production gates](../../../PRODUCTION_READINESS.md).
