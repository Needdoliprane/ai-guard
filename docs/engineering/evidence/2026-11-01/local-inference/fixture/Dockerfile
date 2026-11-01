FROM docker.io/library/rust:1.99.1-slim-bookworm@sha256:24707e70a4f1e23d9a7b25202ec43126d28a52294b96aad5c670df1fd1f1d400 AS build
COPY fixture-v2.rs /tmp/fixture.rs
COPY probe-legacy.rs /tmp/probe-legacy.rs
RUN /usr/local/cargo/bin/rustc --edition=2021 -C opt-level=2 /tmp/fixture.rs -o /fixture && /usr/local/cargo/bin/rustc --edition=2021 -C opt-level=2 /tmp/probe-legacy.rs -o /probe-legacy && chmod 0555 /fixture /probe-legacy
FROM docker.io/library/rust:1.99.1-slim-bookworm@sha256:24707e70a4f1e23d9a7b25202ec43126d28a52294b96aad5c670df1fd1f1d400
COPY --from=build /fixture /fixture
COPY --from=build /probe-legacy /probe-legacy
