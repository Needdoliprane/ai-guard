//! Permission-based 360° views over the real gateway and dashboard routers
//! and private test SQLite stores. The data set is the synthetic seed of
//! `support/supervision_seed.rs`; these tests prove server contracts and
//! isolation, never a runtime, a broker process or a worker capacity.
#[path = "support/supervision_seed.rs"]
mod supervision_seed;
mod support;

use agentsandbox::identity::HUMAN_MAX_PAGE_BYTES;
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::BTreeMap;
use supervision_seed::{Seed, SeedOptions, Served, serve};

struct Stack {
    seed: Seed,
    served: Served,
    _files: tempfile::TempDir,
}
struct Reply {
    status: StatusCode,
    body: Value,
    raw: Vec<u8>,
}
impl Stack {
    async fn start() -> Self {
        let files = support::private_tempdir();
        let seed = Seed::build(
            files.path(),
            SeedOptions {
                workers: 24,
                events: 40,
                human_principal_events: None,
            },
        )
        .unwrap_or_else(|error| panic!("build the supervision seed: {error:#}"));
        let front = files.path().join("front");
        std::fs::create_dir(&front).unwrap();
        std::fs::write(front.join("index.html"), "<title>test frontend</title>").unwrap();
        let served = serve(&seed, front).await.expect("serve the test stack");
        Self {
            seed,
            served,
            _files: files,
        }
    }
    fn token(&self, account: &str) -> String {
        self.seed.accounts[account].token.clone()
    }
    /// Every read goes through the dashboard proxy, like the browser.
    async fn get(&self, path: &str, account: &str, context: Option<&str>) -> Reply {
        let mut request = self
            .served
            .client
            .get(format!("{}{path}", self.served.dashboard))
            .bearer_auth(self.token(account));
        if let Some(context) = context {
            request = request.header("X-AiGuard-Context", context);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let raw = response.bytes().await.unwrap().to_vec();
        Reply {
            status,
            body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
            raw,
        }
    }
    async fn identity(&self, account: &str) -> Reply {
        let reply = self.get("/api/v1/human/identity", account, None).await;
        assert_eq!(reply.status, StatusCode::OK, "{account} authenticates");
        reply
    }
    /// Post a JSON body to the gateway itself, like the dashboard's mutations.
    async fn post(
        &self,
        path: &str,
        account: Option<&str>,
        context: Option<&str>,
        body: &Value,
    ) -> Reply {
        let mut request = self
            .served
            .client
            .post(format!("{}{path}", self.served.gateway))
            .json(body);
        if let Some(account) = account {
            request = request.bearer_auth(self.token(account));
        }
        if let Some(context) = context {
            request = request.header("X-AiGuard-Context", context);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let raw = response.bytes().await.unwrap().to_vec();
        Reply {
            status,
            body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
            raw,
        }
    }
    async fn context(&self, account: &str, mode: &str) -> Value {
        self.identity(account).await.body["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| context["mode"] == mode)
            .unwrap_or_else(|| panic!("the control offers {account} a {mode} context"))
            .clone()
    }
}

const OPERATIONS: [&str; 28] = [
    "resources_read",
    "events_read_metadata",
    "events_read_content",
    "events_export",
    "events_stream",
    "summary_read",
    "brokers_read",
    "workers_read",
    "federation_read",
    "quota_read",
    "usage_read",
    "memberships_read",
    "configuration_read",
    "memberships_manage",
    "grants_manage",
    "launch_approve",
    "session_launch_prepare",
    "session_launch",
    "session_message_send",
    "session_messages_read",
    "session_attribution_read",
    "egress_policies_read",
    "egress_attempts_read",
    "egress_grants_read",
    "egress_coverage_read",
    "egress_summary_read",
    "egress_grant_approve",
    "egress_grant_revoke",
];

/// operation → (permission, scope) of one context, after checking the closed
/// shape: every operation exactly once, in catalogue order, scope iff granted.
fn operations(context: &Value) -> BTreeMap<String, (String, Option<String>)> {
    let entries = context["operations"]
        .as_array()
        .expect("each offered context carries its operation catalogue");
    let names: Vec<&str> = entries
        .iter()
        .map(|entry| entry["operation"].as_str().unwrap())
        .collect();
    assert_eq!(
        names, OPERATIONS,
        "closed catalogue in its documented order"
    );
    entries
        .iter()
        .map(|entry| {
            let object = entry.as_object().unwrap();
            let permission = entry["permission"].as_str().unwrap().to_owned();
            let scope = entry.get("scope").map(|s| s.as_str().unwrap().to_owned());
            assert_eq!(
                permission == "granted",
                scope.is_some(),
                "scope is present exactly for a granted operation: {entry}"
            );
            assert!(
                object
                    .keys()
                    .all(|key| ["operation", "permission", "scope", "availability"]
                        .contains(&key.as_str())),
                "closed entry fields: {entry}"
            );
            (
                entry["operation"].as_str().unwrap().to_owned(),
                (permission, scope),
            )
        })
        .collect()
}
fn granted(scope: &str) -> (String, Option<String>) {
    ("granted".into(), Some(scope.into()))
}
fn refused(permission: &str) -> (String, Option<String>) {
    (permission.into(), None)
}

#[tokio::test]
async fn each_context_announces_what_its_mode_and_capabilities_allow() {
    let stack = Stack::start().await;

    // user_a2: personal read_metadata only.
    let user = operations(&stack.context("user_a2", "personal").await);
    assert_eq!(user["resources_read"], granted("own_and_shared"));
    assert_eq!(user["events_stream"], granted("own_and_shared"));
    assert_eq!(user["events_read_content"], refused("not_granted"));
    assert_eq!(user["events_export"], refused("not_granted"));
    assert_eq!(user["brokers_read"], refused("not_granted"));
    assert_eq!(user["session_launch"], refused("not_granted"));
    assert_eq!(user["memberships_manage"], refused("not_granted"));

    // user_a1: personal metadata, content and export.
    let user = operations(&stack.context("user_a1", "personal").await);
    assert_eq!(user["events_read_content"], granted("own_and_shared"));
    assert_eq!(user["events_export"], granted("own_and_shared"));
    assert_eq!(user["workers_read"], refused("not_granted"));

    // admin_a: one membership, two contexts.
    let admin = operations(&stack.context("admin_a", "administration").await);
    assert_eq!(admin["resources_read"], granted("organization"));
    assert_eq!(admin["brokers_read"], granted("tenant"));
    assert_eq!(admin["workers_read"], granted("tenant"));
    assert_eq!(admin["federation_read"], granted("tenant"));
    assert_eq!(admin["quota_read"], granted("tenant"));
    assert_eq!(admin["usage_read"], granted("tenant"));
    assert_eq!(admin["summary_read"], granted("tenant"));
    assert_eq!(admin["memberships_read"], refused("not_granted"));
    assert_eq!(admin["configuration_read"], refused("not_granted"));
    assert_eq!(admin["session_launch"], refused("not_granted"));
    let personal = operations(&stack.context("admin_a", "personal").await);
    assert_eq!(personal["resources_read"], granted("own_and_shared"));
    assert_eq!(personal["brokers_read"], refused("requires_administration"));
    assert_eq!(personal["workers_read"], refused("requires_administration"));
    assert_eq!(personal["summary_read"], granted("own_and_shared"));

    // workers_only_a: the worker registry and nothing else.
    let workers = operations(&stack.context("workers_only_a", "administration").await);
    assert_eq!(workers["workers_read"], granted("tenant"));
    assert_eq!(workers["quota_read"], granted("tenant"));
    assert_eq!(workers["brokers_read"], refused("not_granted"));
    assert_eq!(workers["federation_read"], refused("not_granted"));
    assert_eq!(workers["resources_read"], refused("not_granted"));
    assert_eq!(workers["summary_read"], granted("tenant"));
}

#[tokio::test]
async fn availability_names_what_this_build_and_store_serve() {
    let stack = Stack::start().await;
    let admin = stack.context("admin_a", "administration").await;
    let availability: BTreeMap<&str, &str> = admin["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["operation"].as_str().unwrap(),
                entry["availability"].as_str().unwrap(),
            )
        })
        .collect();
    for served in [
        "resources_read",
        "events_read_metadata",
        "events_read_content",
        "events_export",
        "events_stream",
        "brokers_read",
        "workers_read",
        "federation_read",
        "summary_read",
        "quota_read",
        "usage_read",
        "memberships_read",
        "configuration_read",
    ] {
        assert_eq!(availability[served], "available", "{served}");
    }
    for later in [
        "memberships_manage",
        "grants_manage",
        "launch_approve",
        "session_launch_prepare",
        "session_launch",
        "session_message_send",
    ] {
        assert_eq!(availability[later], "not_in_this_build", "{later}");
    }
}

#[tokio::test]
async fn the_catalogue_of_every_context_fits_the_identity_bound() {
    let stack = Stack::start().await;
    let reply = stack.identity("admin_a").await;
    assert!(reply.raw.len() <= HUMAN_MAX_PAGE_BYTES);
    // The worst case is bounded by construction: 64 contexts, 19 entries each.
    let per_context = reply.body["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|context| serde_json::to_vec(context).unwrap().len())
        .max()
        .unwrap();
    assert!(
        64 * per_context + 4096 <= HUMAN_MAX_PAGE_BYTES,
        "64 contexts of {per_context} bytes must fit the identity bound"
    );
}

async fn summary(stack: &Stack, account: &str, mode: &str, window: &str) -> Reply {
    let context = stack.context(account, mode).await;
    stack
        .get(
            &format!("/api/v1/human/summary?window={window}"),
            account,
            Some(context["id"].as_str().unwrap()),
        )
        .await
}
fn total(counts: &Value) -> u64 {
    counts
        .as_object()
        .unwrap()
        .values()
        .map(|value| value.as_str().unwrap().parse::<u64>().unwrap())
        .sum()
}

#[tokio::test]
async fn the_administration_summary_counts_the_tenant_registries_once() {
    let mut stack = Stack::start().await;
    supervision_seed::post_heartbeat(&mut stack.seed, &stack.served)
        .await
        .unwrap();
    let reply = summary(&stack, "admin_a", "administration", "all").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let body = &reply.body;
    assert_eq!(body["mode"], "administration");
    assert_eq!(body["consistency"], "transaction_snapshot");
    assert_eq!(body["period"]["window"], "all");
    assert!(body["period"]["from"].is_null());
    assert!(body["read_receipt"]["audit_event_id"].is_string());

    let workers = &body["sections"]["workers"];
    assert_eq!(workers["status"], "ready");
    assert_eq!(workers["coverage"], "complete_registry");
    assert_eq!(workers["data"]["runtime_inventory"], "unknown");
    assert_eq!(
        total(&workers["data"]["by_state"]),
        stack.seed.worker_ids_a.len() as u64,
        "every registered worker of tenant A, not a page"
    );
    let groups = workers["data"]["groups"].as_array().unwrap();
    assert_eq!(
        groups
            .iter()
            .map(|group| total(&group["by_state"]))
            .sum::<u64>(),
        stack.seed.worker_ids_a.len() as u64,
        "the broker groups partition the registry"
    );
    let grouped_brokers: Vec<&str> = groups
        .iter()
        .filter_map(|group| group["broker"]["broker_id"].as_str())
        .collect();
    for name in ["current", "revoked", "unseen"] {
        let id = stack.seed.brokers[name].broker_id.as_str();
        if grouped_brokers.contains(&id) {
            let group = groups
                .iter()
                .find(|group| group["broker"]["broker_id"] == id)
                .unwrap();
            assert_eq!(group["association"], "registered");
        }
    }

    let brokers = &body["sections"]["brokers"]["data"]["by_state"];
    assert_eq!(brokers["current"], "1");
    assert_eq!(brokers["revoked"], "1");
    assert_eq!(brokers["unseen"], "1");
    assert_eq!(
        body["sections"]["quota"]["data"]["max_active_workers"],
        agentsandbox::control::QuotaPolicyV1::default()
            .max_active_workers
            .to_string(),
        "the gateway's configured tenant policy"
    );
    assert_eq!(body["sections"]["usage"]["data"]["billable"], false);
    assert_eq!(body["sections"]["resources"]["status"], "ready");
    assert_eq!(body["sections"]["decisions"]["status"], "ready");
    let attention = &body["sections"]["attention"]["data"];
    assert!(
        attention["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "broker_revoked"
                && item["subject_id"] == stack.seed.brokers["revoked"].broker_id.as_str()),
        "a revoked broker is a point of attention: {attention}"
    );
}

#[tokio::test]
async fn each_section_follows_the_capabilities_of_the_context() {
    let stack = Stack::start().await;
    let status = |reply: &Reply, section: &str| reply.body["sections"][section]["status"].clone();

    let workers_only = summary(&stack, "workers_only_a", "administration", "24h").await;
    assert_eq!(workers_only.status, StatusCode::OK);
    assert_eq!(status(&workers_only, "workers"), "ready");
    assert_eq!(status(&workers_only, "quota"), "ready");
    assert_eq!(status(&workers_only, "usage"), "ready");
    assert_eq!(status(&workers_only, "brokers"), "not_granted");
    assert_eq!(status(&workers_only, "resources"), "not_granted");
    assert_eq!(status(&workers_only, "decisions"), "not_granted");

    let brokers_only = summary(&stack, "brokers_only_a", "administration", "24h").await;
    assert_eq!(status(&brokers_only, "brokers"), "ready");
    assert_eq!(status(&brokers_only, "attention"), "ready");
    assert_eq!(status(&brokers_only, "workers"), "not_granted");
    assert_eq!(status(&brokers_only, "quota"), "not_granted");

    let personal = summary(&stack, "user_a2", "personal", "7d").await;
    assert_eq!(personal.status, StatusCode::OK);
    assert_eq!(personal.body["mode"], "personal");
    assert!(personal.body["period"]["from"].is_string());
    assert_eq!(status(&personal, "resources"), "ready");
    assert_eq!(
        personal.body["sections"]["resources"]["coverage"],
        "authorized_selection"
    );
    for section in ["workers", "brokers", "quota", "usage", "attention"] {
        assert_eq!(status(&personal, section), "not_granted", "{section}");
    }
}

#[tokio::test]
async fn no_summary_carries_another_tenant_and_a_withdrawn_right_is_refused() {
    let mut stack = Stack::start().await;
    let reply = summary(&stack, "admin_b", "administration", "all").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let raw = String::from_utf8(reply.raw.clone()).unwrap();
    for id in stack
        .seed
        .worker_ids_a
        .iter()
        .chain(stack.seed.brokers.values().map(|broker| &broker.broker_id))
    {
        assert!(
            !raw.contains(id.as_str()),
            "tenant A identifier {id} in B's summary"
        );
    }

    let before = stack.context("admin_a", "administration").await;
    stack
        .seed
        .set_capabilities(
            "admin_a",
            &[
                agentsandbox::identity::HumanCapabilityV1::ReadMetadata,
                agentsandbox::identity::HumanCapabilityV1::ReadTenantMetadata,
            ],
        )
        .unwrap();
    let stale = stack
        .get(
            "/api/v1/human/summary?window=24h",
            "admin_a",
            Some(before["id"].as_str().unwrap()),
        )
        .await;
    assert!(
        matches!(
            stale.status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "a context issued before the change no longer reads: {}",
        stale.status
    );
    assert!(!String::from_utf8_lossy(&stale.raw).contains("by_state"));
    let after = summary(&stack, "admin_a", "administration", "24h").await;
    assert_eq!(after.status, StatusCode::OK);
    assert_eq!(after.body["sections"]["workers"]["status"], "not_granted");
    assert_eq!(after.body["sections"]["brokers"]["status"], "not_granted");
    assert_eq!(after.body["sections"]["resources"]["status"], "ready");
}

#[tokio::test]
async fn the_summary_query_is_closed() {
    let stack = Stack::start().await;
    let context = stack.context("admin_a", "administration").await;
    let id = context["id"].as_str().unwrap();
    for query in ["window=1h", "window=24h&tenant=tenant-b", "limit=5"] {
        let reply = stack
            .get(
                &format!("/api/v1/human/summary?{query}"),
                "admin_a",
                Some(id),
            )
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{query}");
    }
    let missing = stack.get("/api/v1/human/summary", "admin_a", None).await;
    assert!(missing.status.is_client_error(), "no context, no summary");
}

async fn admin_get(stack: &Stack, account: &str, path: &str) -> Reply {
    let context = stack.context(account, "administration").await;
    stack
        .get(path, account, Some(context["id"].as_str().unwrap()))
        .await
}

#[tokio::test]
async fn memberships_are_an_explicit_administration_read_of_one_organization() {
    use agentsandbox::identity::HumanCapabilityV1 as C;
    let mut stack = Stack::start().await;
    let refused = admin_get(&stack, "admin_a", "/api/v1/human/admin/memberships").await;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "read_tenant_metadata is not read_memberships"
    );
    stack
        .seed
        .set_capabilities(
            "admin_a",
            &[C::ReadMetadata, C::ReadTenantMetadata, C::ReadMemberships],
        )
        .unwrap();
    let reply = admin_get(
        &stack,
        "admin_a",
        "/api/v1/human/admin/memberships?limit=100",
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let items = reply.body["items"].as_array().unwrap();
    let organization = stack.seed.accounts["admin_a"]
        .membership
        .organization_scope
        .clone();
    assert!(
        items
            .iter()
            .all(|item| item["organization_scope"] == organization.as_str())
    );
    let own: Vec<&Value> = items.iter().filter(|item| item["own"] == true).collect();
    assert_eq!(own.len(), 1);
    assert_eq!(
        own[0]["membership_id"],
        stack.seed.accounts["admin_a"]
            .membership
            .membership_id
            .as_str()
    );
    let other = &stack.seed.accounts["other_org_a"].membership.membership_id;
    assert!(
        !String::from_utf8_lossy(&reply.raw).contains(other.as_str()),
        "another organization's membership is not listed"
    );
    for account in stack.seed.accounts.values() {
        assert!(!String::from_utf8_lossy(&reply.raw).contains(&account.credential.sha256_digest));
    }
    assert!(reply.body["read_receipt"]["audit_event_id"].is_string());
}

#[tokio::test]
async fn quota_and_usage_ride_on_the_worker_registry_read() {
    let stack = Stack::start().await;
    for account in ["admin_a", "workers_only_a"] {
        let quota = admin_get(&stack, account, "/api/v1/human/admin/quota").await;
        assert_eq!(quota.status, StatusCode::OK, "{account}: {}", quota.body);
        assert_eq!(
            quota.body["quota"]["max_active_workers"],
            agentsandbox::control::QuotaPolicyV1::default()
                .max_active_workers
                .to_string()
        );
        let usage = admin_get(&stack, account, "/api/v1/human/admin/usage?limit=10").await;
        assert_eq!(usage.status, StatusCode::OK, "{account}: {}", usage.body);
        assert_eq!(usage.body["billable"], false);
        assert_eq!(usage.body["method_version"], 1);
    }
    let brokers_only = admin_get(&stack, "brokers_only_a", "/api/v1/human/admin/quota").await;
    assert_eq!(brokers_only.status, StatusCode::FORBIDDEN);
    let personal = stack.context("user_a1", "personal").await;
    let personal = stack
        .get(
            "/api/v1/human/admin/usage",
            "user_a1",
            Some(personal["id"].as_str().unwrap()),
        )
        .await;
    assert_eq!(personal.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn configuration_lists_kinds_and_modes_never_addresses_or_paths() {
    use agentsandbox::identity::HumanCapabilityV1 as C;
    let mut stack = Stack::start().await;
    let refused = admin_get(&stack, "admin_a", "/api/v1/human/admin/configuration").await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    stack
        .seed
        .set_capabilities("admin_a", &[C::ReadTenantMetadata, C::ReadConfiguration])
        .unwrap();
    let reply = admin_get(&stack, "admin_a", "/api/v1/human/admin/configuration").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let entries = reply.body["entries"].as_array().unwrap();
    let value = |axis: &str, key: &str| {
        entries
            .iter()
            .find(|entry| entry["axis"] == axis && entry["key"] == key)
            .unwrap_or_else(|| panic!("{axis}/{key}"))["value"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(value("storage", "backend"), "sqlite");
    assert_eq!(value("storage", "human_rights_marker"), "installed");
    assert_eq!(value("identity", "oidc"), "disabled");
    for entry in entries {
        let text = entry["value"].as_str().unwrap();
        assert!(
            !text.contains('/') && !text.contains(':') && !text.contains('@'),
            "no address, path or credential in {entry}"
        );
    }
    for account in stack.seed.accounts.values() {
        assert!(!String::from_utf8_lossy(&reply.raw).contains(&account.token));
    }
}

/// The session-launch route is served by the real gateway router and refuses
/// without owned human-source coverage, never a silent 404. The legacy seed has
/// no owned Store14/15 bundle, so the launch commits nothing here; the durable
/// owned happy path (exact preparation consumption, nominative grant, audit
/// before effect, idempotence) is proven on the real owned Store fixture.
#[tokio::test]
async fn session_launch_route_is_served_and_refuses_without_owned_coverage() {
    let stack = Stack::start().await;
    let body = serde_json::json!({
        "version": 1,
        "operation_id": uuid::Uuid::new_v4().to_string(),
        "launch_target_id": uuid::Uuid::new_v4().to_string(),
        "expected_target_revision": 1,
        "preparation_operation_id": uuid::Uuid::new_v4().to_string(),
        "preparation_input_hash": "a".repeat(64),
    });
    let reply = stack
        .post(
            "/api/v1/human/owned/sessions/launch",
            Some("user_a1"),
            None,
            &body,
        )
        .await;
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        reply.body
    );
    assert_eq!(
        reply.body["error"]["code"], "identity_unavailable",
        "{}",
        reply.body
    );
    // An invalid body is refused before any owned effect, never a panic.
    let invalid = stack
        .post(
            "/api/v1/human/owned/sessions/launch",
            Some("user_a1"),
            None,
            &serde_json::json!({"version": 1}),
        )
        .await;
    assert!(
        invalid.status == StatusCode::SERVICE_UNAVAILABLE
            || invalid.status == StatusCode::BAD_REQUEST,
        "{}",
        invalid.body
    );
}
