# Base images are pinned by multi-platform index digest, verified with
# `docker buildx imagetools inspect` on 2026-11-21 (node 24-bookworm-slim, rust 1.99.1-bookworm,
# debian bookworm-slim). Update tag and digest together, never the digest alone.
FROM node:24-bookworm-slim@sha256:dc26ae92c9fb15cf6b1d63e20f0944b2efe0f115b20e00be48fb23fceb061c54 AS front-build
WORKDIR /build/front
COPY front/package.json front/package-lock.json ./
RUN npm ci
COPY front/ ./
ARG AI_GUARD_VERSION=""
ARG AI_GUARD_BUILD_ID=""
ARG AI_GUARD_BUILD_REVISION=""
ENV AI_GUARD_VERSION=${AI_GUARD_VERSION} \
    AI_GUARD_BUILD_ID=${AI_GUARD_BUILD_ID} \
    AI_GUARD_BUILD_REVISION=${AI_GUARD_BUILD_REVISION}
RUN npm run build

FROM rust:1.99.1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS rust-build
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src/ ./src/
# The library compiles the expected provider activity hook configurations in.
COPY images/provider-activity/ ./images/provider-activity/
ARG AI_GUARD_VERSION=""
ARG AI_GUARD_BUILD_ID=""
ARG AI_GUARD_BUILD_REVISION=""
ENV AI_GUARD_VERSION=${AI_GUARD_VERSION} \
    AI_GUARD_BUILD_ID=${AI_GUARD_BUILD_ID} \
    AI_GUARD_BUILD_REVISION=${AI_GUARD_BUILD_REVISION}
RUN cargo build --release --locked --bin agentsandbox --bin ai-guard-audit-checkpoint --bin ai-guard-diagnose

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
LABEL ai-guard.image.recipe="images/control.Dockerfile"
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 agent \
    && useradd --uid 1000 --gid 1000 --no-create-home --home-dir /tmp --shell /usr/sbin/nologin agent \
    && mkdir -p /app/front /app/policies /data /run/secrets \
    && chown 1000:1000 /data /run/secrets \
    && chmod 0700 /data /run/secrets
COPY --from=rust-build /build/target/release/agentsandbox /usr/local/bin/agentsandbox
COPY --from=rust-build /build/target/release/ai-guard-audit-checkpoint /usr/local/bin/ai-guard-audit-checkpoint
COPY --from=rust-build /build/target/release/ai-guard-diagnose /usr/local/bin/ai-guard-diagnose
COPY --from=front-build /build/front/dist/ /app/front/
COPY policies/ /app/policies/
COPY LICENSE /usr/share/licenses/ai-guard/LICENSE
# Public assets must be readable by UID1000 even from a private checkout.
# Data and projected credentials retain their separate private modes.
RUN chmod -R a+rX /app/front /app/policies /usr/share/licenses/ai-guard
WORKDIR /app
USER 1000:1000
EXPOSE 8787 4000
ENTRYPOINT ["agentsandbox"]
CMD ["gateway"]
