# Base images are pinned by multi-platform index digest, verified with
# `docker buildx imagetools inspect` on 2026-11-21 (node 24-bookworm-slim, rust 1.99.1-bookworm,
# debian bookworm-slim). Update tag and digest together, never the digest alone.
FROM rust:1.99.1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS rust-toolchain

FROM rust-toolchain AS provider-build
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src/ ./src/
# The provider bootstrap compiles the expected managed hook configurations in.
COPY images/provider-activity/ ./images/provider-activity/
ARG AI_GUARD_VERSION=""
ARG AI_GUARD_BUILD_ID=""
ARG AI_GUARD_BUILD_REVISION=""
ENV AI_GUARD_VERSION=${AI_GUARD_VERSION} \
    AI_GUARD_BUILD_ID=${AI_GUARD_BUILD_ID} \
    AI_GUARD_BUILD_REVISION=${AI_GUARD_BUILD_REVISION}
RUN cargo build --release --locked --bin ai-guard-provider --bin ai-guard-relay --bin ai-guard-activity

FROM node:24-bookworm-slim@sha256:dc26ae92c9fb15cf6b1d63e20f0944b2efe0f115b20e00be48fb23fceb061c54
ARG CODEX_VERSION=""
ARG CLAUDE_CODE_VERSION=""
# Managed Claude Code hook profile, computed by `agentsandbox build` from
# CLAUDE_CODE_VERSION: v1 before 2.1.295, v1-onfailure from 2.1.295. A direct
# engine build that installs Claude Code must pass one of these two values.
ARG CLAUDE_ACTIVITY_PROFILE=""
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
# The activity client/helper is immutable product code on the readonly root.
# Provider hooks call it with fixed arguments; it reads no environment value.
COPY --from=provider-build --chown=0:0 --chmod=0555 /build/target/release/ai-guard-activity /usr/local/bin/ai-guard-activity
COPY --chown=0:0 --chmod=0444 images/provider-activity/claude-code/managed-settings.v1.json images/provider-activity/claude-code/managed-settings.v1-onfailure.json /usr/share/ai-guard/provider-activity/claude-code/
COPY --chown=0:0 --chmod=0444 images/provider-activity/codex/requirements.toml /usr/share/ai-guard/provider-activity/codex/
COPY LICENSE /usr/share/licenses/ai-guard/LICENSE
RUN chmod -R a+rX /usr/share/licenses/ai-guard
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
# Each installed client gets its managed hook configuration, root-owned and
# read-only; the provider bootstrap refuses to start a client without it.
RUN if [ -n "$CLAUDE_CODE_VERSION" ]; then \
      case "$CLAUDE_ACTIVITY_PROFILE" in v1|v1-onfailure) ;; *) echo "CLAUDE_ACTIVITY_PROFILE must be v1 or v1-onfailure" >&2; exit 1 ;; esac \
      && install -d -o 0 -g 0 -m 0755 /etc/claude-code \
      && install -o 0 -g 0 -m 0444 "/usr/share/ai-guard/provider-activity/claude-code/managed-settings.$CLAUDE_ACTIVITY_PROFILE.json" /etc/claude-code/managed-settings.json; \
    fi \
    && if [ -n "$CODEX_VERSION" ]; then \
      install -d -o 0 -g 0 -m 0755 /etc/codex \
      && install -o 0 -g 0 -m 0444 /usr/share/ai-guard/provider-activity/codex/requirements.toml /etc/codex/requirements.toml; \
    fi
ENV HOME=/home/agent \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/home/agent/.cargo \
    PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    LANG=C.UTF-8
WORKDIR /workspace
USER 1000:1000
CMD ["/usr/bin/id"]
