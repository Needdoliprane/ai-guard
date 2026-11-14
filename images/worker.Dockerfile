FROM rust:1.99.1-bookworm AS rust-toolchain

FROM rust-toolchain AS provider-build
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src/ ./src/
ARG AI_GUARD_VERSION=""
ARG AI_GUARD_BUILD_ID=""
ARG AI_GUARD_BUILD_REVISION=""
ENV AI_GUARD_VERSION=${AI_GUARD_VERSION} \
    AI_GUARD_BUILD_ID=${AI_GUARD_BUILD_ID} \
    AI_GUARD_BUILD_REVISION=${AI_GUARD_BUILD_REVISION}
RUN cargo build --release --locked --bin ai-guard-provider --bin ai-guard-relay

FROM node:24-bookworm-slim
ARG CODEX_VERSION=""
ARG CLAUDE_CODE_VERSION=""
LABEL ai-guard.image.recipe="images/worker.Dockerfile" \
    ai-guard.provider.codex.requested-version="${CODEX_VERSION}" \
    ai-guard.provider.claude-code.requested-version="${CLAUDE_CODE_VERSION}"
RUN apt-get update \
    && apt-get install --yes --no-install-recommends bash git python3 python3-pip python3-venv curl ca-certificates build-essential pkg-config tar coreutils findutils \
    && rm -rf /var/lib/apt/lists/* \
    && usermod --login agent --home /home/agent node \
    && groupmod --new-name agent node \
    && mkdir -p /workspace /home/agent \
    && chown 1000:1000 /workspace /home/agent
COPY --from=rust-toolchain /usr/local/cargo/ /usr/local/cargo/
COPY --from=rust-toolchain /usr/local/rustup/ /usr/local/rustup/
# Builder and final image both use Debian bookworm (glibc 2.36). The bootstrap
# is a native Rust binary; its source and credential configuration stay outside
# the worker image, and credentials arrive only through the private handoff.
COPY --from=provider-build /build/target/release/ai-guard-provider /usr/local/bin/ai-guard-provider
# The relay helper is immutable product code on the readonly image root. It
# receives no workload proof, provider credential or control-network access.
COPY --from=provider-build --chown=0:0 --chmod=0555 /build/target/release/ai-guard-relay /usr/local/bin/ai-guard-relay
# Optional providers are absent unless an exact operator-chosen version is set.
# No credentials, provider configuration, or mutable "latest" version is baked in.
RUN if [ -n "$CODEX_VERSION" ]; then \
      printf '%s' "$CODEX_VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$' \
      && npm install --global --no-audit --no-fund "@openai/codex@$CODEX_VERSION"; \
    fi \
    && if [ -n "$CLAUDE_CODE_VERSION" ]; then \
      printf '%s' "$CLAUDE_CODE_VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$' \
      && npm install --global --no-audit --no-fund "@anthropic-ai/claude-code@$CLAUDE_CODE_VERSION"; \
    fi \
    && npm cache clean --force
ENV HOME=/home/agent \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/home/agent/.cargo \
    PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    LANG=C.UTF-8
WORKDIR /workspace
USER 1000:1000
CMD ["/usr/bin/id"]
