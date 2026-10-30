FROM agentsandbox-broker-clock-linux:base-8e48e6c9b949d58cef12237644a3b13e2ef8448666cf2d6b732473f2a96072aa
USER 0:0
COPY --chown=1000:1000 cargo-registry/ /home/fsqa/.cargo/registry/
RUN mkdir -p /build/journal-source && chown 1000:1000 /build/journal-source
COPY --chown=1000:1000 source/ /build/journal-source/
WORKDIR /build/journal-source
ENV CARGO_TARGET_DIR=/build/target CARGO_INCREMENTAL=0
USER 1000:1000
RUN cargo test --locked --offline --no-run --lib --test broker_journal --test journal_sqlite
ENTRYPOINT []
CMD ["sh", "-c", "set -eu; cargo test --locked --offline --lib broker::journal::tests; cargo test --locked --offline --test broker_journal; cargo test --locked --offline --test journal_sqlite"]
