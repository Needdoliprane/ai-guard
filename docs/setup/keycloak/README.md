[English](README.md) · [Français](README.fr.md)

# Keycloak and OpenID Connect

Use this guide to prepare a local Keycloak identity provider or connect ai-guard to an approved enterprise OIDC provider. Identity, agent authentication and inference are separate choices: signing in to ai-guard does not sign in to Codex or Claude, select a model, or provide a worker with your identity token.

The local profile uses public demonstration accounts on the host loopback. For normal operation, use your own provider, accounts, HTTPS and access policy. The product's [identity contract](../../sso.md) defines the Web, CLI, permissions and audit behaviour described here. The [installation index](../README.md) covers the complete installation.

## Choose the configuration

| File | Purpose |
|---|---|
| [`.env.exemple.local-keycloak`](../../../.env.exemple.local-keycloak) | Local identity provider with the agent/provider settings chosen separately. |
| [`.env.exemple.local-keycloak-qwen`](../../../.env.exemple.local-keycloak-qwen) | Same identity settings combined with the Qwen inference context. Identity does not change the model's role or safety policy. |
| [`compose.yaml`](../../../deploy/keycloak-demo/compose.yaml) | Separate, explicitly enabled identity project with Keycloak and its fixed-destination Rust ingress. |
| [`Dockerfile`](../../../deploy/keycloak-demo/Dockerfile) and [realm](../../../deploy/keycloak-demo/ai-guard-demo-realm.json) | Pinned Keycloak image and initial DEMO realm. |

The examples contain public settings. Keep provider credentials in the installation's private configuration, outside the source workspace and images. Do not pass an existing credential file as this provider's environment file. Compose forwards only the environment entries declared for each service; an example is not a credential handoff to workers.

These four variables configure the local identity provider:

| Variable | Default | Meaning |
|---|---|---|
| `AI_GUARD_DEMO_IDP_HOST` | `keycloak.ai-guard.test` | Public DNS name, without a scheme or path; also the internal network alias. |
| `AI_GUARD_DEMO_IDP_PORT` | `18080` | Same public and container HTTP port. Choose a free port above 1024, other than 18081. |
| `AI_GUARD_DEMO_WEB_ORIGIN` | `http://127.0.0.1:4000` | Exact dashboard origin, without a trailing slash, path, query or fragment. |
| `AI_GUARD_DEMO_CONTROL_IMAGE` | `agentsandbox-control:local` | Prepared ai-guard control image used only as the DEMO ingress. |

Use the same file for every command below. To combine Keycloak with Qwen, replace `.env.exemple.local-keycloak` with `.env.exemple.local-keycloak-qwen` throughout. These variables configure the provider; the product's identity configuration is described separately below. Shell environment values take precedence over values in a Compose environment file; resolve conflicting settings before starting.

Keep `-p ai-guard-keycloak` on every identity Compose command. The examples set `COMPOSE_PROJECT_NAME=ai-guard-local` for the main application; the explicit project option keeps the IdP separate.

## Prepare DNS, images and a new project

Configure local DNS so `keycloak.ai-guard.test` resolves only to the host loopback. An operator-managed hosts entry can use `127.0.0.1 keycloak.ai-guard.test`. Preserve existing entries. The browser, CLI and containerised control must use the same public issuer name; replacing that name with `localhost`, a container service name or an IP creates a different issuer. The Compose network supplies the internal alias independently of host DNS.

With the defaults, the exact issuer is `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo`. The dashboard origin is `http://127.0.0.1:4000`; the normal gateway port is 8787. Port 18081 is a second ingress listener inside its container, bound to that container's loopback and unpublished.

Choose a new project name. The commands use `ai-guard-keycloak`; before the first start, check that no resources already carry that project label:

```sh
docker ps -a --filter label=com.docker.compose.project=ai-guard-keycloak
docker volume ls --filter label=com.docker.compose.project=ai-guard-keycloak
docker network ls --filter label=com.docker.compose.project=ai-guard-keycloak
```

If the name belongs to another installation, choose another name and change `-p` in every command. Reuse it only when deliberately resuming the same DEMO. Never merge this Compose file into an existing application's project or reuse its credentials or data volume.

Prepare the control image as described by the [local installation guide](../local/README.md). For the default tag, its build command is:

