FROM agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984
USER 0:0
COPY probe.rs /tmp/probe.rs
RUN /usr/local/cargo/bin/rustc --edition=2021 -C opt-level=2 /tmp/probe.rs -o /probe && chmod 0555 /probe && rm /tmp/probe.rs
USER 1000:1000
ENTRYPOINT ["/probe"]
