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