```sh
docker build -f images/control.Dockerfile -t agentsandbox-control:local .
```

Choose a distinct tag and set `AI_GUARD_DEMO_CONTROL_IMAGE` accordingly if that tag is already used by another installation. Building the image can download build dependencies; an already prepared image does not need rebuilding for each start.

The Keycloak Dockerfile pins version 26.8.1 and the multi-platform digest below. Acquire that public base image explicitly, then prepare the local optimized image:

```sh
docker pull quay.io/keycloak/keycloak:26.8.1@sha256:3f3c5f8e03c614774b4bd9173b88391fefa4153bfc010d8012d59ad825377332
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http config --quiet
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http build keycloak-demo
```

For an offline installation, prepare or import the required images and build dependencies before entering the isolated environment. Record the actual image identities and architecture with the installation inventory; a mutable local tag alone is insufficient provenance.

## Start the local identity provider

From the repository root, after the images are prepared:

```sh
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http \
  up -d --no-build --pull never keycloak-demo keycloak-demo-ingress
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http ps
```

The profile requires `--profile keycloak-demo-http`. Its two services start independently of the product's gateway and dashboard. Keycloak uses a new identity data volume and an internal network. The ingress publishes only `127.0.0.1:18080`; neither service mounts a runtime socket, the original project or existing control credentials. Keycloak has a 1 GiB memory limit, 2 CPUs and 256 PIDs; the ingress has 192 MiB, 0.5 CPU and 128 PIDs.

Open `http://keycloak.ai-guard.test:18080/admin/` for provider administration. The public discovery document is at `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo/.well-known/openid-configuration`. Check its exact `issuer` before configuring a client. A running container is not a successful product sign-in.

This HTTP mode is explicitly limited to the isolated local DEMO. Keep its known credentials on loopback. HTTPS configuration and client trust are separate steps; changing a hostname or generating a certificate does not change this Compose profile to HTTPS.

## Accounts, clients and token mapping

| Public account | Public password | Intended access |
|---|---|---|
| `admin` | `admin` | Bootstrap administration in **master**. No imported application identity or implicit ai-guard permission. |
| `admin-ai-guard` | `admin-ai-guard` | DEMO realm, client role `ai-guard-demo-api/admin-ai-guard`. Application scope still comes from the control's approved memberships. |
| `user-1` | `user-1` | DEMO realm, no application administrator role. |
| `user-2` | `user-2` | DEMO realm, no application administrator role. |

The bootstrap applies to a fresh DEMO database. These values are never installation defaults for an enterprise realm. An administrator of Keycloak and an administrator of ai-guard have different responsibilities.

| Client | Contract |
|---|---|
| `ai-guard-demo-web` | Public; Authorization Code; PKCE S256 required; registered login return `${AI_GUARD_DEMO_WEB_ORIGIN}/auth/oidc/callback`; exact Web origin. Explicit logout returns preserve `${AI_GUARD_DEMO_WEB_ORIGIN}/` and add `https://127.0.0.1/oidc/logout-complete/web`. |
| `ai-guard-demo-cli` | Separate public client; Code and required S256; registered login reference `http://127.0.0.1/oidc/callback`; no CORS origin. Native login uses an ephemeral loopback port, the same IP and fixed path, requiring provider support for the port exception. Its explicit logout return is `https://127.0.0.1/oidc/logout-complete/cli`, without port inheritance. |
| `ai-guard-demo-api` | Bearer-only resource server and access-token audience; no login flow or service account. |

Neither public client distributes a `client_secret`. Implicit, password grants, device flow, CIBA, service accounts and token exchange are disabled. Do not replace a failed browser flow with a password grant or manual token copy.

Register the CLI reference in the canonical form above, without explicit `:80`. Keycloak 26.8.1 normalizes the default HTTP port away during its native-loopback comparison; a registered literal containing `:80` can prevent that match. The CLI still binds an ephemeral local port and includes it in the actual authorization request. Keep the literal IP, exact `/oidc/callback` path and required S256, without a wildcard. This configuration alone does not prove a successful CLI sign-in. Startup import skips existing realms; use the data-preservation procedure below when changing an existing registration.

### Register logout acknowledgement returns

