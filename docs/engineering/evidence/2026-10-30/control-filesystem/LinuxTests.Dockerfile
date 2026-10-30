FROM rust:1.99.1-bookworm
RUN groupadd --gid 1000 fsqa && useradd --uid 1000 --gid 1000 --create-home fsqa && mkdir -p /build /home/fsqa/.cargo && cp -a /usr/local/cargo/bin /home/fsqa/.cargo/bin && chown -R 1000:1000 /build /home/fsqa/.cargo
WORKDIR /build
ENV CARGO_HOME=/home/fsqa/.cargo RUSTUP_HOME=/home/fsqa/.rustup HOME=/home/fsqa PATH=/home/fsqa/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
COPY --chown=1000:1000 Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY --chown=1000:1000 src/ ./src/
COPY --chown=1000:1000 tests/ ./tests/
USER 1000:1000
RUN rustup toolchain install 1.99.1 --profile minimal && cargo fetch --locked
RUN --network=none cargo test --locked --offline --lib control_fs::descriptor_tests -- --nocapture && cargo test --locked --offline --test control_fs -- --nocapture
