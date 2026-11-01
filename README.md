# ai-guard

[English](README.md) · [Français](README.fr.md)

Run Codex, Claude Code and other AI agents in isolated workspaces. Decide what they may do, follow their activity in one dashboard, inspect changes and export results to a new destination.

ai-guard combines a Rust control plane and execution broker, a static TypeScript dashboard, and Docker/Podman workers. Local use does not require a cloud account. Each tenant has its own authenticated scope and SQLite database; workers receive neither the container-engine socket nor the original project.

## Choose your installation

The user manual describes the **complete product contract**. Implementation progress, qualification results and remaining gaps are recorded separately in [engineering](docs/engineering/CURRENT_STATE.md). A documented target is not a production certification.

| Context | Guide | Configuration example |
|---|---|---|
| Local workstation, Docker or Podman | [Local setup](docs/setup/local/README.md) | [.env.exemple.local](.env.exemple.local) |
| Codex / Claude Code, subscription or API | [AI providers](docs/setup/providers/README.md) | [.env.exemple.providers](.env.exemple.providers) |
| Local Keycloak, Web and CLI identity | [Keycloak and OIDC](docs/setup/keycloak/README.md) | [.env.exemple.local-keycloak](.env.exemple.local-keycloak) |
| Small local model, offline inference | [Local AI](docs/setup/local-ai/README.md) | [.env.exemple.local-qwen](.env.exemple.local-qwen) |
| Local Keycloak and Qwen together | [Combine contexts](docs/setup/README.md#combine-local-contexts) | [.env.exemple.local-keycloak-qwen](.env.exemple.local-keycloak-qwen) |
| Managed installation, private CA and internal inference | [Enterprise configuration](docs/setup/enterprise/README.md) | [.env.exemple.entreprise](.env.exemple.entreprise) |

Start with the [configuration index](docs/setup/README.md): it explains which component reads each file, how settings compose, where secrets belong and how to preserve an existing installation.

## First local start

From the repository root, with the Rust toolchain in `rust-toolchain.toml`, a running Docker engine and Compose:

```sh
cargo build --release --locked
./target/release/agentsandbox init --config-dir "$HOME/.ai-guard"
./target/release/agentsandbox doctor --runtime docker
./target/release/agentsandbox build --runtime docker
docker compose --env-file .env.exemple.local -f deploy/docker/compose.yaml up -d
./target/release/agentsandbox healthcheck --url http://127.0.0.1:8787/healthz --url http://127.0.0.1:4000/healthz
```

Open `http://127.0.0.1:4000`. For the control-key local flow, use the private viewer credential created under `$HOME/.ai-guard/secrets/viewer`; keep the runner credential in the trusted control plane. The [local guide](docs/setup/local/README.md) covers the first command, policy refusal, workspace export, data, stopping and recovery.

Rootless is the default requirement. Docker Desktop and other engines that do not report rootless require the operator's explicit `--allow-rootful-runtime` choice on relevant CLI commands. Follow the guide for Podman, custom engine paths, custom ports and this recorded security exception.

## Configuration and operations

- Examples contain public values only. Copy a template to a **new** private file before editing; preserve existing credentials and data.
- Compose reads `.env.exemple.*` through `--env-file`. The Rust CLI does not source dotenv; its flags and typed configuration files are documented per context.
- Build the images from the supplied Dockerfiles. After preparing dependencies and images, local workflows can run offline.
- Keep identity, AI-provider authentication, network permissions and tenant permissions separate. A provider subscription does not sign a user into ai-guard.
- Resource allocation records are not CPU-consumption invoices. Model advice cannot override mandatory policy, audit or runtime restrictions.

[Operations](docs/operations.md) · [API](docs/api.md) · [Security](docs/security.md) · [Architecture](docs/architecture.md) · [License](LICENSE)

## Contribute

Application code outside the frontend is Rust. See [AGENTS.md](AGENTS.md) for architecture, coding rules and targeted verification. Product behavior changes must update the English/French guides and configuration examples in the same change. Auxiliary Keycloak, Qwen and test-CA experiments stay local and ignored; product security and runtime regressions remain in the repository.

Apache-2.0. The private vulnerability-reporting channel must be established before public release.