The [realm template](../../../deploy/keycloak-demo/ai-guard-demo-realm.json) uses the client attribute `post.logout.redirect.uris`. Keycloak 26.8.1 stores multiple values in one string separated by `##`: the Web value is `${AI_GUARD_DEMO_WEB_ORIGIN}/##https://127.0.0.1/oidc/logout-complete/web`. The CLI has the single explicit `https://127.0.0.1/oidc/logout-complete/cli`. Do not use `+`, inherit login redirects, add wildcards or copy the CLI's ephemeral login port. The old Web logout registration is retained. See the pinned [multivalue encoding](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/AbstractClientConfigWrapper.java#L60-L71), [delimiter](https://github.com/keycloak/keycloak/blob/26.8.1/server-spi-private/src/main/java/org/keycloak/models/Constants.java#L140-L147) and [logout registration handling](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/OIDCAdvancedConfigWrapper.java#L439-L469).

Both public clients explicitly set `logout.confirmation.enabled` to the string `false`, the pinned version's default ([attribute key](https://github.com/keycloak/keycloak/blob/26.8.1/server-spi-private/src/main/java/org/keycloak/protocol/oidc/OIDCConfigAttributes.java#L65), [default](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/OIDCAdvancedConfigWrapper.java#L369-L374)). Provider policy must permit direct completion with the supplied ID-token hint; this setting alone does not prove the response. An interactive HTML confirmation remains unconfirmed by ai-guard. Preserve the installation's approved policy rather than weakening it globally.

The matching [local manifest](../sso/examples/local-demo.json) and [gateway example](../sso/examples/gateway.keycloak-demo.json) select these two HTTPS URIs as protocol acknowledgements only. ai-guard compares a direct `302`/`303` with one exact `Location` containing only its fresh request state and an empty body; it never follows that URI, opens a browser for it or installs a logout callback. No TLS listener, certificate, DNS entry or OS service is needed or created at those paths. This is ai-guard's supported response profile, not a universal OIDC body rule. The issuer and end-session endpoint remain isolated HTTP DEMO endpoints; HTTPS acknowledgement URIs do not qualify this deployment for enterprise HTTPS. See the [full response and compatibility contract](../sso/README.md#configure-provider-logout-acknowledgements). Real Web/native CLI qualification of this new profile is **NOT_RUN** in this template/documentation lot.

Both public clients have exactly `basic` and `profile` as default client scopes, with no optional scopes. `basic` supplies Keycloak's access-token subject mapper; the signed token must contain a non-empty `sub`. Its absence is a provider configuration error, not a reason to weaken validation. `basic` need not appear in the token's textual `scope` field: it is a default client scope whose mapper is applied by the provider.

Two dedicated mappers add `aud=ai-guard-demo-api` and the array `resource_access.ai-guard-demo-api.roles` to access tokens only. Explicit role scope mappings and `fullScopeAllowed=false` limit this projection. Only `admin-ai-guard` receives the application administrator role. An ID token establishes the OIDC login; it is never an API bearer.

## Product identity configuration contract

Record the following in a separate, versioned JSON OIDC installation configuration. This reference contract is distinct from the version 1 tenant registry; do not add these fields to a tenant registry or infer a startup flag from this guide. The configuration example and its documented application mechanism must be kept in sync with this contract.

| Setting | DEMO value or rule |
|---|---|
| Issuer | Exact issuer above; one spelling across browser, CLI and control. |
| Client IDs | Separate Web/CLI IDs above; required Code+S256. |
| Web login return | Exact configured origin and callback; no wildcard. |
| CLI login return | Literal loopback IP, ephemeral listener port, fixed `/oidc/callback` path; close after success, error or deadline. |
| Logout acknowledgement returns | Optional `gateway.provider.web_post_logout_redirect_uri` and `cli_post_logout_redirect_uri`, both requiring `end_session_endpoint`; exact static canonical HTTPS registrations above, without query, fragment, userinfo or wildcard. Distinct from login callbacks. |
| API validation | RS256, issuer and audience exact, mandatory `exp`, `iss`, `sub`, `aud`, `nbf` checked when present; bounded clock tolerance. DEMO token profile: JWT header `typ=JWT`, payload `typ=Bearer`, allowed `azp` restricted to the two public clients. |
| Requested scopes | `openid` and the minimum profile information; provider default client scopes `basic`+`profile`. |
| Administrator mapping | Verified access-token claim path `resource_access.ai-guard-demo-api.roles`, value `admin-ai-guard`; apply only to explicitly assigned application scopes. |
| Memberships | Administered control-side associations keyed by verified `(issuer, sub)`. No email, account name, free header or user-editable profile chooses a tenant. |
| Network and trust | Approved discovery/JWKS/token destinations, bounded requests and caches, explicit public CA trust for HTTPS. No general Internet access for internal control services. |
| Session policy | Token lifetime, refresh rotation/reuse policy, mapping changes, logout and revocation bounds published for this installation. |

