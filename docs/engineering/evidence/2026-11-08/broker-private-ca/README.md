# Explicit gateway trust for the broker

Date: 8 November 2026. This is a scoped software proof, not physical multi-host,
PKI lifecycle or production qualification.

Component commit `b6ce72991aa8d1d2b54090201ffc192611f28a4b` is composed in
`3027bbe0d8118b8e869a14b2f65fb06281b71509`. The seven changed files match t3
exactly; comparison of the component and integration commits across application,
tests, frontend, Cargo files and the three guide/example files is empty.
The commands below ran on frozen t3, not again on the integration checkout.

## Inputs and execution boundary

The pre-correction input t1 is based on `225bfe7` with a test-only regression
and its consumed operator example. Its 2,300-file manifest has SHA-256
`7f5c26ea86c9425f8081140ef640fcb44609198d0474aa74251142f461a4f83c`.
The final t3 manifest has SHA-256
`8fa29fec30bfaf2d260f7c245e2a3757882ca295eaf29608ad38a0793d76f9c8`.
The correction after t1 has SHA-256
`c77cca6a8ad87658614b10dbaa6ea171b669a608d2fec03fb8f08fa282bbfb1f`.
Source copies and every manifest entry were checked before execution. Each
input used its own Cargo target.

Linux ARM64 QA used image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
Rust 1.99.1, UID/GID 1000, four CPUs, 8 GiB memory with swap disabled, PID limit
256, read-only root and public inputs, no network except private loopback,
no published ports or mounted engine socket, no capabilities and no-new-privileges. Compilation
and tests were offline; no host project executable or real worker engine ran.
Synthetic certificates serve a real Rust TLS listener, gateway and SQLite
Store. No system trust, external PKI, provider or cluster was used.

## Reproduction and corrected behavior

```text
cargo test --locked --offline --lib broker::service::gateway_trust_tests::operator_gateway_ca_reaches_broker_authentication_and_durable_audit_without_tls_bypass -- --exact --test-threads=1 --nocapture
```

On t1, a client explicitly using the fixture CA first verifies the live runner
protocol over TLS. The existing broker client then returns `Unavailable`
before application HTTP (`default_http=0`). The operator configuration rejects
the new CA field. The single test returns **RC101, one FAIL, 0.36 s** at its
missing-field assertion, after 2m10s compilation. No configured-broker success
is inferred from that prerequisite TLS probe.

The final t3 invocation returns **RC0, one PASS, 0.52 s**, after 2m03s compilation.
That single function verifies:

- V1 and the default V2 profile retain the original private-CA refusal, with
  zero application HTTP calls.
- Six invalid configurations fail before HTTP: explicit roots with HTTP,
  missing/malformed/oversized CA, nine certificates, and the new field in V1.
- An explicit V2 CA authenticates the actual expected runner identity and the
  same retained audit client writes an event through TLS: two HTTP requests.
  The stored event is matched by kind, returned sequence and UUID; this test
  does not compare the entire event payload.
- Changing the CA file after loading does not change the retained trust;
  loading a fresh registry from that malformed file fails.
- A TCP connection under the wrong name first reaches the same listener.
  The product client then refuses that name before application HTTP. DNS or
  connection failure cannot satisfy this negative assertion.

The fixture TLS server is stopped and joined before the final assertions.
All these cases belong to one function, not a count of separate test functions.

```text
cargo check --all-targets --locked --offline
```

On the same t3 source/target, this returns **RC0, 1m13s**. Two pre-existing
Kubernetes dead-code warnings remain; a warning-free or Clippy result is not
claimed. QA postflight reports no OOM.

| Capture | SHA-256 |
|---|---|
| t1 RED log | `3d60f26c4b680d3b8876792b92976e42eb116c2603c3211f4e7ffa11bc56b33b` |
| t1 RED status | `9e79c30f04bac09c4a2f06a45ba0198f7c15c17075e4982ad7fe7edcbd477a0a` |
| t3 GREEN log | `cdfab8d504d8d4f4d369e27d9b2fc40008989500172b26c16d935f00dfb2f5ea` |
| t3 GREEN status | `8f875fc8ec913a8932bec1b54788e1872e53cc8f588300208cbc0ab3c5143a0b` |
| t3 all-target check log | `326fe878b0bd81ad047e5caa502c2db52cf33d0a7af5fac49a630264decec8dd` |
| t3 all-target check status | `626fe814692fc0edface1baed378eb48589745f0d3f421af27bc1a20d11512ee` |

Independent source review found no remaining blocking defect. Its t1 server-name
oracle concern was corrected before t3. Two cache-preparation failures occurred
before any Cargo invocation; they remain setup incidents, not product RED.
Source archives, logs and controls are retained in private engineering evidence.

## Scope and remaining work

Registry2 alone accepts the explicit CA reference, independently of presence
trust. Roots are captured before IPC and retained by the authenticated audit
client. Default behavior, origin checks, tenant/namespace checks, proxy,
redirect and retry restrictions remain unchanged. The reference grants no
permission. User instructions and the consumed example are synchronized in the
[English](../../../../setup/broker/README.md) and
[French](../../../../setup/broker/README.fr.md) guides.

This proof does not establish physical persistence after power loss, two broker
processes, remote federation, certificate rotation/revocation, worker behavior,
Docker/Podman/Kubernetes confinement, load capacity or completion of a global gate.
