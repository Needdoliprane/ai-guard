FROM agentsandbox-sqlite-tests:qa-20261030-8QMTSx
USER 0:0
RUN mkdir -p /build/source && chown 1000:1000 /build/source
COPY --chown=1000:1000 source/ /build/source/
WORKDIR /build/source
ENV CARGO_TARGET_DIR=/build/target
USER 1000:1000
RUN cargo test --locked --offline --no-run --test broker_clock
ENTRYPOINT []
CMD ["cargo", "test", "--locked", "--offline", "--test", "broker_clock"]
