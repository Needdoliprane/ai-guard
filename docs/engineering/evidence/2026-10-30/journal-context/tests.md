# Journal managed-context and live anti-rollback tests (corrected)

Worktree: `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`; base commit `537f8d2c072139bb4ced0f3b7bd385f15fb1e9dd`. Contract read: `docs/engineering/BROKER_JOURNAL.md`, especially rev0 initial-checkpoint, context validation, CAS, live writer floor, and explicit lack of post-restart anti-restore guarantees.

## Correction to the earlier report

The first two passing tests and the prior version of this report used the wrong worker-owned handle operation ID: the fixture set it to `worker.attempt.attempt_id`. That made those results invalid as proof of the real runner checkpoint. The real producer `src/runtime/managed.rs` copies `ContainerSpec.operation_id` to the handle; the plan creates that spec with `plan.operation_id`. `worker.attempt.attempt_id` is a distinct Store UUID. Root confirmed the recovery consumer already validates `plan.operation_id`.

I corrected the fixture to set the worker-owned handle operation to `plan.operation_id` and added a negative case that sets it back to the distinct attempt UUID. The pre-fix filtered test then failed both managed flow tests at `Journal::advance` with `Conflict`, after rev0 initial acceptance had succeeded. Root changed only the context comparison in `src/runner.rs` from attempt ID to intent/plan operation ID. After this fix the filtered tests and full broker-journal target pass. The earlier passing report must not be used as evidence for the corrected handle identity.

A requested “managed checkpoint accepted directly as rev0” positive was not added: the contract and `Journal::accept` require rev0 to equal `CheckpointPayloadV1::initial(intent)` (`managed=None`). A test now checks that trying to accept `managed=Some` at rev0 returns `Conflict`, leaves `accepted.json` absent, and loads as `Interrupted`. The valid managed checkpoint is advanced from the required initial rev0, then loaded and reopened.

## RED evidence before the application fix

Command: `cargo test --locked --test broker_journal journal_context_managed_checkpoint_ -- --nocapture`; exit code 101. Output showed 2 failing tests: managed advance returned `Conflict` in both the accept/load/advance/reopen path and the live-floor setup. The baseline Store-derived checkpoint builder and initial acceptance had succeeded. This was a real context mismatch caused by the wrong comparison in the product consumer, not a fixture parse failure.

Raw stdout, stderr and exit code plus unchanged before/after source manifests are preserved in `/tmp/ai-guard-journal-ownedop-red-20261030/` (`retry.stdout`, `retry.stderr`, `retry.exit-code`, `retry-before.sha256`, `retry-after.sha256`). Test source at that run was SHA256 `c7e65ab5cdab196798e13a538fd7abe9b236ed2cce2310f62d81764726b87121`; product runner before fix was `3b7f949519bc7f8bcd2f1b504e97e09ad1950f48de7c03c80aed0e660deb3805`. The first compile attempt in `target.stdout/stderr` had two test-only compile mistakes (array size and moved `String`); those were corrected before the behavioral RED and are not product findings.

## Final targeted coverage

Corrected two-test filter:

`cargo test --locked --test broker_journal journal_context_managed_checkpoint_ -- --nocapture`

PASS: 2 passed, 0 failed, 0 ignored, 10 filtered. Raw logs/manifests are in `/tmp/ai-guard-journal-ownedop-red-20261030/` (`green-two.stdout`, `green-two.stderr`, `green-two.exit-code`, before/after manifests); pre/post manifest hashes match (`9b99e0dd3ab0f14dad4d8b8045862fb93a44992c1743fb7f13d190cfd9360f53`).

The Store-backed managed checkpoint fixture creates a real SQLite decision, admission, worker, `Prepare`/`CreateIssued`/`Created` records, and a structurally valid private authority/owned snapshot; it starts no runtime. It proves:

- `accept` accepts only the exact required `initial(intent)` rev0. A managed payload supplied directly at rev0 is refused, with no `accepted.json` marker.
- The valid managed checkpoint advances to rev1, can be loaded at rev1, and is recovered after dropping/reopening the journal.
- Six parseable managed-context mismatches are refused on `advance`: tenant, origin, worker spec hash, worker-owned handle operation set to the distinct attempt UUID, handoff digest, and pending worker identity. The head remains at rev0. A mismatched journal scope origin refuses `accept` with `Conflict`.
- In a separate valid managed job, the test saves the real rev0 `current.json`, receives ACKs at rev1 and rev2, then restores that older valid pointer in place. Both the live writer and its `Journal::clone()` refuse `load`, exact `reserve`, and `advance` with `Conflict`. After dropping both and reopening, the valid old rev0 graph is readable and replayable. This is the documented positive control: the floor is process-local and no post-restart anti-restore guarantee is claimed.

## Final suite and static/build checks

- `cargo test --locked --test broker_journal`: PASS, 12 passed, 0 failed/ignored. Log `/tmp/ai-guard-journal-ownedop-final-20261030/journal.stdout` and `.stderr`; exit code in `journal.exit-code`. Before/after full source manifests match (SHA256 `65bac77362cb0db663c16f1bc75bb52c88e4df4164858a507a670d8f4c3b943e`).
- `cargo fmt --all -- --check`: PASS, exit 0, logs `fmt-final.stdout/stderr`.
- `cargo clippy --locked --all-targets -- -D warnings`: first final attempt found only a test-owned `type_complexity` lint; the test now uses a named type alias. The rerun passed, exit 0, logs `clippy-retry.stdout/stderr`.
- `cargo build --release --locked`: PASS, exit 0, logs `release.stdout/stderr`. Release executable SHA256 `b884e84305f125cdaae610881ecc4e747f3ede0a5e634e4cf3391b5a01482ec6`; `libagentsandbox.rlib` SHA256 `3c520d88d91cc2fd465d9435097c9c55cb02814ae18921a24ae31d10edaac670`.

Full manifest includes Cargo files, `rust-toolchain.toml`, and tracked/untracked non-ignored files under `src/` and `tests/` (111 entries). Release pre/post manifests match exactly (SHA256 `96c711ac7ebad5fbd78e9cf5214ec24ef1ab5f8407e02c61c1351942400ff8d2`), at `/tmp/ai-guard-journal-ownedop-final-20261030/release-before.sha256` and `release-after.sha256`. Final test-source SHA256 is `c5e5c002859356d404866578bc545d48377df7e8c9da093f7eae4f6195d23536`; final runner SHA256 is `9f5861e9e2114846f852525a393554c98e1e5b91de3f534a8c70a6dfe7b4eabb`.

## Scope and limits

These are focused macOS file/process tests of journal semantics, not proof of a real runtime start. No Docker, worker process, engine, provider, model, E2E, full test suite, or Linux run was used. The SQLite audit Store creates records only for the fixture. Power loss, privileged actors, restart fencing, and post-restart anti-rollback remain outside this evidence.
