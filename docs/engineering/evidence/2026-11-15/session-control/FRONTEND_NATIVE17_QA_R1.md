# Native17 frontend reauthorization — targeted QA R1

Date: 2026-11-15. Scope: the frozen frontend source only, not the backend/session/runtime composition.

- Source USTAR SHA-256: 1c4fcb36c239ed5a27904e514fb98680c87f6c95cbe28482095f979a69396953 (87 regular source files).
- Command sequence: npm ci --offline with locked public cache; seven new reauthorization tests; 46 canary tests; TypeScript/Vite build.
- Actual result: 53 distinct active tests PASS, 0 FAIL, 0 SKIP/TODO; build PASS. The earlier full suite was not rerun in this selection.
- New cases cover explicit reauthorization from current options with blank intent/new identifiers, and preservation of lookup-only recovery for an unknown admission. An old task or old provider selection is not silently resubmitted.
- Strictly parsed evidence archive: 0548d0a161646d277904f65fb013f2c9bb79fc6f2b7a69d1fc9332bcd37080ad, 614400 bytes, 125 regular files. Root checked raw TAP names, registration and terminal codes, not only a summary.
- Root receipt SHA-256: 4971882761e6d895c7bd25881327fee9fc652cb781c4732de872a7def3f83649. Luna terminal handle: 51097, RC 0.
- Actual QA: pinned Linux ARM64 public image, UID 1000, network none, root read-only, dropped capabilities, no-new-privileges, bounded tmpfs/PIDs, one public read-only input mount. Twelve admitted execution slots terminated; no OOM observed. Exact QA container closed and removed, absence independently verified.

Browser, live dashboard/gateway, provider, Docker/Podman worker and Kubernetes scenarios remain NOT_RUN for this delta. This does not close a production gate. Product version remains 0.0.1.
