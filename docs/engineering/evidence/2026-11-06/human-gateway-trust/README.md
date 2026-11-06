# Human CLI gateway trust — 6 November 2026

Scope: an explicit public CA bundle for the HTTPS gateway, in both named local
access and foreground OIDC access. Source commit `68f0fb4d4c01d50a2c82b304f82c8075d6ad761e`,
composed as `f295908bd098f7712d5f4f329898f91d90093ce2`, from base `d3300641`.
The fourteen source/test/guide files in the composition match the tested gel.
The canonical SSO documents receive the same flag in the accompanying evidence
commit. No schema, identity grant, broker authority or provider credential changes.

## Behavior

`--gateway-ca-file` loads a bounded, certificate-only bundle before credentials
or login. Explicit roots replace embedded WebPKI for this HTTPS gateway client;
name, chain and validity checks remain enabled. The default transport is unchanged.
One immutable snapshot follows profile, native intent, admission, business reads,
export, refresh and ai-guard logout. The IdP uses its separate approved profile CA.
There is no reload, insecure option, system trust installation or mTLS addition.

The exact bundle bytes join the versioned export-resume binding. A different but
still valid bundle cannot resume that export; identical bytes at a different path
can. Default credential/admission digest bytes remain compatible.

## Validation

Luna executed only the following selectors, offline in an admitted Linux ARM64
sandbox with Rust1.99.1, locked dependencies and an empty allowlisted environment.
HTTP, TLS, the CLI, authentication ledger and tenant SQLite are real local code;
the owned issuer is a protocol fixture. No model, external IdP, browser or worker
runtime was invoked.

```sh
cargo test --locked --offline --test human_gateway_trust -- --nocapture --test-threads=1
cargo test --locked --offline --test human_sso_gateway_trust -- --nocapture --test-threads=1
cargo test --locked --offline --test human_cli human_gateway_ca_ -- --nocapture --test-threads=1
cargo test --locked --offline --test human_cli export_retains_complete_pages_on_transport_failure_and_resumes_exact_snapshot -- --exact
cargo test --locked --offline --test human_cli bearer_file_modes_invalid_origins_and_redirects_never_send_or_print_credentials -- --exact
cargo test --locked --offline --test sso_cli signed_native_session_reads_exports_resumes_and_rotates_without_token_files -- --exact
cargo check --all-targets --locked --offline
```

| Check | Observed result | Retained log file | Log SHA-256 |
|---|---|---|---|
| Baseline, first gateway test before implementation | Expected RED, RC101: missing option after valid local setup; initial test SHA `32ec6319…475d14` | `human-gateway-trust-red.log` | `2f4b08dbb1819b724bff2d02769f3f0776dca6404b4cd992df44794c7d1c01c2` |
| Gateway TLS, four tests | PASS, RC0; CA/default/wrong CA, nine certificates, SAN, expired/future leaf and export binding | `human-gateway-trust-green.log` | `99b59e334d0a01936789a951bf526c308ee1dfbdf46033c6a3f4fdb7237f7c04` |
| Foreground SSO gateway TLS | PASS, RC0; source CA replaced before admission, independent IdP CA, reads/export/refresh/logout | `human-sso-gateway-trust-green.log` | `8a64bdb6cb4b4b2feb9bad2f8edc5c1ef54b1defda4ce500335647f2f892bfa9` |
| Two invalid-configuration tests | PASS, RC0; malformed/key/unknown/oversized PEM and HTTP+CA rejected before credentials/login | `human-cli-gateway-ca-green.log` | `9ecd036070a9d9b7bd9442a354f199c3fe72845e243f1c7ddb507c3535c46222` |
| Export interruption/resume canary | PASS, RC0; includes default credential digest compatibility | `human-cli-transport-failure-green.log` | `1868e263f393b0960e9471e2e0bfb14b041780d1bd4712ce46e190a8f5234a32` |
| Credential-file/origin/redirect canary | PASS, RC0 | `human-cli-bearer-redirect-green.log` | `79aeae6d99513d611b8bd3edd156a69269c6e85c25aa76c65102664adeedadce` |
| Existing native SSO/rotation canary | PASS, RC0 | `human-sso-native-green.log` | `99fa6b9a8560aabc68224b53afb991c568d21c8157d23f319108c2bf3843baee` |
| All-targets compilation | PASS, RC0,34.57s | `human-gateway-trust-check.log` | `eb350cdf26ec938416a87358e285bbe89aa8f32d1a25a2df3593d6f03cbd0000` |

Private retention outside Git: archive `human-gateway-trust-checkpoint`, at
`/Users/louis/Documents/.ai-guard-engineering-20261104.RhClps/kubernetes-held-create-20261105/human-gateway-trust-checkpoint/`.
Its `MANIFEST.sha256` has SHA-256 `9c0759f65b86f527d8d8d0b64cd386584168fc54f32042c0bb081219db309e8d`;
Root verified all12 payloads, retained400 under a private0700 parent. It binds the
eight named logs, initial test, selected source archive/manifest and review note.
The bytes are retained for the maintainer, rather than published in Git; no
sandbox database, executable, wire response, key or credential is in this archive.

Ten unique behavioral functions passed. The logs report118 library warnings;
the new SSO test reports two unused shared-fixture warnings. The all-targets
check also reports three lib-test warnings. These are not a clean lint result;
Clippy remains BLOCKED by the missing component in the pinned QA toolchain.
Root verified hashes, terminal results, formatting, whitespace,96 guide links
and absence of the three known private key literals in the fourteen changed files.

QA image: `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
UID/GID1000, capabilities0, no-new-privileges/seccomp, networknone/loopback only,
read-only source/toolchain/public registry, CPU2, memory6GiB/swap0, PIDs256,
scratch3GiB/65536inodes, bounded file/log sizes and50-minute owner lifetime.
Logs were parent-created0600. After all terminal handles and no remaining project
process, QA J was stopped and removed at05:43:18.89445UTC, PID0/exit143/OOMfalse;
all memory OOM counters were zero. No user volume or persistent control data removed.

## Review and limits

Independent source review found no blocking application defect. Two stale
canonical descriptions of the absent flag were corrected before publication.
The immutable snapshot and changed-valid-bundle export oracles discriminate the
intended behavior. WebPKI exclusion is verified in the explicit builder source;
the private-chain tests do not demonstrate rejection of a publicly trusted CA.
The two PKIs demonstrate distinct operation, without a negative union-of-roots
oracle. The SSO scenario covers ai-guard logout, not IdP logout.

Enterprise PKI, active certificate revocation, real Keycloak/browser, provider
clients, physical multi-host transport and Docker/Podman/Kubernetes workers remain
outside this evidence. G09 remains FAIL; no global gate is closed by this lot.
See [validation](../../../VALIDATION.md), [local identity guide](../../../../setup/identity/README.md)
and [SSO guide](../../../../setup/sso/README.md).
