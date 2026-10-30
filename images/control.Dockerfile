FROM node:24-bookworm-slim AS front-build
WORKDIR /build/front
COPY front/package.json front/package-lock.json ./
RUN npm ci
COPY front/ ./
RUN npm run build

FROM rust:1.99.1-bookworm AS rust-build
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src/ ./src/
RUN cargo build --release --locked --bin agentsandbox

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 agent \
    && useradd --uid 1000 --gid 1000 --no-create-home --home-dir /tmp --shell /usr/sbin/nologin agent \
    && mkdir -p /app/front /app/policies /data \
    && chown 1000:1000 /data \
    && chmod 0700 /data
COPY --from=rust-build /build/target/release/agentsandbox /usr/local/bin/agentsandbox
COPY --from=front-build /build/front/dist/ /app/front/
COPY policies/ /app/policies/
COPY LICENSE /usr/share/licenses/ai-guard/LICENSE
WORKDIR /app
USER 1000:1000
EXPOSE 8787 4000
ENTRYPOINT ["agentsandbox"]
CMD ["gateway"]
