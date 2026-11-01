# Local Keycloak configuration

[English guide](../../docs/setup/keycloak/README.md) · [Guide français](../../docs/setup/keycloak/README.fr.md)

This directory contains the pinned Keycloak Dockerfile, initial public DEMO realm and isolated Compose project. Configuration is supplied through `.env.exemple.local-keycloak` or `.env.exemple.local-keycloak-qwen` at the repository root. The guide covers build, DNS, clients, accounts, identity mapping, TLS and data preservation.

Keep the project name `ai-guard-keycloak` separate from the main ai-guard project. The HTTP profile must be explicitly enabled with `--profile keycloak-demo-http`. Do not reuse its public accounts or transport policy in an enterprise deployment. Existing realm data is preserved; editing the import file does not overwrite an imported realm.

Auxiliary interoperability tests and their credentials/captures are not distributed here.
