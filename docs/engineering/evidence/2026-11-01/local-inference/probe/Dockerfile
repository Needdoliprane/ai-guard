# Official Rust image, resolved from Docker Hub on 2026-11-01.
# linux/arm64 child is recorded separately in provenance.md.
FROM docker.io/library/rust:1.99.1-slim-bookworm@sha256:24707e70a4f1e23d9a7b25202ec43126d28a52294b96aad5c670df1fd1f1d400
USER 0:0
COPY probe.rs /tmp/probe.rs
RUN /usr/local/cargo/bin/rustc --version && /usr/local/cargo/bin/rustc --edition=2021 -C opt-level=2 /tmp/probe.rs -o /probe && chmod 0555 /probe && rm /tmp/probe.rs
USER 1000:1000
ENTRYPOINT ["/probe"]
