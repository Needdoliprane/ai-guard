# Configure ai-guard

[English](README.md) · [Français](README.fr.md) · [Project](../../README.md)

This manual describes the complete product contract. The implementation and qualification ledger is maintained in [engineering](../engineering/CURRENT_STATE.md). Choose the environment that matches your identity and inference needs; local topology, ai-guard identity and AI-provider authentication are independent settings.

## Installation guides

| Guide | What it configures |
|---|---|
| [Local workstation](local/README.md) | Rust CLI, Docker/Podman, control services, first worker, permissions, workspace/export, data and recovery. |
| [Local Kubernetes](kubernetes/README.md) | Docker Desktop with one node, explicit `docker-desktop` context, TLS, readiness, resources, image preparation and diagnosis. Use the local workstation guide to launch ai-guard with Docker/Podman. |
| [Trusted local broker](broker/README.md) | Private Registry2, Docker start profile, service/IPC, replay identities, approval, cancellation, recovery and export. |
| [AI providers](providers/README.md) | Codex and Claude Code with subscription or API, explicit selection, private credentials and optional Jev. |
| [Named local access](identity/README.md) | Private human account provisioning, explicit resource permissions, authenticated reads, export and recovery without an IdP. |
| [ai-guard SSO](sso/README.md) | Fresh local DEMO: own images, inspected identity network, UID1000 volume preparation and normal reopen. Also covers existing/enterprise providers, Web sign-in, foreground CLI and recovery. Complete gateway/Web/CLI interoperability remains unqualified. |
| [Keycloak provider](keycloak/README.md) | Separate local identity-provider project, public Web/CLI clients, PKCE, claims and enterprise IdP preparation. |
| [Local AI](local-ai/README.md) | Pinned small-model image, memory/context limits, internal inference and model replacement. |
| [Enterprise](enterprise/README.md) | Shared control services, tenant boundaries, private CAs, internal inference and operations. |

Each guide contains prerequisites, configuration, startup, expected endpoints, diagnosis, stopping and data preservation. English/French versions and configuration examples change together when a contract changes.

## Files and consumers

For Kubernetes cluster access, Docker Desktop prepares the local cluster configuration and `kubectl` reads the operator's kubeconfig. Follow the [filtered inspection commands](kubernetes/README.md#check-the-context-and-tls) without changing the global context. The cluster guide requires no ai-guard Kubernetes env file; keep application settings with their existing consumers below.

