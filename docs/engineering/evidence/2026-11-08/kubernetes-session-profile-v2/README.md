# Kubernetes session profile V2 — descriptive contract

The component `c4d0cd6`, composed in `9af5e99`, introduces a validated,
descriptive session profile. It creates no Pod, permission, credential reader,
API route or execution factory. V1 remains separate. Bootstrap identity,
storage ownership/exclusivity, output protection and lifecycle qualification
remain open before activation.

## Source and review

The immutable k2 full-source manifest has SHA-256
`e1dc974f186748985fb06c82800da61c65a0c1ee25e051d1ed4a313865a81f15`.
The independently generated QA input manifest is
`0189a1ca14352305deece1b5d4bdba4b755abfe1c572c0502ba33097631e5bc5`.
The three component files match the Root composition byte for byte:

| File | SHA-256 |
|---|---|
| `src/runtime/kubernetes.rs` | `819dc5139132d5eefd504ca88bc71ba5bb39ca19e9cd60840b09700548394bc7` |
| `src/runtime/kubernetes/session_profile.rs` | `e4cb1709aef1d23e4813b117d1509a4515ce365fa35fd3ba5b2b316ef283b152` |
| `src/runtime/kubernetes/session_profile_tests.rs` | `89832b5827cee1e96ff3bba4ab08c23dbb370120b0f2f02837fa2f082eae4387` |

These hashes describe the tested k2 component and its initial composition
`9af5e99`. The final native format check found one declaration-order difference
in `kubernetes.rs`. Formatting commit `f8d1ab4`, composed as `2176765`, moves
`pub mod session_profile` after the adjacent test module. Its new declaration
file SHA is `aec4b7ff57cf18eedc5e4026f1508d714be0c60a41799438757774d198a127cc`.
The profile implementation and test source remain byte-identical to k2.
Explicit native rustfmt checks on all three final files pass. No behavioral
test was repeated for this declaration-only formatting change.

The independent review corrected a test-quality defect before QA: invalid
manifest inputs previously accepted any error, including a hash mismatch.
k2 requires `InvalidProfile`, verifies the valid baseline and preserves the
separate mismatch result for a changed valid manifest. Application code is
unchanged from k1. No blocking source finding remains within the three-file
review; this is not a review of the complete Kubernetes implementation.

## Behavior covered

The profile binds canonical session identifiers, the expected authority,
provider resolution and credential-reference metadata, immutable image syntax,
requested resources and the requested denied network mode. It commits the
structured private invocation and complete validated manifest metadata by
separate hashes. Neither command arguments nor the full path listing are
serialized in the profile. The full manifest remains a separate artifact;
these comparisons do not observe actual file contents, a mounted volume or
runtime confinement.

The profile is bounded to64KiB and uses a private closed wire representation
through the unique-JSON decoder. Errors and Debug output are opaque. The
review confirms the decoder depth64 argument; the deep-root-array test does
not independently prove that limit because the root type is also invalid.
The component does not convert V1 data or a valid DTO into permission to run.

## Commands and observed results

Luna executed these commands on k2 in QA AW, Linux ARM64, Rust1.99.1, offline,
network-none, read-only inputs/root, UID1000, four CPU and8GiB memory. Each
source had its own target directory. No model or cluster was contacted.

| Command | Result | Log SHA-256 |
|---|---|---|
| `cargo check --all-targets --locked --offline` | RC0,1m24s | `1c461e4a1adc66d558a23b1cb8e1659404264fd7cb29cc55fbb6b75dddbe95a0` |
| `cargo test --locked --offline --lib runtime::kubernetes::session_profile::tests:: -- --test-threads=1` | RC0,9 PASS,0.67s | `e55708e5bfa85ba4e7be484493d6f2c6904566a226a798f5e675acf14065d10e` |
| `cargo test --locked --offline --lib runtime::kubernetes::admission::tests::held_profile_matches_an_independent_golden_and_binds_exact_request_bytes -- --exact --test-threads=1` | RC0,1 PASS | `98e4fe0c721028d19f666555f2dadf56e882304e02ea42df62ec48b0b65ec4d4` |
| `cargo test --locked --offline --lib workspace::export_policy_regression_tests::historical_manifest_serialization_preserves_absent_mode_and_hash -- --exact --test-threads=1` | RC0,1 PASS | `aa607f0a8178d6f2b7dad1de532cabf71d171ced78921322387eef24af417ed9` |

Eleven unique functions pass. The profile suite uses independent literal SHA
oracles; the two existing canaries cover V1 profile and historical manifest
serialization. Final native rustfmt/diff and known-literal-secret scans pass;
the declaration-order correction above is retained rather than attributing a
successful three-file format check to the original k2 snapshot.
The QA result checkpoint is retained privately with SHA-256
`f8f1d0cb91d06c19791032cd0a1c4f4f736036df8053a937379406220ca7a3c2`.
Postflight reports OOM0 and peak memory6,393,085,952 bytes for the AW jobs;
this is not a product capacity measurement.

## Retained limits

Clippy k2, activation, API consumption, bootstrap transport, Pod identity,
PVC/mounted-volume identity, exclusive access, CRI output handling, crash
recovery, real engines and capacity100 are NOT_RUN or not implemented by this
component. No release gate or finding from the external audit is closed.

The complete k2 source archive is retained outside the worktree with SHA-256
`722167789777c43a83aae18efdb9199b697e6ff813a190a77a244e2bc978a454`.
Source and text logs remain separate from QA databases and executables; none
of those databases or executables was promoted onto the host.
