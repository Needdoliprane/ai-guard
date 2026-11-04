# Configure and use ai-guard SSO

[Français](README.fr.md) · [Setup index](../README.md) · [Keycloak provider](../keycloak/README.md) · [Named local access](../identity/README.md)

This guide describes the Rust OIDC configuration, the Web sign-in and the foreground human CLI. Use it with the matching ai-guard binary and built frontend. It does not configure Codex or Claude authentication: choose their subscription, API key or approved inference endpoint separately in the [provider guide](../providers/README.md).

Deploy the gateway, built dashboard and native CLI from matching versions when adopting the required `contexts_complete` field in human identity responses. New strict clients reject a missing or malformed field; older strict clients may reject the added field. Coordinate the component update and retain the existing configuration, credentials and audit data. This response-contract change adds no database migration and does not justify replacing an existing store.

The examples also opt in to provider logout acknowledgements through `web_post_logout_redirect_uri` and `cli_post_logout_redirect_uri`. Install matching updated gateway and native CLI versions before activating these fields: older binaries reject unknown configuration/profile fields. Existing configurations that omit both options retain the legacy logout behaviour. Follow the [configuration upgrade procedure](#upgrade-logout-configuration-without-replacing-data) for an existing installation.

Choose the **fresh local DEMO** below to prepare new control data and the three imported application accounts, or use the [manual installation procedure](#prepare-an-existing-or-enterprise-installation) with an existing provider. Use a matching current control image and built frontend; the [engineering record](../../engineering/VALIDATION.md#2-novembre--sso-web-réel-sur-la-page-de-fin-corrigée) distinguishes the three accounts' observed Web sign-in/read/logout paths from the remaining native CLI, negative permission and lifecycle qualification. Browser-specific hostname mapping does not establish the host DNS resolution required by the native CLI. Preparation and historical results retain their dated scope; the [identity contract](../../sso.md) describes the complete product target.

## Prepare a fresh local DEMO

This path uses a new Keycloak project, a new ai-guard project, two locally built images and a new private control volume. It creates schema5 tenant storage, the separate authentication audit, technical credentials, accounts and memberships from the public [local-demo.json](examples/local-demo.json). It creates no sessions, resource grants, worker activity or human execution delegation. An empty authorized resource list after preparation is expected.

The shipped [Docker](../../../deploy/docker/compose.sso.yaml) and [Podman](../../../deploy/podman/compose.sso.yaml) entry points are standalone opt-in stacks. Do not combine them with the legacy `deploy/*/compose.yaml` or its host-secret mounts. The setup container has no network; only the gateway later joins the selected internal identity network. Dashboard and workers do not join it. No preparation or application container receives an engine socket or the project being executed.

### 1. Check prerequisites and choose new names

Use a supported local Docker or Podman installation with Compose available, a shell, and the current source checkout. Build dependencies/base images must be available; prepare them before working offline. Node is used while building the frontend image, not as a production server. The previously validated control image beginning `954246…` predates `setup prepare`: build a new image from the source containing this command. No public prebuilt ai-guard image is required.

Before startup, have your normal host DNS configuration resolve `keycloak.ai-guard.test` to `127.0.0.1` for the browser and CLI. Verify that resolution on the workstation. This guide does not edit OS DNS or trust. The container's identity-network alias supplies its own route to the same issuer; do not replace the issuer with `localhost`, a service name or a different IP.

Ports 8787, 4000 and 18080 must be free on host loopback. The fresh preset fixes these five public settings:

| Variable | Required value |
|---|---|
| `AI_GUARD_DEMO_IDP_HOST` | `keycloak.ai-guard.test` |
| `AI_GUARD_DEMO_IDP_PORT` | `18080` |
| `AI_GUARD_DEMO_WEB_ORIGIN` | `http://127.0.0.1:4000` |
| `AI_GUARD_GATEWAY_PORT` | `8787` |
| `AI_GUARD_DASHBOARD_PORT` | `4000` |

`--demo-defaults` checks those values and refuses incompatible overrides; it does not rewrite endpoints. Ordinary startup uses the fixed ports/origin in the SSO Compose file. For another topology, prepare a reviewed generic manifest and matching service configuration instead of editing these advertised values alone.

From the repository root, select the engine, matching SSO file and new names. The examples below use Docker; for Podman set `AIGUARD_ENGINE="podman"` and `AIGUARD_SSO_COMPOSE="deploy/podman/compose.sso.yaml"` together. Keep these choices for the whole lifecycle:

```sh
AIGUARD_ENGINE="docker"
AIGUARD_SSO_COMPOSE="deploy/docker/compose.sso.yaml"
AIGUARD_SSO_PROJECT="ai-guard-sso-fresh"
AIGUARD_IDP_PROJECT="ai-guard-keycloak-sso-fresh"
AIGUARD_SSO_ENV="/absolute/private/ai-guard-demo/sso.env"
```

Check that neither project name belongs to existing resources. The filters and inspection format below use Docker Compose's labels. With Podman, verify the actual Compose provider's project/network labels and inspection fields, and adapt the inventory commands if they differ; an empty unmatched filter does not establish that a name is unused. The Podman deployment requires its own real qualification.

```sh
"$AIGUARD_ENGINE" ps -a --filter "label=com.docker.compose.project=$AIGUARD_SSO_PROJECT"
"$AIGUARD_ENGINE" volume ls --filter "label=com.docker.compose.project=$AIGUARD_SSO_PROJECT"
"$AIGUARD_ENGINE" network ls --filter "label=com.docker.compose.project=$AIGUARD_SSO_PROJECT"
"$AIGUARD_ENGINE" ps -a --filter "label=com.docker.compose.project=$AIGUARD_IDP_PROJECT"
"$AIGUARD_ENGINE" volume ls --filter "label=com.docker.compose.project=$AIGUARD_IDP_PROJECT"
"$AIGUARD_ENGINE" network ls --filter "label=com.docker.compose.project=$AIGUARD_IDP_PROJECT"
```

If a name is already in use, choose another; do not remove the resources. Create a **new** operator file at `AIGUARD_SSO_ENV`, copying [`.env.exemple.local-keycloak-sso`](../../../.env.exemple.local-keycloak-sso) without overwriting an existing file. Its parent must already be private. This file contains public deployment settings; keep it outside an executed workspace. Use the same `--env-file` and explicit `-p` options throughout. Shell values override Compose interpolation, so remove or resolve conflicting overrides before proceeding; do not `source` the file.

### 2. Build your control and identity images

Choose unused local tags so an existing installation's image reference is preserved. The public env example selects:

```dotenv
AI_GUARD_CONTROL_IMAGE=agentsandbox-control:local-sso
AI_GUARD_DEMO_CONTROL_IMAGE=agentsandbox-control:local-sso
AI_GUARD_DEMO_IDP_IMAGE=ai-guard-keycloak-demo:local-sso
```

If a tag is already used, choose a new one in the new env file and in the build commands. The first two settings must reference your newly built control image; the second supplies the fixed-destination IdP ingress. Build both images from this checkout:

```sh
"$AIGUARD_ENGINE" build -f images/control.Dockerfile -t agentsandbox-control:local-sso .
"$AIGUARD_ENGINE" build -f deploy/keycloak-demo/Dockerfile -t ai-guard-keycloak-demo:local-sso deploy/keycloak-demo
```

The [control Dockerfile](../../../images/control.Dockerfile) compiles Rust and the static frontend. The [Keycloak Dockerfile](../../../deploy/keycloak-demo/Dockerfile) pins Keycloak 26.8.1 and its upstream digest, then copies this checkout's realm file into the local image for import at startup. Reusing an older local image would omit the explicit demo subjects. Record the actual image IDs with the installation inventory. `AI_GUARD_DEMO_IDP_IMAGE` selects the local IdP tag; old profiles that omit it retain their previous image default.

### 3. Start the new IdP and inspect its network

Use the separate project and explicit HTTP DEMO profile:

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_IDP_PROJECT" \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http \
  up -d --no-build --pull never keycloak-demo keycloak-demo-ingress
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_IDP_PROJECT" \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http ps
"$AIGUARD_ENGINE" network ls --filter "label=com.docker.compose.project=$AIGUARD_IDP_PROJECT"
```

Inspect each returned network name; substitute the actual name in this command, without inventing a project-derived name:

```sh
"$AIGUARD_ENGINE" network inspect ACTUAL_NETWORK_NAME \
  --format '{{.Name}} internal={{.Internal}} logical={{index .Labels "com.docker.compose.network"}}'
```

Select the network whose logical Compose label is `identity_demo` and whose `Internal` value is true, then write its exact `.Name` into `AI_GUARD_SSO_IDENTITY_NETWORK` in the new env file. Do not select `publication_demo` or another installation's network. The SSO Compose file treats this as an external network and refuses an empty selector. It does not infer or create the identity network from the project name.

The exact issuer is `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo`; its discovery document is at that URL plus `/.well-known/openid-configuration`. The provider administration page is `http://keycloak.ai-guard.test:18080/admin/`. Keep this public-password HTTP DEMO on host loopback. Keycloak receives `AI_GUARD_DEMO_WEB_ORIGIN` inside its container for the supported realm-import environment substitution; setting it only in the host shell or omitting the container environment does not prepare the same realm.

### 4. Prepare the private control volume once

After selecting the network, validate the service configuration without exposing a raw rendered configuration in shared logs:

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" config --quiet
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" --profile sso-prepare run --no-deps --pull never -T \
  setup setup prepare --input - --output-dir /data/installation --demo-defaults \
  < docs/setup/sso/examples/local-demo.json
```

The first `setup` selects the Compose service; the second is the Rust subcommand after the image entrypoint. Input redirection sends the **public manifest**, not a credential file. Do not use a terminal/TTY or manually paste input: stdin is bounded to 262144 bytes and must finish within 5 seconds. The process runs as UID/GID 1000:1000, on a new `installation_data` volume whose `/data` parent is private. It generates private files under that identity instead of mounting a host-owned 0600 OIDC file into a different UID. No network/provider/engine/model is contacted by preparation.

Proceed only after exit 0 and JSON status `prepared` for `/data/installation`. Keep the one-shot container and its outcome for diagnosis. Successful preparation publishes the final directory only after its durable steps. Its important files are:

| Location under `/data/installation` | Purpose |
|---|---|
| `tenants.json`, `oidc.json` | Private gateway registry and OIDC configuration referring to the final paths. |
| `authentication.sqlite` | Separate initialized authentication audit. |
| `tenants/<tenant>/audit.sqlite` | Tenant-bound schema5 storage, with retained migration backups and identity receipts. |
| `tenants/<tenant>/runner`, `viewer` | Generated technical credentials; never browser or worker credentials. |
| `preparation.json`, `completion.json` and origin/binding records | Private recovery identities, fingerprints, bindings and completion receipts. |
| `secret-material.json` | **Private credential recovery material containing the generated secrets. Never display, log, export to a report or distribute it.** |

Files remain 0600 and directories 0700 under the control UID. Preserve the whole private installation and recovery material. Do not run `init` over it, repeat `prepare` on the final path or regenerate missing credentials. Recovery is described below.

### 5. Start ai-guard and use the prepared accounts

Normal startup reopens the completed tenant stores and authentication audit. It does not repeat provisioning and has **no `--initialize-oidc-audit` flag**:

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" up -d --no-build --pull never gateway dashboard ingress
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" ps
```

Open `http://127.0.0.1:4000` for the dashboard or use gateway `http://127.0.0.1:8787` from the foreground CLI described below. A healthy container is not a successful login. Complete a real sign-in before concluding the IdP path works; an error is not an empty activity list.

| Public DEMO account/password | Prepared application access |
|---|---|
| `admin / admin` | Keycloak **master** administration only. No ai-guard account or membership is prepared for it. |
| `admin-ai-guard / admin-ai-guard` | DEMO application account with an explicit membership including `read_tenant_metadata`; the matching verified client role also has to satisfy the server mapping. |
| `user-1 / user-1` | DEMO personal context with metadata/content/export capabilities; resources require separate grants. |
| `user-2 / user-2` | Same personal capabilities, without the application administrator capability. |

The three application subjects are explicit IDs shared by the fresh realm import and `local-demo.json`; they are not derived from usernames. Their import has been checked with Keycloak's official CLI on a fresh realm. Existing realms are **not** reset, adopted or migrated by this procedure, and startup import skips an existing realm. Rebuilding the image does not change its stored subject IDs. For an existing realm, supply a reviewed manifest of its actual verified `(issuer, sub)` values and use generic preparation into a separate new destination. Do not grant membership from a name match or copy the fresh-demo IDs into an unrelated realm.

### Stop, restart and resume a failed preparation

Keep the same engine, env file, project names, images, external network and volumes. Stop only the services belonging to this installation:

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" stop ingress dashboard gateway
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_IDP_PROJECT" \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http stop
```

After an ordinary successful installation, resume its existing services with the same start commands above. Do not rerun preparation. Gateway restart loses live human admissions; browser/CLI users must explicitly sign in again. It preserves the audit and prepared memberships, without restoring tokens, execution authority or sessions from recovery records.

If preparation fails, read its reported retained directory and stop every control/broker/automatic restart that could consume that installation before recovery. A pre-publication failure normally retains `/data/.setup-…`; a failure after publication may retain `/data/installation` even when the command reported an error. Use the exact reported/existing directory, not a guessed hash. Override the one-shot service command to resume the retained preparation:

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" --profile sso-prepare run --no-deps --pull never -T \
  setup setup resume --recovery-dir /data/RETAINED_DIRECTORY_FROM_ERROR --confirm-control-stopped
```

`--confirm-control-stopped` is an operator attestation, not process fencing. Resume uses the exact retained files, inode/owner bindings, credentials and operation identities, on the **same volume, UID and `/data` path**. Moving or copying the recovery tree does not preserve those bindings. It does not reread a new manifest, regenerate secrets or adopt an unrelated final directory. A missing/partial intent, substituted file, both staging and final directories present, or conflicting receipt requires preserving the material and resolving the reported cause. Do not delete a directory/volume, edit the recovery JSON, use `down -v`, or rerun `prepare` to force success. If a new destination is required after an incomplete intent, preserve the old material and deliberately prepare a separate installation; the fixed Compose path must then be adapted explicitly.

## Understand the connections

| Component | Responsibility |
|---|---|
| Gateway Rust | Owns Web PKCE transactions, validates provider responses and API access tokens, admits sessions, applies claim mapping and control-side permissions, and records authentication audit. |
| Dashboard Rust | Serves the compiled frontend and proxies only fixed authentication routes to its configured gateway. It does not authenticate users by a business-session cookie. |
| Browser | Uses the public Web client. Keeps access/refresh tokens in memory after the gateway's one-time delivery. A short-lived opaque cookie correlates the login; it contains no token or PKCE verifier. |
| Human CLI | Uses a separate public client, an external browser and an ephemeral loopback listener. Owns its PKCE exchange; the gateway independently validates and admits the access token. |
| Broker and worker | Keep their separate execution identities. A human SSO login provides no `runner` credential and places no human token in a worker. |

SSO currently provides authorized human consultation and export. Do not pass its token to `broker submit` or interpret sign-in as permission to launch an agent. Human execution delegation and the complete administration view are separate product requirements.

## Prepare an existing or enterprise installation

The following sections are the manual route for existing storage or an operator-approved provider. They are an alternative to the fresh DEMO procedure above. Preserve that installation's actual identities and data; do not import the public DEMO users into it.

1. Prepare the gateway/dashboard images and built frontend using the [local guide](../local/README.md), or your approved enterprise deployment. Preserve the existing tenant registry, credential files, policy, data volumes and service identity.
2. Stop the gateway, brokers and automatic restarts before offline identity maintenance. The flag `--confirm-control-stopped` is an operator attestation; it does not fence another process.
3. Upgrade each configured tenant database explicitly to schema5 for human reads, with a new verified backup at each migration step. Follow the [storage procedure](../broker/README.md#upgrade-storage-explicitly) and the [human-access migration example](../identity/README.md#prepare-storage-and-authority). Neither adding OIDC nor reopening a gateway migrates a database.
4. Obtain the exact verified issuer and stable subject for each approved user through the identity administration process. Use `(issuer, sub)` as the principal. A username, email, display name or token pasted by a user does not establish an approved membership.
5. Inventory reachability from the browser, CLI and gateway, plus the dashboard-to-gateway route. They must use the same issuer identity. A host loopback address identifies the container itself from inside that container; browser access alone proves no container route.

For container-owned databases, perform offline maintenance with the existing control UID/GID and the existing volume, with all other consumers stopped. The [named local access guide](../identity/README.md#prepare-storage-and-authority) explains the isolated one-shot maintenance pattern. OIDC account mutations use the same operator authority and need no local human credential.

### Prepare new storage for an approved existing provider

Generic `setup prepare` can create a separate new installation from a reviewed manifest of actual verified subjects. It never adopts an existing tenant database or verifies the IdP online. Its closed version1 input is distinct from the runtime `gateway.https.json` example:

| Input field | Content |
|---|---|
| `version` | `1`. |
| `gateway` | Approved `OidcGatewayConfigV1`: provider, exact Web origin, label and role mapping. |
| `authentication_policy` | Audit quotas and exactly the configured issuer's two public clients. |
| `ca_bundle_pem` | Optional/null public certificate-only PEM bundle, at most 64 KiB / 8 certificates; no private key. |
| `tenants` | Array of `tenant_id`, `organization_id`, optional `worker_quota` and `identities`. |
| Each identity | `principal` with `kind:"human_oidc"`, exact approved `issuer` and verified `subject`, plus `display_name` and `capabilities`. |

Run under the actual trusted control UID with a new destination below an existing private parent. The CLI form is:

```sh
AIGUARD_BIN="/absolute/path/to/agentsandbox"
"$AIGUARD_BIN" setup prepare --input /absolute/private/ai-guard/approved-manifest.json \
  --output-dir /absolute/private/ai-guard/new-installation
```

For container-owned storage, execute this same Rust command inside a no-network one-shot container under the control UID and its prepared private volume; use `--input -` with public stdin to avoid a host-owner bind mismatch. Omit `--demo-defaults` for a generic manifest. This does **not** adapt the standalone DEMO Compose's hardcoded ports, origin or provider network; use reviewed matching service configuration. The generated `oidc.json` references `ca-bundle.pem` only when supplied. That public bundle configures provider trust, not browser OS trust or the separate CLI→gateway connection. Existing storage instead follows the explicit maintenance/mutation sections below.

## Register public clients and the API audience

Register distinct Web and CLI public clients with Authorization Code and required PKCE S256. Do not distribute a `client_secret`. Disable implicit and password grants. The current login requests `openid`; arbitrary scopes, algorithms or token formats are not configurable through this file.

| Setting | Required value |
|---|---|
| Issuer | One exact URL, including scheme, host, port and realm/path. Discovery is fetched at `<issuer>/.well-known/openid-configuration`. |
| Web redirect | Exactly `<web_origin>/auth/oidc/callback`. The configured origin has no path or trailing slash. |
| CLI redirect | `http://127.0.0.1:<ephemeral-port>/oidc/callback`. Register the provider's supported native loopback-port exception, preserving the literal IP and exact path. The CLI binds its own listener; `localhost` and wildcard paths are not substitutes. |
| API audience | A distinct identifier from both client IDs, present in the access token's `aud`. |
| Access-token profile | RS256; header `typ=JWT`, nonempty `kid`; claims `typ=Bearer`, `iss`, `sub`, `aud`, positive integer `iat` and `exp`; `azp` is one of the two configured public clients. `nbf` is checked when present. ID tokens are not API bearers. |
| Role projection | Operator-controlled array of strings in the verified access token, at the configured claim path. Profile fields editable by a user must not grant permissions. |
| Refresh policy | Provider-side rotation/reuse protection and suitably short token lifetimes. ai-guard also records admission and refresh generations durably; configure and qualify the provider's policy separately. |
| Logout endpoint | Optional exact end-session endpoint. The private POST includes `client_id` and `id_token_hint`; it does not open an interactive logout page. |
| Logout returns | When configured, register the exact static HTTPS `web_post_logout_redirect_uri` on the Web client and `cli_post_logout_redirect_uri` on the CLI client. These are separate from login callbacks and the CLI's ephemeral port. See [logout acknowledgements](#configure-provider-logout-acknowledgements). |

The current access-token profile is narrower than OIDC in general. An Entra ID or other provider configuration is usable only if its actual signed tokens and discovery responses satisfy this profile; its name or general OIDC support does not prove compatibility.

## Prepare the private gateway JSON

Start from one public example and copy it into a new private operator file outside source workspaces, frontend assets and worker exports:

| Example | Use |
|---|---|
| [gateway.https.json](examples/gateway.https.json) | HTTPS shape with deliberately nonfunctional `example.invalid` addresses. Replace issuer, endpoints, both logout return URIs, client IDs, audience, origin, role rule and installation UUID with approved values. |
| [gateway.keycloak-demo.json](examples/gateway.keycloak-demo.json) | Exact defaults of the distributed isolated HTTP DEMO provider. Requires separate account/membership setup and an approved gateway route to that provider. Replace the sample installation UUID. |

These files contain public example values, not credentials or existing identities. Generate a fresh canonical lowercase non-nil installation UUID for a new audit. Keep it unchanged when reopening that audit. The JSON parser does **not** expand environment variables: `${NAME}` is not a configuration mechanism. The `.env.exemple.*` Keycloak variables configure its Compose services; they do not enable gateway SSO.

The configuration is version 1 and rejects unknown fields at every declared object. Its top-level fields are:

| Field | Meaning |
|---|---|
| `version` | Must be `1`. |
| `gateway` | Version 1 object: `provider`, `provider_label`, `web_origin`, `mapping`. |
| `authentication_audit` | `path`, `installation_id` and `policy`. A separate private authentication database, distinct from every tenant database. |
| `ca_bundle_file` | Optional/null, or a path to approved **public** PEM CA certificates for the OIDC provider. |

The provider object in the examples shows every required field. `end_session_endpoint`, `web_post_logout_redirect_uri` and `cli_post_logout_redirect_uri` are optional/null; either configured logout return requires `end_session_endpoint`. Omitted `limits` uses the defaults below. `transport` is exactly `{"kind":"https_only"}` for normal installations. The isolated DEMO uses `{"kind":"isolated_demo_http","exact_issuer":"..."}`; its exact issuer must match `issuer`. HTTP is limited to literal loopback issuers or the fixed distributed DEMO issuer. Its Web origin/redirect must be a literal HTTP loopback address. This exception cannot approve an arbitrary intranet HTTP provider.

Authorization, token, JWKS and optional logout endpoints are exact operator-approved URLs. Discovery must confirm them; it cannot add destinations. URLs cannot carry userinfo, queries or fragments. The provider HTTP client disables ambient proxies, redirects and automatic retries. Approve DNS and outbound destinations in the deployment too: the URL allowlist is not a container firewall.

Paths in `authentication_audit.path` and `ca_bundle_file` are absolute or relative to the OIDC file's parent. Parent traversal and symlinks are refused. Prepare files as the actual control UID, with private files `0600`, private directories `0700` and protected ancestors/ACLs. The authentication database parent must be writable by that UID; making the configuration file read-only does not make its whole parent suitable as a read-only audit destination. The OIDC input is limited to 1 MiB.

For an enterprise CA, set `ca_bundle_file` to a checked file containing only the approved public certificates, at most 64 KiB and 8 certificates. These roots replace the OIDC client's built-in roots while preserving chain, name and validity checks. The gateway also publishes these public certificates in the CLI profile. This option does not change the browser's trust store or the human CLI's TLS trust for the **gateway**. There is currently no custom gateway-CA flag on `human sso`; use an already trusted gateway certificate and qualify that route. Never supply a private key or disable TLS verification.

### Configure provider logout acknowledgements

Set the optional fields under `gateway.provider`, using exact provider registrations on their respective public clients:

| Field | Distributed DEMO value |
|---|---|
| `web_post_logout_redirect_uri` | `https://127.0.0.1/oidc/logout-complete/web` |
| `cli_post_logout_redirect_uri` | `https://127.0.0.1/oidc/logout-complete/cli` |

Each URI must be canonical, static HTTPS, without userinfo, query, fragment or wildcard, and distinct from the Web login callback. Do not derive the native value from its login callback or ephemeral listener port. The HTTPS template instead uses `https://guard.example.invalid/oidc/logout-complete/web` and `/cli`; replace both placeholders with approved registrations. The [Keycloak guide](../keycloak/README.md#register-logout-acknowledgement-returns) documents its explicit per-client entries.

For a client with a configured return, ai-guard sends the private end-session POST with `client_id`, its retained `id_token_hint`, `post_logout_redirect_uri` and a fresh private `state`. Confirmation requires a direct `302` or `303`, an empty body and exactly one `Location`, byte-for-byte equal to that configured URI followed only by `?state=<that request's state>`. A `200`, `204`, HTML confirmation page, different return, extra parameter, missing/duplicate `Location` or wrong state is not confirmation. No fallback to legacy acceptance occurs while that client's return is configured. These strict response/body rules are ai-guard's supported product profile, not a universal OIDC requirement; see [RP-Initiated Logout](https://openid.net/specs/openid-connect-rpinitiated-1_0.html).

ai-guard compares this acknowledgement URI but never follows or contacts it, opens a browser for it, or handles a logout callback. No HTTPS listener, certificate, DNS change or OS service is created for these DEMO values. The issuer/end-session transport remains the isolated HTTP DEMO; an HTTPS acknowledgement URI does not make it enterprise HTTPS compliant. Provider policy must permit direct completion with the supplied ID-token hint; an interactive confirmation page remains unconfirmed. Do not weaken that policy installation-wide merely to force a successful result.

When the option for that client is absent/null, the legacy operation sends neither return URI nor state and retains its supported noninteractive response profile: empty `204`, or `200` with an empty body or empty JSON object. A redirect or interactive HTML remains unconfirmed. Real Web/native CLI interoperability for the new acknowledgement profile is **NOT_RUN** in this documentation/template lot; historical logout observations do not qualify it.

### Upgrade logout configuration without replacing data

Startup import skips an existing Keycloak realm. Updating the realm template or rebuilding its image does not update stored clients. Use the [manual client-registration procedure](../keycloak/README.md#update-existing-logout-registrations) rather than deleting a realm or volume.

Before activation, inventory the existing client registrations, private OIDC configuration and exact component versions. Preserve coherent provider/control backups under their approved procedures, including authentication audit identity/policy, credentials and memberships. Prepare the two scoped client registrations and a reviewed private OIDC file with matching return values; do not rerun `setup prepare`, edit its recovery records or replace `authentication.sqlite`. An unfinished preparation must use its original retained input and [resume procedure](#stop-restart-and-resume-a-failed-preparation).

Deploy matching updated gateway/dashboard/CLI components and apply the reviewed configuration using the installation's existing stop/restart procedure. Keep its UID, data paths, audit UUID/policy and volumes. A gateway restart loses live admissions and pending logins, so users must sign in again. No database migration is introduced by these two options. Retain the previous configuration for a coordinated rollback; an older binary requires these fields to be absent and must not consume a newer public CLI profile. Check local closure, durable audit and provider acknowledgement separately before recording a result; real qualification remains open.

### Request and session limits

If `gateway.provider.limits` is present, supply the complete closed object from [OidcLimitsV1](../../../src/oidc/config.rs). Defaults are:

| Setting | Default |
|---|---|
| `request_timeout_ms` | `5000` |
| `max_response_bytes` / `max_token_bytes` | `131072` / `16384` |
| `max_jwks_keys` / `max_inflight` | `32` / `8` |
| `metadata_ttl_seconds` / `jwks_ttl_seconds` | `300` / `300` |
| `unknown_kid_cooldown_seconds` | `30` |
| `pkce_ttl_seconds` | `300` |
| `clock_skew_seconds` | `5` |
| `max_token_lifetime_seconds` | `3600` |

The gateway additionally limits tokens to 16 KiB and PKCE transactions to 300 seconds. Admission is bounded by access-token expiry, an absolute limit of 1 hour and an inactivity limit of 15 minutes; refresh does not extend the absolute deadline. Web token delivery must be redeemed within 30 seconds. These are current implementation bounds, not measured end-to-end revocation or availability guarantees.

### Work admission and cancelled refresh

HTTP Store reads, mutations and authentication/audit checks run in blocking tasks outside Tokio workers. The gateway rechecks identity, session, mapping, expiry and permissions in the relevant segment at I/O, after any queueing. Each SSE poll/export page repeats its checks. I/O slots are released before client transport; cancelling a request does not free a running segment's slot before its actual completion or undo a durable commit.

New work is refused with `429 capacity_exhausted` when its concurrency or read-frequency bound is reached. Reduce parallel reads/streams and polling frequency, then retry a refused read explicitly. Runner control has a separate lane; it does not consume the viewer read budget. [Current API admission bounds](../../api.md#erreurs-et-limites) distinguish tenant read limits, shared work, authentication and cleanup. They are internal bounds, separate from `provider.limits`, and establish no latency, availability or 100-worker capacity guarantee. An interrupted stream/export remains partial. An ambiguous refresh requires a new sign-in, without automatic exchange retry.

Read admits four concurrent Store leases per tenant for profiles/pages and a known human source. Source and selected context both need admission; when they are the same Store, the segment reuses its live permit. Initial resolution/collision metadata has a separate Discovery bound of four probes per consulted tenant. The shared initial authentication pool admits sixteen tasks. Neighboring Discovery probes performed inside Control, Read or cleanup segments retain their caller's global bound; they do not enter the initial pool again. Their per-Store Discovery bound still applies. A refused metadata probe is unavailable, not proof of an absent binding. Readable collisions remain dynamically checked; an unavailable neighbor cannot authenticate a source implicitly. A resolved runner uses its existing Control lease for its own Store rather than depending on its source's Discovery capacity. These are separate work classes, not a claim that four Read permits cover all authentication I/O.

The gateway reserves bounded refresh completion/cleanup capacity before mutation, including tasks that may later queue for a blocking worker. Normal completion reuses that reservation. If gateway refresh processing is cancelled, the pending/rotating generation remains refused during deferred cleanup. Cleanup checks the exact session, generation, operation and audit receipt so a delayed job cannot close a newer rotation. Runtime shutdown may prevent a queued job from running; cancellation does not guarantee a durable terminal record and has no synchronous SQLite fallback. Pending authority must remain unusable. After restart, live admissions are lost: sign in with a new token instead of reusing an old pending generation. These mechanisms do not purge or repair a full audit.

### Pending login and durable audit

An initial Web login or native CLI login reserves bounded, expiring state in gateway memory. Its cookie, PKCE transaction or native intent correlates an attempt; it does not establish an authenticated user. Starting, cancelling, rejecting or expiring an attempt before verified access-token proof does not append to the durable authentication audit. A gateway restart loses pending attempts: begin a new login rather than replaying the old callback or intent.

After verifying the access token, the gateway must durably record an authentication intention before consulting the account/memberships or admitting a session. Missing audit capacity or a failed write returns `503 audit_unavailable`; no session is admitted. Authenticated refresh, logout and session-expiry auditing remain required. Volatile login expiry does not reclaim durable audit capacity. This change neither purges existing records nor repairs a ledger already filled by earlier attempts: preserve it and its original policy, diagnose the refusal, and resolve capacity through an approved maintenance procedure. Do not delete or replace the audit to resume sign-in.

## Establish accounts, memberships and resources

On the manual route, the operator must create an active OIDC account and membership in each intended tenant database. Sign-in does not provision either. The fresh DEMO preparer already performs these account/membership mutations for its reviewed manifest. For existing storage use these public templates in order:

1. [account.json](examples/account.json): the approved `human_oidc` principal and display name.
2. [membership.json](examples/membership.json): the same principal, actual tenant binding, organization and allowed capabilities.
3. [resource-grant.json](examples/resource-grant.json): an already known resource in that tenant, with the approved owner and organization. The example shares nothing.

Replace every sample UUID with a fresh canonical lowercase non-nil UUID for its new operation or membership; keep each identity stable for exact retries. Replace `REPLACE_WITH_VERIFIED_SUBJECT`, issuer, tenant, organization and resource ID consistently. For the DEMO, use its exact HTTP issuer in all three templates. No subject IDs are preassigned by these files. Creating an account without a membership grants no access; a membership alone grants no ownership of historical resources.

Save each reviewed mutation privately under the existing control authority. For an existing validated schema4–7 database with inactive supervision, run the following command, then repeat it with `oidc-membership.json` and `oidc-resource-grant.json` after each preceding command succeeds:

```sh
AIGUARD_BIN="/absolute/path/to/agentsandbox"
AIGUARD_DATA="/absolute/private/ai-guard/tenant.sqlite"
AIGUARD_TENANT="actual-configured-tenant"
AIGUARD_PRIVATE="/absolute/private/ai-guard"
"$AIGUARD_BIN" human apply \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --mutation-file "$AIGUARD_PRIVATE/oidc-account.json" \
  --max-events 100000 --confirm-control-stopped
```

The input accepts only `version`, `operation_id` and typed `change`; an `actor` field is refused. The effective filesystem operator supplies authority. Each mutation and its audit commit together; the three separate commands are not one transaction. On an ambiguous result, preserve the file and retry the exact operation and bytes. A changed meaning needs a new operation UUID. `expected_revision:0`/`revision:1` means initial creation; changing an existing record requires its exact previous revision and the next revision. Do not edit SQLite projections directly.

### Configure effective permissions

`gateway.mapping` defines a capability ceiling over verified access-token claims. Start with `default_capabilities`, then add the capabilities of matching `rules`. Each `claim_path` is an array of object keys; its leaf must be an array of bounded strings. `equals` matches one exact value. An absent claim does not match; a malformed claim type refuses authentication. Rules with the same path/value are rejected.

Effective rights are the intersection of this ceiling and the active control-side membership, then the selected context and resource permissions. The examples allow `read_metadata`, `read_content` and `export` in personal contexts. The administrator rule adds `read_tenant_metadata`; the intended administrator's membership must also explicitly contain it. Other users' memberships must not acquire it merely because they share an organization. Administrative capabilities cannot appear in `default_capabilities`.

The accepted capability names are `read_metadata`, `read_content`, `export`, `read_tenant_metadata`, `manage_memberships`, `manage_grants` and `read_broker_registry`. A configurable capability does not imply that a complete Web mutation or administration workflow exists. Broker registry access is a separate explicit opt-in described below. Personal and administration contexts are distinct and must be offered by Rust. Realm administration, account names and roles from another client confer no implicit application permission or cross-tenant access.

Change `mapping.revision` whenever the approved mapping changes and restart the gateway with reviewed configuration. For membership/account/grant withdrawal, perform a new audited offline mutation with incremented revision and `active:false` as appropriate. Stop all consumers first, then restart and require users to authenticate again. Changing an IdP role alone is not immediate revocation of an already issued token; role changes become visible through new verified claims, while control-side permissions are revalidated by human reads.

### Opt in to broker registry reads

The distributed setup/example manifests do **not** grant `read_broker_registry`. `setup prepare` produces schema5 and rejects an identity requesting this capability before publishing any installation artifacts. Leave the fresh DEMO and existing ordinary memberships unchanged unless the operator deliberately approves this tenant infrastructure access. The public `admin-ai-guard` role and realm administrator do not acquire it automatically.

For each intended tenant, stop every database consumer and automatic restart, keep the existing prepared installation/volume, then perform [5→6→7 with a verified new backup at each step](../workloads/README.md#enable-named-broker-registry-reads). An existing6 needs only6→7. The prepared path is `/data/installation/tenants/<tenant>/audit.sqlite`; use the actual container UID/GID and inspected volume, not an unrelated host store. Migration adds no account, membership grant or execution activation; a cold reopen remains inactive maintenance. Do not use fresh-store bootstrap, metadata edits or replacement storage as reactivation.

Broker reads require **both** an active control-side membership explicitly containing `read_broker_registry` and an explicit rule in `gateway.mapping.rules` whose verified access-token claim ceiling includes it. The [typed membership procedure](../identity/README.md#authorize-named-broker-registry-readers) shows `human apply` with `version`, `operation_id` and `change.action:"membership"`, exact prior/next revisions, immutable principal/organization and no caller `actor`. For OIDC retain the exact `human_oidc` issuer/subject. Use the same `--max-events` and explicit `--audit-capacity-file` policy as the installation. Only trusted authenticated/offline control can add or reactivate this capability; organization admins cannot promote themselves or others through `manage_memberships`.

In a reviewed private copy of the existing OIDC JSON, a **single rule template inside `gateway.mapping.rules`** is:

```json
{
  "claim_path": ["resource_access", "ai-guard-api", "roles"],
  "equals": "guard-broker-reader",
  "capabilities": ["read_broker_registry"]
}
```

Replace the claim path/client and role with the actual operator-controlled verified claim. This fragment is not a complete OIDC file or an IdP role creation command. Preserve provider URLs, audit identity/policy and unrelated rules. If the same `claim_path`/`equals` already exists, deliberately update its capability list instead of adding a duplicate. Increment `gateway.mapping.revision`, retaining its string format, for example `company-policy-2-brokers`. Never put an administration capability in `default_capabilities`. A raw claim or matching role without the explicit local membership still grants no access; membership without a matching ceiling also grants no broker read. No role mapping chooses another tenant or turns a browser into runner.

Restart matching gateway/dashboard components with the existing configuration and data, without an initialization flag. Reauthenticate and choose a currently offered administration context carrying the capability. **Brokers** opens **Brokers enregistrés du tenant.**; a context with only this right lands there without resource/event/export/SSE reads. `read_tenant_metadata` retains its old organization-filtered resource semantics. Neither ordinary `human` nor foreground SSO CLI has a brokers command; use this dashboard view or the [typed human GET contract](../../api.md#registre-brokers-humain--human-broker-registry).

To withdraw access, have the operator apply a new audited membership revision removing the capability or deactivating the membership, or revise the approved claim ceiling and require a new login. An organization administrator can retain/remove an existing right or keep it dormant, but cannot add it or reactivate its inactive membership. A recognized context/permission refusal clears prior data, detail and cursors; late replies cannot restore them. Revocation cannot retract bytes already received, and the visible30second polling trigger is no end-to-end withdrawal deadline.

The page requires mandatory durable human audit, including empty results, and shares identity/resource F1 tenant/principal budgets. The dashboard preserves `429 audit_read_budget_exhausted`/`contact_operator` from its identity preflight as well as the page read. It stops automatic reads and keeps any permitted previous page dated/stale. Preserve the tenant database, policy and receipts, inspect occupancy/reserves and repair the cause; do not reset audit or rotate credentials for capacity. After intervention, **Relire après intervention de l’opérateur** performs an explicit retry. Temporary capacity or transport errors remain separate. A503 `migration_required` needs the offline upgrade, not a viewer fallback. See [named access](../identity/README.md#authorize-named-broker-registry-readers) for page states and receipt/freshness limits.

Actual broker registration and heartbeat operations must already have populated the real registry. Empty is valid; do not insert fixture observations. This view does not qualify a real browser/OIDC deployment, federation, complete runtime inventory or production release. Refer to the matching engineering validation for what was actually run.

## Start or reopen the existing services

Add the following arguments to your existing gateway service, retaining its actual registry, policy and decision-engine settings. This is an argument contract for a prepared container deployment, **not** a complete new Compose stack. `/control/*` paths are replaceable private container paths. With the standard image entrypoint, the service command starts at `gateway`:

```json
["gateway", "--listen", "0.0.0.0:8787", "--container-listen",
 "--tenants-file", "/control/tenants.json", "--policy", "/control/policy.json",
 "--oidc-config", "/control/oidc.json", "--initialize-oidc-audit"]
```

Use `--initialize-oidc-audit` only for an exclusively new authentication database at the configured path. It refuses an existing file, including an empty or partial one. A failure may retain a partial new file: preserve it for diagnosis; initialization is not a repair operation. The flag does not create/migrate tenant identities or databases.

On every normal restart, **remove `--initialize-oidc-audit`** and retain `--oidc-config`. Reopen requires the existing valid audit with its original installation UUID and identical audit policy, including approved-client ordering and quotas. Editing quota values or clients is not an implemented audit-policy migration. Keep the original records and resolve capacity/configuration changes explicitly; do not replace the database to clear a refusal. Without `--oidc-config`, the gateway enables no SSO and makes no OIDC provider calls.

Enable the dashboard's fixed proxy with the same exact public Web origin as `gateway.web_origin`:

```json
["dashboard", "--listen", "0.0.0.0:4000", "--container-listen",
 "--upstream", "http://gateway:8787", "--shared-tenants",
 "--front-dir", "/app/front", "--oidc-web-origin", "https://guard.example.invalid"]
```

Adapt the upstream to the actual internal gateway route and verify the built asset path in your image. `--shared-tenants` removes the fixed viewer-file requirement; it does not grant users all tenants. If retaining a fixed viewer deployment, keep its existing viewer-file configuration instead. For the DEMO set `--oidc-web-origin` to `http://127.0.0.1:4000`. The dashboard needs only its fixed gateway route, not direct access to the IdP. Neither service requires an engine socket or a mount of the project being executed.

The Rust listeners are HTTP listeners; configuring an HTTPS public origin does not create TLS termination. Prepare and qualify the approved transport before exposing the installation. The existing [Keycloak guide](../keycloak/README.md) covers the provider project, DNS and public DEMO accounts. The legacy base Compose files do not attach the gateway to the identity network. The standalone SSO files above do, using the explicitly inspected external network. For a manual deployment, prepare that narrowly scoped gateway connection while preserving the same issuer and leaving dashboard/workers outside it; a declared topology does not prove successful interoperability.

## Sign in from the browser

Open the exact configured dashboard origin. The connection screen checks SSO status and displays the provider label and issuer. Choose **Se connecter avec …**, complete sign-in in the provider, then choose one of the contexts returned by ai-guard. A valid provider account without an approved application membership is refused. A successful sign-in with no authorized resources may show an empty resource page; an authentication, storage or network failure must remain an error.

The gateway consumes its callback and sends the browser to `/auth/oidc/complete` without forwarding code/state query parameters. Tokens are delivered once and kept in memory, outside URLs, Web Storage, logs and exports. Reloading, closing the page or restarting the gateway requires a new explicit sign-in. Do not copy tokens from developer tools into a file or another CLI command. The browser coordinates refresh when a refresh token is available; a refusal or ambiguous result closes the local session. New generations replace the authorized view rather than retaining old context data.

Use the application's disconnect action. Local browser state is cleared even when the control/provider cannot confirm logout. Keep the outcomes separate:

| Outcome | Meaning |
|---|---|
| `local_session` | `closed` confirms closure of this ai-guard admission by the control; `unknown` does not confirm that closure. Clearing browser/process memory is a separate local action. |
| `audit` | `recorded` or `unavailable` reports the control's mandatory logout audit; a provider response cannot substitute for it. |
| `provider_logout` | `accepted` means the component attempting provider logout received its configured supported response. `failed`, `unavailable`, `unsupported` or `not_attempted` does not confirm provider logout. |
| `control_provider_logout` | Native CLI report of the gateway's provider outcome. It stays `not_attempted` for native sessions: the CLI retains the ID-token hint privately and makes its own provider call, reported in `provider_logout`. The gateway does not recover or receive that hint for native logout. |

An accepted acknowledgement does not assert global JWT revocation, closure of every other browser/client session or delivery of a browser redirect. JWTs validated offline can remain valid under the installation's token/revocation policy.

### When the context list is partial

A neighboring profile that cannot obtain read admission also makes the identity response partial. Its skipped lookup is not reconciled as an empty profile and does not revoke previously issued contexts. Their next use still checks the selected store, authorization and limits. Saturation of a local credential's source or the selected context returns `429` rather than a false empty result; an unavailable mandatory store remains an error. Refresh the context list after the load or storage fault is resolved.

The identity response includes the required boolean `contexts_complete`. `true` means all configured tenant profile lookups for that response succeeded. `false` means at least one source is unavailable: the offered contexts are usable candidates from readable sources, and the response does not name unavailable tenants or guess their memberships. This applies to named local credentials as well as admitted OIDC identities. No request-supplied tenant hint or neighboring outage changes your scope.

The dashboard shows **Liste des contextes partielle** and **Actualiser les contextes**. Use that refresh action after the operator repairs the affected store. A missing context, or even an empty partial list, does not prove that access was revoked. Choose only a context currently offered by the control. Its own authorization and mandatory audit are checked again when used; an unavailable selected store returns `503` instead of an empty resource list. A local credential's source store must also remain readable before it can authenticate you.

OIDC sign-in can succeed when an approved active membership is established in a healthy source. If no approved membership can be established while a source is unavailable, the error remains `503`; it is not a definitive `permission_denied` result. Preserve the store and its audit, repair the fault, then retry explicitly. A verified lack of membership across readable sources remains a refusal.

## Sign in and work in the foreground CLI

Build a native host CLI from the same source revision if one is not already available. The control image contains a Linux binary; do not extract it for use on a macOS workstation. From the repository root, with the repository's Rust toolchain:

```sh
cargo build --locked --bin agentsandbox
```

If you set `CARGO_TARGET_DIR`, use the binary in that directory instead. Keep the selected native binary matched to the installed gateway version.

Run the trusted CLI on the user's workstation, pointing to the **gateway origin**, without `/api` or another path. This is ai-guard login, separate from provider CLI login:

```sh
AIGUARD_BIN="$(pwd)/target/debug/agentsandbox"
"$AIGUARD_BIN" human sso --gateway https://guard-api.example.invalid
```

Use the actual gateway address. An isolated local gateway can use `http://127.0.0.1:8787`. The CLI verifies HTTPS, disables ambient proxies, redirects and automatic retries, and accepts HTTP only on a literal loopback IP. If the supported system browser launcher is unavailable, restart explicitly with `--browser manual`. That option prints a transient authorization URL to open in an external browser; it is not a headless/device/password flow. Keep that URL private and out of logs. A browser must return to this same workstation's ephemeral listener.

Keep the process and standard input open. After `session_ready`, enter `help` or these commands; replace uppercase placeholder IDs with exact values returned by the preceding read:

```text
identity
context CONTEXT_ID_RETURNED_BY_IDENTITY
resources --limit 25
events --view metadata --resource-kind session --resource-id AUTHORIZED_SESSION_ID --limit 25
export --view metadata --resource-kind session --resource-id AUTHORIZED_SESSION_ID --output-dir /absolute/private/ai-guard/new-export
refresh
identity
logout
```

`context` accepts only an ID offered to this identity. Resource kinds are `session`, `sandbox`, `agent` and `project`; choose `metadata` or `content` explicitly. Content and export need separate permissions. Pagination uses the exact opaque `next_cursor` with unchanged selection; one page is not a total. Commands support quoting but no shell/environment expansion, substitution or execution. Do not prefix foreground commands with `human` or paste bearer tokens.

Inspect `contexts_complete` in the `identity` result before treating its context list as complete. After a neighboring store recovers, run `identity` again and select an offered context. A partial list does not revoke a missing membership; it also does not authorize using an invented or unchecked context.

Tokens and the PKCE verifier remain in this process; no durable token cache or credential file is created. `refresh` is explicit in the CLI. After it succeeds, request `identity` and select a newly offered context; old contexts/cursors and export generations are invalid. A failed or ambiguous refresh closes the foreground session without automatic retry. `logout`, `quit`, `exit`, EOF and Ctrl-C close local state; a later login is a new session. Gateway restart loses live admissions even though the audit remains durable.

The native CLI uses its own retained ID-token hint and configured CLI logout return; the login listener is not reused. Read `provider_logout` alongside `control_provider_logout`, `local_session` and `audit` using the [separate outcome meanings](#sign-in-from-the-browser). A failed provider acknowledgement does not restore the foreground session or trigger a browser/callback fallback.

Export needs an existing private parent and a **new** final destination. It has a budget of 25 pages, 4 MiB and 60 seconds from the initial preparation. On interruption retain the adjacent `.preparing` directory. `resume-export --output-dir /absolute/private/ai-guard/new-export` works only with that exact preparation, context, still-live foreground session and admission generation, within the original deadline. Refresh, process exit/restart, expired authorization or an exhausted budget requires a new explicitly authorized export into another fresh destination; preserve the unresolved preparation. A read receipt records the control's audit, not proof that bytes reached their destination or a complete signed audit verification.

## Diagnose and recover

`GET /auth/oidc/status` reports enablement, issuer and provider label; `GET /auth/oidc/cli-profile` supplies the public CLI provider profile. They do not probe IdP reachability. A healthy process or enabled status therefore does not establish a working login. Diagnose with reviewed configuration, public certificate metadata and redacted errors; do not collect callback queries, cookies, authorization headers, raw provider replies or complete environment dumps.

| Result or symptom | Action |
|---|---|
| SSO disabled / `oidc_not_configured` | Check explicit gateway `--oidc-config` and dashboard `--oidc-web-origin`, with the matching binary. No local/viewer fallback occurs automatically. |
| Invalid configuration / startup refusal | Check the closed version1 fields, exact URLs, distinct clients/audience, file ownership/ACLs and separate audit path. No environment interpolation occurs. |
| `origin_denied` / callback refusal | Open the exact configured origin; verify callback and client registration. Begin a fresh login after expiry/cancellation; never replay a callback/code. |
| `permission_denied` | Check verified `(issuer, sub)`, active account/membership and intersection with mapped capabilities. Then check organization/resource grants. IdP administration is not application authority. |
| `credential_invalid` / `session_conflict` | The admission is expired, revoked, rotated, consumed or lost on restart. Clear the view and sign in again. Do not retry an ambiguous exchange. |
| `migration_required` | Stop controls, retain backups and perform the explicit next storage migration. Do not initialize a replacement store. |
| TLS, discovery, signature or provider failure | Verify approved DNS/routes, issuer/endpoints, public chain/SAN/time, token profile and key rotation. Repair the source configuration; preserve verification and fixed destinations. |
| Logout provider `failed` / `unavailable` | Check the exact per-client post-logout registration and direct-completion policy, end-session endpoint and approved transport. HTML, a wrong `Location`/state or a body-bearing redirect is unconfirmed; preserve local closure and audit outcomes. Do not follow the returned URI, log its state/hint or retry through an interactive fallback. |
| `audit_unavailable` | Authentication or mandatory read audit cannot be trusted/written, or capacity is exhausted. Preserve data and repair storage/configuration before retrying. No bypass is available. |
| Partial contexts / `contexts_complete:false` | Preserve the affected store and repair its availability. Use **Actualiser les contextes** or run foreground `identity` again; an incomplete list is not proof of absent or revoked access. Each selected context still requires its own healthy store. |
| Missing or incompatible human identity field | Deploy matching gateway, built dashboard and CLI versions together. Do not drop `contexts_complete`, substitute a tenant hint or replace the database to bypass a contract error. |
| `capacity_exhausted` / HTTP `429` | Work admission, read frequency, live authentication or cleanup capacity is bounded. Reduce parallel reads/streams and polling; let running work finish or attempts expire before an explicit retry. Cancelling requests does not immediately free running I/O slots. Do not create a new audit to evade the bound; treat ambiguous refresh separately as a new sign-in. |
| Lost connection / stale data | Retain an explicit disconnected/stale state. Reauthenticate and select a current context before resuming; loss is not zero activity. |

Keep the OIDC file, exact binary/image identities, tenant registry, private identities/grants and their tenant databases together with the separate authentication audit inventory. The audit stores typed outcomes and token fingerprints, not recoverable tokens. It is not atomic with an external IdP: a provider exchange followed by a crash can leave an unknown outcome requiring a new login.

No authentication-audit backup/restore CLI is exposed by these commands. The Rust library has a verified-new backup operation; do not invent a shell wrapper or treat a live SQLite main-file copy as coherent. Before backup/recovery, stop every consumer and preserve a coherent SQLite set, installation identity, audit policy and existing configuration under the installation's approved storage procedure. Keep existing material and validate a recovery in a fresh isolated destination before any cutover. Recovery does not restore tokens, live sessions or execution authority.

The audit verifies its chain and live process floor, but a complete database rollback across restart needs an independent witness to detect it. Do not promise anti-rollback, signed external checkpoints or release qualification from this configuration. Quotas count records (including genesis) and serialized payload bytes; SQLite pages, WAL/SHM, backups and filesystem use add overhead. An exhausted audit refuses new mandatory operations; an audit-policy migration/retention procedure remains to be supplied before operating at that boundary.
