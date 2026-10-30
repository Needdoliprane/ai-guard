# Build only after explicit QA authorization. Verify the source tag's immutable
# ID first; compile with the Rust toolchain already copied into that exact image.
FROM agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984
USER root
COPY interactive_probe.rs /tmp/ai-guard-interactive-probe.rs
RUN rustc --edition=2024 --crate-name ai_guard_interactive_probe -C opt-level=2 /tmp/ai-guard-interactive-probe.rs -o /usr/local/bin/ai-guard-interactive-probe \
    && chmod 0755 /usr/local/bin/ai-guard-interactive-probe \
    && rm /tmp/ai-guard-interactive-probe.rs
USER 1000:1000