For the local profile, the provider sets access tokens to 120 s, authorization codes to 30 s, idle SSO sessions to 900 s and absolute sessions to 3600 s. Refresh rotation is enabled with reuse forbidden. These values do not make offline JWT revocation immediate. Approve the actual cache and revocation bounds with the installation policy.

Rust verifies signatures and the access-token profile before mapping roles or memberships. Transactions bind issuer, client, return, state, nonce and a private verifier, with expiry and one-time consumption. Web tokens stay in memory, outside browser storage, URLs and logs. A broker keeps its own workload identity; a human login never provides a `runner` credential or sends a human token to a worker.

## Use the application with these identities

Configure the DEMO's explicit memberships and administration scope in the control. Open the dashboard at the configured Web origin and authenticate `user-1` through the provider. Sign in through the CLI's native browser flow with the same account; verify that both identify the same `(issuer, sub)` and allowed context.

Submit a short, non-destructive task through the trusted broker. Inspect its request, authorisation, execution and result in the audit; keep the initiator distinct from the broker and agent actors. Review changes and export only into a new destination. Use `user-2` to check that private resources and unauthorized contexts are refused, then use `admin-ai-guard` to inspect only its assigned administration scope. The master account must not gain application access implicitly.

On logout, clear the local identity and private transaction state, cancel pending requests and close subscriptions. Confirm the provider logout separately; local sign-out must remain possible during a provider outage. A role or membership removal must also withdraw formerly visible data and subscriptions within the configured revocation bound. SSO failure never switches to another identity or to a paid inference provider.

