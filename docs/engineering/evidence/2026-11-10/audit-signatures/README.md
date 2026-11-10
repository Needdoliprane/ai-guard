# Signed audit checkpoints — composed software validation

This curated record keeps the original Claude delivery distinct from the integration and runtime qualification. The [delivery register](../../../CLAUDE_DELIVERIES_20261110.md) preserves its original references and private evidence.

## Source and independent review

Original Claude A: `ad9367d29607f54114d518632e97e61a137d4ccc`, base `7672078`; historical validated source `6a05ca8`. Integration source `197b246587fadd339b3b795da7b5e439224103b0` composes the component on `2cc29ee`, then corrects unchanged-continuation destination validation (S4), adds the explicit 27-line CLI wire, and extends the migration regression through schema12. No older central module is substituted. Independent bounded source review SHA-256: `ee60eb46466e57efd740b0c4d904b35c6a86a0826a3facb4832c3176886326f2`.

The executed FIX freeze has **2,403 regular files**, manifest `600148e68e3fe7a9929cb95732e38a9d9eb918a1dd487a4071d28550f401938a`: public `8697e54` plus A's 28 paths and four documentation-curation paths. It does not contain Root's uncommitted Full3 source or the newer Kubernetes composition. The RED freeze `4c8cfefdc172443b94be32744b30f81d5369f7f91f6c258cf27eb8faf6450480` differs only in the old S4 implementation of `create.rs`; the regression test is identical. Public publication preserves the exact tested Rust files. Two later EN/FR format-guide sentences correct the scope of rotation checks; these are documentation-only changes, not new execution.

## Actual results — 10 November, UTC

| Status | Command or exact selector | Observed result |
|---|---|---|
| PASS | RED CLI target build and direct list | Build RC0, 21:46:24–21:48:54; 14 listed tests. |
| FAIL, expected RED | `unchanged_continuation_checks_the_destination_without_writing` | RC101 at 21:49:59: incomplete-record count 0 versus 1, after absent/empty controls. Later RED cases were not reached. |
| PASS | FIX three integration targets build and direct lists | Build RC0, 21:51:48–21:54:53; CLI14 / main51 / wire1 listed; 82 build warnings retained. |
| PASS | `unchanged_continuation_checks_the_destination_without_writing` | One passed, RC0, 22:01:03. Real CLI; destination refusal and absence of writes. |
| PASS | `verify_audit_renders_the_signed_verdict_only_when_asked` | One passed, RC0, 22:01:11. Plain hash-only control and explicitly requested signed verdict remain distinct. |
| PASS | `checkpoints_stay_verifiable_across_schema_migrations` | One passed, RC0, 22:01:19. Schema12, control genesis and signed history. |
| PASS | `modified_statement_or_signature_is_refused` | One passed, RC0, 22:01:27. |
| PASS | `nv01_coherent_offline_rewrite_after_signature_is_refused` | One passed, RC0, 22:01:35. |
| PASS | `rollback_of_log_and_database_is_detected_against_independent_reference` | One passed, RC0, 22:01:42–22:01:43. |
| PASS | `cargo check --frozen --offline --all-targets` | RC0, 22:01:52–22:03:21; existing warnings retained. This is not strict Clippy. |

Each FIX selector ran once with `--exact --nocapture --test-threads=1`, a 180-second process timeout and 15-second kill grace; no complete-suite/provider rerun. Builds and check used `--frozen --offline`, exclusive RED/FIX targets and an empty environment plus the recorded toolchain/cache allowlist. Linux ARM64 Rust1.99.1 QA used pinned image `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`: UID1000, read-only root, no network, private scratch, dropped capabilities, no-new-privileges, seccomp, CPU4/RAM9GiB/no-swap/PID128 limits, and OOM priority500. Synthetic test keys and fixtures are not an external PKI integration.

Raw stdout/stderr, UTC start/end and actual exit status, source archives/manifests, cache pins and admission records remain private in `qa-signed-checkpoints-20261110-r1/`. They are not published wholesale. Claude's historical acceptance51/CLI13/unit6 and neighboring suites remain historical on their named source and are not counted as new composition passes.

Post-check revalidated all 2,403 FIX files, their exact path set, zero nonregular entries and the four cache pins. Actual RED/FIX archives (25,251,840 bytes each) match their manifests: `fa44948f53e4471ec6945aa9c0095df726a502fc0a041c1793837fb7d09ebf5c` and `8f06933d97d42a92eb2add893b62368c94b57b4213b4598ce1e09ba8fbd9a64f`. The QA reached 6,237,265,920 bytes of cgroup memory peak with zero OOM events; after capture only its idle sleep remained. It stopped at 22:09:50 UTC (idle exit 137, OOM false), was removed normally, and its exact ID is absent. Watch RC0. Private closure SHA-256: `7d6dbf0b28d84d55f91f14233881096b3a9fb1ab7c8db4b135f3828811077454`.

## Remaining boundaries

The offline signer covers tenant Store chains. It does not sign authentication V4, configure an independent trust holder, or make the HTTP integrity route verify signatures. The optional `verify-audit` checkpoint arguments use the legacy configuration layout; SetupV2 resolution and a combined authority verdict are not delivered by this wire. Without checkpoint arguments the existing command remains hash-chain-only.

Image packaging is retained in source; control-image build, Compose journey, actual Docker/Podman/Kubernetes, provider/SSO, multi-process writer and restoration qualification remain NOT_RUN here. Independent rollback continuity requires an independently held trusted anchor; signing a file on the same compromised authority does not establish it. Control-genesis uniqueness remains a separate open observation. NV-01 is partially treated, no primary audit verdict is silently rewritten and no production gate closes. See the [audit register](../../../AUDIT_FOLLOWUP.md#registre-des-audits).