| File | Reader and use |
|---|---|
| `.env.exemple.local` | Compose `--env-file`, base Docker or Podman stack. |
| `.env.exemple.local-rules` | Base stack plus `deploy/docker/compose.jev-rules.example.yaml`, finite Rust decision catalogue without a model service. |
| `.env.exemple.local-keycloak` | Base Compose and the separate Keycloak project. |
| [`.env.exemple.local-keycloak-sso`](../../.env.exemple.local-keycloak-sso) | Separate Keycloak project and standalone `deploy/{docker,podman}/compose.sso.yaml`; select new image tags/projects and the exact inspected external identity network. The explicit setup profile prepares its private volume once. |
| `.env.exemple.local-qwen` | Base Compose and the separate local-AI project. |
| `.env.exemple.local-qwen-tls-volumes` + published `volumes.env` | Fresh-volume TLS model project and gateway override; follow [explicit provisioning](local-ai/TLS.md). |
| `.env.exemple.local-qwen-tls` | Explicit TLS bind alternative on actually supported filesystems; follow [the TLS guide](local-ai/TLS.md). |
| `.env.exemple.local-keycloak-qwen` | Same settings combined; use the separate commands below. |
| `.env.exemple.providers` | Template for a **new private provider input**; follow the provider guide for explicit import. Not a Compose or SSO configuration. |
| `.env.exemple.entreprise` | Base Compose transport and optional Jev override; identities, tenants and inference policies have separate typed configuration. |
| `<config-dir>/providers.json` | Rust provider selection, created/inspected with `providers init/status`; override through `--providers-file`. |
| Private OIDC JSON and its public [examples](sso/README.md#prepare-the-private-gateway-json) | Gateway `--oidc-config`; a separate versioned file, with no environment interpolation. Explicit fresh audit initialization differs from normal reopen. |
| Private runner/viewer/Jev files | Explicit Rust/control credential references, never global worker environment. |

The volume profile first requires `model-tls provision`: pass its base file, then the published `volumes.env`, using two `--env-file` options. The public example alone cannot activate it; do not invent selectors. The Rust CLI does not source dotenv. Compose expands `${HOME}` and reads these examples only when `--env-file` names one. Host environment variables may override Compose interpolation; inspect your shell configuration when values differ. CLI flags configure their named component, not every service.

Public examples describe the documented local defaults; required empty selectors must be completed as their guide specifies. To customize, create a new private file with a different name and use that path consistently. Never overwrite an existing `.env`, credential directory or database. A private provider file should be0600; its parent should be0700. Do not `source` a downloaded or project-supplied configuration.

For a new local SSO installation, start with the [fresh DEMO procedure](sso/README.md#prepare-a-fresh-local-demo). Its env example deliberately leaves the identity-network selector empty until inspection; it cannot be started unchanged. Build the current control image containing `setup prepare` and the current realm image, then prepare the new volume through public stdin. No host-owned private OIDC bind or public prebuilt image is required. The older control image beginning `954246…` does not contain this new preparer.

## Combine local contexts

First build ai-guard and initialize the same private configuration directory as the [local guide](local/README.md). Then, from the repository root:

```sh
docker compose --env-file .env.exemple.local-keycloak-qwen -p ai-guard-local -f deploy/docker/compose.yaml up -d
docker compose --env-file .env.exemple.local-keycloak-qwen -p ai-guard-keycloak -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http up --build -d
docker compose --env-file .env.exemple.local-keycloak-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml up --build -d
```

These are three Compose projects with one ai-guard dashboard. Keeping project names explicit preserves separate data and networks. Do not merge their Compose files as overrides. Initial builds acquire public images and the pinned model; prepare them beforehand for offline use.

These legacy commands start the separate projects; they do not enable ai-guard SSO, create human memberships or attach the gateway to the identity network. Use the [dedicated SSO files and preparation](sso/README.md#prepare-a-fresh-local-demo) for a new SSO installation. The real complete gateway/Web/CLI path still needs its own qualification.

| Access | Local default |
|---|---|
| Dashboard | `http://127.0.0.1:4000` |
| Gateway | `http://127.0.0.1:8787` |
| Keycloak issuer | `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo` |
| Local model health | `http://127.0.0.1:18090/health` |
| Local Chat Completions base | `http://127.0.0.1:18090/v1` |

Configure the Keycloak hostname as explained in its guide. Published ports are bound to loopback. The gateway, dashboard and model keep fixed container ports when their host ports change; update CLI gateway URLs and Keycloak's registered Web origin accordingly. Keycloak is different: `AI_GUARD_DEMO_IDP_PORT` changes both its internal and published ports, and the complete issuer must remain identical for browser, CLI and control. Rootless remains the default engine requirement.

## Stop and resume

Use the **same env file, project name and Compose path** for `ps`, `stop`, `start` and `logs`. For example:

```sh
docker compose --env-file .env.exemple.local-keycloak-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml stop
```

Repeat for the projects you intend to stop. Stop active workers through the broker before stopping their control services. `stop` preserves containers and named data volumes; restarting an IdP does not re-import or replace an existing realm. An updated model requires a deliberate image change, not deletion of identity or audit data.

The CLI `up/down` workflow uses its own fixed project/default ports. Use the direct Compose workflow consistently when choosing the configurable examples above; do not mix lifecycle commands for different project names.

## Secrets, TLS and responsibilities

Generate legacy local ai-guard credentials with `init`, or prepare a new SSO installation with `setup prepare` as its guide specifies; do not initialize over an existing prepared volume. Obtain enterprise identity and AI credentials through the chosen operator/provider flow. A Keycloak administrator is not automatically an ai-guard administrator. Each tenant and permission mapping is administered by the control plane. A model backend may be shared only through distinct authenticated scopes, permissions and quotas.

For local certificate preparation, see [the Rust CA generator](../demo-pki.md). Keep CA private keys in the trusted control environment; clients receive only the appropriate public CA chain. Configure SAN/issuer names consistently and preserve TLS verification. The local HTTP examples use public demonstration accounts and are not enterprise transport settings.