Read `local_session` (ai-guard admission closure), `audit` (control audit) and `provider_logout` (supported provider response) independently. In native CLI output, `control_provider_logout:not_attempted` means the gateway did not attempt provider logout: the native CLI retains the hint privately and reports its own provider call in `provider_logout`. An `accepted` acknowledgement does not assert global JWT revocation or closure of every other session. The [SSO guide](../sso/README.md#sign-in-from-the-browser) explains failure and unknown outcomes.

## Connect an enterprise provider and HTTPS

Obtain an approved issuer, exact Web and CLI returns, public clients, API audience, token profile, claim mapping and session policy from the identity administrator. Use a separate normal realm or provider configuration, without importing this DEMO's accounts or role names. Keep local/offline identity and agent subscription/API choices independent of OIDC.

Use the same HTTPS issuer inside and outside the container network. Configure DNS, certificate SANs, chain and trusted public CA consistently. Serve the leaf certificate and intermediate chain at the approved TLS endpoint; deliver only the public trust anchors to clients. Private CA keys never enter an image, worker, repository or report. For local certificate preparation, see the [local CA guide](../../demo-pki.md); install trust explicitly per client, without a global automatic trust change or an insecure bypass.

Approve the control's route to the provider before enabling the connection. Gateway and dashboard keep their internal network boundary; a browser reaching an issuer does not establish container reachability. Allow only the required identity destinations and preserve origin/callback guards. Discovery, redirects and JWT-provided URLs must not become a general proxy or arbitrary JWKS fetch mechanism.

Before admitting users, exercise Web/CLI sign-in, two separate application scopes, role removal, expiry, refresh reuse refusal, provider outage and key rotation. Check the new key through the configured issuer, and refuse an unknown key after bounded refresh. Document recovery without relaxing issuer, audience, signature, TLS or tenant validation. The [enterprise installation guide](../enterprise/README.md) covers the surrounding deployment choices.

## Preserve data and resume

Keep the project name, environment file, image identities and volume reference with the installation record. A restart retains the DEMO's users and provider data:

```sh
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http stop
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http start
```

Startup import skips an existing realm. Editing the realm JSON, changing the Web origin or rebuilding an image does not migrate that realm. Plan an explicit, scoped provider change, or use a new DEMO project for another configuration. Never delete an existing volume to repair login.

Before a provider upgrade, preserve a coherent backup and the exact prior image/configuration under the provider's supported backup procedure. A realm JSON export alone is not a complete database backup. Restore into a fresh isolated destination first, then verify issuer, users, mappings and the application's authorization policy. Keep the previous data and identities until recovery is confirmed.

### Update existing logout registrations

1. Keep the current realm, users, subjects, credentials, mappings and volumes. Inventory the two clients' existing settings and take the coherent provider/control backups described above and in the [SSO recovery guide](../sso/README.md#diagnose-and-recover).
2. Through the operator's approved Keycloak administration interface, add the exact Web acknowledgement URI to that client's **Valid post logout redirect URIs**, preserving its existing exact Web return. Add the explicit CLI acknowledgement URI to the native client's post-logout list. For the DEMO use the literal HTTPS values above; replace `${AI_GUARD_DEMO_WEB_ORIGIN}/` with the installation's actual origin plus `/` when editing a stored client. `##` is the serialized attribute separator, not part of either URI entered separately in the console. Preserve other approved exact entries; do not use `+`, login-list inheritance or wildcards. Do not change the canonical native login registration, public-client flags, Code/S256, disabled grants or token mappers.
3. Verify the clients' approved direct-completion policy (`logout.confirmation.enabled:false` in the distributed DEMO). Do not change unrelated realm-wide session policy. Prepare a reviewed private OIDC configuration with matching `web_post_logout_redirect_uri` and `cli_post_logout_redirect_uri` and the existing exact end-session endpoint.
4. Activate only with matching updated gateway and native CLI binaries, following the [existing-installation upgrade procedure](../sso/README.md#upgrade-logout-configuration-without-replacing-data), approved restart and reauthentication. Older binaries reject these new fields; omitting them retains legacy operation. Do not rerun fresh preparation, reset the realm, delete a volume or replace an audit to apply this change. Check closure, audit and provider response separately; no real logout result is supplied by editing these settings.

## Diagnose a refused connection

| Symptom | Check and recovery |
|---|---|
| Name not found or connection refused | Host DNS, internal alias, configured port, published loopback listener and correct project. Restore the approved route. |
| Issuer mismatch | Discovery, hostname, scheme, port and realm must match exactly. Fix configuration; do not rewrite a returned issuer. |
| Missing subject | Check default `basic` on both public clients and its `oidc-sub-mapper`. Retain required `sub` validation. |
| Audience or role refused | Check access-token audience, the API role mapper and approved memberships. Master administration is not application authorization. |
| Callback rejected | Exact origin/path, selected client, transaction expiry, state and nonce. Begin a fresh login; do not replay a code. |
| TLS rejected | SAN, chain, validity and the client's explicit CA bundle. Repair trust or certificates; keep TLS verification active. |
| Provider unavailable or session expired | Show the connection error, preserve audit provenance and request authentication when required. Do not show a disconnected view as empty activity. |
| Logout unconfirmed | Check the exact per-client post-logout URI and direct-completion policy. A confirmation page or nonmatching/body-bearing redirect is not an accepted acknowledgement. Preserve the separate local/audit result; never follow or log the returned Location, state or hint. |

Keep diagnostics free of bearer tokens, authorization codes, verifiers, cookies and callback queries. Do not persist raw provider replies or complete environment/inspection dumps. Local auxiliary test harnesses are not distributed product commands.

## References

- [Keycloak containers](https://www.keycloak.org/server/containers), [realm import](https://www.keycloak.org/server/importExport), [hostname configuration](https://www.keycloak.org/server/hostname).
- [Keycloak 26.8.1 basic client scope source](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/OIDCLoginProtocolFactory.java#L563-L579), [loopback redirect handling](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/utils/RedirectUtils.java) and [canonical URI builder](https://github.com/keycloak/keycloak/blob/26.8.1/common/src/main/java/org/keycloak/common/util/KeycloakUriBuilder.java).
- [OIDC Core](https://openid.net/specs/openid-connect-core-1_0.html), [PKCE RFC 7636](https://www.rfc-editor.org/rfc/rfc7636.html), [native clients RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html), [OAuth security RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html).
