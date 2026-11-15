# Composed baseline diagnostic — compiler check R2

Date: 2026-11-15. This is the initial composed baseline with five minimal compiler repairs, not the later live API/runtime/Presence/E2 candidate.

- Source USTAR SHA-256: 5beef547e333364ec65e65cefb980591a4b528cd71583ae24340550c22d5d037 (3,147 files). The five overlays repair module paths, missing imports and visibility without changing the associated constants.
- Command: cargo check --all-targets --frozen --offline --message-format=json -j1, exactly one actual Cargo invocation.
- Actual result: FAIL, Cargo/host RC 101. Four coded errors remain: E0609 three times (removed review_binding_id field in broker/policy) and E0308 once (typed revalidation error returned as anyhow::Result). Two additional code-less summary records are not two extra defects. Forty warnings; build-finished=false. Cargo stopped in the library; no test was executed.
- The prior 12 coded diagnostics are not transferred to the live tree. A root-local ?/Ok(()) conversion has since been applied to the live credential revalidation; it remains uncompiled after this change. The API owner is correcting the policy union without dropping its identity guard.
- Strictly parsed evidence archive: 8fb71ec9e713feae6be9cce9797dea7163d4230994bc92e89474c505b57161e5, 2835968 bytes, 54 regular files.
- Root receipt SHA-256: 59ef0537e27ad8346812aa2ae9bbcd9ad3a61ef233dab4e3c08792b124107fd3. Luna terminal handle: 99382, RC 101.
- Collector/source/public cache/three sandbox guard checks PASS; three actual execution slots returned 0/101/0. No cgroup OOM observed; measured peak 4,845,662,208 bytes under the 10 GiB cap. Exact QA container closed and removed, absence independently verified.
- A first host preflight returned 75 because Root supplied the wrong receipt filename. Before any container start or Cargo execution, Root verified State=created, preserved its original claim and supplied the required receipt. The resumed orchestration ran the single admitted Cargo invocation. This was a harness error, not a product behavior.

All tests, new live composition, real runtimes and provider paths remain NOT_RUN. No production gate is closed by this diagnostic.
