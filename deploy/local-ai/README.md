# Local inference configuration

[English guide](../../docs/setup/local-ai/README.md) · [Guide français](../../docs/setup/local-ai/README.fr.md)

The Dockerfile packages the pinned Qwen2.5-0.5B-Instruct Q4_K_M weight and llama.cpp runtime. Compose starts the model on its own internal network and publishes a fixed-destination Rust ingress on loopback. Use `.env.exemple.local-qwen` or `.env.exemple.local-keycloak-qwen` with the separate project name `ai-guard-local-ai`.

The runtime index is `sha256:4b7be0376002b4450a357a109bab804093ff9251193cc75ed4ce0cb39953ff5c`: Linux AMD64 child `fc3b8378bd02bbff0b83659163fd249e57e5a9283e7b37926f6e9affd6d71786`, ARM64 child `b0a70ac8e2d1e663ba36a98e3706301e4a4a9b6094c9f99398f0928a8dd2ea62`. Pinning available platforms does not qualify them. The operator guide covers resource settings, offline preparation, protocols, replacement and stopping.

`AI_GUARD_LOCAL_AI_DEFAULT_TOKENS` is a prediction default, not a mandatory output quota. Model advice does not authorize actions. Auxiliary Qwen experiments stay local and ignored.

`compose.tls.yaml` is an explicit alternative with the same model limits, a single TLS Rust relay and two internal networks. It publishes no host port. Use `.env.exemple.local-qwen-tls` and project `ai-guard-local-ai-tls`; add `deploy/docker/compose.jev-local-tls.example.yaml` to exactly one control base. See [TLS setup in English](../../docs/setup/local-ai/TLS.md) or [en français](../../docs/setup/local-ai/TLS.fr.md) for private projections, diagnostics and the stop/renewal/recovery procedure. Do not start both model profiles or combine multiple judge overrides by default.
