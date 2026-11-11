# Explicit viewer credential for the SetupV2 audit check

`agentsandbox verify-audit --viewer-token-file FILE` reads the specified private
viewer credential through the existing validated token reader. Without the
option it retains `<config-dir>/secrets/viewer`. The gateway selects the tenant
from the authenticated credential; the CLI does not accept a tenant selector.
English and French local, SSO and checkpoint guides describe this choice.

The candidate starts at `e95724a0cedbf1b2cbfed6c1e7527c4a0cb57807` and changes
nine paths. The frozen regression is identical for RED and GREEN:
`verify_audit_uses_setup_v2_viewer_token_and_preserves_legacy_default`, test-file
SHA-256 `8eee0d26d3941100c1770f805ab76363be355b9bb1d9936bdd5668b2ef3c0bf9`.

| Check | Observed result |
|---|---|
| RED build and exact list | RC0; one matching test |
| RED execution | RC101 after the legacy positive control; the CLI rejects the unknown explicit option with exit code 2 |
| GREEN `cargo test --frozen --offline --test audit_signed_checkpoint_verify_audit --no-run --message-format=json` | RC0, `build-finished success:true` |
| GREEN direct binary exact list | RC0; one matching test |
| GREEN direct binary `--exact verify_audit_uses_setup_v2_viewer_token_and_preserves_legacy_default --nocapture --test-threads=1` | RC0; 1 passed, 0 failed, 0 ignored, 0.31 s; executed once |
| GREEN `cargo check --frozen --offline --all-targets` | RC0; recorded start 11 November 10:49:44 UTC, exact end instant not captured |
| Scoped rustfmt and staged diff checks | RC0 |

The test prepares a real SetupV2 installation, starts the Rust gateway on
isolated loopback and queries two SQLite tenant stores. It first verifies A
through the legacy layout, then B through the explicit file, asserting that
both valid audit heads match their respective stores and differ from each other.
No provider, worker runtime or external identity provider participates.

The 2,413-file GREEN source manifest is
`bec47b85f39f20ed6a22b68d930f73b34d7c7a248d5ef6f58f505483df671394`;
its input archive is
`cb2db56e66b6139750475948a0850eafef3b30eb5e5680177de083405e8a33ef`.
Independent source review accepted the delta, including token validation,
legacy compatibility and the unchanged optional signature path. All nine
integrated postimages match that input manifest. The 124 independent working
changes were checked unchanged before and after applying the patch.

The QA used Linux ARM64, Rust 1.99.1, UID/GID1000, a read-only root and inputs,
no network, zero capabilities, no-new-privileges, seccomp, private scratch,
CPU/memory/process/file-size limits and explicit empty-environment launches.
Source and five public dependency pins were verified before compilation. The
first GREEN build expired before test execution and is not counted here. A
later harness grep expected fields the passing test does not print; it stopped
after the test, which was not repeated. The all-targets check then ran once.

Capture limits: individual GREEN build/test start instants were not recorded.
The output modification times are not command start/end captures. In particular,
the check's `end.utc` file was derived from a modification time and is not used
as an actual end timestamp.
The source was **not exported after the GREEN execution** (`POST_NOT_CAPTURED`);
the input archive is not a post-run observation. Removal was observed in the
tool transcript and absence of the exact container was subsequently verified;
no missing stop/removal log is reconstructed. Raw results and the independent
review remain in the private engineering archive.

The optional signed verdict, unreadable explicit-file rejection and precedence
when both layouts exist were reviewed in source, not exercised by this test.
This does not qualify signed SetupV2 operation, authentication-audit V4
signatures, OIDC, restoration, runtime isolation or a release. See the
[checkpoint evidence](../../2026-11-10/audit-signatures/README.md) for the
separate signed-verification scope.
