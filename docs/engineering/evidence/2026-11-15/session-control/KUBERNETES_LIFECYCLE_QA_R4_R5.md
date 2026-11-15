# Kubernetes lifecycle/attach — targeted software QA, 15 November 2026

These are software-only results on frozen sources. No Kubernetes cluster, Docker/Podman worker, provider or model was exercised. No production gate is closed.

## Initial campaign: R4

Source commit `3b7aecfc2caa06fe301b7465443fef5ebdae6788`, source archive SHA-256 `8904cdd875a8f2e5df33970e4cfd260eb4a7298b9d46f78e15e42e28a4a0eba5` (3,055 files).

- `cargo check --all-targets --frozen --offline --message-format=json -j1`: PASS.
- `cargo test --frozen --offline --lib --no-run --message-format=json -j1`: PASS.
- Seventeen registered active exact software cases: **16 PASS, 1 FAIL**. The real CLI case returned all five required closed V2 reports.
- The failing case, `api::kubernetes_sessions::lifecycle::tests::changed_reservation_and_wrong_broker_cannot_overwrite_the_original`, stopped in its fixture before the intended HTTP assertion: a second broker reused an already-owned authority identifier. The store correctly returned a durable-content conflict. This historical failure is retained.

The successor commit `0d011697a84097205f02e54b81c4f4d9acf5556b` changes only that test fixture (+27/-2): it keeps the conflict/audit/owner assertions, then uses a distinct broker authority and real grant/authentication to reach the forbidden HTTP request. The other 3,054 source blobs and modes are identical. Production authorization was not relaxed.

R4 evidence archive SHA-256 `86b6f91fc5a6669f1eb838024dd863b5fbec909fc24dce6e7048f55e289f6a14`; Root verification receipt `0ae5ff28cce07fb68ea85f95342f853ef417442eb5f31829983c6aaaa96af6aa`. All 24 execution slots terminated; collector and source/cache guards passed; no OOM was observed. Peak observed QA memory was 10,262,073,344 bytes. The exact QA container was stopped and removed.

## Targeted retest: R5

Source commit `0d011697a84097205f02e54b81c4f4d9acf5556b`, tree `f5f3f807f1268851d7de72419971e13f0a537ce2`, source archive SHA-256 `46969f4442663f6faab3f579678771da5405a29a4c8e86f2ab37b62386254ffe` (3,055 files).

One offline/frozen libtest build used the same command above. The real artifact list and ignored list established two unique active full test names. Each was invoked with `--exact <full-name> --test-threads=1`:

1. `api::kubernetes_sessions::lifecycle::tests::changed_reservation_and_wrong_broker_cannot_overwrite_the_original`: **PASS**.
2. `api::kubernetes_sessions::lifecycle::tests::authenticated_tenant_b_cannot_select_tenant_a_by_body_or_header`: **PASS**.

R5 is **2 PASS, 0 FAIL, 0 selected ignored**, not a new 17-case campaign. R4 results are not transferred to the successor as an executed suite. All eight execution slots terminated, none remained unknown; collector, source/cache guards and no-OOM checks passed. Root reparsed the strict evidence archive, checked the raw exact-test output and process return codes against the host copies, and independently verified that the exact QA container was absent.

Evidence archive: 3,453,952 bytes, 103 regular files, SHA-256 `d8b0851bf681fb11435a3d7ea92f143b1213b1dc1e9cc6e19c9bb73a44317ed3`. Root verification receipt: `1923edc995fb99023341c88f5e5f024e835915117902b3432458effbbba7ef46`. Private raw evidence is retained under the engineering recovery directory in `kubernetes-attach-qa-r4/` and `kubernetes-attach-qa-r5/`; no credential or raw private configuration is published here.

## Limits

R5 did not rerun all-target checking. Rustfmt and Clippy binaries were unavailable in its isolated toolchain; formatting and Clippy remain **NOT_RUN**. Real cluster/runtime behavior, image builds, attach against a Kubernetes API server, provider authentication, workload isolation, recovery under failure and capacity remain **NOT_RUN**. These results do not qualify the composed native session-control candidate or the published branch.
