# Requires Dockerfile ADD --checksum support (Dockerfile frontend >= 1.6).
# The build downloads the pinned public weight once; inference needs no Internet.
# Pinned index includes linux/amd64 and linux/arm64; the ARM child is b0a70ac8...
FROM ghcr.io/ggml-org/llama.cpp:server-b11718@sha256:4b7be0376002b4450a357a109bab804093ff9251193cc75ed4ce0cb39953ff5c
ADD --checksum=sha256:74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db --chmod=0444 https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/9217f5db79a29953eb74d5343926648285ec7e67/qwen2.5-0.5b-instruct-q4_k_m.gguf /models/model.gguf
# The runtime UID needs traversal of the model directory as well as file access.
RUN ["chmod", "0555", "/models"]
USER 65532:65532
ENTRYPOINT ["/app/llama-server"]
