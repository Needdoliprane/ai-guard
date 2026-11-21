# Contributing to ai-guard

Thank you for your interest in contributing to ai-guard!

## Getting Started

Please read the [architecture contract](docs/engineering/ARCHITECTURE_CONTRACT.md) and [coding rules](docs/engineering/CODING_RULES.md) before submitting any changes.

## Development Workflow

1.  Fork the repository.
2.  Create a feature branch.
3.  Ensure your changes comply with the project's [production readiness gates](docs/engineering/PRODUCTION_READINESS.md).
4.  Run the tests locally:
    *   Rust: `cargo test --locked`
    *   Frontend: `cd front && npm test`
5.  Submit a pull request.

## Supply Chain Security

We take the security of our supply chain seriously:
*   We use pinned versions for dependencies (Rust toolchain, Node version).
*   All code is reviewed according to the [coding rules](docs/engineering/CODING_RULES.md).
*   We use containerized environments for builds and tests to ensure isolation and reproducibility.
*   We avoid pre-built public binaries where possible, preferring to build from source within controlled environments.
*   Any dependency update should be carefully reviewed for security implications.
*   Base images in `images/*.Dockerfile` are pinned by index digest. Update a tag and its digest together.
*   A `Cargo.lock` change that introduces a licence expression missing from `.github/third-party-license-expressions.txt` fails CI: review the licence and add it in the same change.
*   Releases ship `SHA256SUMS`, a CycloneDX SBOM and `BUILD-INFO`, without cryptographic signatures; see [Install from a GitHub Release](docs/setup/release/install.md).

## Reporting a vulnerability

Do not open a public issue. Follow [SECURITY.md](SECURITY.md) ([FR](SECURITY.fr.md)).
