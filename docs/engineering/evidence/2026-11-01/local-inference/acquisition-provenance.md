# Public source and pinned acquisition provenance — private capture

Date2026-11-01. No application HTTP, inference, test, service/worker/probe run,
existing credential or sensitive environment/argv inspection.
Host available space before acquisition:86146768KiB; this is not a direct
measurement of free space inside the Docker VM. Docker metadata reports
overlay2/extfs, root /var/lib/docker; no VM-free-space value is inferred.

Official Rust: https://hub.docker.com/_/rust lists1.99.1-slim-bookworm.
Tag metadata:
https://hub.docker.com/v2/repositories/library/rust/tags/1.99.1-slim-bookworm
Index digest:sha256:24707e70a4f1e23d9a7b25202ec43126d28a52294b96aad5c670df1fd1f1d400
linux/arm64 child:sha256:efecb7694d37171c478bd45d0cc88d9acc6e7fd31d7a1a39891ed4a294d0ce6f
The Docker CLI manifest lookup by index confirms the same ARM64 child.
Source Dockerfile:
https://github.com/rust-lang/docker-rust/blob/de3b3171ec576fd91eb8e28d3d37fa117b135fcb/stable/bookworm/slim/Dockerfile
Official catalog:
https://github.com/docker-library/official-images/blob/master/library/rust
Its source states RUST_VERSION1.99.1. No image ID is used as a registry digest.

Model runtime acquisition remains the previously approved official artifact:
ghcr.io/ggml-org/llama.cpp:server-b11718@sha256:b0a70ac8e2d1e663ba36a98e3706301e4a4a9b6094c9f99398f0928a8dd2ea62
Source:https://github.com/ggml-org/llama.cpp

New context contains only probe.rs and Dockerfile, both private0600.
Probe source unchanged from V2:
40d78b0047642c8f63328c0ea752d8922cad411edb61c0c711a8760d16e62178.
Dockerfile:
efc353fb57bc29823762b65bc5f652ec47dd35cf1f9dcbf3d31c0637d9e1e2d7.
Image IDs and terminal command outcomes are recorded only after observation.
Qwen historical FAIL as an authorization judge remains FAIL.
