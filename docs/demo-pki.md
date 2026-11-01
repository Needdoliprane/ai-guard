# A local certificate chain

[English](demo-pki.md) · [Français](demo-pki.fr.md) · [Configuration](setup/README.md)

Create a new local test root CA, intermediate CA and server certificate in one command:

```sh
cargo run --locked --bin ai-guard-demo-pki -- --out .agentsandbox/demo-pki
```

Use the repository Rust toolchain and a trusted OpenSSL/LibreSSL executable. The default is `/usr/bin/openssl`; select another absolute path with `--openssl /absolute/path/openssl`. The tool does not search the ambient PATH, contact an issuer or install system trust.

## Configure names and validity

The default SANs cover `keycloak.ai-guard.test`, `llm.ai-guard.test`, `localhost`, `127.0.0.1` and `::1`. Replace the DNS names and choose server validity explicitly:

```sh
cargo run --locked --bin ai-guard-demo-pki -- \
  --out .agentsandbox/demo-pki-other \
  --dns keycloak.ai-guard.test --dns llm.ai-guard.test --days 14
```

At most eight ASCII DNS names are accepted, without wildcards; loopback IP SANs remain present. Server validity is 1–30 days, 7 by default. The root lasts 90 days and the intermediate 60. Keys use RSA 2048 and signatures SHA-256 for this local test environment.

The output directory must be **new**, including when an existing directory is empty. Parent directories are checked under the private-storage rules. Links and path traversal are refused. The directory is 0700 and files 0600. A failure preserves partial output: inspect the closed diagnostic and choose a new destination; do not delete existing keys to retry.

## Use the generated files

| File | Use |
|---|---|
| `root-ca.pem` | Public trust anchor explicitly configured in test clients. |
| `server-chain.pem` | Server certificate followed by its intermediate, served by the TLS endpoint. |
| `server.key` | Private key of that test server only. |
| `root-ca.key`, `intermediate-ca.key` | Private CA keys; retain in the trusted private directory. |
| `intermediate-ca.pem`, CSR files, `pki.cnf` | Local generation/diagnostic material. |
| `READY` | Final marker; also require a successful command exit. |

Mount only the server key and chain into the intended trusted server. A different container UID requires a separate private projection with the appropriate ownership; do not make the CA directory generally readable. Clients receive the public root, never a CA private key. No CA key belongs in an image, worker, report or repository. `.agentsandbox/` is ignored and excluded from agent workspace copies.

Inspect the public certificate and verify its chain:

```sh
/usr/bin/openssl x509 -in .agentsandbox/demo-pki/server.pem -noout -subject -issuer -dates
/usr/bin/openssl verify -CAfile .agentsandbox/demo-pki/root-ca.pem -untrusted .agentsandbox/demo-pki/intermediate-ca.pem .agentsandbox/demo-pki/server.pem
```

Then configure TLS at the actual Keycloak/inference endpoint and the public CA chain in each client. Keep certificate SAN, endpoint hostname and OIDC issuer consistent. DNS/hosts configuration and system trust installation are separate operator actions; preserve existing entries. Generating files does not automatically enable HTTPS on running services. Never disable TLS verification to compensate for a name or chain error.

## Rotation and operations

Generate replacement material in another new private directory, validate it, project only the required files to the server/client and follow the service's explicit reload procedure. Keep the previous material until the transition is verified. Protect and retain CA keys independently of application volumes.

This helper does not operate a production CA or provide automated renewal/revocation. Enterprise PKI requirements are described in the [enterprise guide](setup/enterprise/README.md) and [infrastructure contract](engineering/INFRASTRUCTURE_PKI.md). Auxiliary verification instruments stay local and ignored; recorded engineering results are in [VALIDATION.md](engineering/VALIDATION.md).
