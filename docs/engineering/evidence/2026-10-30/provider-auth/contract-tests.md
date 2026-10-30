# Provider authorization test baseline

Status: RED reproduced against the immutable pre-contract release library.

No `.env` file was read. No repository or test source was modified.

## Preserved immutable candidate

- `target/release/libagentsandbox.rlib` copied to `/tmp/ai-guard-provider-baseline.jRQ2l0/libagentsandbox.rlib`
- SHA-256: `0958aa6e9f1c08f61306e47abd966c5d65377ad58f855cf977bda44a855b496f`
- serde_json dependency: `target/release/deps/libserde_json-0c9640e1c5119a6c.rlib`
- serde_json SHA-256: `71ed5438e41acae9894453dbe11b0d05560af26bc1e8c6dca432adc98c896647`

## Build and result

Rust `1.99.1 (58cf8f9c3 2026-10-13)`. Plain direct-rustc linking initially failed because Apple ld's LLVM21 LTO reader could not parse LLVM23.1.1 bitcode. Retried with only `-C lto=thin -C opt-level=3` added; compilation succeeded.

Command:

```sh
rustc --edition=2024 -C lto=thin -C opt-level=3 --test tests/provider_authorization.rs \
  --extern agentsandbox=/tmp/ai-guard-provider-baseline.jRQ2l0/libagentsandbox.rlib \
  --extern serde_json=target/release/deps/libserde_json-0c9640e1c5119a6c.rlib \
  -L dependency=target/release/deps \
  -o /tmp/ai-guard-provider-baseline.jRQ2l0/provider-auth-red
```

Result: **2 FAIL, 1 PASS**, exit 101. Both provider-resolution tests fail during deserialization with `unknown field provider_resolution` against the old `DecisionRequest` schema. The historical request/admission serialization and pinned hash test passes.

Captured output: `/tmp/ai-guard-provider-baseline.jRQ2l0/test-output.txt`, SHA-256 `51e2f3bbd32672505cc237895bbc70b4aed500eb9988a00d88075ee1c233df24`. Test binary SHA-256 `c932ebddb503099cb6e1b5e2a8cff00073c73a482cfa0cedbd5094da39c6ac9d`.

## GREEN run

After root's explicit go-ahead that the contract changes were in place, ran exactly once:

```sh
CARGO_INCREMENTAL=0 cargo test --locked --test provider_authorization
```

Exit 0: **3 passed, 0 failed, 0 ignored**. The compiler emitted one warning: `BootstrapArguments.prompt` in `src/provider/selection.rs:141` is never read. This targeted test result does not qualify provider CLI execution or any runtime/provider integration.

## Expanded targeted regressions

After root's authorization, ran once:

```sh
CARGO_INCREMENTAL=0 cargo test --locked --test provider_authorization --test worker_registry --test workers_api --test decision_idempotence --test runtime_authorization --test credential_handoff
```

Exit 0: 72 passed, 0 failed, 0 ignored across the six requested targets: provider_authorization (6), worker_registry (36), workers_api (6), decision_idempotence (9), runtime_authorization (8), credential_handoff (7). No failures or compiler warnings appeared in this run. Runtime/provider engines were not launched.

## Legacy preset authorization RED (2026-10-30)

Root requested one execution of the existing focused case before adding the production guard:

```sh
cargo test --locked --test provider_authorization managed_decision_and_worker_reopen_with_exact_provider_identity -- --exact --nocapture
```

Compilation succeeded (8.71s); test behavior failed as expected: `unbound launch field: legacy_preset`, one test failed, five filtered out, exit 101. This is a behavioral RED, not a compile failure. Captured log `/tmp/ai-guard-provider-legacy-preset-red.log`, SHA-256 `cb7f8f970bb434e878d00e5e57a00e1686fffe35eb17e179e83fbc5f1bb99e81`. No source/test changes by this reviewer.

## Legacy preset GREEN attempt after guard

After root added the `requires_no_engine_logs()` launch guard, attempted the authorized targeted command once:

```sh
CARGO_INCREMENTAL=0 cargo test --locked --test provider_authorization --test worker_registry
```

Status: BLOCKED before tests ran (exit 101, 3s). Concurrent source state did not compile: `src/runner.rs:547` references missing `crate::provider::load_config_source`. This is a compile error, not a behavioral regression result; neither suite ran. Captured log `/tmp/ai-guard-provider-legacy-preset-green.log`, SHA-256 `6335d00620ff1d83ee428490e485c06c10084a44d73a496d94a22f0a89375c72`. Per instruction, no retry until core/runner state is declared stable.
